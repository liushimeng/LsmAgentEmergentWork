//! Agent 核心循环实现(2026-09-17 自 mod.rs 拆分,方案见 tmpPlan/2026-09-17_01-单文件1800行超标拆分重构方案.md)。
//!
//! 承载 `run_once` / `run_session*` 家族与溢出恢复、截断续接、max_tokens 收尾等
//! 执行细节;`Agent` 结构体与构造器/访问器仍在 [`crate::agent`] 根模块。

use super::*;

impl Agent {
    /// 单轮任务:传入用户提示,返回最终文本、本次累计 token 用量与执行轨迹。
    pub async fn run_once(&self, user_input: &str) -> Result<(String, Usage, ExecutionTrace)> {
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user(user_input));
        self.run_session(&mut session).await
    }

    /// [`run_session`] 的可取消版本:传入取消 token,任何阶段命中取消都先补全
    /// orphan tool_use 再返回 `Err(AgentError::Cancelled)`(对齐 atomcode
    /// `run_turn` 的 select + backfill 语义,知识库第五轮 §2.1)。
    ///
    /// `None` 时行为与 [`run_session`] 完全一致。
    pub async fn run_session_cancellable(
        &self,
        session: &mut Session,
        cancel: Option<&CancelToken>,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        if let Some(token) = cancel {
            if token.is_cancelled() {
                backfill_cancelled_tool_results(session.context_mut());
                return Err(AgentError::Cancelled);
            }
        }
        self.run_session_inner(session, cancel).await
    }

    /// 复用 Session 上下文的对话循环(用于 TUI 多轮对话)。
    ///
    /// 返回 `(最终回复文本, 本次循环累计 token 用量, 执行轨迹)`。
    /// 后两者包含所有 LLM 调用的 input/output tokens 之和(由 LlmClient 在 SSE 流中收集),
    /// 以及本次循环的工具调用明细(成功/失败/截断/早终止),供 QC 与编排器使用。
    ///
    /// **自动截断续接**:当 LLM 输出因 token 上限被截断时(`stop_reason = "max_tokens"`
    /// / `"length"`),自动注入 nudge 消息让模型从断点继续,最多续接 `max_truncation_resume`
    /// 次(默认 4 次),避免无限循环。累计所有部分输出后返回完整文本。
    ///
    /// **执行轨迹(2026-09-09 第 05 轮)**:每次循环迭代同步累计工具调用成功/失败计数、
    /// 连续失败长度、截断续接次数,单元结束时聚合成 `ExecutionTrace` 返回。
    pub async fn run_session(
        &self,
        session: &mut Session,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        self.run_session_cancellable(session, None).await
    }

    /// 循环主体(`run_session` / `run_session_cancellable` 共用)。
    async fn run_session_inner(
        &self,
        session: &mut Session,
        cancel: Option<&CancelToken>,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        let tool_defs = self.profile.tools.defs();
        // 会话级 max_tokens 升级状态机(2026-09-09 第 09 轮,实现 L1037)。
        // 起始 8K,被 max_tokens 截断时翻倍,封顶 64K;每会话重置,
        // 避免「上次会话把 max 撑满,下次新会话也按 64K 起跳」的浪费。
        let max_tokens_state = Arc::new(MaxTokensState::new());
        // 每次迭代注入 override 的可变 meta:从 session.meta() 复制后改写。
        // 注:session.meta() 本身不可变拿 ID,这里手工重建 RequestMeta 以避免改 Session API。
        let mut meta: RequestMeta = session.meta();
        meta.max_tokens_override = Some(max_tokens_state.current());
        // UA 逐请求注入(2026-09-09 第 08 轮):让抓包层面 8 角色各自可辨识;
        // meta.user_agent 为空时协议层回退到客户端构造期默认 UA(见 RequestMeta::resolve_user_agent)。
        meta.user_agent = self.profile.user_agent();
        // 结构化输出强制通道(L6/L19,2026-09-09 第 13 轮):profile 声明了 emit 工具时
        // 注入 forced tool_choice(协议层指名调用),模型必须以 tool_use 返回结构化结果;
        // `LAEW_FORCED_TOOLS=off` 可全局关闭(仅关 wire 注入,循环短路逻辑保留,
        // 模型若仍主动调用 emit 工具也会被接住)。
        if forced_tools_enabled() {
            meta.forced_tool = self.profile.emit_tool.clone();
        }
        let mut total_usage = Usage::default();
        let mut accumulated_text = String::new();
        let mut truncation_resumes: usize = 0;

        // 关联报告: 20260908_203854 D-001
        // 连续相同「工具名 + 目标参数」失败的短路过流:当上游 LLM 反复引导同一个
        // 失败调用(典型表现:幻觉一个不存在的文件路径后持续 Read)时,提前终止避免
        // 耗尽 max_iterations。命中条件:连续 N(默认 3)次失败且失败键(工具名+稳定
        // JSON)相同;成功调用或换工具/换目标会立即重置计数。
        const REPEATED_FAILURE_THRESHOLD: usize = 3;
        let mut last_fail_key: Option<String> = None;
        let mut consecutive_failures: usize = 0;

        // 关联报告: 2026-09-09_05 E-001 「无文本收敛短路」
        // 对称短路:当 LLM 在多轮迭代中持续产生 tool_use 但**始终没有输出 final_text**
        // (典型表现:不停探查/验证/重写同一目标),即使成功调用把失败计数重置了,
        // 循环也会一直跑到 max_iterations 耗尽。命中条件:连续 N(默认 8)轮「仅 tool_use
        // 无 final_text」;命中后注入系统级 nudge 让 LLM 收敛,并立即返回已观察到的工具
        // 结果聚合作为最终答复。阈值 = max_iterations/2 防止误伤收敛慢但最终成功的任务。
        const NO_TEXT_CONVERGE_THRESHOLD: usize = 8;
        let mut consecutive_no_text_rounds: usize = 0;
        // F9(2026-09-10 第 25 轮):无文本收敛「宽限轮」。
        // 此前命中阈值后注入 nudge 却立即 return 兜底文本,nudge 从未到达 LLM
        // (F-002 的「让 LLM 看到工具历史自纠」意图落空),合法的多步执行单元被硬杀
        // (D06 实测:SubAgent 8 轮环境探查被终止,核心写入从未发生)。
        // 现在首次命中:注入 nudge + 重置计数,给 LLM 一轮真实继续机会(工具照常执行);
        // 第二次命中才按原语义硬终止。上界仍受 max_iterations 约束。
        let mut no_text_grace_used = false;

        // 执行轨迹累计(2026-09-09 第 05 轮):每次循环同步填充 trace 字段,
        // 单元结束时由 ExecutionTrace::collect_failure_signals 统一打标。
        let mut trace = ExecutionTrace::default();

        // 关联报告: 2026-09-09_06 F-002 — 最近工具调用历史(用于无文本收敛短路时
        // 输出「叙事化摘要」,而非纯机械的次数统计;最多保留 RECENT_TOOL_HISTORY_LIMIT
        // 条避免无限增长)。
        const RECENT_TOOL_HISTORY_LIMIT: usize = 16;
        let mut recent_tool_history: Vec<(String, String, bool)> = Vec::new(); // (tool, args_digest, is_error)

        // trace.artifacts 上限(2026-09-09 第 15 轮):防止极端任务写大量文件时轨迹膨胀。
        const ARTIFACTS_LIMIT: usize = 8;

        // 2026-09-16 第 63 轮:首迭代强制工具状态跟踪。
        // 首迭代强制调用指定工具(如 BrowserNew)后,后续轮次恢复 auto,
        // 避免全程强制导致 LLM 无法自由决策。
        let mut first_iter_forced_done = false;
        // 2026-09-16 第 68 轮:首迭代 forced tool 是否真正生效(写回 trace)。
        // iter=0 调用完成后,检查 LLM 是否真的调用了 forced tool;
        // 若未调用(被降级或 LLM 忽略),标记 false 供 TUI 证据段展示。
        let mut forced_tool_maybe_effective = if self.first_iter_forced_tool.is_some() {
            Some(false) // 先假设未生效,iter=0 完成后更新
        } else {
            None
        };
        // 2026-09-16 第 68 轮 P1-C:连续无工具调用计数 —— 连续 N 轮无 tool_use 时
        // 提前终止,避免 WindowUse/WebUse 等专项 Agent 在 nudge 失效时跑满 max_iterations。
        const NO_TOOL_USE_THRESHOLD: usize = 3;
        let mut consecutive_no_tool_rounds: usize = 0;

        for iter in 0..self.max_iterations {
            trace.iterations = iter + 1;
            // 迭代边界:取消检查(轻量 is_cancelled,热路径零 await 开销)
            if let Some(token) = cancel {
                if token.is_cancelled() {
                    backfill_cancelled_tool_results(session.context_mut());
                    return Err(AgentError::Cancelled);
                }
            }
            // 2026-09-16 第 63 轮:首迭代强制工具注入。
            // iter==0 且 first_iter_forced_tool 已设置且未执行过 → 强制指定工具;
            // 后续轮次恢复 auto(清除 forced_tool,保留 emit_tool 语义)。
            if iter == 0 {
                if let Some(tool) = &self.first_iter_forced_tool {
                    meta.forced_tool = Some(tool.clone());
                    debug!(forced_tool = %tool, "首迭代强制工具注入");
                }
            } else if !first_iter_forced_done {
                // 首迭代已完成,恢复 auto(但保留 emit_tool 结构化通道)
                if self.first_iter_forced_tool.is_some() {
                    meta.forced_tool = if forced_tools_enabled() {
                        self.profile.emit_tool.clone()
                    } else {
                        None
                    };
                    first_iter_forced_done = true;
                }
            }
            debug!(iteration = iter, "agent step");
            // runtime hints 拼接(2026-09-09 第 09 轮,联动 L771 失败计数预警):
            // 仅在对应计数器 > 0 时追加,全 0 时返回空串,不影响 LLM 上下文;
            // 拼到 system 末尾,不破坏 cache_control 缓存前缀。
            let base_system = self.profile.system_prompt.render(self.llm.protocol());
            // 工作区运行时快照(2026-09-13 第 01 轮,D4):让**执行层** Agent
            // (SubAgent-Work / Main-Work / Plan)也拿到工程类型、工具链、git 状态、
            // 平台与日期 —— 此前只有 Yolo 通过 PROJECT_CONTEXT 消息可见。
            // 进程级 TTL 缓存(默认 5s),同任务内多次调用复用同一快照,开销可忽略;
            // 空目录返回空串,不改变既有 system 语义。
            let workspace_hint = crate::agent::workspace::hint_block();
            let runtime_hints = build_runtime_hints(&trace, consecutive_failures);
            let system = if workspace_hint.is_empty() && runtime_hints.is_empty() {
                base_system
            } else {
                format!("{base_system}{workspace_hint}{runtime_hints}")
            };
            // 上下文溢出自动恢复(L1038/L1044,2026-09-09 第 06 轮):
            // LLM 调用命中 prompt-too-long 类溢出错误时,自动执行
            // 排水(Level 1)→ 折叠(Level 2)→ 重试;两轮无效则上抛(三级暴露)。
            let completion: Completion = self
                .complete_with_overflow_recovery(
                    session, cancel, &system, &tool_defs, &meta, &mut trace,
                )
                .await?;

            // 累计 usage
            total_usage.input_tokens = total_usage
                .input_tokens
                .saturating_add(completion.usage.input_tokens);
            total_usage.output_tokens = total_usage
                .output_tokens
                .saturating_add(completion.usage.output_tokens);
            total_usage.cache_read_input_tokens = total_usage
                .cache_read_input_tokens
                .saturating_add(completion.usage.cache_read_input_tokens);
            total_usage.cache_creation_input_tokens = total_usage
                .cache_creation_input_tokens
                .saturating_add(completion.usage.cache_creation_input_tokens);

            if !completion.has_tool_calls() {
                // 累计文本(续接场景下可能多次进入此分支)
                if !completion.text.trim().is_empty() {
                    if !accumulated_text.is_empty() {
                        accumulated_text.push('\n');
                    }
                    accumulated_text.push_str(&completion.text);
                    session
                        .context_mut()
                        .push(ChatMessage::assistant(vec![ContentBlock::text(
                            completion.text.clone(),
                        )]));
                }

                // 2026-09-16 第 68 轮 P1-C:连续无工具调用计数 —— 避免专项 Agent
                // (WindowUse/WebUse)在 forced tool 降级 + nudge 失效时跑满 max_iterations。
                // 阈值 NO_TOOL_USE_THRESHOLD=3:给 LLM 3 次机会(含 nudge 引导),仍无工具
                // 调用则提前终止,把失败信息返回 Runner/QC 而不是空跑 16 轮。
                consecutive_no_tool_rounds += 1;
                if consecutive_no_tool_rounds >= NO_TOOL_USE_THRESHOLD {
                    warn!(
                        rounds = consecutive_no_tool_rounds,
                        total_iters = iter + 1,
                        "连续多轮无工具调用,提前终止以避免空跑 max_iterations"
                    );
                    trace.early_terminated = true;
                    trace.early_terminate_reason =
                        format!("no_tool_use_{}_rounds", consecutive_no_tool_rounds);
                    trace.collect_failure_signals(&accumulated_text);
                    return Self::finalize_with_forced_flag(
                        trace,
                        &accumulated_text,
                        total_usage,
                        max_tokens_state.as_ref(),
                        forced_tool_maybe_effective,
                    );
                }

                // 2026-09-16 第 68 轮 P0-A(修复 v2):首迭代 forced tool 未生效时的强引导。
                // 原 P0-A 的 nudge 仅在 iter==1 触发,但 LLM iter=0 返回纯文本时循环直接退出,
                // nudge 永远无法触发。现在:iter=0 检测到 forced tool 设置但 LLM 未调用,
                // 立即注入强引导 nudge,给 LLM 第二次机会。
                if iter == 0 && self.first_iter_forced_tool.is_some() {
                    warn!(
                        forced_tool = self.first_iter_forced_tool.as_deref().unwrap_or("?"),
                        text_len = completion.text.len(),
                        "首迭代 forced tool 未生效,LLM 返回纯文本,注入强引导 nudge"
                    );
                    session
                        .context_mut()
                        .push(ChatMessage::user(FORCED_TOOL_NUDGE_TEXT));
                    continue;
                }

                // 2026-09-16 第 58 轮 P0-A(原):窗口操控型 Agent nudge 兜底。
                // 2026-09-16 第 68 轮 P0-B 修复:移除 truncation_resumes==0 约束,
                // 截断续接后仍应 nudge;扩展触发范围到 iter <= 2(给更多机会)。
                //
                // 闸门双锁:
                // 1) iter <= 2 —— 前 3 轮均可 nudge(0,1,2),覆盖 forced tool 失效场景;
                // 2) profile.tools 含 "WindowList" —— 强白名单,只对窗口操控类 Agent 触发,
                //    其它 Agent(Yolo/Main-Work/QC/SubAgent 普通任务)走原路径。
                //
                // 后续:P0-B 在 WindowUseRunner 出口兜底,即便 nudge 后 LLM 仍只回文本,
                // 也会被 Runner 标 failed,不会逃过 QC。
                if should_nudge_window_ops(&self.profile.tools.names(), iter)
                    && !completion.text.trim().is_empty()
                {
                    info!(
                        iter = iter,
                        "WindowUse 无工具调用,注入 nudge 强制 LLM 使用窗口操控工具"
                    );
                    session
                        .context_mut()
                        .push(ChatMessage::user(WINDOW_OPS_NUDGE_TEXT));
                    continue;
                }

                // 2026-09-16 第 61 轮:WebUse 第 1 轮无工具调用同款 nudge(与窗口版同构)。
                // 2026-09-16 第 68 轮:移除 truncation_resumes==0 约束。
                if should_nudge_web_ops(&self.profile.tools.names(), iter)
                    && !completion.text.trim().is_empty()
                {
                    info!(
                        iter = iter,
                        "WebUse 无工具调用,注入 nudge 强制 LLM 使用浏览器操控工具"
                    );
                    session
                        .context_mut()
                        .push(ChatMessage::user(web_ops_nudge_text(iter)));
                    continue;
                }

                // 检测截断:输出被 token 上限截断时自动续接
                if is_truncation_stop_reason(completion.stop_reason.as_deref()) {
                    // max_tokens 静默升级(2026-09-09 第 09 轮,L1037):
                    // LLM 返回截断时,翻倍下一次请求的 max_tokens 上限(封顶 64K),
                    // 下一轮 LLM 调用通过 `meta.max_tokens_override` 自动注入。
                    let old_max = max_tokens_state.current();
                    let new_max = max_tokens_state.note_truncated();
                    if new_max != old_max {
                        info!(
                            old = old_max,
                            new = new_max,
                            "max_tokens 被截断,静默升级(下一轮 LLM 调用注入)"
                        );
                    }
                    meta.max_tokens_override = Some(new_max);

                    if truncation_resumes >= self.max_truncation_resume {
                        // 达到上限,返回已累计的文本(优雅降级)
                        warn!(
                            truncation_resumes = truncation_resumes,
                            "截断续接达到上限,返回已累计文本"
                        );
                        trace.truncation_resumes = truncation_resumes;
                        return Self::finalize_with_forced_flag(
                            trace,
                            &accumulated_text,
                            total_usage,
                            max_tokens_state.as_ref(),
                            forced_tool_maybe_effective,
                        );
                    }
                    // 注入 nudge 并续接
                    truncation_resumes += 1;
                    trace.truncation_resumes = truncation_resumes;
                    let nudge = format!(
                        "[输出被截断,请从断点继续。第 {}/{} 次续接]",
                        truncation_resumes, self.max_truncation_resume
                    );
                    info!(
                        truncation_resumes = truncation_resumes,
                        "检测到输出截断,自动续接"
                    );
                    session.context_mut().push(ChatMessage::user(nudge));
                    continue; // 继续循环
                }

                // 非截断,正常返回
                debug!("agent finished with text answer");
                return Self::finalize_with_forced_flag(
                    trace,
                    &accumulated_text,
                    total_usage,
                    max_tokens_state.as_ref(),
                    forced_tool_maybe_effective,
                );
            }

            // 2026-09-16 第 68 轮 P1-B:记录首迭代 forced tool 是否真正生效。
            // 如果 iter=0 且设置了 first_iter_forced_tool,检查 LLM 返回的 tool_calls
            // 中是否包含 forced tool 名;若包含则标记 true,否则保持 false(说明被降级或忽略)。
            if iter == 0 {
                if let Some(ref forced_name) = self.first_iter_forced_tool {
                    let effective = completion
                        .tool_calls
                        .iter()
                        .any(|c| c.name == *forced_name);
                    if let Some(ref mut flag) = forced_tool_maybe_effective {
                        *flag = effective;
                    }
                    debug!(forced_tool = %forced_name, effective = effective, "首迭代 forced 效果检测");
                }
            }

            // 记录 assistant 的工具调用请求(同时附带文本,如果有)
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if !completion.text.is_empty() {
                assistant_blocks.push(ContentBlock::text(completion.text.clone()));
            }
            for call in &completion.tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.arguments.clone(),
                });
            }
            session
                .context_mut()
                .push(ChatMessage::assistant(assistant_blocks));

            // 2026-09-16 第 68 轮 P1-C:有工具调用时重置连续无工具调用计数。
            // (必须在工具执行前重置,因为工具执行中可能因取消/失败提前退出)
            consecutive_no_tool_rounds = 0;

            // 关联报告: 2026-09-09_05 E-001 —— 快照本轮文本/工具状态供后续短路判断使用
            // (completion.text 与 completion.tool_calls 在下面的循环会被 move)
            let this_round_text_empty = completion.text.trim().is_empty();
            let this_round_had_tool_calls = !completion.tool_calls.is_empty();

            // 逐个执行工具并把结果回填上下文(失败也作为 tool_result,is_error=true)
            let mut any_success_this_round = false;
            // 结构化输出通道命中标记(L6/L19,2026-09-09 第 13 轮):emit 工具的
            // tool_use 不执行,input 即最终结构化结果;本轮剩余调用只回填不执行
            // (防孤儿 tool_use,对齐第五轮「工具结果回填」专题)。
            let mut structured_output: Option<String> = None;
            for call in completion.tool_calls {
                let name = call.name.clone();
                let id = call.id.clone();
                let args = call.arguments;

                // ---- 结构化输出通道短路(先于 schema 预校验与执行) ----
                if self.profile.emit_tool.as_deref() == Some(name.as_str()) {
                    let json =
                        serde_json::to_string_pretty(&args).unwrap_or_else(|_| args.to_string());
                    // 回填 tool_result 保持上下文配对(assistant tool_use ↔ tool_result)
                    session.context_mut().push(ChatMessage::tool_result(
                        id,
                        "(结构化输出已接收,循环终止)",
                        false,
                    ));
                    trace.structured_emits += 1;
                    info!(tool = %name, "结构化输出通道命中,短路终止 Agent 循环");
                    structured_output = Some(json);
                    continue;
                }
                if structured_output.is_some() {
                    // emit 已命中:本轮剩余并行调用不再执行,仅回填防孤儿
                    session.context_mut().push(ChatMessage::tool_result(
                        id,
                        "(已忽略:结构化输出已接收,循环终止)",
                        false,
                    ));
                    continue;
                }

                info!(tool = %name, "executing tool");

                // 工具执行取消:select 命中后工具 future 被 drop——Bash 工具的
                // `kill_on_drop` 会随之 SIGKILL 子进程、setsid+killpg 清理整组
                // (知识库第五轮 §6.1「取消必须级联到进程组」,bash.rs ef84cec 已就位)
                //
                // ★ Schema 预校验(L16):工具执行前校验参数类型/必填/越界/多余字段,
                // 校验失败直接返回结构化错误,避免浪费一次工具执行往返
                let executed = match self.profile.tools.get(&name) {
                    Ok(tool) => {
                        // Schema 预校验(校验失败 → 返回错误,不执行工具)
                        if let Err(e) = crate::agent::tool_schema_validator::validate_tool_args(
                            &name,
                            &tool.parameters(),
                            &args,
                        ) {
                            Some(Err(e))
                        } else {
                            match cancel {
                                Some(token) => {
                                    tokio::select! {
                                        biased;
                                        _ = token.cancelled() => None,
                                        r = tool.execute(args.clone()) => Some(r),
                                    }
                                }
                                None => Some(tool.execute(args.clone()).await),
                            }
                        }
                    }
                    Err(e) => Some(Err(e)),
                };
                // 2026-09-16 第 57 轮:工具调用墙钟计时 —— 从执行入口开始,
                // 不论 Ok/Err/取消都走 elapsed.as_millis() 取时长,记入
                // ExecutionTrace.tool_call_log.elapsed_ms,供 TUI 反推卡在哪一步。
                let tool_call_started = std::time::Instant::now();
                let (output, is_error, error_summary) = match executed {
                    Some(Ok(out)) => (out, false, String::new()),
                    Some(Err(e)) => {
                        warn!(tool = %name, error = %e, "tool failed");
                        // F1(2026-09-14 第 51 轮):工具不存在时,回填文本明示
                        // 可用工具边界——auto tool_choice 降级后模型可能尝试
                        // 越权工具(如 Yolo 调 Bash),裸错误「工具不存在」不足以
                        // 让模型收敛,导致同一轮内反复试错浪费迭代。
                        let brief = format!("{e}");
                        let brief_short: String = brief.chars().take(120).collect();
                        if matches!(e, AgentError::ToolNotFound(_)) {
                            let avail = self.profile.tools.names().join(", ");
                            (
                                format!(
                                    "[工具执行失败] {name}: {e}。你当前可用的工具仅有: [{avail}]。\
                                     禁止再次调用 {name};请立即改用上述可用工具完成任务,或直接给出最终回答。"
                                ),
                                true,
                                format!("ToolNotFound: {brief_short}"),
                            )
                        } else {
                            (
                                format!("[工具执行失败] {}: {}", name, e),
                                true,
                                format!("{name}: {brief_short}"),
                            )
                        }
                    }
                    // 取消:本条 + 本轮剩余未执行的 tool_use 由 backfill 统一补全
                    None => {
                        backfill_cancelled_tool_results(session.context_mut());
                        return Err(AgentError::Cancelled);
                    }
                };
                // 2026-09-11 第三十六轮 LA-2:无论工具返回成功与否,
                // bash 工具的输出文本都含 `<exit_code>N</exit_code>`,
                // 即使工具返回 Ok(exit_code=1)也累计 trace 失败信号,
                // 让 QC 看到工具层真实失败证据(典型场景:python3 抛 RuntimeError)。
                // 2026-09-11 第三十八轮 BUG-1(LA-4):采集收敛到 record_bash_exit_code ——
                // 成功(code=0)也必须刷新 last_bash_exit_code,否则「先失败后成功」
                // 的预期负例单元永远过不了 QC trace 门的 last==0 豁免条件。
                // 守卫 code >= 0:无标签时(-1)不覆盖历史退出码。
                if name == "Bash" {
                    trace.record_bash_exit_code(&output);
                }
                if is_error {
                    trace.tool_calls_err += 1;
                    // 失败键:工具名 + 稳定 JSON(对象按 key 排序后序列化)
                    let fail_key = format!("{}|{}", name, stable_json_string(&args));
                    if last_fail_key.as_deref() == Some(fail_key.as_str()) {
                        consecutive_failures += 1;
                    } else {
                        last_fail_key = Some(fail_key);
                        consecutive_failures = 1;
                    }
                    trace.max_consecutive_failures =
                        trace.max_consecutive_failures.max(consecutive_failures);
                    if consecutive_failures >= REPEATED_FAILURE_THRESHOLD {
                        warn!(
                            tool = %name,
                            consecutive = consecutive_failures,
                            "检测到连续相同失败调用,提前终止以避免耗尽迭代"
                        );
                        trace.early_terminated = true;
                        trace.early_terminate_reason =
                            format!("tool={} attempts={}", name, consecutive_failures);
                        return Err(AgentError::RepeatedToolFailure {
                            tool: name,
                            attempts: consecutive_failures,
                            last_error: output,
                        });
                    }
                } else {
                    trace.tool_calls_ok += 1;
                    any_success_this_round = true;
                    // 产物采集(2026-09-09 第 15 轮):Write 成功落盘时记录 路径+字节数,
                    // 让 QC 能看到"成果在文件里"而非仅凭收尾文本 output_bytes 误判。
                    if name == "Write" && trace.artifacts.len() < ARTIFACTS_LIMIT {
                        let path = args["file_path"].as_str().unwrap_or("?");
                        let bytes = args["content"].as_str().map(|c| c.len()).unwrap_or(0);
                        trace.artifacts.push(format!("Write {path} ({bytes}B)"));
                    }
                }
                trace.tool_calls += 1;
                // 2026-09-16 第 56 轮:工具调用日志(供 WindowUseRunner 恢复窗口状态 +
                // QC 拿到真实调用证据);FIFO 上限 MAX_TOOL_CALL_LOG。
                // 第 57 轮:补 elapsed_ms(墙钟耗时)与 error_summary(失败原因摘要),
                // TUI 据此反推「哪个工具哪一步卡死 / 为何失败」。
                let tool_call_elapsed_ms = tool_call_started.elapsed().as_millis() as u64;
                trace.record_tool_call(
                    &name,
                    &stable_json_string(&args),
                    !is_error,
                    output.len(),
                    tool_call_elapsed_ms,
                    &error_summary,
                    if is_error { "" } else { output.as_str() },
                );
                // 关联报告: 2026-09-09_06 F-002 — 累计最近工具调用历史
                let args_digest = crate::agent::extrace::compact_args_digest(&args);
                recent_tool_history.push((name.clone(), args_digest, is_error));
                if recent_tool_history.len() > RECENT_TOOL_HISTORY_LIMIT {
                    let drop_n = recent_tool_history.len() - RECENT_TOOL_HISTORY_LIMIT;
                    recent_tool_history.drain(0..drop_n);
                }
                session
                    .context_mut()
                    .push(ChatMessage::tool_result(id, output, is_error));
            }
            if any_success_this_round {
                // 任一成功调用重置失败计数
                last_fail_key = None;
                consecutive_failures = 0;
            }

            // 结构化输出通道命中 → 立即终止循环(L6/L19,2026-09-09 第 13 轮)。
            // emit input 序列化为 ```json 代码块追加到累计文本,下游解析链
            // (`parse_classification` / `parse_quality_report` 的 extract_json_block)
            // 直接命中,无需感知本机制的存在。
            if let Some(json) = structured_output {
                if !accumulated_text.is_empty() {
                    accumulated_text.push('\n');
                }
                accumulated_text.push_str(&format!("```json\n{json}\n```"));
                debug!("agent finished with structured tool_use output");
                return Self::finalize_with_forced_flag(
                    trace,
                    &accumulated_text,
                    total_usage,
                    max_tokens_state.as_ref(),
                    forced_tool_maybe_effective,
                );
            }

            // 关联报告: 2026-09-09_05 E-001 —— 无文本收敛计数
            // 本轮:有 tool_use 但 completion.text 为空(LLM 只输出工具调用指令没附文本)
            // → 增加计数;否则(有文本 / 无工具调用)归零
            // 注:completion.text 与 completion.tool_calls 在前面循环已被消费,
            // 这里通过本轮迭代开始时记录的 snapshot 判断。
            if this_round_text_empty && this_round_had_tool_calls {
                consecutive_no_text_rounds += 1;
            } else {
                consecutive_no_text_rounds = 0;
            }
            if consecutive_no_text_rounds >= NO_TEXT_CONVERGE_THRESHOLD {
                // 关联报告: 2026-09-09_06 F-002 — 把「最近工具调用叙事化摘要」一并
                // 注入,让 LLM 看到自己刚才做了什么(而非纯次数统计),下次任务能自我纠正。
                let history_lines: Vec<String> = recent_tool_history
                    .iter()
                    .enumerate()
                    .map(|(i, (tool, args, err))| {
                        let status = if *err { "失败" } else { "成功" };
                        format!("  [{:>2}] {} {} → {}", i + 1, status, tool, args)
                    })
                    .collect();
                let history_block = if history_lines.is_empty() {
                    "  (无工具调用历史)".to_string()
                } else {
                    history_lines.join("\n")
                };
                if !no_text_grace_used {
                    // F9 宽限轮:nudge 真正送达 LLM,工具调用照常执行一轮
                    no_text_grace_used = true;
                    warn!(
                        consecutive_no_text_rounds = consecutive_no_text_rounds,
                        "检测到连续无文本收敛轮次,注入 nudge 宽限一轮(观察 LLM 自纠)"
                    );
                    let narrative = format!(
                        "[无文本收敛已达上限 {} 轮,关联报告 2026-09-09_06 F-002]\n\
                         你本会话累计执行 {total_tc} 次工具调用(成功 {total_ok},失败 {total_err}),\
                         最近 {hist_len} 次摘要:\n{history}\n\
                         请基于以上观察立即推进任务核心步骤(如写入/修改目标产物)或直接给出结论性答复;\
                         若任务已无法继续,请明确说明卡点。",
                        NO_TEXT_CONVERGE_THRESHOLD,
                        total_tc = trace.tool_calls,
                        total_ok = trace.tool_calls_ok,
                        total_err = trace.tool_calls_err,
                        hist_len = recent_tool_history.len(),
                        history = history_block,
                    );
                    session.context_mut().push(ChatMessage::user(narrative));
                    consecutive_no_text_rounds = 0;
                    continue;
                }
                warn!(
                    consecutive_no_text_rounds = consecutive_no_text_rounds,
                    "宽限轮后仍无文本收敛,提前终止以避免耗尽迭代"
                );
                trace.early_terminated = true;
                trace.early_terminate_reason =
                    format!("no_text_converge rounds={}", consecutive_no_text_rounds);
                let narrative = format!(
                    "[无文本收敛已达上限 {} 轮(宽限轮已用),关联报告 2026-09-09_06 F-002]\n\
                     你本会话累计执行 {total_tc} 次工具调用(成功 {total_ok},失败 {total_err}),\
                     最近 {hist_len} 次摘要:\n{history}\n\
                     请基于以上观察直接给出结论性答复;若任务已无法继续,请明确说明卡点。",
                    NO_TEXT_CONVERGE_THRESHOLD,
                    total_tc = trace.tool_calls,
                    total_ok = trace.tool_calls_ok,
                    total_err = trace.tool_calls_err,
                    hist_len = recent_tool_history.len(),
                    history = history_block,
                );
                session.context_mut().push(ChatMessage::user(narrative));
                // 兜底文本:同步告知用户层
                let fallback_text = format!(
                    "[SubAgent 已达无文本收敛上限 {} 轮(含宽限轮)]\n\
                     - 共执行 {tc} 次工具调用(成功 {ok},失败 {err})\n\
                     - 最近 {hist_len} 次摘要:\n{history}",
                    NO_TEXT_CONVERGE_THRESHOLD,
                    tc = trace.tool_calls,
                    ok = trace.tool_calls_ok,
                    err = trace.tool_calls_err,
                    hist_len = recent_tool_history.len(),
                    history = history_block,
                );
                return Self::finalize_with_forced_flag(
                    trace,
                    &fallback_text,
                    total_usage,
                    max_tokens_state.as_ref(),
                    forced_tool_maybe_effective,
                );
            }
        }

        Err(AgentError::MaxIterationsExceeded(self.max_iterations))
    }

    /// 单次 LLM 调用 + 上下文溢出三级恢复(L1038/L1044,第 06 轮)。
    ///
    /// 溢出错误(`prompt is too long` / `context_length_exceeded` 类 400)时:
    /// - Level 1 排水:截短超长 tool_result 后**立即重试**(本次调用内,不消耗
    ///   `max_iterations` 预算);
    /// - Level 2 折叠:排水无效时把非保护段历史合并为压缩摘要后重试;
    /// - Level 3 暴露:两轮无效 / 全会话恢复预算(`max_overflow_recoveries`)用完
    ///   → 原错误上抛,进入既有 Yolo 失败回流。
    ///
    /// 每次调用最多尝试 1 次排水 + 1 次折叠(跨循环迭代重新武装,新的大工具
    /// 结果可再次排水);取消语义不变(select 命中即 backfill + `Err(Cancelled)`)。
    ///
    /// `system` 由调用方在循环外构造(已拼 runtime hints),重试循环内复用,
    /// 避免 hints 在每次重试重新计算 + 字符串拼接。
    async fn complete_with_overflow_recovery(
        &self,
        session: &mut Session,
        cancel: Option<&CancelToken>,
        system: &str,
        tool_defs: &[crate::llm::ToolDef],
        meta: &RequestMeta,
        trace: &mut ExecutionTrace,
    ) -> Result<Completion> {
        // 本次调用内的恢复档位状态(排水 / 折叠各最多尝试一次)
        let mut drained = false;
        let mut folded = false;
        loop {
            // None = 取消命中(completion 未返回,本轮无新 tool_use;
            // 借用约束:select 分支 future 持有 session 不可变借用,backfill 需在 select 外做)
            let outcome: Option<Result<Completion>> = match cancel {
                Some(token) => tokio::select! {
                    biased;
                    _ = token.cancelled() => None,
                    r = self.llm.complete(system, session.context(), tool_defs, meta) => Some(r),
                },
                None => Some(
                    self.llm
                        .complete(system, session.context(), tool_defs, meta)
                        .await,
                ),
            };
            let result = match outcome {
                Some(r) => r,
                None => {
                    backfill_cancelled_tool_results(session.context_mut());
                    return Err(AgentError::Cancelled);
                }
            };
            let completion = match result {
                Ok(c) => return Ok(c),
                Err(e) => e,
            };

            // 非溢出错误 / 恢复预算已用完 → 原样上抛
            if !overflow::is_context_overflow(&completion)
                || trace.overflow_recoveries >= self.max_overflow_recoveries
            {
                return Err(completion);
            }
            // Level 1 排水(尚未尝试过且确有可排水内容)
            let action = if !drained {
                drained = true;
                let n = overflow::drain_tool_results(session.context_mut());
                (n > 0).then(|| format!("排水:截短 {n} 个超长工具结果"))
            } else {
                None
            };
            // Level 2 折叠(排水无效 / 已排水过)
            let action = match action {
                Some(desc) => Some(desc),
                None if !folded => {
                    folded = true;
                    overflow::fold_history(session)
                        .map(|n| format!("折叠:合并 {n} 条历史消息为压缩摘要"))
                }
                None => None,
            };
            match action {
                Some(desc) => {
                    trace.overflow_recoveries += 1;
                    warn!(
                        recoveries = trace.overflow_recoveries,
                        action = %desc,
                        error = %completion,
                        "上下文溢出,本地恢复后重试"
                    );
                    continue; // 恢复后立即重试(同一次调用内的内部重试)
                }
                None => {
                    warn!(
                        error = %completion,
                        "上下文溢出且本地恢复手段已穷尽,上抛错误(三级暴露)"
                    );
                    return Err(completion);
                }
            }
        }
    }

    /// 同步填充 trace 的输出字节数 + 失败信号 + max_tokens 升级历史(2026-09-09 第 09 轮)
    /// 后返回 Ok 三元组(在循环正常结束后调用,异常路径由调用方继续包装)。
    ///
    /// `forced_tool_effective` 由调用方传入(None = 未设置 forced tool;
    /// Some(true/false) = forced tool 是否被 LLM 真正执行)。
    pub(super) fn finalize_with_max_tokens(
        mut trace: ExecutionTrace,
        text: &str,
        total_usage: Usage,
        max_tokens_state: &MaxTokensState,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        trace.output_bytes = text.len();
        trace.max_tokens_upscalings = max_tokens_state.upscalings();
        trace.max_tokens_history = max_tokens_state.history_snapshot();
        trace.collect_failure_signals(text);
        Ok((text.to_string(), total_usage, trace))
    }

    /// 2026-09-16 第 68 轮 P1-B:包装 finalize,写入 forced_tool_effective 字段。
    /// 所有正常/提前返回路径统一经本函数,确保 trace.forced_tool_effective 被填充。
    fn finalize_with_forced_flag(
        mut trace: ExecutionTrace,
        text: &str,
        total_usage: Usage,
        max_tokens_state: &MaxTokensState,
        forced_tool_effective: Option<bool>,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        trace.forced_tool_effective = forced_tool_effective;
        Self::finalize_with_max_tokens(trace, text, total_usage, max_tokens_state)
    }
}
