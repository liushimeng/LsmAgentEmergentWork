//! WorkFlow 执行单元(2026-09-17 自 orchestrator.rs 拆分)。
//!
//! `execute_workflows` 按 `depends_on` Kahn 分层、同层 SubAgent 并行;
//! `run_wf_unit` 是单个执行单元(SubAgent 执行 + QC + Debug 采集),
//! 串行直通与 tokio::spawn 并行两种调用路径共用。

use super::*;

impl MultiAgentOrchestrator {
    // ========== 通用:执行 WorkFlow 列表 ==========

    pub(super) async fn execute_workflows(
        &self,
        c: &TaskClassification,
        plan: &WorkFlowPlan,
        pre_usage: Usage, // 2026-09-09 第 14 轮:承接上游(Main-Work / Quality-Main)累计的 LLM 用量
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
        unit_retry_hint: &str,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 依赖分层:同层 WorkFlow 互相无依赖,自动并行;跨层严格串行(自动感知 depends_on)
        // 第 174 轮:未知依赖降级为警告并丢弃(局部重拆的悬空边),循环依赖仍失败。
        let (layers, dropped_deps) =
            main_work::topo_layers_lenient(&plan.workflows).map_err(|e| QualityFailure {
            source: AgentRole::MainWork,
            reason: format!("拓扑分层失败: {e}"),
            retryable: false,
            suggestion: "Plan 中存在循环或未知依赖".into(),
            cancelled: false,
            trace: None,
            usage: Usage::default(),
                completed_digest: String::new(),
            })?;
        for d in &dropped_deps {
            tracing::warn!("[拓扑分层] 未知依赖降级(边已丢弃,按无依赖执行): {d}");
        }
        // 第 165 轮:全并行退化警示 —— 多单元落同一层意味着零依赖边,执行顺序完全
        // 由 Semaphore 槽位释放顺序决定(实测乱序互踩重试风暴,详见 tmpPlan/2026-10-13
        // _WorkFlow依赖解析全角标点丢失与串行兜底方案.md)。正常应已被串行链兜底
        // (`fallback_serial_chain`)拦截,到达这里通常意味着杀开关被关闭,留观测点。
        if layers.len() == 1 && plan.workflows.len() >= 2 {
            tracing::warn!(
                units = plan.workflows.len(),
                "[拓扑退化] {} 个 WorkFlow 全部同层并行(零依赖边):执行顺序不受控,存在乱序互踩风险",
                plan.workflows.len()
            );
        }

        let mut results = Vec::new();
        let mut dep_outputs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut total_usage = pre_usage;
        let total_layers = layers.len();
        // 2026-09-16 第 57 轮:任务级阶段耗时 + 分层执行记录收集器。
        let mut stage_durations: Vec<StageDuration> = Vec::new();
        let mut layer_log: Vec<LayerInfo> = Vec::new();

        for (layer_idx, layer) in layers.into_iter().enumerate() {
            // 层边界:取消短路(下一层不再启动)
            if let Err(e) = Self::check_cancelled(cancel) {
                return Err(QualityFailure::from_agent_error(
                    AgentRole::MainWork,
                    "WorkFlow 层调度",
                    &e,
                ));
            }
            // 2026-09-16 第 57 轮:本层开始时刻(用于 LayerInfo.elapsed_ms 与
            // StageDuration.started_offset_ms 的统一基准)。
            let layer_started = std::time::Instant::now();
            // 运行日志(2026-09-17 第 69 轮):拓扑分层执行 —— 层号/单元 id/并行与否
            info!(
                session = %session.id(),
                layer = layer_idx + 1,
                total_layers,
                parallel = layer.len() > 1,
                wf_ids = %layer.iter().map(|w| w.id.clone()).collect::<Vec<_>>().join(","),
                "拓扑分层执行"
            );
            if layer.len() > 1 {
                // F3(2026-09-14 第 51 轮):改走 progress 通道而非裸 eprintln!
                // TUI 下 eprintln! 不感知 waiting 行原地重写纪律,会把本条通知
                // 拼接到 spinner 行尾(实测 c05:「…可 Ctrl-C 取消[laew] WorkFlow
                // 并行调度…」同行粘连);经 progress 通道由打印协程统一
                // 先 \r\x1b[K 清行再输出,非 TTY 下仍落 stderr,行为不回退。
                emit_progress(
                    progress,
                    format!(
                        "[laew] WorkFlow 并行调度:第 {}/{} 层 {} 个流程并发执行(上限 {})",
                        layer_idx + 1,
                        total_layers,
                        layer.len(),
                        self.cfg.max_parallel_workflows,
                    ),
                );
            }

            // 构造本层全部单元的输入(上游产物按层传递)
            let units: Vec<(WorkFlowSpec, SubFlowInput)> = layer
                .iter()
                .map(|wf| {
                    (
                        wf.clone(),
                        build_subflow_input(wf, &dep_outputs, unit_retry_hint),
                    )
                })
                .collect();

            // 执行本层:单个直通(零 spawn 开销),多个 tokio::spawn + Semaphore 有界并发
            // 第 95 轮:单元级局部重试预算(单元素语义=num_attempt - 1)
            let retry_budget = self.cfg.unit_retry_budget;
            let layer_outcomes = if units.len() == 1 {
                let (wf, input) = units.into_iter().next().expect("len==1");
                let outcome = run_wf_unit(
                    self.sub_agent.clone(),
                    self.quality.clone(),
                    self.reflection.clone(),
                    self.cfg.debug.clone(),
                    input,
                    c.goal_summary.clone(),
                    session.id().to_string(),
                    None,
                    cancel.clone(),
                    progress.clone(),
                    retry_budget,
                )
                .await;
                vec![(wf, outcome)]
            } else {
                let semaphore =
                    Arc::new(tokio::sync::Semaphore::new(self.cfg.max_parallel_workflows));
                let mut handles = Vec::with_capacity(units.len());
                for (wf, input) in units {
                    let sub_agent = self.sub_agent.clone();
                    let quality = self.quality.clone();
                    let reflection = self.reflection.clone();
                    let debug = self.cfg.debug.clone();
                    let goal = c.goal_summary.clone();
                    let sid = session.id().to_string();
                    let sem = semaphore.clone();
                    // 取消传播:同层每个并行单元持同一 token 的 clone,
                    // 父任务取消 → 全部单元即时中断(原子级联,无需逐个通知)
                    let cancel_tok = cancel.clone();
                    let progress_tx = progress.clone();
                    handles.push(tokio::spawn(async move {
                        let outcome = run_wf_unit(
                            sub_agent,
                            quality,
                            reflection,
                            debug,
                            input,
                            goal,
                            sid,
                            Some(sem),
                            cancel_tok,
                            progress_tx,
                            retry_budget,
                        )
                        .await;
                        (wf, outcome)
                    }));
                }
                // 按层内原始顺序 await:保证结果顺序确定;失败不中断在飞的独立任务
                let mut outcomes = Vec::with_capacity(handles.len());
                for h in handles {
                    match h.await {
                        Ok(pair) => outcomes.push(pair),
                        Err(e) => {
                            // JoinError(panic 等):映射为失败回流,不吞掉
                            outcomes.push((
                                WorkFlowSpec {
                                    id: "unknown".into(),
                                    name: "并行任务".into(),
                                    steps: vec![],
                                    branches: vec![],
                                    loops: vec![],
                                    depends_on: vec![],
                                    acceptance: vec![],
                                    delegate_to: AgentRole::SubAgent,
                                    // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
                                    max_iterations: None, original_prompt: None, pre_explore: false,
                                },
                                Err(QualityFailure {
                                    source: AgentRole::SubAgent,
                                    reason: format!("并行 WorkFlow 任务 join 失败: {e}"),
                                    retryable: true,
                                    suggestion: "重试".into(),
                                    cancelled: false,
                                    trace: None,
                                    usage: Usage::default(),
                                    completed_digest: String::new(),
                                }),
                            ));
                        }
                    }
                }
                outcomes
            };

            // 汇总本层:按序取首个失败回流(fail-fast 语义与串行版一致)
            let mut first_failure: Option<QualityFailure> = None;
            let mut ok_units = Vec::new();
            for (wf, outcome) in layer_outcomes {
                match outcome {
                    Ok(ok) => ok_units.push((wf, ok)),
                    Err(f) => {
                        if first_failure.is_none() {
                            first_failure = Some(f);
                        }
                    }
                }
            }
            if let Some(mut f) = first_failure {
                // 第 146 轮:失败升级时携带**已完成且 QC 通过**的单元摘要 ——
                // 档位级重试/Hard 重规划拿到它后只规划剩余部分,不再从零重跑
                // (实测 v2 轮把 v1 已通过的登录/建档单元整个重跑了一遍)。
                f.completed_digest = build_completed_digest(
                    &results,
                    ok_units.iter().map(|(wf, ok)| (wf.id.clone(), wf.name.clone(), ok.outcome_text.as_str())),
                );
                return Err(f);
            }

            // 2026-09-16 第 57 轮:本层墙钟(用于 LayerInfo.elapsed_ms;并行层取
            // 最长单元耗时,串行层取累计 = 实时差)。
            let layer_elapsed_ms = layer_started.elapsed().as_millis() as u64;

            for (wf, ok) in ok_units {
                // 2026-09-09 第 14 轮:同时累加 SubAgent 用量与 Quality-Check 用量
                total_usage = add_usage(total_usage, ok.usage);
                total_usage = add_usage(total_usage, ok.qc_usage);
                dep_outputs.insert(wf.id.clone(), ok.outcome_text.clone());
                let wf_id = wf.id.clone();
                let wf_name = wf.name.clone();
                let sub_text = ok.outcome_text;
                let qc_report = ok.qc;
                // D9-8 决策审计(2026-09-19):QC 质检判定写入审计 JSONL
                //(在 qc_report move 进 WorkflowResult 前提取字段)。
                {
                    let verdict_str =
                        if qc_report.verdict == crate::agent::quality::Verdict::Pass {
                            "pass"
                        } else {
                            "fail"
                        };
                    crate::agent::decision_audit::record_verdict(
                        session.id(),
                        &wf.id,
                        verdict_str,
                        &qc_report.issues,
                        qc_report.retryable,
                        &qc_report.evidence,
                    );
                }
                let exec_role = ok.exec_role;
                let wallclock_ms = ok.wallclock_ms;
                let qc_wallclock_ms = ok.qc_wallclock_ms;
                let sub_trace = ok.trace;
                let sub_usage = ok.usage;
                let qc_usage = ok.qc_usage;
                // 2026-09-16 第 57 轮:每个 WF + 每个 QC 单独写阶段耗时(供 TUI 时间线)。
                let layer_elapsed_ms_now = layer_started.elapsed().as_millis() as u64;
                stage_durations.push(StageDuration {
                    stage: "wf".to_string(),
                    wf_id: Some(wf_id.clone()),
                    started_offset_ms: layer_elapsed_ms_now.saturating_sub(wallclock_ms),
                    elapsed_ms: wallclock_ms,
                });
                stage_durations.push(StageDuration {
                    stage: "qc_wf".to_string(),
                    wf_id: Some(wf_id.clone()),
                    started_offset_ms: layer_elapsed_ms_now.saturating_sub(qc_wallclock_ms),
                    elapsed_ms: qc_wallclock_ms,
                });
                results.push(WorkflowResult {
                    id: wf_id,
                    name: wf_name,
                    subflow_outcome: sub_text,
                    quality_report: qc_report,
                    usage: sub_usage,
                    subflow_trace: Some(sub_trace),
                    exec_role,
                    wallclock_ms,
                    qc_wallclock_ms,
                });
                let _ = qc_usage; // 已在 stage_durations / total_usage 累加
            }

            // 2026-09-16 第 57 轮:本层信息汇总。
            layer_log.push(LayerInfo {
                layer_idx,
                wf_ids: layer.iter().map(|w| w.id.clone()).collect(),
                parallel: layer.len() > 1,
                started_offset_ms: 0, // 由 TUI 在最终呈现时计算相对任务起点的偏移
                elapsed_ms: layer_elapsed_ms,
            });
        }

        Ok(TaskResult {
            goal: c.goal_summary.clone(),
            classification: c.clone(),
            plan_doc: None,
            workflows: results,
            summary: String::new(),
            total_usage,
            stage_durations,
            retry_log: Vec::new(),
            layer_log,
            wallclock_ms: 0, // 由 handle_inner 在收口时根据入口 Instant 覆盖
        })
    }
}

