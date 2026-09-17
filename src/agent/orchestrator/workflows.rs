//! WorkFlow 执行单元(2026-09-17 自 orchestrator.rs 拆分)。
//!
//! `execute_workflows` 按 `depends_on` Kahn 分层、同层 SubAgent 并行;
//! `run_wf_unit` 是单个执行单元(SubAgent / WindowUse / WebUse 委派路由 + QC +
//! Debug 采集),串行直通与 tokio::spawn 并行两种调用路径共用。

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
        let layers = main_work::topo_layers(&plan.workflows).map_err(|e| QualityFailure {
            source: AgentRole::MainWork,
            reason: format!("拓扑分层失败: {e}"),
            retryable: false,
            suggestion: "Plan 中存在循环或未知依赖".into(),
            cancelled: false,
            trace: None,
            usage: Usage::default(),
        })?;

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
            let layer_outcomes = if units.len() == 1 {
                let (wf, input) = units.into_iter().next().expect("len==1");
                let outcome = run_wf_unit(
                    self.sub_agent.clone(),
                    self.window_use.clone(),
                    self.web_use.clone(),
                    self.quality.clone(),
                    self.cfg.debug.clone(),
                    wf.delegate_to,
                    input,
                    c.goal_summary.clone(),
                    session.id().to_string(),
                    None,
                    cancel.clone(),
                    progress.clone(),
                )
                .await;
                vec![(wf, outcome)]
            } else {
                let semaphore =
                    Arc::new(tokio::sync::Semaphore::new(self.cfg.max_parallel_workflows));
                let mut handles = Vec::with_capacity(units.len());
                for (wf, input) in units {
                    let sub_agent = self.sub_agent.clone();
                    let window_use = self.window_use.clone();
                    let web_use = self.web_use.clone();
                    let quality = self.quality.clone();
                    let debug = self.cfg.debug.clone();
                    let delegate_to = wf.delegate_to;
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
                            window_use,
                            web_use,
                            quality,
                            debug,
                            delegate_to,
                            input,
                            goal,
                            sid,
                            Some(sem),
                            cancel_tok,
                            progress_tx,
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
                                },
                                Err(QualityFailure {
                                    source: AgentRole::SubAgent,
                                    reason: format!("并行 WorkFlow 任务 join 失败: {e}"),
                                    retryable: true,
                                    suggestion: "重试".into(),
                                    cancelled: false,
                                    trace: None,
                                    usage: Usage::default(),
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
            if let Some(f) = first_failure {
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

/// 单个 WorkFlow 执行单元的成功产物
struct WfUnitOk {
    outcome_text: String,
    /// SubAgent 自身的 LLM 用量(2026-09-09 第 14 轮:Quality 用量单独累加,见 `qc_usage`)。
    usage: Usage,
    qc: QualityReport,
    /// Quality-Check 调用的 LLM 用量(2026-09-09 第 14 轮:从 check_subagent 返回值中带回)。
    qc_usage: Usage,
    /// SubAgent 执行轨迹(2026-09-09 第 05 轮)。
    trace: ExecutionTrace,
    /// 2026-09-16 第 57 轮:执行器角色(SubAgent / WindowUse)
    exec_role: AgentRole,
    /// 2026-09-16 第 57 轮:SubAgent/WindowUse 墙钟耗时(毫秒)
    wallclock_ms: u64,
    /// 2026-09-16 第 57 轮:QC LLM 调用单独耗时(毫秒)
    qc_wallclock_ms: u64,
}

/// 执行一个 WorkFlow 单元:SubAgent 执行 + Quality-Check(+ Debug 采集)。
///
/// 按 `delegate_to` 路由执行 Runner(2026-09-14 第 9 角色 WindowUse):
/// `SubAgent` → SubAgent-Work;`WindowUse` → WindowUse(桌面窗口操控专项);
/// QC / Debug / 取消 / 进度通道两种委派完全复用。
///
/// 自由函数 + Arc 参数化,串行直通与 tokio::spawn 并行两种调用路径共用同一份逻辑;
/// `semaphore` 为并行路径的有界并发许可(串行路径传 None);
/// `cancel` 为任务级取消 token(传播进 SubAgent 的 Agent 循环,LLM/工具即时中断);
/// `progress` 为阶段进度通道 clone(并行单元各自持有,2026-09-10 第 23 轮)。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_wf_unit(
    sub_agent: Arc<SubAgentRunner>,
    window_use: Arc<WindowUseRunner>,
    web_use: Arc<WebUseRunner>,
    quality: Arc<QualityRunner>,
    debug: Option<Arc<DebugCollector>>,
    delegate_to: AgentRole,
    input: SubFlowInput,
    goal: String,
    session_id: String,
    semaphore: Option<Arc<tokio::sync::Semaphore>>,
    cancel: CancelToken,
    progress: Option<ProgressTx>,
) -> std::result::Result<WfUnitOk, QualityFailure> {
    // 有界并发:先抢许可(对齐 atomcode Semaphore(3) FIFO 惯例)
    let _permit = match &semaphore {
        Some(sem) => Some(sem.acquire().await.map_err(|e| QualityFailure {
            source: AgentRole::SubAgent,
            reason: format!("并行调度信号量已关闭: {e}"),
            retryable: true,
            suggestion: "重试".into(),
            cancelled: false,
            trace: None,
            usage: Usage::default(),
        })?),
        None => None,
    };

    let wf_id = input.id.clone();
    let exec_role = match delegate_to {
        AgentRole::WindowUse => AgentRole::WindowUse,
        AgentRole::WebUse => AgentRole::WebUse,
        _ => AgentRole::SubAgent,
    };
    let exec_label = match exec_role {
        AgentRole::WindowUse => "WindowUse",
        AgentRole::WebUse => "WebUse",
        _ => "SubAgent",
    };
    // 2026-09-16 第 62 轮:拆分 stage 短标题 / 详情面板。
    // 短标题(≤80 字符)进入 TUI stage 流 + waiting 心跳,避免每 1s
    // 原地重写整段超长文本;详情面板走 [laew] 前缀,立即冲刷、不进
    // waiting 心跳,任务快速完成时也保留。
    emit_progress(
        &progress,
        format!("{wf_id} {exec_label} 执行中"),
    );
    emit_progress(
        &progress,
        format!(
            "[laew] {wf_id} {exec_label} 详情 | 职责: {} | 期望: {}",
            truncate_progress_text(&input.description, 80),
            truncate_progress_text(&input.expected_output, 80)
        ),
    );
    // 2026-09-16 第 57 轮:SubAgent/WindowUse 墙钟计时 ——
    // 用于 TaskResult.wallclock_ms / TUI 时间线展示。
    let sub_started = std::time::Instant::now();
    let outcome = match exec_role {
        AgentRole::WindowUse => {
            window_use
                .run_unit_with_cancel(&input, &session_id, &cancel)
                .await
        }
        AgentRole::WebUse => {
            web_use
                .run_unit_with_cancel(&input, &session_id, &cancel)
                .await
        }
        _ => {
            sub_agent
                .run_unit_with_cancel(&input, &session_id, &cancel)
                .await
        }
    }
    .map_err(|e| {
        QualityFailure::from_agent_error(
            exec_role,
            &format!("{exec_label} 执行失败(wf={wf_id})"),
            &e,
        )
    })?;
    let wallclock_ms = sub_started.elapsed().as_millis() as u64;
    // 2026-09-16 第 62 轮:单元结束走 [laew] 详情面板 + 短标题 stage。
    // 详情面板显示 iter / tools / early_term / 错误摘要,即使 LLM 没调
    // 工具也立即可见,避免用户看到「卡住 100 秒没动作」。
    let summary_line = format!(
        "{wf_id} {exec_label} 完成 | {:.1}s",
        wallclock_ms as f64 / 1000.0
    );
    emit_progress(&progress, summary_line);
    let detail_summary = format!(
        "[laew] {wf_id} {exec_label} 执行证据 | iter={} tools={}({}成功/{}失败) early_term={} reason={}",
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
    // 「窗口操控到底调了哪些工具、参数是什么、为什么失败」。
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
                let args_part = tool_args_digest(&entry.args_json);
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
                "[laew] {wf_id} {exec_label} 工具调用(最近 {} 条): {}",
                recent.len(),
                recent.join(" | ")
            ),
        );
    }
    if !outcome.trace.failure_signals.is_empty() {
        emit_progress(
            &progress,
            format!(
                "[laew] {wf_id} {exec_label} 失败信号: {}",
                truncate_progress_text_default(&outcome.trace.failure_signals.join(","), 180)
            ),
        );
    }

    // 2026-09-16 第 57 轮:QC LLM 调用单独计时。
    let qc_started = std::time::Instant::now();
    let (qc, qc_usage) = quality
        .check_subagent_with_source(
            exec_role,
            &goal,
            &input.description,
            &input.expected_output,
            &outcome.text,
            &outcome.trace,
            &session_id,
        )
        .await
        .map_err(|e| {
            QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
        })?;
    let qc_wallclock_ms = qc_started.elapsed().as_millis() as u64;
    if let Some(d) = &debug {
        d.record_quality(&qc);
    }
    emit_progress(
        &progress,
        format!(
            "{wf_id} QC:{}",
            if qc.verdict == Verdict::Pass {
                "✅ 通过"
            } else {
                "❌ 未通过"
            }
        ),
    );

    if qc.verdict == Verdict::Fail {
        // 2026-09-16 第 62 轮:QC 失败详情走 [laew] 立即冲刷,即使任务快速
        // 完成也保留 issues / suggestion,便于用户/QC 后续定位。
        if !qc.issues.is_empty() {
            emit_progress(
                &progress,
                format!(
                    "[laew] {wf_id} QC问题: {}",
                    truncate_progress_text_default(&qc.issues.join(" | "), 180)
                ),
            );
        }
        if !qc.suggestion.is_empty() {
            emit_progress(
                &progress,
                format!(
                    "[laew] {wf_id} QC建议: {}",
                    truncate_progress_text_default(&qc.suggestion, 180)
                ),
            );
        }
        return Err(QualityFailure {
            source: exec_role,
            reason: format!("wf={}: {}", wf_id, qc.issues.join("; ")),
            retryable: qc.retryable,
            suggestion: qc.suggestion,
            cancelled: false,
            trace: Some(Arc::new(outcome.trace)),
            usage: add_usage(outcome.usage, qc_usage),
        });
    }

    Ok(WfUnitOk {
        outcome_text: outcome.text,
        usage: outcome.usage,
        qc,
        qc_usage,
        trace: outcome.trace,
        exec_role,
        wallclock_ms,
        qc_wallclock_ms,
    })
}

pub(super) fn build_subflow_input(
    wf: &WorkFlowSpec,
    dep_outputs: &std::collections::HashMap<String, String>,
    retry_hint: &str,
) -> SubFlowInput {
    let deps: Vec<String> = wf
        .depends_on
        .iter()
        .filter_map(|id| dep_outputs.get(id).cloned())
        .collect();
    // 2026-09-11 第三十三轮:#P-A 修复 — medium/hard 路径同样需要把用户原始
    // prompt 透传给 SubAgent。由于 MainWork/Plan 拆解时已隐含用户原始需求
    // (WorkFlow.name + steps 是对原始需求的分解),这里把 wf.name 作为原始
    // 提示词的近似透传。如需更精确(传整段原始 prompt),可在 MainWorkRunner
    // 拆分 WorkFlowSpec 时额外携带 original_prompt 字段。
    let mut description = format!("{}\n\n步骤:\n{}", wf.name, wf.steps.join("\n"));
    // 2026-09-16 第 64 轮:WebUse 跨单元 page_id 复用提示。
    if wf.delegate_to == AgentRole::WebUse {
        description.push_str(
            "\n\n【WebUse 跨单元上下文】\n\
             - 第一个 WebUse 单元的 BrowserNew 返回 page_id(如 p_xxx);\n\
             - 后续 WebUse 单元必须复用同一 page_id(WebUseRunner 自动注入到 user prompt 尾部);\n\
             - 若 BrowserList 显示 page_id 已失效,重新 BrowserNew,新 page_id 替换 Runner 内 last_page_id。",
        );
    }
    SubFlowInput {
        id: format!("{}.step", wf.id),
        description,
        expected_output: wf.acceptance.join("; "),
        original_prompt: Some(wf.name.clone()),
        depends_on_outputs: deps,
        sibling_outputs: vec![],
        window_context: None,
        pending_agent_messages: vec![],
    }
}
