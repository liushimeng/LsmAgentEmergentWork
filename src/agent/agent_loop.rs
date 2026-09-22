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

    /// 循环主体 + 动态子 Agent 作用域(2026-09-22 第 114 轮)。
    ///
    /// 支持动态启动的角色(见 `AgentProfile::spawn_policy`)在进入循环前建立
    /// `tokio::task_local` 运行时作用域:作用域内 LLM 调用 `SubAgent` 工具时,
    /// 工具经 `dynamic_subagent::current()` 取到 LLM 客户端 / 深度 / 预算 / 取消 token。
    /// 不参与的角色(或 `LAEW_SELF_SPAWN=off`)返回 `None` → 原样直通,**零开销**。
    ///
    /// 注:原循环体一字未改,只是改名为 [`Self::run_session_body`] 并被本函数包裹。
    async fn run_session_inner(
        &self,
        session: &mut Session,
        cancel: Option<&CancelToken>,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        match crate::agent::dynamic_subagent::runtime_for(
            &self.profile.name,
            &self.profile,
            self.llm.clone(),
            session.id(),
            cancel,
        ) {
            Some(rt) => {
                crate::agent::dynamic_subagent::scope(rt, self.run_session_body(session, cancel))
                    .await
            }
            None => self.run_session_body(session, cancel).await,
        }
    }

    /// 循环主体(`run_session` / `run_session_cancellable` 共用)。
    async fn run_session_body(
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

        // ★ 第 82 轮 P1-3(第 89 轮单工具化):浏览器失败学习 —— 同一 (tool, selector)
        // 连续失败 ≥2 次时,自动在 error_summary 追加换姿势提示(scroll_into_view /
        // wait / 换 selector / eval_js),避免 LLM 在 16 次迭代里用同一 selector 反复重试。
        // 用 HashMap 维护累计计数;selector 从 MCP_Web_Use 的 args.params.selector
        // 字段提取(兼容顶层 args.selector),非浏览器工具不参与。
        use std::collections::HashMap;
        let mut browser_failure_counts: HashMap<(String, String), u32> = HashMap::new();

        // 2026-09-16 第 68 轮 P1-C:连续无工具调用计数 —— 连续 N 轮无 tool_use 时
        // 提前终止,避免专项任务空跑跑满 max_iterations。
        const NO_TOOL_USE_THRESHOLD: usize = 3;
        let mut consecutive_no_tool_rounds: usize = 0;

        // Agent 会话开始(2026-09-17 第 70 轮运行日志):--debug/--info 日志文件的
        // 角色级起点事件;所有角色都经本函数,一处埋点全角色覆盖。
        info!(
            agent = %self.profile.name,
            session = %session.id(),
            max_iterations = self.max_iterations,
            tools = %self.profile.tools.names().join(","),
            "Agent 会话开始"
        );

        for iter in 0..self.max_iterations {
            trace.iterations = iter + 1;
            // 第 118 轮:探索预算耗尽时在 trace 上打标,build_runtime_hints 会在
            // 后续 system 拼接时追加「进入执行期」提示到 LLM 上下文,引导 LLM 收敛
            // 到 input_text/click/wait 等写操作,减少 inspect/screenshot/eval_js 重复探查。
            // 预算 = 0 关闭该机制(等同旧行为,见 Agent::with_explore_budget)。
            if self.explore_budget > 0 && iter == self.explore_budget {
                trace.explore_budget_exhausted = true;
                debug!(
                    iteration = iter,
                    explore_budget = self.explore_budget,
                    "explore_budget 耗尽,进入执行期提示"
                );
            }
            // 迭代边界:取消检查(轻量 is_cancelled,热路径零 await 开销)
            if let Some(token) = cancel {
                if token.is_cancelled() {
                    backfill_cancelled_tool_results(session.context_mut());
                    return Err(AgentError::Cancelled);
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
            // LLM 请求(2026-09-17 第 70 轮运行日志):--debug 级记录每轮请求元信息,
            // 排查「模型看到了什么」(消息条数/system 规模/强制工具/输出上限)。
            debug!(
                agent = %self.profile.name,
                iter,
                messages = session.context().len(),
                system_chars = system.chars().count(),
                forced_tool = meta.forced_tool.as_deref().unwrap_or(""),
                max_tokens = meta.max_tokens_override.unwrap_or(0),
                "LLM 请求"
            );
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

            // LLM 响应(2026-09-17 第 70 轮运行日志):思考文本(assistant 可见文本)+
            // 工具调用意图逐条记录 —— 排查「模型想了什么、打算做什么」的核心事件。
            {
                let calls_desc = completion
                    .tool_calls
                    .iter()
                    .map(|c| {
                        format!(
                            "{}#{} args={}",
                            c.name,
                            c.id,
                            crate::logging::clip(&c.arguments.to_string())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                debug!(
                    agent = %self.profile.name,
                    iter,
                    stop_reason = completion.stop_reason.as_deref().unwrap_or(""),
                    usage_in = completion.usage.input_tokens,
                    usage_out = completion.usage.output_tokens,
                    cache_read = completion.usage.cache_read_input_tokens,
                    thinking = %crate::logging::clip(&completion.text),
                    tool_calls = %calls_desc,
                    "LLM 响应"
                );
            }

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

                // 2026-09-16 第 68 轮 P1-C:连续无工具调用计数 —— 避免专项任务
                // 在引导失效时跑满 max_iterations。
                // 阈值 NO_TOOL_USE_THRESHOLD=3:给 LLM 3 次机会(含 nudge 引导),仍无工具
                // 调用则提前终止,把失败信息返回 Runner/QC 而不是空跑 16 轮。
                consecutive_no_tool_rounds += 1;
                if consecutive_no_tool_rounds >= NO_TOOL_USE_THRESHOLD {
                    // 2026-09-17 第 70 轮:已有工具调用产出时降级为「优雅收尾」。
                    // 第 68 轮 P1-C 的 no_tool_use 硬失败针对「全程 0 工具调用的空跑」;
                    // 但浏览器类任务的常见合法路径是「先完成工具动作、再输出终答文本」,
                    // 硬失败会把真实成果被 early_terminate 吞掉(e2e 5e-1 回归)。
                    // 改为正常 finalize 交 QC 判定,仅 trace.tool_calls == 0 时保留
                    // 硬失败语义(防空跑 max_iterations,第 68 轮本意)。
                    if trace.tool_calls > 0 {
                        info!(
                            rounds = consecutive_no_tool_rounds,
                            tool_calls = trace.tool_calls,
                            "连续文本轮但本单元已有工具调用,按正常收尾处理(交 QC 判定)"
                        );
                        return Self::finalize_logged(
                            &self.profile.name,
                            trace,
                            &accumulated_text,
                            total_usage,
                            max_tokens_state.as_ref(),
                        );
                    }
                    warn!(
                        rounds = consecutive_no_tool_rounds,
                        total_iters = iter + 1,
                        "连续多轮无工具调用,提前终止以避免空跑 max_iterations"
                    );
                    trace.early_terminated = true;
                    trace.early_terminate_reason =
                        format!("no_tool_use_{}_rounds", consecutive_no_tool_rounds);
                    trace.collect_failure_signals(&accumulated_text);
                    return Self::finalize_logged(
                        &self.profile.name,
                        trace,
                        &accumulated_text,
                        total_usage,
                        max_tokens_state.as_ref(),
                    );
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
                        return Self::finalize_logged(
                            &self.profile.name,
                            trace,
                            &accumulated_text,
                            total_usage,
                            max_tokens_state.as_ref(),
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
                return Self::finalize_logged(
                    &self.profile.name,
                    trace,
                    &accumulated_text,
                    total_usage,
                    max_tokens_state.as_ref(),
                );
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

                // 工具调用(2026-09-17 第 70 轮运行日志):名称 + 参数(截断)入日志,
                // 所有角色所有工具(Bash/Read/Write/Window*/Browser*)统一经此记录。
                info!(
                    agent = %self.profile.name,
                    iter,
                    tool = %name,
                    args = %crate::logging::clip(&stable_json_string(&args)),
                    "工具调用"
                );

                // 工具执行取消:select 命中后工具 future 被 drop——Bash 工具的
                // `kill_on_drop` 会随之 SIGKILL 子进程、setsid+killpg 清理整组
                // (知识库第五轮 §6.1「取消必须级联到进程组」,bash.rs ef84cec 已就位)
                //
                // ★ Schema 预校验(L16):工具执行前校验参数类型/必填/越界/多余字段,
                // 校验失败直接返回结构化错误,避免浪费一次工具执行往返
                //
                // ★ 第 82 轮 P0-1:工具调用墙钟计时 —— 必须放在 tool.execute() 之前,
                // 否则 Instant::now() 在 future 已 await 完成后才被采样,elapsed_ms 永远为 0
                // (原第 57 轮修复把计时器放错位置,实测所有工具耗时都显示 0ms,
                //  见 tmpPlan/2026-09-17_13 §3.1)。改为在拿到 tool 句柄后立即采样。
                let tool_call_started = std::time::Instant::now();
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
                // ★ 第 82 轮:Instant 已上移至 tool.execute 之前,这里只读 elapsed。
                // (label `_ = tool_call_started` 仅用来压制 unused warning)
                debug_assert!(tool_call_started.elapsed().as_nanos() > 0);
                let (output, is_error, mut error_summary) = match executed {
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
                // ★ 第 82 轮 P1-3(第 89 轮单工具化):浏览器失败学习 —— 同一 (tool, selector)
                // 连续失败 ≥2 次时,在 error_summary 追加换姿势提示,避免 LLM 死磕同 selector。
                // 适用工具:MCP_Web_Use(从 args.params.selector / args.selector 提 key);
                // 非浏览器工具不参与。失败计数实时递增,成功调用同一 key 不重置(简化)。
                if is_error && name.as_str() == "MCP_Web_Use" {
                    let sel_key = args
                        .get("params")
                        .and_then(|p| p.get("selector"))
                        .or_else(|| args.get("selector"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<no_selector>".to_string());
                    let map_key = (name.clone(), sel_key);
                    let count = browser_failure_counts.entry(map_key).or_insert(0);
                    *count += 1;
                    if *count >= 2 {
                        let hint = format!(
                            " [提示:已连续失败 {} 次,建议:1) scroll_into_view 2) MCP_Web_Use(control_action=wait) 等 500ms 3) 换 selector 或加 nth=N 4) eval_js 直接触发]",
                            *count
                        );
                        error_summary.push_str(&hint);
                        warn!(
                            tool = %name,
                            selector = %args
                                .get("params")
                                .and_then(|p| p.get("selector"))
                                .or_else(|| args.get("selector"))
                                .and_then(|v| v.as_str())
                                .unwrap_or(""),
                            consecutive = *count,
                            "浏览器工具连续失败,自动追加换姿势提示"
                        );
                    }
                }
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
                // 工具结果(2026-09-17 第 70 轮运行日志):成功/失败都记录,且早于
                // RepeatedToolFailure 等提前返回,保证失败调用的结果也不丢日志。
                info!(
                    agent = %self.profile.name,
                    iter,
                    tool = %name,
                    is_error,
                    elapsed_ms = tool_call_started.elapsed().as_millis() as u64,
                    output = %crate::logging::clip(&output),
                    "工具结果"
                );
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
                            // 2026-09-17 第 75 轮:携带真实 trace,Runner 不再丢工具调用历史
                            trace: Box::new(trace),
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
                // 2026-09-16 第 56 轮:工具调用日志(供执行器 trace 恢复与
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
                // 2026-09-17 第 74 轮(第 89 轮单工具化):MCP_Web_Use post-open 引导
                // (放在 push 之前,这样可以借用 output 不需要 clone)。action=open 成功
                // 返回的 next_steps 是关键指引(4 步最常见动作),注入 LLM 上下文作为
                // user message,显著降低「多次迭代 tool_calls=0」类失败模式的概率。
                let web_open_hint = if name == "MCP_Web_Use"
                    && !is_error
                    && args.get("action").and_then(|v| v.as_str()) == Some("open")
                {
                    if let Some(page_id) =
                        crate::agent::tools::mcp_web_use::extract_page_id_from_text(&output)
                    {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&output) {
                            v.get("data")
                                .and_then(|d| d.get("next_steps"))
                                .map(|next_steps| {
                                    format!(
                                        "【MCP_Web_Use 浏览器启动后续步骤引导】\n\
                                         action=open 成功 → page_id={page_id}。\n\
                                         严格按以下 4 步执行(next_steps 已为你准备好 selector_hint):\n\
                                         {next_steps}\n\
                                         说明:\n\
                                         - 每次 control/inspect 都要带 page_id={page_id}\n\
                                         - 如果某一步 selector_hint 不命中,改用 inspect(info=elements) 查看真实 DOM\n\
                                         - 按钮是图片(img 元素)时,可直接 click(img 元素) 也能触发提交\n\
                                         - 输入框有 JS 框架(React/Vue)拦截时,改 use_js=false 走 sendkeys 路径",
                                        page_id = page_id,
                                        next_steps = serde_json::to_string_pretty(next_steps)
                                            .unwrap_or_else(|_| next_steps.to_string()),
                                    )
                                })
                        } else { None }
                    } else { None }
                } else { None };
                session
                    .context_mut()
                    .push(ChatMessage::tool_result(id, output, is_error));
                if let Some(hint) = web_open_hint {
                    session.context_mut().push(ChatMessage::user(&hint));
                }
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
                return Self::finalize_logged(
                    &self.profile.name,
                    trace,
                    &accumulated_text,
                    total_usage,
                    max_tokens_state.as_ref(),
                );
            }

            // 关联报告: 2026-09-09_05 E-001 —— 无文本收敛计数
            // 本轮:有 tool_use 但 completion.text 为空(LLM 只输出工具调用指令没附文本)
            // → 增加计数;否则(有文本 / 无工具调用)归零
            // 注:completion.text 与 completion.tool_calls 在前面循环已被消费,
            // 这里通过本轮迭代开始时记录的 snapshot 判断。
            //
            // 第 103 轮:MCP_Web_Use(浏览器)与 MCP_Window_Use(桌面窗口)任务天然需要
            // 多轮 tool_use(open→inspect→control)才能产出最终结果。若本轮有浏览器/窗口
            // 工具调用且返回 code=0(成功获取到页面数据/窗口信息),视为"有效产出轮",不纳入
            // 无文本收敛计数,避免合法的多步浏览器任务被提前终止。
            let browser_tool_productive = if this_round_text_empty && this_round_had_tool_calls {
                // 检查最近工具调用中是否有成功的 MCP_Web_Use / MCP_Window_Use
                // 浏览器/窗口任务天然多轮 tool_use,成功调用即视为有效产出
                recent_tool_history.iter().rev().take(3).any(|(tool, _args, err)| {
                    !err && (tool == "MCP_Web_Use" || tool == "MCP_Window_Use")
                })
            } else {
                false
            };
            if this_round_text_empty && this_round_had_tool_calls && !browser_tool_productive {
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
                return Self::finalize_logged(
                    &self.profile.name,
                    trace,
                    &fallback_text,
                    total_usage,
                    max_tokens_state.as_ref(),
                );
            }
        }

        // 2026-09-17 第 75 轮:携带中断时刻的真实 trace(工具调用历史/失败信号),
        // Runner 侧不再用 default 兜底丢证据。
        Err(AgentError::MaxIterationsExceeded {
            iterations: self.max_iterations,
            trace: Box::new(trace),
        })
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

    /// finalize 包装:所有正常/提前返回路径统一经本函数。
    ///
    /// 2026-09-17 第 70 轮:同时作为 Agent 会话结束日志的单一收口
    /// (迭代数 / 工具成败计数 / 提前终止原因 / 用量)。
    fn finalize_logged(
        agent_name: &str,
        trace: ExecutionTrace,
        text: &str,
        total_usage: Usage,
        max_tokens_state: &MaxTokensState,
    ) -> Result<(String, Usage, ExecutionTrace)> {
        info!(
            agent = agent_name,
            iterations = trace.iterations,
            tool_calls = trace.tool_calls,
            tool_ok = trace.tool_calls_ok,
            tool_err = trace.tool_calls_err,
            early_terminated = trace.early_terminated,
            early_reason = %trace.early_terminate_reason,
            usage_in = total_usage.input_tokens,
            usage_out = total_usage.output_tokens,
            output_chars = text.chars().count(),
            "Agent 会话结束"
        );
        Self::finalize_with_max_tokens(trace, text, total_usage, max_tokens_state)
    }
}