/// 单个 WorkFlow 执行单元的成功产物(类型及字段 pub(super) 供 pipeline 简单档复用包装)。
pub(super) struct WfUnitOk {
    pub(super) outcome_text: String,
    /// SubAgent 自身的 LLM 用量(2026-09-09 第 14 轮:Quality 用量单独累加,见 `qc_usage`;第 95 轮起含局部重试各次累计)。
    pub(super) usage: Usage,
    pub(super) qc: QualityReport,
    /// Quality-Check 调用的 LLM 用量(2026-09-09 第 14 轮:从 check_subagent 返回值中带回)。
    pub(super) qc_usage: Usage,
    /// SubAgent 执行轨迹(2026-09-09 第 05 轮)。
    pub(super) trace: ExecutionTrace,
    /// 2026-09-16 第 57 轮:执行器角色(SubAgent / WebUse)
    pub(super) exec_role: AgentRole,
    /// 2026-09-16 第 57 轮:执行器墙钟耗时(毫秒;第 95 轮起为局部重试各次累计)
    pub(super) wallclock_ms: u64,
    /// 2026-09-16 第 57 轮:QC LLM 调用单独耗时(毫秒;第 95 轮起为各次累计)
    pub(super) qc_wallclock_ms: u64,
}

/// 执行一个 WorkFlow 单元:SubAgent 执行 + Quality-Check(+ Debug 采集)。
///
/// 第 89 轮(2026-09-18)起执行器统一为 SubAgentRunner(原 Chromium-WebUse 专项
/// Runner 已删除,浏览器操控由 SubAgent-Work 的 MCP_Web_LsmAgentBrowser_Use 工具承担;
/// 桌面窗口操控由 MCP_Window_Use 工具承担)。QC / Debug / 取消 / 进度通道全链路复用。
///
/// 自由函数 + Arc 参数化,串行直通与 tokio::spawn 并行两种调用路径共用同一份逻辑;
/// `semaphore` 为并行路径的有界并发许可(串行路径传 None);
/// `cancel` 为任务级取消 token(传播进 SubAgent 的 Agent 循环,LLM/工具即时中断);
/// `progress` 为阶段进度通道 clone(并行单元各自持有,2026-09-10 第 23 轮)。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_wf_unit(
    sub_agent: Arc<SubAgentRunner>,
    quality: Arc<QualityRunner>,
    reflection: Arc<crate::agent::reflection::ReflectionRunner>,
    debug: Option<Arc<DebugCollector>>,
    mut input: SubFlowInput,
    goal: String,
    session_id: String,
    semaphore: Option<Arc<tokio::sync::Semaphore>>,
    cancel: CancelToken,
    progress: Option<ProgressTx>,
    // 第 95 轮:执行单元级局部重试预算(0 = 首执行 QC 拒即升级,旧行为)
    retry_budget: usize,
) -> std::result::Result<WfUnitOk, QualityFailure> {
    // 有界并发:先抢许可(对齐 atomcode Semaphore(3) FIFO 惯例)。
    // 整单元(含局部重试各次尝试)期间持续持有许可,互斥域不放大(同层单元间仍并行)。
    let _permit = match &semaphore {
        Some(sem) => Some(sem.acquire().await.map_err(|e| QualityFailure {
            source: AgentRole::SubAgent,
            reason: format!("并行调度信号量已关闭: {e}"),
            retryable: true,
            suggestion: "重试".into(),
            cancelled: false,
            trace: None,
            usage: Usage::default(),
            completed_digest: String::new(),
        })?),
        None => None,
    };

    // ===== 第 95 轮:单元级局部重试循环(方案 tmpPlan/2026-09-19_06) =====
    // retry_hint 分层:attempt=0 用编排器注入的档位级 hint(build_subflow_input 已拼入
    // description);attempt≥1 用本单元 QC 结论原位覆盖 hint 段,写入 retry_count 供 QC 对账。
    // QC 拒 retryable=true 且预算未满 → 仅重试该单元(同层姊妹单元不受影响);预算耗尽
    // 或不可重试 → 升级为档位级失败,走旧路径(档位级 retry_hint 回灌 / Yolo 回流)。
    // usage 各次尝试累计进单元 usage(成功随 WfUnitOk,失败随 QualityFailure);
    // wallclock 采用历次累计 —— 语义更准(单元总耗时),TUI 时间线不再被短跑误导。
    let mut attempt = 0usize;
    let mut unit_retry_hint = String::new();
    let mut acc_sub_elapsed_ms: u64 = 0;
    let mut acc_qc_elapsed_ms: u64 = 0;
    let mut acc_usage = Usage::default();
    // Reflection(反思)模式:单元内跨 attempt 的教训账本 + 最近一次深度反思结论。
    // 条件触发(只在即将带着错误重跑时深度反思),机械基线零 LLM 始终可合并;
    // `LAEW_REFLECTION=off` 全关,行为与改造前一致。
    let mut lessons_acc: Vec<String> = Vec::new();
    let mut last_reflection: Option<crate::agent::reflection::ReflectionReport> = None;
    let mut deep_reflections = 0usize;

    loop {
        if attempt > 0 {
            // retry_hint 分层:attempt≥1 时用本单元上一轮 QC 结论原位覆盖 description 的
            // hint 段(基础任务描述保留),retry_count 写入供 failure_signals 对账(第 91 轮)
            input.retry_count = attempt;
            apply_retry_hint_overlay(&mut input, &unit_retry_hint, attempt);
        }
        // ===== SubAgent 执行(原逻辑) =====
        // 2026-09-16 第 62 轮:拆分 stage 短标题 / 详情面板。
        // 短标题(≤80 字符)进入 TUI stage 流 + waiting 心跳,避免每 1s
        // 原地重写整段超长文本;详情面板走 [laew] 前缀,立即冲刷、不进
        // waiting 心跳,任务快速完成时也保留。
        // 运行日志(2026-09-17 第 69 轮):单元开始(职责 + 期望输出)
        info!(
            session = %session_id,
            wf_id = %input.id,
            executor = "SubAgent",
            description = %crate::logging::clip(&input.description),
            expected = %crate::logging::clip(&input.expected_output),
            attempt = attempt,
            "WorkFlow 单元开始"
        );
        emit_progress(
            &progress,
            format!("{} SubAgent 执行中{}",
                input.id,
                if attempt > 0 { "(局部重试)" } else { "" }
            ),
        );
        emit_progress(
            &progress,
            format!(
                "[laew] {} SubAgent 详情 | 职责: {} | 期望: {}",
                input.id,
                truncate_progress_text(&input.description, 80),
                truncate_progress_text(&input.expected_output, 80)
            ),
        );
        // 2026-09-16 第 57 轮:执行器墙钟计时 ——
        // 用于 TaskResult.wallclock_ms / TUI 时间线展示。
        let sub_started = std::time::Instant::now();
        // 第 146 轮:访问台账快照(本单元本 attempt 的导航足迹起点;
        // QC 用 footprint delta 做覆盖类期望的机械对账,零 LLM 成本)。
        let nav_seq0 = crate::agent::tools::mcp_web_lsm_browser_use::visit_ledger::seq();
        let outcome = sub_agent
            .run_unit_with_cancel(&input, &session_id, &cancel)
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(
                    AgentRole::SubAgent,
                    &format!("SubAgent 执行失败(wf={})", input.id),
                    &e,
                )
            })?;
        let sub_elapsed_ms = sub_started.elapsed().as_millis() as u64;
        acc_sub_elapsed_ms = acc_sub_elapsed_ms.saturating_add(sub_elapsed_ms);
        // 2026-09-16 第 62 轮:单元结束走 [laew] 详情面板 + 短标题 stage。
        // 详情面板显示 iter / tools / early_term / 错误摘要,即使 LLM 没调
        // 工具也立即可见,避免用户看到「卡住 100 秒没动作」。
        let summary_line = format!(
            "{} SubAgent 完成 | {:.1}s",
            input.id,
            sub_elapsed_ms as f64 / 1000.0
        );
        // 运行日志(2026-09-17 第 69 轮):单元执行完成 —— 迭代/工具成败/耗时/产物规模
        info!(
            session = %session_id,
            wf_id = %input.id,
            executor = "SubAgent",
            iterations = outcome.trace.iterations,
            tool_calls = outcome.trace.tool_calls,
            tool_ok = outcome.trace.tool_calls_ok,
            tool_err = outcome.trace.tool_calls_err,
            early_terminated = outcome.trace.early_terminated,
            early_reason = %outcome.trace.early_terminate_reason,
            wallclock_ms = sub_elapsed_ms,
            output_chars = outcome.text.chars().count(),
            "WorkFlow 单元执行完成"
        );
        emit_progress(&progress, summary_line);
        let detail_summary = format!(
            "[laew] {} SubAgent 执行证据 | iter={} tools={}({}成功/{}失败) early_term={} reason={}",
            input.id,
            outcome.trace.iterations,
            outcome.trace.tool_calls,
            outcome.trace.tool_calls_ok,
            outcome.trace.tool_calls_err,
            outcome.trace.early_terminated,
            if outcome.trace.early_terminate_reason.is_empty() {
                "<none>".to_string()
            } else {
                truncate_progress_text_default(&outcome.trace.early_terminate_reason, 180)
            }
        );
        emit_progress(&progress, detail_summary);
        // 2026-09-16 第 62 轮:工具调用明细走 [laew] 详情面板,让 TUI 用户一眼看到
        // 「到底调了哪些工具、参数是什么、为什么失败」。
        // 2026-09-16 第 67 轮:明细 5 → 8 条,且追加**参数摘要**(query/window_id/path/
        // action/text/x/y/command 等关键字段,截 60 字符)—— 微信任务复盘时
        // 「到底让它点了哪个坐标」此前无从排查。
        let tool_log = &outcome.trace.tool_call_log;
        if !tool_log.is_empty() {
            let recent: Vec<String> = tool_log
                .iter()
                .rev()
                .take(8)
                .rev()
                .map(|entry| {
                    let status = if entry.ok { "✓" } else { "✗" };
                    let err_part = if entry.error_summary.is_empty() {
                        String::new()
                    } else {
                        format!(" err={}", truncate_progress_text_default(&entry.error_summary, 40))
                    };
                    let args_part = tool_args_digest(&entry.tool, &entry.args_json);
                    let args_part = if args_part.is_empty() {
                        String::new()
                    } else {
                        format!(" {args_part}")
                    };
                    format!(
                        "{}{} ({}ms){}{}",
                        status,
                        entry.tool,
                        entry.elapsed_ms,
                        args_part,
                        err_part
                    )
                })
                .collect();
            emit_progress(
                &progress,
                format!(
                    "[laew] {} SubAgent 工具调用(最近 {} 条): {}",
                    input.id,
                    recent.len(),
                    recent.join(" | ")
                ),
            );
        }
        if !outcome.trace.failure_signals.is_empty() {
            emit_progress(
                &progress,
                format!(
                    "[laew] {} SubAgent 失败信号: {}",
                    input.id,
                    truncate_progress_text_default(&outcome.trace.failure_signals.join(","), 180)
                ),
            );
        }

        // 2026-09-16 第 57 轮:QC LLM 调用单独计时。
        let qc_started = std::time::Instant::now();
        // 第 146 轮:实际输出尾部拼【机械导航足迹】(open/navigate/new_tab 的
        // 工具层增量统计)—— clip_qc_input 保尾 2000 字符,天然存活到 QC 眼前;
        // 覆盖类期望(「遍历 P1~P4」)从此有机械对账依据,漏页/打转一眼可见。
        let actual_for_qc = format!(
            "{}\n\n【机械导航足迹(工具层统计,零幻觉)】{}",
            outcome.text,
            crate::agent::tools::mcp_web_lsm_browser_use::visit_ledger::footprint_since(nav_seq0),
        );
        let (mut qc, qc_usage) = quality
            .check_subagent_with_source(
                AgentRole::SubAgent,
                &goal,
                &input.description,
                &input.expected_output,
                &actual_for_qc,
                &outcome.trace,
                &session_id,
                // 2026-09-19 第 93 轮:QC 透传用户原始输入(第一性事实),
                // 识别「验收标准本身偏离任务」的编排降级(保活冒充聊天)。
                input.original_prompt.as_deref(),
            )
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        let qc_elapsed_ms = qc_started.elapsed().as_millis() as u64;
        acc_qc_elapsed_ms = acc_qc_elapsed_ms.saturating_add(qc_elapsed_ms);
        acc_usage = add_usage(acc_usage, add_usage(outcome.usage, qc_usage));
        // 2026-09-17 第 69 轮:单元质检判定(详细报告由 quality.rs 统一记录)
        info!(
            session = %session_id,
            wf_id = %input.id,
            executor = "SubAgent",
            verdict = if qc.verdict == Verdict::Pass { "pass" } else { "fail" },
            retryable = qc.retryable,
            qc_wallclock_ms = qc_elapsed_ms,
            "WorkFlow 单元质检"
        );
        if let Some(d) = &debug {
            d.record_quality(&qc);
        }
        emit_progress(
            &progress,
            format!(
                "{} QC:{}",
                input.id,
                if qc.verdict == Verdict::Pass {
                    "✅ 通过"
                } else {
                    "❌ 未通过"
                }
            ),
        );

        // ===== 死循环根因裁决(Quality-Check 专用通道) =====
        // 仅当轨迹带 `dead_loop` 诊断时追加一次 QC 调用:把「止损之后换哪条路」
        // 从 `abort_fallback_text` 的固定机械文案,升级成 QC 的结构化裁决
        // (根因枚举 + 可执行换路径 + retryable),并前置并入本单元重试 hint。
        // 未命中死循环的单元零开销。`LAEW_DEAD_LOOP=off` 时整段跳过。
        if let Some(diag) = outcome
            .trace
            .dead_loop
            .as_ref()
            .filter(|_| crate::agent::dead_loop::dead_loop_enabled())
        {
            match quality
                .check_dead_loop(
                    &input.description,
                    &input.expected_output,
                    &outcome.text,
                    diag,
                    &session_id,
                )
                .await
            {
                Ok((resolution, u)) => {
                    acc_usage = add_usage(acc_usage, u);
                    resolution.merge_into(&mut qc);
                    emit_progress(
                        &progress,
                        format!(
                            "[laew] {} 死循环根因裁决: {} → {}",
                            input.id,
                            diag.headline(),
                            truncate_progress_text_default(&qc.suggestion, 180)
                        ),
                    );
                }
                Err(e) => {
                    // fail-open:裁决通道失败不应扩大失败面(单元本就在失败路径上)。
                    tracing::warn!(
                        session = %session_id,
                        wf_id = %input.id,
                        error = %e,
                        "死循环根因裁决通道失败,fail-open:沿用常规 QC 结论"
                    );
                }
            }
        }

        if qc.verdict == Verdict::Fail {
            // ===== Reflection(反思)模式:复盘推理路径,产出跨 attempt 累积的教训 =====
            // 机械基线零 LLM,每 attempt 都合并;深度反思条件触发(只在即将重跑时)。
            // 全关(LAEW_REFLECTION=off)时本块零动作,行为与改造前一致。
            if crate::agent::reflection::reflection_enabled() {
                use crate::agent::reflection as reflect;
                let mechanical = reflect::mechanical_lessons(&outcome.trace);
                lessons_acc = reflect::merge_lessons(
                    &lessons_acc,
                    &reflect::ReflectionReport {
                        lessons: mechanical,
                        ..Default::default()
                    },
                );
                let will_retry = qc.retryable && attempt < retry_budget;
                let deep_allowed = will_retry
                    && reflect::reflection_mode() == reflect::ReflectionMode::Deep
                    && deep_reflections < reflect::reflection_budget();
                if deep_allowed {
                    let focus = if attempt == 0 {
                        "这是第 1 次执行失败、局部重试前的复盘".to_string()
                    } else {
                        format!(
                            "这是第 {} 次局部重试后仍失败、下一次重试前的复盘(此前教训已在重试提示中给出)",
                            attempt
                        )
                    };
                    let reflect_input = reflect::ReflectionInput {
                        goal: &goal,
                        unit_scope: &input.description,
                        expected_output: &input.expected_output,
                        actual_output: &outcome.text,
                        qc_issues: &qc.issues,
                        qc_suggestion: &qc.suggestion,
                        trace: &outcome.trace,
                        focus: &focus,
                    };
                    let reflect_started = std::time::Instant::now();
                    match reflection.reflect(&reflect_input, &session_id).await {
                        Ok((report, reflect_usage)) => {
                            deep_reflections += 1;
                            acc_usage = add_usage(acc_usage, reflect_usage);
                            if let Some(r) = report {
                                let max_score = r.max_dimension_score();
                                let root_cause = r.root_cause.clone();
                                let lesson_count = r.lessons.len();
                                lessons_acc = reflect::merge_lessons(&lessons_acc, &r);
                                last_reflection = Some(r);
                                emit_progress(
                                    &progress,
                                    format!(
                                        "[laew] {} 🔍 Reflection 复盘 | 根因: {} | 教训 {} 条 | 维度峰值 {}",
                                        input.id,
                                        truncate_progress_text_default(&root_cause, 80),
                                        lesson_count,
                                        max_score
                                    ),
                                );
                                crate::agent::decision_audit::record_reflection(
                                    &session_id,
                                    &input.id,
                                    attempt + 1,
                                    true,
                                    "deep",
                                    max_score,
                                    lesson_count,
                                    &root_cause,
                                );
                            } else {
                                emit_progress(
                                    &progress,
                                    format!(
                                        "[laew] {} 🔍 Reflection 复盘(无增量结论,沿用机械教训)",
                                        input.id
                                    ),
                                );
                            }
                        }
                        Err(e) => {
                            // fail-open:反思失败不阻断重试,机械教训仍生效
                            emit_progress(
                                &progress,
                                format!(
                                    "[laew] {} 🔍 Reflection 调用失败(fail-open 落回机械教训): {}",
                                    input.id,
                                    truncate_progress_text_default(&e.to_string(), 80)
                                ),
                            );
                        }
                    }
                    let _ = reflect_started; // 计时保留给未来 stage_durations 扩展
                }
            }

            // 2026-09-16 第 62 轮:QC 失败详情走 [laew] 立即冲刷,即使任务快速
            // 完成也保留 issues / suggestion,便于用户/QC 后续定位。
            if !qc.issues.is_empty() {
                emit_progress(
                    &progress,
                    format!(
                        "[laew] {} QC问题: {}",
                        input.id,
                        truncate_progress_text_default(&qc.issues.join(" | "), 180)
                    ),
                );
            }
            if !qc.suggestion.is_empty() {
                emit_progress(
                    &progress,
                    format!(
                        "[laew] {} QC建议: {}",
                        input.id,
                        truncate_progress_text_default(&qc.suggestion, 180)
                    ),
                );
            }
            // ★第 95 轮:局部重试判定 —— retryable=true 且预算未满 → 注入本单元 QC 结论,
            // 仅重试该单元(attempt+1);否则升级为档位级失败。
            // QC 调用本身失败(LLM 层错)不消耗预算,直接抛(重试大概率同样失败)。
            let reflection_block = crate::agent::reflection::render_reflection_block(
                &lessons_acc,
                last_reflection.as_ref(),
            );
            if qc.retryable && attempt < retry_budget {
                // hint 暂存,下一次循环开头统一覆盖 input(description hint 段原位替换 +
                // retry_count 写入),不在此处覆盖以免与循环头职责重复。
                // Reflection 教训段并入 hint(设计 docs/Reflection反思模式 §6.4):
                // QC issues 说「哪里不达标」,反思教训说「下一轮策略怎么改」。
                unit_retry_hint = format!(
                    "{}\nqc suggestion: {}{}",
                    qc.issues.join("; "),
                    qc.suggestion,
                    reflection_block
                );
                emit_progress(
                    &progress,
                    format!(
                        "[laew] {} 局部重试预算内(第 {}/{} 次尝试) → 注入本单元 QC 结论重试该单元",
                        input.id,
                        attempt + 1,
                        retry_budget + 1
                    ),
                );
                info!(
                    session = %session_id,
                    wf_id = %input.id,
                    attempt = attempt + 1,
                    hint_chars = unit_retry_hint.chars().count(),
                    lessons = lessons_acc.len(),
                    "WorkFlow 单元局部重试"
                );
                attempt += 1;
                continue;
            }
            // 预算耗尽或不可重试 → 升级为档位级失败(旧路径)
            // 反思教训随失败原因上抛,档位级重试 / Yolo 回流同样看得到。
            let reason = if attempt > 0 {
                format!(
                    "wf={}: 单元局部重试 {} 次仍失败: {}{}",
                    input.id,
                    attempt,
                    qc.issues.join("; "),
                    reflection_block
                )
            } else {
                format!("wf={}: {}{}", input.id, qc.issues.join("; "), reflection_block)
            };
            return Err(QualityFailure {
                source: AgentRole::SubAgent,
                reason,
                retryable: qc.retryable,
                suggestion: qc.suggestion,
                cancelled: false,
                trace: Some(Arc::new(outcome.trace)),
                usage: acc_usage,
                // 第 146 轮:同层姊妹单元的完成摘要由 run_wf_units 汇总回填
                //(本函数看不到兄弟单元结果)。
                completed_digest: String::new(),
            });
        }

        // QC 通过但轨迹有失败信号/工具错误率高:机械轻量反思留痕(零 LLM,设计 §4
        // 触发矩阵第二行)。本轮无重试消费者,教训只进审计供 Debug/人工回看与
        // 同任务后续单元对照,不注入任何提示。
        if crate::agent::reflection::reflection_enabled() {
            use crate::agent::reflection as reflect;
            let residual = reflect::mechanical_lessons(&outcome.trace);
            if !residual.is_empty() {
                crate::agent::decision_audit::record_reflection(
                    &session_id,
                    &input.id,
                    attempt + 1,
                    true,
                    "light",
                    0,
                    residual.len(),
                    "qc_pass_with_residual_signals",
                );
            }
        }

        return Ok(WfUnitOk {
            outcome_text: outcome.text,
            usage: acc_usage,
            qc,
            qc_usage,
            trace: outcome.trace,
            exec_role: AgentRole::SubAgent,
            wallclock_ms: acc_sub_elapsed_ms,
            qc_wallclock_ms: acc_qc_elapsed_ms,
        });
    }
}

