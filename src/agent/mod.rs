//! Agent 核心循环:协议无关的 LLM 规划 -> 工具执行 -> 观察。
//!
//! 多 Agent 架构(6 角色):
//! - Yolo:入口层(任务识别 / 难度分级 / 失败回流)
//! - Plan:规划层(hard 档 Markdown 方案)
//! - Main-Work:流程层(WorkFlow 编排)
//! - SubAgent-Work:执行层(最小单元)
//! - Quality-Check:质检层
//! - SessionContext:会话层
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md`。

pub mod cancel;
pub mod compact;
pub mod context;
pub mod debug;
pub mod extrace;
pub mod json_repair;
pub mod main_work;
pub mod max_tokens_state;
pub mod memory;
pub mod orchestrator;
pub mod overflow;
pub mod permissions;
pub mod plan;
pub mod profile;
pub mod project_context;
pub mod quality;
pub mod safety;
pub mod sandbox_hook;
pub mod session_context;
pub mod subagent;
pub mod system_prompt;
pub mod tool_schema_validator;
pub mod tools;
pub mod yolo;

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::agent::cancel::{backfill_cancelled_tool_results, CancelToken};
use crate::agent::extrace::ExecutionTrace;
use crate::agent::max_tokens_state::MaxTokensState;
use crate::agent::profile::AgentProfile;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Completion, ContentBlock, LlmClient, RequestMeta, Usage};
use crate::session::Session;

const DEFAULT_MAX_ITERATIONS: usize = 16;

/// 默认最大截断续接次数(对齐 AtomCode `MAX_TRUNCATION_RESUME = 4` 惯例)。
///
/// 当 LLM 输出因 token 上限被截断时(`stop_reason = "max_tokens"` Anthropic /
/// `"length"` OpenAI),自动注入 nudge 消息让模型从断点继续,最多续接此数值次。
const DEFAULT_MAX_TRUNCATION_RESUME: usize = 4;

/// 默认最大上下文溢出恢复次数(对齐截断续接的有界设计)。
///
/// 当 LLM 返回 prompt-too-long 类溢出错误时,自动执行「排水 → 折叠」两级本地
/// 恢复后重试;全会话累计恢复不超过此值,防止恢复与溢出之间打转。
const DEFAULT_MAX_OVERFLOW_RECOVERIES: usize = 4;

/// 一个可运行的 Agent 实例。
///
/// 持有 [`AgentProfile`](profile::AgentProfile)(名称 / 系统提示词 / 工具集),
/// 为后续多 Agent 切换预留扩展口。
pub struct Agent {
    llm: Arc<dyn LlmClient>,
    profile: AgentProfile,
    max_iterations: usize,
    /// 最大截断续接次数(输出被 token 上限截断时自动续接的上限)。
    max_truncation_resume: usize,
    /// 最大上下文溢出恢复次数(排水/折叠重试的全会话预算)。
    max_overflow_recoveries: usize,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, profile: AgentProfile) -> Self {
        Self {
            llm,
            profile,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_truncation_resume: DEFAULT_MAX_TRUNCATION_RESUME,
            max_overflow_recoveries: DEFAULT_MAX_OVERFLOW_RECOVERIES,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// 设置最大截断续接次数(测试 / 特殊场景用)。
    pub fn with_max_truncation_resume(mut self, n: usize) -> Self {
        self.max_truncation_resume = n;
        self
    }

    /// 设置最大上下文溢出恢复次数(测试 / 特殊场景用)。
    pub fn with_max_overflow_recoveries(mut self, n: usize) -> Self {
        self.max_overflow_recoveries = n;
        self
    }

    pub fn llm(&self) -> Arc<dyn LlmClient> {
        self.llm.clone()
    }
    pub fn profile(&self) -> &AgentProfile {
        &self.profile
    }
    pub fn max_iterations(&self) -> usize {
        self.max_iterations
    }
    pub fn max_truncation_resume(&self) -> usize {
        self.max_truncation_resume
    }
    pub fn max_overflow_recoveries(&self) -> usize {
        self.max_overflow_recoveries
    }

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

        for iter in 0..self.max_iterations {
            trace.iterations = iter + 1;
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
            let runtime_hints = build_runtime_hints(&trace, consecutive_failures);
            let system = if runtime_hints.is_empty() {
                base_system
            } else {
                format!("{base_system}{runtime_hints}")
            };
            // 上下文溢出自动恢复(L1038/L1044,2026-09-09 第 06 轮):
            // LLM 调用命中 prompt-too-long 类溢出错误时,自动执行
            // 排水(Level 1)→ 折叠(Level 2)→ 重试;两轮无效则上抛(三级暴露)。
            let completion: Completion = self
                .complete_with_overflow_recovery(session, cancel, &system, &tool_defs, &meta, &mut trace)
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
                        return Self::finalize_with_max_tokens(
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
                return Self::finalize_with_max_tokens(
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
                    let json = serde_json::to_string_pretty(&args)
                        .unwrap_or_else(|_| args.to_string());
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
                let (output, is_error) = match executed {
                    Some(Ok(out)) => (out, false),
                    Some(Err(e)) => {
                        warn!(tool = %name, error = %e, "tool failed");
                        (format!("[工具执行失败] {}: {}", name, e), true)
                    }
                    // 取消:本条 + 本轮剩余未执行的 tool_use 由 backfill 统一补全
                    None => {
                        backfill_cancelled_tool_results(session.context_mut());
                        return Err(AgentError::Cancelled);
                    }
                };
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
                return Self::finalize_with_max_tokens(
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
            if this_round_text_empty && this_round_had_tool_calls {
                consecutive_no_text_rounds += 1;
            } else {
                consecutive_no_text_rounds = 0;
            }
            if consecutive_no_text_rounds >= NO_TEXT_CONVERGE_THRESHOLD {
                warn!(
                    consecutive_no_text_rounds = consecutive_no_text_rounds,
                    "检测到连续无文本收敛轮次,注入 nudge 并提前终止以避免耗尽迭代"
                );
                trace.early_terminated = true;
                trace.early_terminate_reason =
                    format!("no_text_converge rounds={}", consecutive_no_text_rounds);
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
                let narrative = format!(
                    "[无文本收敛已达上限 {} 轮,关联报告 2026-09-09_06 F-002]\n\
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
                    "[SubAgent 已达无文本收敛上限 {} 轮]\n\
                     - 共执行 {tc} 次工具调用(成功 {ok},失败 {err})\n\
                     - 最近 {hist_len} 次摘要:\n{history}",
                    NO_TEXT_CONVERGE_THRESHOLD,
                    tc = trace.tool_calls,
                    ok = trace.tool_calls_ok,
                    err = trace.tool_calls_err,
                    hist_len = recent_tool_history.len(),
                    history = history_block,
                );
                return Self::finalize_with_max_tokens(
                    trace,
                    &fallback_text,
                    total_usage,
                    max_tokens_state.as_ref(),
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
    fn finalize_with_max_tokens(
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
}

/// 拼装运行时 hint(2026-09-09 第 09 轮,联动 L771 失败计数早期预警)。
///
/// 仅当对应计数器 > 0 时追加对应行,全 0 时返回空串(零开销)。
/// 拼到 system_prompt 末尾(不破坏 cache_control 缓存前缀;详见
/// 第七轮 PromptCaching 专题),让 LLM 自我感知「正在被短路保护」并主动收敛。
///
/// `<<<LAEW:RUNTIME_HINTS>>>` 标记保证幂等探测 + 与用户提示词严格隔离,
/// 与现有 `LAEW:PROJECT_CONTEXT` / `LAEW:SESSION_HISTORY` / `LAEW:COMPACTED_CONTEXT`
/// 标记风格一致。
pub(crate) fn build_runtime_hints(trace: &ExecutionTrace, consecutive_failures: usize) -> String {
    let mut hints: Vec<String> = Vec::new();
    if trace.truncation_resumes > 0 {
        hints.push(format!(
            "本会话已续接 {} 次截断输出(因 max_tokens 触发),如非必要请缩短回复或减少一次性工具调用。",
            trace.truncation_resumes
        ));
    }
    if trace.overflow_recoveries > 0 {
        hints.push(format!(
            "上下文已自动恢复 {} 次(排水/折叠历史),请避免一次性读取超大文件或拼装超长 prompt。",
            trace.overflow_recoveries
        ));
    }
    if trace.max_tokens_upscalings > 0 {
        hints.push(format!(
            "max_tokens 已升级 {} 次(当前 {} K),这是为解决截断自动翻倍;请控制单次回复长度。",
            trace.max_tokens_upscalings,
            trace.max_tokens_upscalings * 8 // 8K 起,展示近似值即可
        ));
    }
    if consecutive_failures >= 2 {
        hints.push(format!(
            "连续 {} 次工具调用失败,请先停下核对目标参数(路径/工具名/必填字段)再继续,避免在错误路径上重复打转。",
            consecutive_failures
        ));
    }
    if hints.is_empty() {
        return String::new();
    }
    format!(
        "\n\n<<<LAEW:RUNTIME_HINTS>>>\n{}\n<<<END>>>",
        hints.join("\n")
    )
}

/// 判断 `stop_reason` 是否为截断(输出被 token 上限截断)。
///
/// - Anthropic:`"max_tokens"` 表示输出达到 `max_tokens` 上限被截断
/// - OpenAI:`"length"` 表示输出达到 `max_tokens` 上限被截断
fn is_truncation_stop_reason(stop_reason: Option<&str>) -> bool {
    matches!(stop_reason, Some("max_tokens") | Some("length"))
}

/// 结构化输出强制通道总开关(L6/L19,2026-09-09 第 13 轮)。
///
/// 环境变量 `LAEW_FORCED_TOOLS=off|0|false|no` 关闭 wire 层 forced tool_choice
/// 注入(对齐 `LAEW_INJECTION_GUARD` 惯例);默认开启。关闭后 emit 工具仍在
/// registry,Agent 循环的短路逻辑也保留——模型若仍主动调用 emit 工具同样被接住。
fn forced_tools_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        forced_tools_enabled_from(std::env::var("LAEW_FORCED_TOOLS").unwrap_or_default())
    })
}

/// 开关取值解析(独立出来便于单测,OnceLock 缓存进程级一次)。
fn forced_tools_enabled_from(raw: String) -> bool {
    !matches!(raw.trim().to_lowercase().as_str(), "off" | "0" | "false" | "no")
}

/// 将 `serde_json::Value` 序列化为「对象 key 排序后的字符串」,作为失败键的稳定摘要。
/// 顺序无关,LLM 调换参数顺序不触发「不同目标」误判。
fn stable_json_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", k, stable_json_string(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(stable_json_string).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========== 取消传播(第 07 轮,方案 tmpPlan/2026-09-08_07) ==========

    /// 永远挂起的 LLM(模拟长时间未响应的请求)。
    struct HangLlm;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for HangLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            std::future::pending().await
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    /// 第 1 次返回 bash 工具调用(长睡眠),之后返回最终文本。
    struct SlowToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SlowToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(Completion {
                    text: String::new(),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: "call-slow-1".into(),
                        // 注意:注册表键为 "Bash"(工具 name() 原样)
                        name: "Bash".into(),
                        arguments: json!({"command": "sleep 30"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            } else {
                Ok(Completion {
                    text: "done".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn cancel_during_llm_call_returns_promptly() {
        let agent = Agent::new(
            std::sync::Arc::new(HangLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("慢任务"));
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            t2.cancel();
        });
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            agent
                .run_session_cancellable(&mut session, Some(&token))
                .await
        })
        .await
        .expect("取消后应及时返回,而非等 LLM 挂起")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
        // 没有工具调用发生,上下文不应被塞入取消回填
        assert_eq!(session.context().len(), 1);
    }

    #[tokio::test]
    async fn cancel_during_tool_execution_backfills_orphan() {
        let agent = Agent::new(
            std::sync::Arc::new(SlowToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑个长命令"));
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            // 等 bash sleep 30 已启动后再取消,验证工具执行中断 + 进程清理
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            t2.cancel();
        });
        let start = std::time::Instant::now();
        let res = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            agent
                .run_session_cancellable(&mut session, Some(&token))
                .await
        })
        .await
        .expect("工具执行中取消应及时返回,而非等 sleep 30 跑完")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
        // 及时性:远小于 sleep 30(允许 mock LLM 与进程启动开销)
        assert!(start.elapsed() < std::time::Duration::from_secs(4));
        // 消息一致性:末条应为 is_error 的取消 tool_result(无 orphan tool_use)
        let last = session.context().last().expect("应有取消回填");
        assert_eq!(last.role, crate::llm::Role::Tool);
        match &last.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "call-slow-1");
                assert!(content.contains("cancelled"));
                assert!(*is_error);
            }
            other => panic!("末条应为 ToolResult,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn precancelled_token_short_circuits() {
        let agent = Agent::new(
            std::sync::Arc::new(HangLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session
            .context_mut()
            .push(ChatMessage::user("已取消的任务"));
        let token = CancelToken::new();
        token.cancel();
        let res = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            agent.run_session_cancellable(&mut session, Some(&token)),
        )
        .await
        .expect("预取消应立即短路")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
    }

    #[test]
    fn is_truncation_stop_reason_detects_max_tokens() {
        assert!(is_truncation_stop_reason(Some("max_tokens")));
        assert!(is_truncation_stop_reason(Some("length")));
        assert!(!is_truncation_stop_reason(Some("end_turn")));
        assert!(!is_truncation_stop_reason(Some("stop")));
        assert!(!is_truncation_stop_reason(None));
    }

    // 关联报告: 20260908_203854 D-001
    #[test]
    fn stable_json_string_is_key_order_independent() {
        let a = json!({"path": "/tmp/missing", "limit": 10});
        let b = json!({"limit": 10, "path": "/tmp/missing"});
        assert_eq!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_nested_objects() {
        let a = json!({"outer": {"a": 1, "b": [1, 2, 3]}});
        let b = json!({"outer": {"b": [1, 2, 3], "a": 1}});
        assert_eq!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_different_values_produce_different_keys() {
        let a = json!({"path": "/tmp/missing_a"});
        let b = json!({"path": "/tmp/missing_b"});
        assert_ne!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_arrays_preserve_order() {
        // 数组按设计保持顺序(LLM 调换数组元素顺序确实代表不同输入)
        let a = json!({"items": [1, 2, 3]});
        let b = json!({"items": [3, 2, 1]});
        assert_ne!(stable_json_string(&a), stable_json_string(&b));
    }

    // ========== ExecutionTrace 累计(2026-09-09 第 05 轮) ==========

    /// 模拟 1 个工具调用成功(第 1 次返回工具,第 2 次返回最终文本)。
    struct OneOkToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for OneOkToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(Completion {
                    text: String::new(),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: "call-ok".into(),
                        name: "Bash".into(),
                        arguments: json!({"command": "echo ok"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            } else {
                Ok(Completion {
                    text: "工具成功完成".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn run_session_returns_trace_with_tool_ok() {
        let agent = Agent::new(
            std::sync::Arc::new(OneOkToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑命令"));
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        // 第 1 轮 mock LLM 返回 Bash echo ok → 累计 1 次成功;第 2 轮返回最终文本
        assert_eq!(trace.tool_calls_ok, 1, "应累计 1 次工具成功");
        assert_eq!(trace.tool_calls_err, 0);
        assert_eq!(trace.tool_calls, 1);
        assert_eq!(text, "工具成功完成");
        assert!(!trace.is_failed());
        assert!(trace.failure_signals.iter().any(|s| s == "ok"));
    }

    /// 模拟连续相同失败 → trace 应累计 early_terminated=true。
    struct SameFailLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SameFailLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            // 始终尝试调用一个 sandbox 违规路径,触发 Write 沙箱失败,
            // 3 次连续 → 早终止。
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![crate::llm::ToolCallReq {
                    id: "call-bad".into(),
                    name: "Bash".into(),
                    arguments: json!({"command": "rm -rf /"}), // 必触发 dangerous 命令拦截
                }],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn repeated_tool_failure_returns_error_and_trace_populated() {
        let agent = Agent::new(
            std::sync::Arc::new(SameFailLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑命令"));
        let res = agent.run_session(&mut session).await;
        // 早终止返回 Err(RepeatedToolFailure),此处不是 trace 路径,而是 Agent 错误
        assert!(res.is_err(), "RepeatedToolFailure 应上抛 Err");
        match res.unwrap_err() {
            AgentError::RepeatedToolFailure { tool, attempts, .. } => {
                assert_eq!(tool, "Bash");
                assert!(attempts >= 3);
            }
            other => panic!("预期 RepeatedToolFailure,实际 {other:?}"),
        }
    }

    // ========== 无文本收敛短路(第 05 轮 E-001,方案 tmpPlan/2026-09-09_05) ==========

    /// 永远只产生 Bash tool_use、text 为空的 LLM。模拟「死循环不停探查」场景。
    struct OnlyToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for OnlyToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![crate::llm::ToolCallReq {
                    id: format!("call-{n}"),
                    name: "Bash".into(),
                    arguments: json!({"command": "echo only-tool"}),
                }],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    /// 验证: 持续 tool_use 无 text 应在阈值(默认 8)轮内触发「无文本收敛短路」,
    /// 返回 Ok 而不是 MaxIterationsExceeded。
    #[tokio::test]
    async fn no_text_converge_short_circuit_returns_ok() {
        let agent = Agent::new(
            std::sync::Arc::new(OnlyToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("陷入循环"));
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        // 应返回 Ok 而非 MaxIterationsExceeded
        assert!(
            trace.early_terminated,
            "无文本收敛短路应标记 early_terminated"
        );
        assert!(
            trace.early_terminate_reason.starts_with("no_text_converge"),
            "早终止原因应为 no_text_converge,实际 {}",
            trace.early_terminate_reason
        );
        // 兜底文本应包含「无文本收敛上限」字样
        assert!(
            text.contains("无文本收敛上限"),
            "兜底文本应提示无文本收敛,实际: {text}"
        );
        // 短路触发时,Bash 工具应至少被调用了阈值次数
        assert!(
            trace.tool_calls >= 8,
            "应至少执行 8 次 Bash 才短路,实际 {}",
            trace.tool_calls
        );
    }

    /// 验证: 工具调用 + 文本混合的正常 LLM 不应触发短路。
    struct MixedTextToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for MixedTextToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 3 {
                Ok(Completion {
                    text: format!("第 {} 轮说明", n + 1),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: format!("call-{n}"),
                        name: "Bash".into(),
                        arguments: json!({"command": "echo mix"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            } else {
                Ok(Completion {
                    text: "已完成".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn mixed_text_and_tool_does_not_short_circuit() {
        let agent = Agent::new(
            std::sync::Arc::new(MixedTextToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("混合任务"));
        let (_text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        // 混合文本+工具不应触发无文本收敛短路
        assert!(
            !trace.early_terminate_reason.starts_with("no_text_converge"),
            "混合场景不应触发无文本收敛短路,实际 reason={}",
            trace.early_terminate_reason
        );
    }

    // ========== 上下文溢出三级恢复(第 06 轮,方案 tmpPlan/2026-09-09_06) ==========

    /// 预置一条超长 tool_result 的会话(模拟上一轮循环产生的大工具输出)。
    fn session_with_fat_tool_result(fat_chars: usize) -> Session {
        let mut s = Session::new();
        s.context_mut().push(ChatMessage::user("跑个大命令"));
        s.context_mut()
            .push(ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t-fat".into(),
                name: "Bash".into(),
                input: json!({"command": "seq 1 5000"}),
            }]));
        s.context_mut().push(ChatMessage::tool_result(
            "t-fat",
            "x".repeat(fat_chars),
            false,
        ));
        s
    }

    fn overflow_err() -> AgentError {
        AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "HTTP 400: {\"error\":{\"message\":\"This model's maximum context length is 8192 tokens. However, your messages resulted in 54321 tokens\",\"code\":\"context_length_exceeded\"}}".into(),
        }
    }

    /// 第 1 次返回溢出 400,之后返回最终文本(验证排水后重试成功)。
    struct OverflowOnceLlm {
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for OverflowOnceLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err(overflow_err())
            } else {
                Ok(Completion {
                    text: "溢出恢复后成功".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn overflow_recovers_via_drain_and_retries() {
        let agent = Agent::new(
            std::sync::Arc::new(OverflowOnceLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = session_with_fat_tool_result(20_000);
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        assert_eq!(text, "溢出恢复后成功");
        assert_eq!(trace.overflow_recoveries, 1, "应恰好排水恢复 1 次");
        assert!(
            trace
                .failure_signals
                .iter()
                .any(|s| s.starts_with("overflow_recovered:")),
            "恢复应产生弱失败信号,实际 {:?}",
            trace.failure_signals
        );
        assert!(!trace.is_failed(), "恢复成功不算失败");
        // 排水真实生效:超长 tool_result 已被截短
        match &session.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("overflow-drain"));
                assert!(content.chars().count() < overflow::DRAIN_MIN_CHARS);
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    /// 永远返回溢出 400(验证排水 + 折叠两级穷尽后按三级暴露上抛)。
    struct AlwaysOverflowLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for AlwaysOverflowLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            Err(overflow_err())
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn overflow_exhausts_recovery_and_exposes_error() {
        let agent = Agent::new(
            std::sync::Arc::new(AlwaysOverflowLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        // 会话带可折叠的胖历史(触发 Level 2) + 超长工具结果(触发 Level 1)
        let mut session = session_with_fat_tool_result(20_000);
        for i in 0..6 {
            session.context_mut().insert(
                1,
                ChatMessage::user(format!("历史{i} {}", "话".repeat(400))),
            );
        }
        let res = agent.run_session(&mut session).await;
        let err = res.expect_err("两级恢复穷尽后应上抛原始溢出错误");
        assert!(
            matches!(&err, AgentError::LlmHttp { status: 400, .. }),
            "应原样上抛 LlmHttp 400,实际 {err:?}"
        );
        // Level 1 + Level 2 都真实执行过:工具结果被截短 + 折叠标记存在
        let has_drained = session.context().iter().any(|m| {
            m.content.iter().any(
                |b| matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("overflow-drain")),
            )
        });
        assert!(has_drained, "排水应真实执行");
        let has_folded = session.context().iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains(compact::COMPACT_MARKER_START)))
        });
        assert!(has_folded, "折叠应真实执行");
    }

    #[tokio::test]
    async fn overflow_budget_zero_disables_recovery() {
        let agent = Agent::new(
            std::sync::Arc::new(OverflowOnceLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        )
        .with_max_overflow_recoveries(0);
        let mut session = session_with_fat_tool_result(20_000);
        let res = agent.run_session(&mut session).await;
        assert!(res.is_err(), "预算为 0 时不恢复,直接上抛");
        // 未排水:工具结果保持原长
        match &session.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.chars().count(), 20_000);
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_overflow_error_not_recovered() {
        let agent = Agent::new(
            std::sync::Arc::new(FailGenericLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = session_with_fat_tool_result(20_000);
        let res = agent.run_session(&mut session).await;
        let err = res.expect_err("非溢出错误应原样上抛");
        assert!(matches!(err, AgentError::Llm(_)));
        // 上下文未被排水
        match &session.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.chars().count(), 20_000, "非溢出错误不应触发排水");
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    /// 普通失败(非溢出),不应触发任何恢复。
    struct FailGenericLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for FailGenericLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            Err(AgentError::Llm("mock down".into()))
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    // ========== Agent 身份逐请求注入(第 08 轮,方案 tmpPlan/2026-09-09_08) ==========

    /// 捕获 complete() 收到的 RequestMeta(含 User-Agent),验证 Agent 循环逐请求注入。
    struct MetaCaptureLlm {
        seen: std::sync::Mutex<Vec<RequestMeta>>,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for MetaCaptureLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            meta: &RequestMeta,
        ) -> Result<Completion> {
            self.seen.lock().expect("meta capture").push(meta.clone());
            Ok(Completion {
                text: "done".into(),
                tool_calls: vec![],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn run_session_injects_profile_user_agent_per_request() {
        // Yolo profile → 请求 UA 必须是 Yolo 的(而非共享客户端的构造期默认)
        let cap = std::sync::Arc::new(MetaCaptureLlm {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(cap.clone(), AgentProfile::yolo_profile());
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("分类一下"));
        agent.run_session(&mut session).await.unwrap();
        let seen = cap.seen.lock().expect("meta capture");
        assert!(!seen.is_empty(), "应至少发起一次 LLM 调用");
        for meta in seen.iter() {
            assert!(
                meta.user_agent.starts_with("LsmAgentEmergentWork-Yolo/"),
                "UA 应逐请求注入为 Yolo,实际: {}",
                meta.user_agent
            );
        }
    }

    #[tokio::test]
    async fn run_session_injects_subagent_user_agent() {
        // SubAgent-Work profile → 同一注入路径下角色名跟随 profile 变化
        let cap = std::sync::Arc::new(MetaCaptureLlm {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(cap.clone(), AgentProfile::sub_agent_work_profile());
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑命令"));
        agent.run_session(&mut session).await.unwrap();
        let seen = cap.seen.lock().expect("meta capture");
        assert!(
            seen.iter()
                .all(|m| m.user_agent.starts_with("LsmAgentEmergentWork-SubAgent-Work/")),
            "UA 应为 SubAgent-Work,实际: {:?}",
            seen.iter().map(|m| m.user_agent.clone()).collect::<Vec<_>>()
        );
    }

    // ========== L1037 + L771 联动(2026-09-09 第 09 轮) ==========

    #[test]
    fn runtime_hints_empty_when_no_counters_active() {
        // 所有计数器为 0 时,返回空字符串(零开销,不影响 LLM 上下文)
        let t = ExecutionTrace::default();
        let h = build_runtime_hints(&t, 0);
        assert!(h.is_empty(), "全 0 应返回空串,实际: {h:?}");
        let h = build_runtime_hints(&t, 1);
        assert!(h.is_empty(), "consecutive_failures < 2 也不应触发,实际: {h:?}");
    }

    #[test]
    fn runtime_hints_truncation_when_resumes_present() {
        let mut t = ExecutionTrace::default();
        t.truncation_resumes = 2;
        let h = build_runtime_hints(&t, 0);
        assert!(h.contains("已续接 2 次"));
        assert!(h.contains("LAEW:RUNTIME_HINTS"));
        assert!(h.contains("END"));
    }

    #[test]
    fn runtime_hints_overflow_when_recoveries_present() {
        let mut t = ExecutionTrace::default();
        t.overflow_recoveries = 1;
        let h = build_runtime_hints(&t, 0);
        assert!(h.contains("自动恢复 1 次"));
        assert!(h.contains("排水/折叠"));
    }

    #[test]
    fn runtime_hints_max_tokens_when_upscalings_present() {
        let mut t = ExecutionTrace::default();
        t.max_tokens_upscalings = 2;
        let h = build_runtime_hints(&t, 0);
        assert!(h.contains("max_tokens 已升级 2 次"));
    }

    #[test]
    fn runtime_hints_consecutive_failures_threshold() {
        // consecutive_failures >= 2 触发
        let t = ExecutionTrace::default();
        assert!(build_runtime_hints(&t, 2).contains("连续 2 次"));
        assert!(build_runtime_hints(&t, 5).contains("连续 5 次"));
        // < 2 不触发
        assert!(build_runtime_hints(&t, 1).is_empty());
    }

    #[test]
    fn runtime_hints_multiple_lines_joined() {
        // 多个信号并存时全部出现
        let mut t = ExecutionTrace::default();
        t.truncation_resumes = 1;
        t.overflow_recoveries = 1;
        t.max_tokens_upscalings = 1;
        let h = build_runtime_hints(&t, 3);
        assert!(h.contains("已续接 1 次"));
        assert!(h.contains("自动恢复 1 次"));
        assert!(h.contains("max_tokens 已升级 1 次"));
        assert!(h.contains("连续 3 次"));
    }

    #[test]
    fn finalize_trace_records_max_tokens_state() {
        // 验证 finalize_with_max_tokens 把 MaxTokensState 写入 trace
        use crate::agent::max_tokens_state::MaxTokensState;
        let state = MaxTokensState::new();
        state.note_truncated(); // 8K → 16K
        state.note_truncated(); // 16K → 32K
        let mut t = ExecutionTrace::default();
        t.iterations = 2;
        let (text, _u, t) =
            crate::agent::Agent::finalize_with_max_tokens(t, "hello", Usage::default(), &state).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(t.max_tokens_upscalings, 2);
        assert_eq!(t.max_tokens_history, vec![(8192, 16384), (16384, 32768)]);
    }

    // ========== 结构化输出通道短路(L6/L19,2026-09-09 第 13 轮) ==========

    /// 返回固定 Completion 的 mock LLM(记录每次收到的 meta.forced_tool)。
    struct EmitLlm {
        replies: std::sync::Mutex<Vec<Completion>>,
        seen_forced: std::sync::Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for EmitLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            meta: &RequestMeta,
        ) -> Result<Completion> {
            self.seen_forced
                .lock()
                .expect("seen_forced")
                .push(meta.forced_tool.clone());
            let mut replies = self.replies.lock().expect("replies");
            if replies.is_empty() {
                return Err(AgentError::Other("mock 耗尽".into()));
            }
            Ok(replies.remove(0))
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    fn emit_completion(
        calls: Vec<(&'static str, serde_json::Value)>,
        text: &str,
    ) -> Completion {
        Completion {
            text: text.to_string(),
            tool_calls: calls
                .into_iter()
                .enumerate()
                .map(|(i, (name, args))| crate::llm::ToolCallReq {
                    id: format!("call_{i}"),
                    name: name.to_string(),
                    arguments: args,
                })
                .collect(),
            usage: Usage::default(),
            stop_reason: Some("tool_use".into()),
        }
    }

    #[tokio::test]
    async fn emit_tool_short_circuits_loop() {
        // Yolo profile + 模型返回 submit_task_classification tool_use:
        // 循环应 1 轮终止,最终文本含 ```json 块,trace.structured_emits = 1,
        // 且 meta.forced_tool 已注入协议层。
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![emit_completion(
                vec![(
                    "submit_task_classification",
                    json!({
                        "task_level": "medium",
                        "purpose": "验证",
                        "goal_summary": "结构化输出验证",
                        "intent": "verify",
                        "decomposition_plan": ["步骤一"],
                        "direct_answer": null
                    }),
                )],
                "这是中等难度任务,已提交分类。",
            )]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm.clone(), AgentProfile::yolo_profile());
        let (text, _usage, trace) = agent.run_once("测试任务").await.unwrap();

        // 最终文本 = 模型文本 + emit input 的 ```json 块
        assert!(text.contains("结构化输出验证"), "文本应含 emit input: {text}");
        assert!(text.contains("```json"), "应输出 json 代码块: {text}");
        // 下游解析链直接命中
        let c = crate::agent::yolo::parse_classification(&text).unwrap();
        assert_eq!(c.task_level, crate::agent::yolo::TaskLevel::Medium);
        assert!(!c.yolo_degraded, "emit 路径不应降级");
        // 1 轮终止 + 计数
        assert_eq!(trace.iterations, 1);
        assert_eq!(trace.structured_emits, 1);
        assert_eq!(trace.tool_calls, 0, "emit 通道不计入工具执行计数");
        // 协议层收到 forced 注入
        let seen = llm.seen_forced.lock().expect("seen_forced");
        assert_eq!(
            seen.first().and_then(|f| f.as_deref()),
            Some("submit_task_classification"),
            "meta.forced_tool 应注入 emit 工具名,实际: {seen:?}"
        );
    }

    #[tokio::test]
    async fn emit_tool_mixed_parallel_calls_ignored() {
        // emit + Read 并行调用(emit 在前):emit 之后的 Read 不执行,仅回填「已忽略」;
        // 上下文 tool_use 与 tool_result 一一配对(无孤儿)。
        // 注:Read 在 emit 之前的场景是正常工作流(先探查后提交),由
        // emit_tool_short_circuits_loop 与 non_emit_profile_never_forced 覆盖。
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![emit_completion(
                vec![
                    (
                        "submit_quality_report",
                        json!({
                            "verdict": "pass",
                            "source": "subagent",
                            "retryable": false
                        }),
                    ),
                    ("Read", json!({"file_path": "/definitely/not/exists.txt"})),
                ],
                "",
            )]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm, AgentProfile::quality_check_profile());
        let mut session = crate::session::Session::new();
        session.context_mut().push(ChatMessage::user("质检"));
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();

        assert_eq!(trace.structured_emits, 1);
        assert_eq!(trace.tool_calls, 0, "emit 之后的 Read 不应被执行");
        assert!(text.contains("\"verdict\": \"pass\""));

        // 上下文配对检查:assistant ToolUse × 2 ↔ tool_result × 2
        let ctx = session.context();
        let tool_uses: Vec<&ContentBlock> = ctx
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect();
        let tool_results: Vec<&ContentBlock> = ctx
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .collect();
        assert_eq!(tool_uses.len(), 2, "assistant 应含 2 个 tool_use");
        assert_eq!(tool_results.len(), 2, "每个 tool_use 都应有 tool_result 回填");
        // 忽略回填的内容标记
        let ignored = tool_results
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .any(|c| c.contains("已忽略"));
        assert!(ignored, "Read 的回填应标记「已忽略」");
    }

    #[tokio::test]
    async fn non_emit_profile_never_forced() {
        // 无 emit_tool 的 profile(SubAgent)不注入 forced,模型普通工具照常执行
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![
                emit_completion(vec![("Bash", json!({"command": "echo forced-check"}))], ""),
                Completion {
                    text: "done".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                },
            ]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm.clone(), AgentProfile::sub_agent_work_profile());
        let (text, _usage, trace) = agent.run_once("echo 测试").await.unwrap();
        assert_eq!(text.trim(), "done");
        assert_eq!(trace.tool_calls, 1, "Bash 应正常执行");
        assert_eq!(trace.structured_emits, 0);
        let seen = llm.seen_forced.lock().expect("seen_forced");
        assert!(
            seen.iter().all(|f| f.is_none()),
            "无 emit_tool 的 profile 不应注入 forced,实际: {seen:?}"
        );
    }

    #[test]
    fn forced_tools_switch_parsing() {
        // 环境变量取值解析(off/0/false/no 关闭,其余含空值开启)
        assert!(!forced_tools_enabled_from("off".into()));
        assert!(!forced_tools_enabled_from("0".into()));
        assert!(!forced_tools_enabled_from("FALSE".into()));
        assert!(!forced_tools_enabled_from(" no ".into()));
        assert!(forced_tools_enabled_from(String::new()));
        assert!(forced_tools_enabled_from("on".into()));
    }
}
