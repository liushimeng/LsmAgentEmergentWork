//! 编排主管线(2026-09-17 自 orchestrator.rs 拆分)。
//!
//! `handle_inner` 主流程 + 取消检查 + 原始 prompt 透传 + 三档执行链路
//! (`run_simple` / `run_medium` / `run_hard`),是编排器的核心路径。

use super::*;

impl MultiAgentOrchestrator {
    pub(super) async fn handle_inner(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
    ) -> Result<OrchestrationOutcome> {
        // 2026-09-16 第 57 轮:任务级墙钟锚点(所有 StageDuration.started_offset_ms
        // / wallclock_ms 都相对这一刻)。
        let task_started = std::time::Instant::now();
        let mut stage_durations: Vec<StageDuration> = Vec::new();
        let mut retry_log: Vec<RetryRecord> = Vec::new();

        // 任务开始(2026-09-17 第 69 轮运行日志):感知输入 —— 用户原始 prompt
        // (注入项目上下文/历史摘要之前的原始形态)入日志。
        {
            let prompt_desc = Self::original_user_prompt(session)
                .unwrap_or_else(|| "(无用户文本输入)".to_string());
            info!(
                session = %session.id(),
                messages = session.context().len(),
                prompt = %crate::logging::clip(&prompt_desc),
                "任务开始(感知输入)"
            );
        }

        // 0) 项目上下文首次注入(幂等)
        if let Some(work_dir) = project_context::current_work_dir() {
            project_context::inject_once(session, work_dir);
        }

        // 0.1) 历史 Session 摘要注入(幂等)
        let summaries = self
            .db
            .latest_summaries(session.id(), self.cfg.history_limit)
            .unwrap_or_default();
        inject_history_with_entries(session, &summaries);

        // 0.2) Context 自动压缩(达到当前 Provider context_max_size 的 80% 阈值时触发)
        if let Ok(Some(active)) = self.db.get_active_or_env() {
            match self
                .compact
                .maybe_compact(session, active.context_max_size)
                .await
            {
                Ok(Some(rep)) => {
                    // F3 同族(2026-09-14 第 51 轮):经 progress 通道输出,
                    // 避免 TUI 下与 waiting 行同行粘连;非 TTY 仍落 stderr。
                    emit_progress(
                        progress,
                        format!(
                            "[laew] Context 已自动压缩:档位={} 估算 token {} → {}(覆盖 {} 条历史消息{})",
                            rep.tier.as_str(),
                            rep.before_tokens,
                            rep.after_tokens,
                            rep.compacted_messages,
                            if rep.fallback { ",硬截断降级" } else { "" },
                        ),
                    );
                    // D9-8 决策审计(2026-09-19):Compact 压缩决策写入审计 JSONL。
                    crate::agent::decision_audit::record_compact(
                        session.id(),
                        rep.tier.as_str(),
                        rep.before_tokens,
                        rep.after_tokens,
                        rep.compacted_messages,
                        rep.fallback,
                        0,
                    );
                }
                Ok(None) => {}
                Err(e) if matches!(e, AgentError::Cancelled) => return Err(e),
                Err(e) => tracing::warn!(error = %e, "Context 自动压缩失败(不中断任务)"),
            }
        }

        // 1) Yolo 入口
        let yolo_started = std::time::Instant::now();
        let (mut classification, yolo_usage) = self.run_yolo_classification(session).await?;
        Self::check_cancelled(cancel)?;
        let yolo_elapsed_ms = yolo_started.elapsed().as_millis() as u64;
        stage_durations.push(StageDuration {
            stage: "yolo".to_string(),
            wf_id: None,
            started_offset_ms: 0,
            elapsed_ms: yolo_elapsed_ms,
        });
        self.dbg_classify(&classification);
        // Yolo 分类(2026-09-17 第 69 轮运行日志):决策事件 —— 三步分析
        // (目的/目标/意图)+ 档位 + 委派建议 + 直答有无,全量入日志。
        info!(
            session = %session.id(),
            task_level = %classification.task_level.display_name(),
            purpose = %crate::logging::clip(&classification.purpose),
            goal = %crate::logging::clip(&classification.goal_summary),
            intent = %crate::logging::clip(&classification.intent),
            delegate = classification.suggested_delegate.as_deref().unwrap_or(""),
            direct_answer = classification.direct_answer.is_some(),
            decomposition = classification.decomposition_plan.len(),
            degraded_parse = classification.yolo_degraded,
            "Yolo 分类(决策)"
        );
        // D9-8 决策审计(2026-09-19):Yolo 分类决策写入审计 JSONL。
        crate::agent::decision_audit::record_classify(
            session.id(),
            classification.task_level.as_str(),
            &classification.purpose,
            &classification.goal_summary,
            classification.suggested_delegate.as_deref(),
            classification.yolo_degraded,
            yolo_elapsed_ms,
            yolo_usage,
        );
        // 2026-09-11 第三十三轮:#P-C 修复 — 此处只能确定档位,无法确定 simple
        // 是否走 direct_answer 短路(短路判断在 stage 之后)。文案采用「预期路径」:
        // - simple 期望委派 SubAgent → "SubAgent 委派执行"
        // - medium/hard 期望委派 MainWork/Plan → 拆解/规划
        // direct_answer 短路时,在下面的分支再额外输出 "[trace] subagent=skipped
        // (direct_answer=true)" 行,让用户看到真实执行链路。
        emit_progress(
            progress,
            format!(
                "Yolo 分类:{} → {}{}",
                classification.task_level.display_name(),
                match classification.task_level {
                    TaskLevel::Simple => "SubAgent 委派执行",
                    TaskLevel::Medium => "Main-Work 拆解",
                    TaskLevel::Hard => "Plan 规划",
                },
                if classification.yolo_degraded {
                    "(降级本地解析)"
                } else {
                    ""
                },
            ),
        );
        // 2026-09-09 第 14 轮:累加 Yolo 分类调用的 LLM 用量(此前被 `let _ = usage;` 显式丢弃,
        // 导致 stdout 用量仅显示最后一次调用,与 Debug 报告严重不一致)。
        let mut total_usage = yolo_usage;

        // 1.1) 记录 Yolo 输入事件
        let _ = self
            .db
            .insert_session_memory(&crate::config::SessionMemoryEntry {
                session_id: session.id().to_string(),
                role: AgentRole::Yolo,
                event_type: EventType::Input,
                content: format!("goal: {}", classification.goal_summary),
                usage_input: 0,
                usage_output: 0,
            });

        let mut retry_count = 0;
        // F3(2026-09-10 第 25 轮):上一轮失败原因,重试轮回灌 Main-Work(消除盲重试);
        // 档位升级回流 Yolo 后清空。同时作为 Failed 变体的 reason(F4)。
        let mut retry_hint = String::new();
        // 2026-09-16 第 58 轮 P0-C:跨轮透传最近一次失败的 ExecutionTrace,
        // 供 Failed outcome 复用 format_task_result 渲染 [trace] [tool] [failure] 段。
        let mut last_failure_trace: Option<Arc<ExecutionTrace>> = None;
        // 2026-09-16 第 67 轮:medium 档计划复用缓存(效率优化)。
        // 执行层失败(wf 单元 QC 拒)→ 计划本身没问题 → 下一轮跳过 Main-Work LLM 重拆,
        // 失败反馈直接注入各执行单元;计划级失败(解析/校验)→ 清缓存强制重拆。
        // 实测省 1-2 次 Main-Work 调用(30-120s/任务)。
        let mut medium_plan_cache: Option<WorkFlowPlan> = None;

        // 1.2) simple + direct_answer 短路(2026-09-09 第 15 轮 AQ03 实测发现):
        // Yolo 已给出完整直接答案时,不再空转一轮 SubAgent+QC(实测多花 ~2.5 分钟
        // 且引入额外失败面)。本分支接通 OrchestrationOutcome::DirectAnswer ——
        // 该变体与 main.rs/tui 消费端早已存在,但此前无任何构造点(死代码),
        // 旧兼容 API `yolo::run_yolo` 的 DirectAnswer 语义在此对齐到主编排链路。
        // 仍跑 SessionContext 收口(workflows 为空),保住 session_memory 连续性。
        //
        // 占位字符串兜底(2026-09-10 第 27 轮 F12 / 实测 BUG-2026-09-10-TUI-NULL):
        // 部分上游 LLM 在需要委派执行时,会把 `direct_answer` 写成字符串字面量
        // `"null"`/`"None"`/`"NULL"`(而非 JSON 的 null),导致本分支错误短路,
        // TUI 直接打印字面量 `null` 让用户以为没输出。此处把 4 类常见占位归一
        // 为"未填",继续走 loop → run_simple 委派 SubAgent。
        if classification.task_level == TaskLevel::Simple {
            if let Some(answer) = classification
                .direct_answer
                .as_ref()
                .filter(|a| !is_placeholder_direct_answer(a))
                .cloned()
            {
                Self::check_cancelled(cancel)?;
                emit_progress(progress, "Yolo 直接作答(跳过执行层)");
                // 运行日志(第 69 轮):直答短路决策
                info!(
                    session = %session.id(),
                    answer_chars = answer.chars().count(),
                    "Yolo 直接作答短路(感知→决策→直答)"
                );
                // 2026-09-11 第三十三轮:#P-C 修复 — 显式输出 trace 行,
                // 让用户/调试脚本区分「Yolo 直答」与「SubAgent 委派执行」两条路径。
                emit_progress(progress, "[trace] subagent=skipped(direct_answer=true)");
                let sc_started = std::time::Instant::now();
                // 2026-09-22 第 112 轮:把 TodoWrite 当前快照传给 SessionContext,
                // 写入 session_memory Summary 行末尾(下游可选解析)。
                let todo_snapshot_str = crate::agent::todo_state::global()
                    .map(|s| s.render_for_session_memory())
                    .unwrap_or_else(|| "{}".to_string());
                let summary = self
                    .session_context
                    .summarize(
                        &classification.goal_summary,
                        "(用户原始输入已记录)",
                        None,
                        &[],
                        &total_usage,
                        session.id(),
                        classification.yolo_degraded,
                        &classification.task_level,
                        Some(&todo_snapshot_str),
                    )
                    .await?;
                let sc_elapsed_ms = sc_started.elapsed().as_millis() as u64;
                stage_durations.push(StageDuration {
                    stage: "session_context".to_string(),
                    wf_id: None,
                    started_offset_ms: 0,
                    elapsed_ms: sc_elapsed_ms,
                });
                total_usage = add_usage(total_usage, summary.usage);
                self.dbg_task_end("direct_answer", total_usage);
                // 运行日志(第 69 轮):任务收口 —— 直答结局
                info!(
                    session = %session.id(),
                    outcome = "direct_answer",
                    usage_in = total_usage.input_tokens,
                    usage_out = total_usage.output_tokens,
                    wallclock_ms = task_started.elapsed().as_millis() as u64,
                    "任务收口"
                );
                return Ok(OrchestrationOutcome::DirectAnswer {
                    text: answer,
                    classification,
                    usage: total_usage,
                });
            }
        }

        loop {
            // 重试轮入口:取消短路(取消不是失败,不消耗重试预算)
            Self::check_cancelled(cancel)?;
            retry_count += 1;
            // 2026-09-16 第 57 轮:本轮入口计时(用于 retry_log.elapsed_ms)。
            let retry_started = std::time::Instant::now();
            if retry_count > self.cfg.max_retry_per_level {
                // 超过最大重试,输出失败 / 建议
                // 关联报告: 2026-09-09_05 E-003 —— 当 Yolo 没提供 user_suggestion 时,
                // 给出 actionable 化的兜底建议(根据 trace 推测失败原因)。
                let suggestion = if classification.user_suggestion_if_fail.is_empty() {
                    fallback_suggestion(&total_usage)
                } else {
                    classification.user_suggestion_if_fail.clone()
                };
                self.record_failure_event(session.id(), &classification, &suggestion);
                self.dbg_task_end(&format!("failed: {suggestion}"), total_usage);
                // 运行日志(第 69 轮):重试预算耗尽,任务最终失败
                info!(
                    session = %session.id(),
                    outcome = "failed",
                    attempts = retry_count,
                    reason = %crate::logging::clip(&retry_hint),
                    suggestion = %crate::logging::clip(&suggestion),
                    usage_in = total_usage.input_tokens,
                    usage_out = total_usage.output_tokens,
                    wallclock_ms = task_started.elapsed().as_millis() as u64,
                    "任务收口"
                );
                // 2026-09-16 第 58 轮 P0-C:把累计的 stage_durations / retry_log /
                // 最近一次失败 trace / 任务总耗时 透传给 Failed outcome,让
                // TUI Failed 分支复用 format_task_result 渲染 [trace] [tool] [failure] 段。
                let wallclock_ms = task_started.elapsed().as_millis() as u64;
                return Ok(OrchestrationOutcome::Failed {
                    classification,
                    reason: retry_hint.clone(),
                    suggestion,
                    usage: total_usage,
                    last_trace: last_failure_trace.clone(),
                    stage_durations: stage_durations.clone(),
                    retry_log: retry_log.clone(),
                    wallclock_ms,
                });
            }

            // 2) 调度执行(执行层取消:token 贯穿 SubAgent / 并行层)
            // 运行日志(第 69 轮):执行链路路由决策(simple/medium/hard)
            info!(
                session = %session.id(),
                route = %classification.task_level.display_name(),
                retry_count,
                "进入执行链路"
            );
            let exec_result = match classification.task_level {
                TaskLevel::Simple => {
                    self.run_simple(&classification, session, cancel, progress, &retry_hint)
                        .await
                }
                TaskLevel::Medium => {
                    self.run_medium(
                        &classification,
                        session,
                        cancel,
                        progress,
                        &retry_hint,
                        &mut medium_plan_cache,
                    )
                    .await
                }
                TaskLevel::Hard => {
                    self.run_hard(&classification, session, cancel, progress, &retry_hint)
                        .await
                }
            };

            match exec_result {
                Ok(mut task_result) => {
                    // 3) SessionContext 收口(收口前再查一次:取消后不再发摘要请求)
                    Self::check_cancelled(cancel)?;
                    // 2026-09-10 第 18 轮:摘要的用量口径与终端「本次用量」对齐 ——
                    // Yolo 分类 + 执行层累计(不含 SessionContext 自身)。
                    // 此前直接传 task_result.total_usage(仅执行层),漏记 Yolo 分类调用,
                    // 导致写入 session_memory 的摘要用量系统性偏小。
                    let usage_for_summary = add_usage(yolo_usage, task_result.total_usage);
                    let sc_started = std::time::Instant::now();
                    // 2026-09-22 第 112 轮:D19 TODO 持久化 — 把 TodoWrite 当前快照
                    // 传给 SessionContext,写入 session_memory Summary 行末尾。
                    let todo_snapshot_str = crate::agent::todo_state::global()
                        .map(|s| s.render_for_session_memory())
                        .unwrap_or_else(|| "{}".to_string());
                    let summary = self
                        .session_context
                        .summarize(
                            &task_result.goal,
                            "(用户原始输入已记录)",
                            task_result.plan_doc.as_deref(),
                            &task_result
                                .workflows
                                .iter()
                                .map(|w| {
                                    (
                                        w.id.clone(),
                                        w.name.clone(),
                                        w.quality_report.verdict == Verdict::Pass,
                                    )
                                })
                                .collect::<Vec<_>>(),
                            &usage_for_summary,
                            session.id(),
                            task_result.classification.yolo_degraded,
                            &task_result.classification.task_level,
                            Some(&todo_snapshot_str),
                        )
                        .await?;
                    let sc_elapsed_ms = sc_started.elapsed().as_millis() as u64;
                    stage_durations.push(StageDuration {
                        stage: "session_context".to_string(),
                        wf_id: None,
                        started_offset_ms: 0,
                        elapsed_ms: sc_elapsed_ms,
                    });
                    task_result.summary = summary.text.clone();
                    // 2026-09-09 第 14 轮:total_usage 累加策略
                    // - handle_inner 在 L260 已用 yolo_usage 初始化 total_usage
                    // - 执行层 run_simple/run_medium/run_hard 返回的 task_result.total_usage
                    //   自身已含其内部所有 LLM 调用的累计(SubAgent + Quality 等)
                    // - 因此这里用 add_usage 把执行层累计 + session_context 累计 累加到
                    //   已含 yolo_usage 的 total_usage 上,保证不丢任何角色的 token。
                    total_usage = add_usage(total_usage, task_result.total_usage);
                    total_usage = add_usage(total_usage, summary.usage);
                    task_result.total_usage = total_usage;
                    // 2026-09-16 第 57 轮:把本函数收集的 stage_durations / retry_log /
                    // wallclock_ms 合并进 task_result(执行层自己也有 stage_durations)。
                    task_result.stage_durations.extend(stage_durations);
                    task_result.retry_log = retry_log;
                    task_result.wallclock_ms = task_started.elapsed().as_millis() as u64;
                    self.dbg_task_end("executed", task_result.total_usage);
                    // 运行日志(第 69 轮):任务收口 —— 执行成功结局
                    info!(
                        session = %session.id(),
                        outcome = "executed",
                        workflows = task_result.workflows.len(),
                        summary_chars = task_result.summary.chars().count(),
                        usage_in = total_usage.input_tokens,
                        usage_out = total_usage.output_tokens,
                        wallclock_ms = task_result.wallclock_ms,
                        "任务收口"
                    );
                    return Ok(OrchestrationOutcome::Executed {
                        result: task_result,
                    });
                }
                Err(failure) => {
                    // 用户取消:短路退出整个任务,不回流不重试(H9 语义)
                    if failure.cancelled {
                        self.dbg_task_end("cancelled", total_usage);
                        return Err(AgentError::Cancelled);
                    }
                    total_usage = add_usage(total_usage, failure_usage(&failure));
                    // 2026-09-16 第 58 轮 P0-C:捕获最近一次失败的 ExecutionTrace,
                    // 跨轮透传到 Failed outcome,让 TUI 能展示「哪个工具失败 / early_terminate_reason / failure_signals」。
                    if let Some(trace) = failure.trace.as_ref() {
                        last_failure_trace = Some(trace.clone());
                    }
                    // 2026-09-16 第 57 轮:本轮 retry 结束 ——
                    // 把 retry_count / retry_hint / 本轮耗时记入 retry_log,
                    // 供 TUI 在「重试链路」段呈现(此前只能从 progress 阶段日志反推)。
                    let retry_elapsed_ms = retry_started.elapsed().as_millis() as u64;
                    retry_log.push(RetryRecord {
                        retry_count,
                        retry_hint: retry_hint.clone(),
                        started_offset_ms: 0,
                        elapsed_ms: retry_elapsed_ms,
                    });
                    // 升级或重试
                    // 运行日志(第 69 轮):执行失败,回流/重试决策
                    info!(
                        session = %session.id(),
                        retry_count,
                        retryable = failure.retryable,
                        source_role = %failure.source.as_str(),
                        reason = %crate::logging::clip(&failure.reason),
                        "执行失败(决策:回流/重试)"
                    );
                    if !failure.retryable {
                        emit_progress(progress, "失败回流 Yolo 重新评估…");
                        // 升级到上一层(由 Yolo 重新评估)
                        match self
                            .run_yolo_with_failure(&classification, &failure, session)
                            .await
                        {
                            Ok(new_c) => {
                                classification = new_c;
                                self.dbg_classify(&classification);
                                // 运行日志(第 69 轮):失败回流 Yolo 后的新分类
                                info!(
                                    session = %session.id(),
                                    task_level = %classification.task_level.display_name(),
                                    goal = %crate::logging::clip(&classification.goal_summary),
                                    "Yolo 重新分类(失败回流)"
                                );
                                // 档位升级后旧失败原因不再适用(F3);
                                // 分类可能变化 → 计划缓存一并作废(第 67 轮)
                                retry_hint.clear();
                                medium_plan_cache = None;
                                continue;
                            }
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    }
                    // 2026-09-16 第 67 轮:执行层失败(单元 QC 拒 / Agent 执行错)保留计划缓存,
                    // 下一轮 run_medium 直接复用(跳过重拆);计划级失败(MainWork/QC 调用错)
                    // 由 run_medium 内部清空缓存。
                    let is_exec_level_failure = matches!(failure.source, AgentRole::SubAgent);
                    if !is_exec_level_failure {
                        medium_plan_cache = None;
                    }
                    // retryable=true 留在当前档位继续重跑
                    // F3:记录失败原因,下一轮回灌 Main-Work / 供 Failed 呈现
                    retry_hint = failure.reason.clone();
                    emit_progress(
                        progress,
                        if is_exec_level_failure && medium_plan_cache.is_some() {
                            format!("第 {retry_count} 轮重试(复用 WorkFlow 计划,失败反馈注入执行单元)…")
                        } else {
                            format!("第 {retry_count} 轮重试当前档位…")
                        },
                    );
                    continue;
                }
            }
        }
    }

    // ========== 简单档 ==========

    /// 取消检查:命中即返回 `Err(Cancelled)`(由调用方短路,不回流)。
    pub(super) fn check_cancelled(cancel: &CancelToken) -> Result<()> {
        if cancel.is_cancelled() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// 取 session 最后一条 user 消息文本作为「用户原始 prompt」。
    ///
    /// 2026-09-11 第三十三轮 #P-A:SubAgent 透传原始 prompt 防抽象摘要漂移;
    /// 2026-09-11 第三十四轮 LA-1:提取为公共 helper,Main-Work 拆解同样透传
    /// (run_simple / run_medium 共用,行为一致)。
    pub(super) fn original_user_prompt(session: &Session) -> Option<String> {
        session
            .context()
            .iter()
            .rev()
            .find(|m| matches!(m.role, crate::llm::Role::User))
            .map(|m| m.content_text())
            .filter(|s| !s.trim().is_empty())
    }

    /// 为 simple 档重试回灌上一轮失败原因,避免 SubAgent 每轮看到完全相同输入。
    pub(super) fn prompt_with_retry_hint(original_prompt: Option<String>, retry_hint: &str) -> Option<String> {
        let hint = retry_hint.trim();
        if hint.is_empty() {
            return original_prompt;
        }
        Some(match original_prompt {
            Some(prompt) => format!(
                "{prompt}\n\n【上一轮失败反馈,必须改变策略】\n{hint}\n不要重复完全相同的工具调用。"
            ),
            None => {
                format!("【上一轮失败反馈,必须改变策略】\n{hint}\n不要重复完全相同的工具调用。")
            }
        })
    }

    pub(super) async fn run_simple(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
        retry_hint: &str,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 2026-09-11 第三十三轮:#P-A 修复 — 取 session 最后一条 user 消息作为
        // 原始 prompt 透传给 SubAgent,避免具体任务被 Yolo 抽象摘要漂移
        // (实测 P10「对 p10_inject.txt 做词频统计」被改写为「完成 laew 端到端链路验证」)。
        let original_prompt =
            Self::prompt_with_retry_hint(Self::original_user_prompt(session), retry_hint);

        // 2026-09-18 第 89 轮:simple 档统一 SubAgentRunner(原 WebUseRunner 已删除,
        // 浏览器操控由 SubAgent-Work 的 MCP_Web_Use 工具承担;出口兜底与页面复用
        // 提示已在 SubAgentRunner 内实现)。
        let exec_role = AgentRole::SubAgent;
        let exec_label = "SubAgent";
        // 运行日志(第 69 轮):simple 档委派决策,记录 Yolo 建议 vs 实际 Runner 路由
        info!(
            suggested = c.suggested_delegate.as_deref().unwrap_or(""),
            actual = exec_role.as_str(),
            yolo_degraded = c.yolo_degraded,
            "simple 档委派路由决策"
        );

        let input = SubFlowInput {
            id: "wf-1".into(),
            description: c.goal_summary.clone(),
            expected_output: c
                .decomposition_plan
                .first()
                .cloned()
                .unwrap_or_else(|| "完成用户请求".into()),
            original_prompt,
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
                pending_agent_messages: vec![],
            // 写入 trace 供 QC + TUI delegate_mismatch 诊断
            intended_role: Some(exec_role),
            // 2026-09-17 第 82+ 轮 P0-1:simple 档无目标应用自动启动(simple 任务通常无需)。
            // 2026-09-19 第 91 轮 P0-7/P0-8:
            retry_count: 0, retry_hint: String::new(), max_iterations: None,
            pre_explore: false,
        };
        emit_progress(progress, format!("wf-1 {exec_label} 执行中…"));
        let retry_budget = self.cfg.unit_retry_budget;
        // 第 95 轮:simple 档单元素同样享受单元级局部重试(QC 拒 retryable → 注入本单元
        // 结论重试该元素,预算耗尽才升级为档位级失败)。
        // 原内联 SubAgent + QC 执行路径统一收敛到 run_wf_unit,重复实现消除。
        let ok = run_wf_unit(
            self.sub_agent.clone(),
            self.quality.clone(),
            self.cfg.debug.clone(),
            input,
            c.goal_summary.clone(),
            session.id().to_string(),
            None,
            cancel.clone(),
            progress.clone(),
            retry_budget,
        )
        .await?;
        let sub_elapsed_ms = ok.wallclock_ms;
        let qc_elapsed_ms = ok.qc_wallclock_ms;
        let qc = ok.qc;
        let outcome_text = ok.outcome_text;
        let outcome_usage = ok.usage;
        let outcome_trace = ok.trace;
        let exec_role = ok.exec_role;
        let total_usage = outcome_usage;

        // D9-8 决策审计(2026-09-19):simple 档 QC 质检判定写入审计 JSONL
        //(medium/hard 档在 workflows.rs 每 WorkFlow 判定后写入;
        // QC 自身已在 run_wf_unit 内完成并 record_quality,此处不再重复 dbg_qc)。
        crate::agent::decision_audit::record_verdict(
            session.id(),
            "wf-1",
            if qc.verdict == Verdict::Pass { "pass" } else { "fail" },
            &qc.issues,
            qc.retryable,
            &qc.evidence,
        );
        emit_progress(
            progress,
            format!(
                "wf-1 QC:{}",
                if qc.verdict == Verdict::Pass {
                    "✅ 通过"
                } else {
                    "❌ 未通过"
                }
            ),
        );

        // 2026-09-09 第 14 轮:累加 Quality-Check 调用的 LLM 用量
        // 第 95 轮:total_usage 已在 run_wf_unit 内部累计完成(QC usage 含于 ok.usage)

        {
            // WorkFlow 名称使用 goal_summary 截断(最多 20 字符),便于在 TUI 区分不同任务
            let wf_name = if c.goal_summary.chars().count() > 20 {
                format!("{}…", c.goal_summary.chars().take(19).collect::<String>())
            } else {
                c.goal_summary.clone()
            };
            Ok(TaskResult {
                goal: c.goal_summary.clone(),
                classification: c.clone(),
                plan_doc: None,
                workflows: vec![WorkflowResult {
                    id: "wf-1".into(),
                    name: wf_name,
                    subflow_outcome: outcome_text,
                    quality_report: qc,
                    usage: outcome_usage,
                    subflow_trace: Some(outcome_trace),
                    exec_role,
                    wallclock_ms: sub_elapsed_ms,
                    qc_wallclock_ms: qc_elapsed_ms,
                }],
                summary: String::new(),
                total_usage,
                stage_durations: vec![
                    StageDuration {
                        stage: "wf".to_string(),
                        wf_id: Some("wf-1".into()),
                        started_offset_ms: 0,
                        elapsed_ms: sub_elapsed_ms,
                    },
                    StageDuration {
                        stage: "qc_wf".to_string(),
                        wf_id: Some("wf-1".into()),
                        started_offset_ms: 0,
                        elapsed_ms: qc_elapsed_ms,
                    },
                ],
                retry_log: Vec::new(),
                layer_log: Vec::new(),
                wallclock_ms: 0,
            })
        }
    }

    // ========== 中等档 ==========

    pub(super) async fn run_medium(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
        retry_hint: &str,
        plan_cache: &mut Option<WorkFlowPlan>,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 0) 2026-09-16 第 67 轮:计划复用(效率优化)。
        //    上一轮执行层失败(wf 单元 QC 拒)→ 计划本身没问题 → 跳过 Main-Work LLM 重拆,
        //    失败反馈注入每个执行单元的 description。失败单元也会因 retry_hint 调整策略。
        if retry_hint.trim().is_empty() {
            // 首轮(或档位切换后)不复用
            *plan_cache = None;
        }
        if let Some(cached) = plan_cache.take() {
            emit_progress(
                progress,
                format!(
                    "复用上一轮 WorkFlow 计划({} 个流程单元,跳过 Main-Work 重拆),失败反馈注入各单元",
                    cached.workflows.len()
                ),
            );
            let mut result = self
                .execute_workflows(c, &cached, Usage::default(), session, cancel, progress, retry_hint)
                .await?;
            result.stage_durations.push(StageDuration {
                stage: "main_work".to_string(),
                wf_id: None,
                started_offset_ms: 0,
                elapsed_ms: 0, // 复用轮无 Main-Work LLM 调用
            });
            return Ok(result);
        }

        // 1) Main-Work 拆 WorkFlow
        emit_progress(progress, "Main-Work 拆解中…");
        // 2026-09-11 第三十四轮 LA-1:Main-Work 拆解同样透传用户原始 prompt,
        // 与 SubAgent #P-A 修复对齐,防止拆解只基于 Yolo 抽象摘要脱离用户意图。
        let original_prompt = Self::original_user_prompt(session);
        let mainwork_started = std::time::Instant::now();
        // 2026-09-16 第 59 轮:透传 Yolo 推断的 suggested_delegate,引导 Main-Work 正确委派窗口操控类任务
        let suggested_delegate = c.suggested_delegate.as_deref();
        // 2026-09-16 第 66 轮:Main-Work LLM 调用超时(默认 120s,环境变量 LAEW_MAINWORK_TIMEOUT),
        // 防止 LLM 响应慢/挂起导致无限等待(用户反馈 WebUse 任务卡住 58.8s)。
        let mainwork_timeout = std::env::var("LAEW_MAINWORK_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(120);
        let (mut plan, mainwork_usage) = tokio::time::timeout(
            std::time::Duration::from_secs(mainwork_timeout),
            self.main_work.plan_workflows_with_delegate(
                &c.goal_summary,
                &c.decomposition_plan,
                session.id(),
                retry_hint,
                original_prompt.as_deref(),
                suggested_delegate,
            ),
        )
        .await
        .map_err(|_| {
            QualityFailure {
                source: AgentRole::MainWork,
                reason: format!("Main-Work 拆解超时({}s)", mainwork_timeout),
                retryable: true,
                suggestion: "重试".into(),
                cancelled: false,
                trace: None,
                usage: Usage::default(),
            }
        })?
        .map_err(|e| {
            QualityFailure::from_agent_error(AgentRole::MainWork, "Main-Work 拆解失败", &e)
        })?;
        let mainwork_elapsed_ms = mainwork_started.elapsed().as_millis() as u64;

        // 1.2) 2026-09-16 第 66 轮 P0-1/P0-2:确定性自动修复 + 阻断校验,先于一切 LLM QC。
        // 根因:QC-main(LLM)对计划做品相质检(loops.max_iterations=null / branches 为空 /
        // Unicode 上标名归一化差异),误判率与成本双高,形成确定性必败重试风暴
        // (实测 162.8s 三轮重试零执行)。程序可判定的部分收回程序判定:
        // - auto_repair:loops.max_iterations 从文本回填(最多N次/max_iterations=N/无界滚动兜底10);
        // - validate_blocking:workflows 空 / id 重复 / steps 空 / 依赖未知或成环。
        crate::agent::plan_validate::auto_repair_plan(&mut plan);
        let blocking_issues = crate::agent::plan_validate::validate_plan_blocking(&plan);
        if !blocking_issues.is_empty() {
            // 秒级失败回流:不调 QC LLM,精确原因回灌 Main-Work 下一轮重拆
            emit_progress(
                progress,
                format!("计划确定性校验未通过:{}(秒级回流重拆)", blocking_issues[0]),
            );
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: blocking_issues.join("; "),
                retryable: true,
                suggestion: "请补齐缺失字段(id/name/steps)、消除重复 id 与循环依赖后重新拆解"
                    .to_string(),
                cancelled: false,
                trace: None,
                usage: mainwork_usage,
            });
        }

        // 1.5) F2(2026-09-10 第 25 轮):兜底计划跳过 QC-main 直接执行。
        // 兜底 WorkFlowPlan 的 summary 自证「解析失败」,送 QC 必然 fail+retryable,
        // 形成确定性必败重试循环(D06 实测连烧 3 轮 ~10 分钟零产出)。
        // 真实产物质量仍由 execute_workflows 内每 WorkFlow 的 QC 把守。
        if plan.degraded {
            emit_progress(
                progress,
                format!(
                    "Main-Work 解析失败,已使用单 WorkFlow 兜底(跳过计划 QC,直接执行 {} 个流程)",
                    plan.workflows.len()
                ),
            );
            // 兜底计划不进复用缓存(degraded 拆解质量不足,失败后应重新拆)
            let mut result = self
                .execute_workflows(c, &plan, mainwork_usage, session, cancel, progress, retry_hint)
                .await?;
            result.stage_durations.push(StageDuration {
                stage: "main_work".to_string(),
                wf_id: None,
                started_offset_ms: 0,
                elapsed_ms: mainwork_elapsed_ms,
            });
            return Ok(result);
        }

        // 2026-09-16 第 67 轮:确定性校验通过的真实计划进复用缓存(执行层失败时下一轮跳过重拆)
        *plan_cache = Some(plan.clone());

        // D9-8 决策审计(2026-09-19):Main-Work 流程拆解决策写入审计 JSONL。
        {
            let layers = crate::agent::main_work::topo_layers(&plan.workflows)
                .map(|l| l.len())
                .unwrap_or(0);
            crate::agent::decision_audit::record_decompose(
                session.id(),
                &c.goal_summary,
                plan.workflows.len(),
                layers,
                mainwork_elapsed_ms,
                mainwork_usage,
            );
        }

        // 2) Quality 校验 Main-Work 输出
        // 2026-09-16 第 66 轮 P0-2:确定性校验已通过的计划默认跳过 LLM QC-main ——
        // 计划品相问题(可省略字段为空 / 名称归一化差异 / UI 类验收措辞)不再阻断执行,
        // 真实产物质量仍由 execute_workflows 内每 WorkFlow 的 QC 把守(对齐 degraded 路径)。
        // 逃生门:LAEW_QC_MAIN_LLM=1 恢复 LLM 计划级质检。
        let qc_main_llm_enabled = std::env::var("LAEW_QC_MAIN_LLM")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        if !qc_main_llm_enabled {
            emit_progress(
                progress,
                format!(
                    "Main-Work 拆解 {} 个流程单元,确定性校验通过(跳过计划级 LLM 质检)",
                    plan.workflows.len()
                ),
            );
            let pre_usage = mainwork_usage;
            let mut result = self
                .execute_workflows(c, &plan, pre_usage, session, cancel, progress, retry_hint)
                .await?;
            result.stage_durations.push(StageDuration {
                stage: "main_work".to_string(),
                wf_id: None,
                started_offset_ms: 0,
                elapsed_ms: mainwork_elapsed_ms,
            });
            return Ok(result);
        }
        let wf_json = serde_json::to_string(&plan).unwrap_or_default();
        let qc_started = std::time::Instant::now();
        let (qc_main, qc_usage) = self
            .quality
            .check_main(&c.goal_summary, &wf_json, session.id())
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        let qc_main_elapsed_ms = qc_started.elapsed().as_millis() as u64;
        self.dbg_qc(&qc_main);
        emit_progress(
            progress,
            format!("Main-Work 拆解 {} 个流程单元", plan.workflows.len()),
        );

        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
                cancelled: false,
                trace: None,
                usage: add_usage(mainwork_usage, qc_usage),
            });
        }

        // 2026-09-09 第 14 轮:累加 Main-Work + Quality-Main 调用的 LLM 用量,
        // 透传到 execute_workflows 内部继续累加。
        let pre_usage = add_usage(mainwork_usage, qc_usage);

        // 3) 拓扑排序并执行
        let mut result = self
            .execute_workflows(c, &plan, pre_usage, session, cancel, progress, retry_hint)
            .await?;
        // 2026-09-16 第 57 轮:把 run_medium 收集的 Main-Work + QC-Main 阶段耗时
        // 写入 result.stage_durations,供 TUI 时间线展示。
        result.stage_durations.push(StageDuration {
            stage: "main_work".to_string(),
            wf_id: None,
            started_offset_ms: 0,
            elapsed_ms: mainwork_elapsed_ms,
        });
        result.stage_durations.push(StageDuration {
            stage: "qc_main".to_string(),
            wf_id: None,
            started_offset_ms: 0,
            elapsed_ms: qc_main_elapsed_ms,
        });
        Ok(result)
    }

    // ========== 高等档 ==========

    pub(super) async fn run_hard(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
        retry_hint: &str,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 1) Plan 生成(2026-09-09 第 14 轮:带回 LLM Usage 用于累加)
        // I3(2026-09-14 第 51 轮):重试轮回灌上一轮 QC 拒绝理由,
        // Plan 针对性修复而非盲重生成(此前 hard 档重试链路唯一无反馈环)。
        emit_progress(progress, "Plan 规划中…");
        let plan_started = std::time::Instant::now();
        let (plan_output, plan_usage) = self
            .plan
            .generate_with_retry_hint(
                &c.goal_summary,
                &c.purpose,
                &c.intent,
                &c.decomposition_plan,
                session.id(),
                retry_hint,
            )
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::Plan, "Plan 生成失败", &e))?;
        let plan_elapsed_ms = plan_started.elapsed().as_millis() as u64;
        emit_progress(
            progress,
            format!(
                "Plan 已生成:{}",
                plan_output
                    .path
                    .file_name()
                    .map(|n| n.display().to_string())
                    .unwrap_or_default()
            ),
        );
        // D9-8 决策审计(2026-09-19):Plan 规划决策写入审计 JSONL。
        crate::agent::decision_audit::record_plan(
            session.id(),
            &c.goal_summary,
            c.decomposition_plan.len(),
            plan_elapsed_ms,
            plan_usage,
        );

        // 2) Quality 校验 Plan
        let qc_plan_started = std::time::Instant::now();
        let (qc_plan, qc_plan_usage) = self
            .quality
            .check_plan(&plan_output.markdown, session.id())
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        let qc_plan_elapsed_ms = qc_plan_started.elapsed().as_millis() as u64;
        self.dbg_qc(&qc_plan);
        if qc_plan.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::Plan,
                reason: qc_plan.issues.join("; "),
                retryable: qc_plan.retryable,
                suggestion: qc_plan.suggestion,
                cancelled: false,
                trace: None,
                usage: add_usage(plan_usage, qc_plan_usage),
            });
        }

        // 3) Main-Work 解析 Plan → WorkFlow
        let plan = self.main_work.parse_plan(&plan_output.path).map_err(|e| {
            QualityFailure::from_agent_error(AgentRole::MainWork, "解析 Plan 失败", &e)
        })?;

        let qc_main_started = std::time::Instant::now();
        let (qc_main, qc_main_usage) = self
            .quality
            .check_main(
                &c.goal_summary,
                &serde_json::to_string(&plan).unwrap_or_default(),
                session.id(),
            )
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        let qc_main_elapsed_ms = qc_main_started.elapsed().as_millis() as u64;
        self.dbg_qc(&qc_main);
        emit_progress(
            progress,
            format!("Main-Work 解析为 {} 个流程单元", plan.workflows.len()),
        );
        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
                cancelled: false,
                trace: None,
                usage: add_usage(add_usage(plan_usage, qc_plan_usage), qc_main_usage),
            });
        }

        // 2026-09-09 第 14 轮:累加 Plan + Quality-Plan + Quality-Main 三次 LLM 调用的用量
        let mut pre_usage = add_usage(plan_usage, qc_plan_usage);
        pre_usage = add_usage(pre_usage, qc_main_usage);

        // 4) 执行 WorkFlow
        let mut task_result = self
            .execute_workflows(c, &plan, pre_usage, session, cancel, progress, retry_hint)
            .await?;
        task_result.plan_doc = Some(plan_output.path);
        // 2026-09-16 第 57 轮:把 run_hard 收集的 Plan + QC-Plan + QC-Main 阶段耗时
        // 写入 task_result.stage_durations,供 TUI 时间线展示。
        task_result.stage_durations.push(StageDuration {
            stage: "plan".to_string(),
            wf_id: None,
            started_offset_ms: 0,
            elapsed_ms: plan_elapsed_ms,
        });
        task_result.stage_durations.push(StageDuration {
            stage: "qc_main".to_string(),
            wf_id: None,
            started_offset_ms: 0,
            elapsed_ms: qc_main_elapsed_ms,
        });
        // QC-Plan 阶段(在 qc_main 之前)
        task_result.stage_durations.insert(
            task_result.stage_durations.len() - 2,
            StageDuration {
                stage: "qc_plan".to_string(),
                wf_id: None,
                started_offset_ms: 0,
                elapsed_ms: qc_plan_elapsed_ms,
            },
        );
        Ok(task_result)
    }
}