/// 第 95 轮:把「上一轮本单元失败原因」hint 段覆盖为局部重试版本。
///
/// description 中 hint 段由 build_subflow_input 以固定段头
/// `"\n\n⚠️ 上一轮本单元失败原因"` 拼入;线圈 N 次重试时原位替换该段(基础段即任务名与
/// 步骤完整保留),避免多轮 retry hint 线性叠加撑爆 context。
pub(super) fn apply_retry_hint_overlay(input: &mut SubFlowInput, local_hint: &str, next_attempt: usize) {
    const HINT_MARK: &str = "\n\n⚠️ 上一轮本单元失败原因";
    let base = match input.description.find(HINT_MARK) {
        Some(idx) => input.description[..idx].to_string(),
        None => std::mem::take(&mut input.description),
    };
    // 第 106 轮:提取上一轮失败方法列表,注入到重试 hint,让 LLM 明确看到哪些方法已失败,
    // 避免完全重复上一轮工具调用序列(实证连续 3 轮都完全重复 Bash tesseract 调用)。
    let failed_methods = extract_failed_methods(local_hint);
    let anti_repeat = if failed_methods.is_empty() {
        String::new()
    } else {
        format!(
            "\n❌ 以下方法已失败,禁止重复: {}",
            failed_methods.join(" → ")
        )
    };
    // 第 107 轮:Window/Web 混用是桌面任务的高成本失败根因。QC 同时提到两个工具
    // 时,必须明确“桌面目标不降级为 Web”;纯 Web 任务不受该护栏影响。
    let tool_route_guard = if local_hint.contains("MCP_Window_Use") && local_hint.contains("MCP_Web_LsmAgentBrowser_Use") {
        "\n🚫 桌面目标路由护栏:目标仍是独立桌面软件时,禁止把 MCP_Web_LsmAgentBrowser_Use 作为完成证据;\
         必须根据 MCP_Window_Use 的真实错误修正 app_name/bundle_id/query 后继续。"
    } else {
        ""
    };
    input.description = format!(
        "{}{}\n第 {} 轮局部重试,失败原因,必须改变策略改变策略\n{}\n❌ 禁止完全重复上一轮工具调用序列;必须分析失败根因并调整(更换 action / 改变参数 / 拆细步骤 / 换环境/换账号等)。{}{}",
        base,
        HINT_MARK,
        next_attempt,
        local_hint,
        anti_repeat,
        tool_route_guard
    );
}

/// 第 106 轮:从 QC 提示中提取已失败的方法名列表。
/// 解析 QC suggestion 中的关键方法名(如 tesseract/psm/ddddocr 等),构建禁止重复列表。
fn extract_failed_methods(qc_hint: &str) -> Vec<String> {
    let keywords = ["tesseract", "pytesseract", "ddddocr", "easyocr", "psm", "二值化", "灰度", "base64"];
    keywords
        .iter()
        .filter(|kw| qc_hint.contains(**kw))
        .map(|kw| kw.to_string())
        .collect()
}

/// 上游产物注入截断(第 140 轮):头 1500 + 尾 500 字符,中间省略标注。
/// 与 `quality::clip_qc_input` 同思路,预算更紧(下游 prompt 里 deps 只是背景材料)。
fn clip_dep_output(s: &str) -> String {
    const DEP_CLIP_HEAD: usize = 1500;
    const DEP_CLIP_TAIL: usize = 500;
    let total = s.chars().count();
    if total <= DEP_CLIP_HEAD + DEP_CLIP_TAIL {
        return s.to_string();
    }
    let head: String = s.chars().take(DEP_CLIP_HEAD).collect();
    let tail: String = s.chars().skip(total - DEP_CLIP_TAIL).collect();
    format!(
        "{head}\n…(上游产物中段省略 {} 字符,原文 {total} 字符)…\n{tail}",
        total - DEP_CLIP_HEAD - DEP_CLIP_TAIL
    )
}

pub(super) fn build_subflow_input(
    wf: &WorkFlowSpec,
    dep_outputs: &std::collections::HashMap<String, String>,
    retry_hint: &str,
) -> SubFlowInput {
    // 第 140 轮提速优化:上游产物注入下游 prompt 时截断(头 1500 + 尾 500 字符)。
    // 链式依赖下 wf-N 的输入会携带 wf-1..N-1 全部输出,输入 token 逐单元膨胀
    // (实测 avg_in=23K/次);产物全文本就留在 WorkflowResult 与 artifacts/,下游
    // 需要细节可自行 Read,不靠 prompt 灌全文。
    let deps: Vec<String> = wf
        .depends_on
        .iter()
        .filter_map(|id| dep_outputs.get(id))
        .map(|s| clip_dep_output(s))
        .collect();
    // 2026-09-11 第三十三轮:#P-A 修复 — medium/hard 路径同样需要把用户原始
    // prompt 透传给 SubAgent。由于 MainWork/Plan 拆解时已隐含用户原始需求
    // (WorkFlow.name + steps 是对原始需求的分解),这里把 wf.name 作为原始
    // 提示词的近似透传。如需更精确(传整段原始 prompt),可在 MainWorkRunner
    // 拆分 WorkFlowSpec 时额外携带 original_prompt 字段。
    // (2026-09-18 第 89 轮:跨单元浏览器页面复用不再由编排层注入提示 ——
    //  SubAgentRunner 入口统一探测存活页面并注入「已打开的浏览器页面」列表。)
    // 2026-09-19 第 91 轮 P0-7:retry_hint 真正接入 description 避免子单元重复撞同样墙(连续 3 轮 retry 都撞 max_iter 失败)
    let mut description = format!("{}\n\n步骤:\n{}", wf.name, wf.steps.join("\n"));
    if !retry_hint.trim().is_empty() {
        // 第 146 轮措辞修正:档位级 retry_hint 描述的是**任务级**失败(失败单元未必
        // 是本单元)。旧文案「上一轮本单元失败原因」会让上一轮已通过的单元误以为
        // 自己失败而换策略重做(实测 v2 轮 wf-1/wf-2 被误导后把已验证页面又遍历一遍)。
        description.push_str(&format!(
            "\n\n⚠️ 上一轮**任务**失败原因(失败单元未必是本单元;若与你的职责无关,按原步骤执行即可,勿重做已完成的工作):\n{retry_hint}\n若本单元正是失败单元:❌ 禁止完全重复上一轮工具调用序列;必须分析失败根因并调整(更换 action / 改变参数 / 拆细步骤 / 换环境/换账号等)。"
        ));
    }
    // 第 119 轮:pre_explore 标记为 true 时, 在 description 内附加「批量优先」硬约束;
    // 与 SubAgentRunner 注入的 system prompt hint 形成双保险(description 进 user 消息,
    // system hint 进 system 字段, 两者都在 LLM 上下文中)。
    if wf.pre_explore {
        // 第 178 轮:批量优先提示按**用户强制的浏览器工具**分流。
        // 旧实现无条件写死 MCP_Web_LsmAgentBrowser_Use,在「全程只用
        // MCP_Web_Chrome_Use」的任务里持续诱导模型换工具(F1 回归)。
        // 判定依据优先取用户原始提示词(不含编排层自造的步骤文本)。
        let mandatory = wf.original_prompt.as_deref().unwrap_or("");
        let chrome_mandated = mandated_tool(mandatory, "MCP_Web_Chrome_Use");
        let lsm_mandated = mandated_tool(mandatory, "MCP_Web_LsmAgentBrowser_Use");
        if chrome_mandated && !lsm_mandated {
            description.push_str(
                "\n\n⚡ 连续优先(本单元硬约束,仅适用于 MCP_Web_Chrome_Use):\n\
                 ① 一次 `eval_js` 读取多个输入框/按钮/可见文本,别链式单步 inspect;\n\
                 ② 纯文字按钮用 `click_text(params.text=\"选为终点\")`,不要反复猜 CSS 选择器;\n\
                 ③ 联想/回车/快捷键用 `key_press(params.key=\"Enter\"/\"ArrowDown\"/\"Tab\")`;\n\
                 ④ 截图一律 `screenshot(params.save_path=...)`;\n\
                 ✅ 单次任务的 LLM round-trip 应力争 ≤ 10 次。",
            );
        } else if lsm_mandated && !chrome_mandated {
            description.push_str(
                "\n\n⚡ 批量优先(本单元硬约束,仅适用于 MCP_Web_LsmAgentBrowser_Use):\n\
                 ① 第一步用 MCP_Web_LsmAgentBrowser_Use(action=explore, page_id=<页面 id>, queries=[\n\
                 {\"info\":\"elements\"}, {\"info\":\"dom\",\"params\":{\"selector\":\"form\"}}, {\"info\":\"screenshot\"}\n\
                 ]) 一次收集页面状态;\n\
                 ② 结构清晰后用 MCP_Web_LsmAgentBrowser_Use(action=batch, steps=[input_text × N → click → wait → inspect 验证])\n\
                 一次提交全部写操作 + 验证;\n\
                 ❌ 禁止链式单步(inspect → click → inspect → screenshot → ...);\n\
                 ✅ 单次任务的 LLM round-trip 应 ≤ 6 次。",
            );
        }
        // 未声明强制工具时不注入任何工具专属批量范式,避免把选用决策写死。
    }
    // 用户原文的显式顺序约束必须传到执行单元;不能只靠上游摘要或规划文档自觉保留。
    if crate::agent::task_prerequisites::requires_login_first(
        wf.original_prompt.as_deref().unwrap_or(""),
    ) {
        description.push_str("\n\n");
        description.push_str(crate::agent::task_prerequisites::LOGIN_FIRST_CONSTRAINT);
    }
    // 第 181 轮(2026-10-27 京东实测):任务级浏览器工具约束传导。
    // 实测任务原文「分别使用 Chrome 浏览器,打开京东」不含 mandated_tool 认定的
    // 「只使用/必须使用」句式,执行单元完全不知道任务指定了 Chrome——wf-2 与
    // layer 3 三个并行单元系统性双开 MCP_Web_LsmAgentBrowser_Use(Lsm 侧被京东
    // 风控拦成「京东验证」,每单元浪费 3~6 轮;还意外拉起有头查看器触发 SIGPIPE
    // 进程死亡,见 crash::ignore_sigpipe 注释)。这里对**所有单元**(不限
    // pre_explore)注入用户指定的浏览器工具硬约束。
    if let Some(mandate) = browser_mandate(wf.original_prompt.as_deref().unwrap_or("")) {
        match mandate {
            "chrome" => {
                description.push_str(
                    "\n\n🔒 任务级浏览器工具约束(用户任务原文指定了 Chrome 浏览器,本单元硬约束):\n\
                     本单元全部网页操作必须使用 MCP_Web_Chrome_Use;禁止改用 MCP_Web_LsmAgentBrowser_Use\n\
                     (它是另一个独立内核:会话/登录态不共享,同一站点会被视作两个来源,放大风控风险)。\n\
                     唯一例外:Chrome 反复被站点风控拦截(验证码循环/整页抹除/访问被拒)且本单元已无法推进时,\n\
                     可在产出中如实说明后改用另一工具,并说明理由。",
                );
            }
            "lsm" => {
                description.push_str(
                    "\n\n🔒 任务级浏览器工具约束(用户任务原文指定了 LsmAgentBrowser 内核,本单元硬约束):\n\
                     本单元全部网页操作必须使用 MCP_Web_LsmAgentBrowser_Use;禁止改用 MCP_Web_Chrome_Use\n\
                     (用户选择自研内核通常有反-反爬/离线等明确理由)。\n\
                     唯一例外:目标站点依赖 Lsm 内核尚不支持的能力(WebGL 地图底图等)且本单元已无法推进时,\n\
                     可在产出中如实说明后改用另一工具,并说明理由。",
                );
            }
            _ => {}
        }
    }
    // 2026-09-19 第 91 轮 P0-8:wf.original_prompt 透传整段用户原始 prompt(兜底回退 wf.name)
    let original_prompt = wf.original_prompt.clone().or_else(|| Some(wf.name.clone()));
    SubFlowInput {
        id: format!("{}.step", wf.id),
        description,
        expected_output: wf.acceptance.join("; "),
        original_prompt,
        depends_on_outputs: deps,
        sibling_outputs: vec![],
        pending_agent_messages: vec![],
        // 2026-09-17 第 75 轮:把 WorkFlow 期望的角色写入 SubFlowInput,
        // Runner 在 trace.intended_role 落地,供 collect_failure_signals 计算
        // delegate_mismatch 弱信号。
        intended_role: Some(wf.delegate_to),
        retry_count: 0,
        retry_hint: retry_hint.to_string(),
        max_iterations: wf.max_iterations,
        pre_explore: wf.pre_explore,
    }
}

/// 判断用户提示词是否把某个工具定为**强制**工具。
///
/// 不能只看「工具名出现过」:提示词常同时写「只使用 A(不得使用 B)」,
/// 两个工具名都在文中。这里只认「只使用/仅使用/只用/全程使用/必须使用」
/// 之后 120 字符内出现的工具名。
pub(super) fn mandated_tool(prompt: &str, tool: &str) -> bool {
    const KWS: &[&str] = &["只使用", "仅使用", "只用", "全程使用", "必须使用"];
    KWS.iter().any(|kw| {
        prompt.match_indices(kw).any(|(i, _)| {
            let tail: String = prompt[i..].chars().take(120).collect();
            // 只看到同一子句结束为止:「只使用 A(不得使用 B)」里的 B 不算强制。
            let clause = tail
                .split(|c: char| matches!(c, ';' | '。' | '(' | '（' | '\n' | '；'))
                .next()
                .unwrap_or("");
            clause.contains(tool)
        })
    })
}

/// 检测用户提示词是否指定了某个浏览器内核(第 181 轮)。
///
/// 与 [`mandated_tool`] 的区别:那套关键词(只使用/必须使用…)要求句式精确,
/// 实测任务原文「分别使用 Chrome 浏览器」不命中,执行单元拿不到任何工具约束。
/// 这里做**正向存在性检测**——提示词里点名了某个浏览器本身就是强信号
/// (注入的也只是正向指引+例外出口,不是硬禁令):
/// - 只出现 Chrome 系词 → `"chrome"`(注入「全程用 MCP_Web_Chrome_Use」);
/// - 只出现 Lsm 系词 → `"lsm"`;
/// - 两者都出现或都不出现 → `None`(不注入,选用决策留给模型)。
pub(super) fn browser_mandate(prompt: &str) -> Option<&'static str> {
    crate::agent::task_prerequisites::browser_mandate(prompt)
}

// ===================== 第 146 轮:完成单元摘要(断点续跑) =====================

/// 摘要总长上限(字符)—— 注入 Plan/Main-Work 提示词须保持紧凑。
const COMPLETED_DIGEST_MAX_CHARS: usize = 1400;
/// 每单元产物要点截断(字符)。
const DIGEST_OUTCOME_CLIP: usize = 150;

/// 生成「已完成且 QC 通过单元」摘要(每单元一行),供档位级重试/Hard 重规划
/// 注入 Plan/Main-Work,实现**只规划剩余部分**而非从零重跑。
///
/// `results` = 本轮此前各层已完成单元;`current` = 失败层中通过的姊妹单元。
/// 末行附工具层访问台账的覆盖度口径,重规划可直接对账「哪些页面已经去过」。
pub(super) fn build_completed_digest<'a>(
    results: &[WorkflowResult],
    current: impl Iterator<Item = (String, String, &'a str)>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for r in results {
        let line = format!(
            "- {}({}): QC=pass, 产物要点: {}",
            r.id,
            r.name,
            clip_chars(&r.subflow_outcome, DIGEST_OUTCOME_CLIP),
        );
        out.push_str(&line);
        out.push('\n');
    }
    for (id, name, outcome) in current {
        let line = format!(
            "- {}({}): QC=pass, 产物要点: {}",
            id,
            name,
            clip_chars(outcome, DIGEST_OUTCOME_CLIP),
        );
        out.push_str(&line);
        out.push('\n');
    }
    if out.is_empty() {
        return String::new();
    }
    // 末行附访问台账覆盖度口径(第 146 轮):重规划时页面级「去过哪」直接可对账。
    let cov = crate::agent::tools::mcp_web_lsm_browser_use::visit_ledger::coverage_payload();
    let s = &cov["summary"];
    let _ = writeln!(
        out,
        "(工具层访问台账: distinct_pages={}, total_visits={};重规划前可经 inspect(info=coverage) 取完整清单)",
        s["distinct_pages"], s["total_visits"],
    );
    clip_chars(&out, COMPLETED_DIGEST_MAX_CHARS)
}

/// 按字符裁剪(保头,尾加省略标注)。`crate::logging::clip` 是日志字段用的
/// 字节口径,这里提示词注入用字符口径防中文截断 panic。
fn clip_chars(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let head: String = chars[..max.saturating_sub(1)].iter().collect();
    format!("{head}…(截断)")
}

/// 合并两份完成单元摘要(第 146 轮):按单元 id 去重,同 id 保留**最近一轮**
/// (重跑过的单元以最新产物为准);台账 footer 行只保留最新一条;总量封顶 2600 字符。
pub(super) fn merge_completed_digest(acc: &mut String, incoming: &str) {
    const MERGE_MAX_CHARS: usize = 2600;
    let unit_id_of = |line: &str| -> Option<String> {
        let body = line.strip_prefix("- ")?;
        Some(
            body.split(['(', ':', ' '])
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        )
        .filter(|id| !id.is_empty())
    };
    let mut unit_lines: Vec<String> = Vec::new();
    let mut footer = String::new();
    // 先放存量,再用 incoming 覆盖同 id 行。
    for src in [acc.as_str(), incoming] {
        for line in src.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('(') {
                footer = line.to_string();
                continue;
            }
            if let Some(id) = unit_id_of(line) {
                unit_lines.retain(|existing| unit_id_of(existing).as_deref() != Some(id.as_str()));
            }
            unit_lines.push(line.to_string());
        }
    }
    let mut merged = unit_lines.join("\n");
    if !footer.is_empty() {
        merged.push('\n');
        merged.push_str(&footer);
    }
    *acc = if merged.chars().count() > MERGE_MAX_CHARS {
        format!("{}…(截断)", merged.chars().take(MERGE_MAX_CHARS).collect::<String>())
    } else {
        merged
    };
}

/// `browser_mandate` 词形矩阵(第 181 轮):京东实测原文必须命中 chrome。
#[cfg(test)]
mod browser_mandate_tests {
    use super::*;

    #[test]
    fn 京东实测原文命中_chrome() {
        // 2026-10-27 京东实测任务原文节选:不含「只使用/必须使用」句式,
        // mandated_tool 判不中,但 browser_mandate 必须命中。
        let prompt = "### 组装一个 1W 块钱左右的台式机\n1. 分别使用 Chrome 浏览器,打开 京东 网站,必须先登录。\n2. 找到搜素工具栏";
        assert_eq!(browser_mandate(prompt), Some("chrome"));
        assert!(!mandated_tool(prompt, "MCP_Web_Chrome_Use"), "旧判定不应命中(回归对照)");
    }

    #[test]
    fn 常见词形与大小写() {
        assert_eq!(browser_mandate("用 chrome 打开京东"), Some("chrome"));
        assert_eq!(browser_mandate("使用谷歌浏览器查询"), Some("chrome"));
        assert_eq!(browser_mandate("用 Chromium 内核"), Some("chrome"));
        assert_eq!(browser_mandate("改用 MCP_Web_LsmAgentBrowser_Use 重试"), Some("lsm"));
        assert_eq!(browser_mandate("用自研内核反反爬"), Some("lsm"));
    }

    #[test]
    fn 双指定与无指定不注入() {
        // 两个内核都被点名(对比类任务)或都没点名:不注入,选用决策留给模型
        assert_eq!(browser_mandate("分别用 Chrome 与 LsmAgentBrowser 对比渲染"), None);
        assert_eq!(browser_mandate("打开京东收集价格"), None);
        assert_eq!(browser_mandate(""), None);
    }

    #[test]
    fn 显式登录前置传入执行单元() {
        let wf = WorkFlowSpec {
            id: "wf-2".into(),
            name: "采集商品".into(),
            steps: vec!["搜索显卡".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec!["wf-1".into()],
            acceptance: vec!["记录价格".into()],
            delegate_to: AgentRole::SubAgent,
            max_iterations: None,
            original_prompt: Some("使用 Chrome 打开京东,必须先登录,需要人工介入;登录后搜索商品".into()),
            pre_explore: false,
        };
        let input = build_subflow_input(&wf, &Default::default(), "");
        assert!(input.description.contains("用户显式登录前置"));
        assert!(input.description.contains("request_human"));
        assert!(input.description.contains("不得搜索、遍历、访问评论/API"));
    }
}

#[cfg(test)]
mod completed_digest_tests {
    use super::*;

    fn wf_result(id: &str, name: &str, outcome: &str) -> WorkflowResult {
        WorkflowResult {
            id: id.into(),
            name: name.into(),
            subflow_outcome: outcome.into(),
            quality_report: crate::agent::quality::QualityReport {
                verdict: crate::agent::quality::Verdict::Pass,
                source: AgentRole::QualityCheck,
                issues: vec![],
                notes: vec![],
                suggestion: String::new(),
                retryable: true,
                evidence: String::new(),
            },
            usage: Usage::default(),
            subflow_trace: None,
            exec_role: AgentRole::SubAgent,
            wallclock_ms: 0,
            qc_wallclock_ms: 0,
        }
    }

    #[test]
    fn digest_lists_completed_units_with_coverage_footer() {
        let results = vec![
            wf_result("wf-1", "登录", "已登录,page_id=p_x"),
            wf_result("wf-2", "建档", "9 页菜单清单已落盘 evidence/inventory.md"),
        ];
        let digest = build_completed_digest(&results, std::iter::empty());
        assert!(digest.contains("- wf-1(登录): QC=pass"), "{digest}");
        assert!(digest.contains("distinct_pages="), "应带台账 footer: {digest}");
    }

    #[test]
    fn digest_empty_when_nothing_completed() {
        assert!(build_completed_digest(&[], std::iter::empty()).is_empty());
    }

    #[test]
    fn merge_dedupes_by_unit_id_keeping_latest() {
        let mut acc = String::from("- wf-1(登录): QC=pass, 产物要点: 旧结论\n");
        merge_completed_digest(
            &mut acc,
            "- wf-1(登录): QC=pass, 产物要点: 新结论\n- wf-2(建档): QC=pass, 产物要点: 清单\n(工具层访问台账: distinct_pages=9, total_visits=20;重规划前可经 inspect(info=coverage) 取完整清单)\n",
        );
        assert!(acc.matches("wf-1").count() == 1, "同 id 应去重: {acc}");
        assert!(acc.contains("新结论"), "同 id 保留最新: {acc}");
        assert!(acc.contains("wf-2"), "新增单元保留: {acc}");
        assert!(acc.contains("distinct_pages=9"), "footer 保留: {acc}");
    }
}
