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
pub mod context;
pub mod compact;
pub mod debug;
pub mod extrace;
pub mod json_repair;
pub mod main_work;
pub mod memory;
pub mod orchestrator;
pub mod overflow;
pub mod permissions;
pub mod plan;
pub mod profile;
pub mod project_context;
pub mod quality;
pub mod sandbox_hook;
pub mod session_context;
pub mod subagent;
pub mod system_prompt;
pub mod tools;
pub mod yolo;

use std::sync::Arc;

use tracing::{info, warn};

use crate::agent::cancel::{backfill_cancelled_tool_results, CancelToken};
use crate::agent::extrace::ExecutionTrace;
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

    pub fn llm(&self) -> Arc<dyn LlmClient> { self.llm.clone() }
    pub fn profile(&self) -> &AgentProfile { &self.profile }
    pub fn max_iterations(&self) -> usize { self.max_iterations }
    pub fn max_truncation_resume(&self) -> usize { self.max_truncation_resume }
    pub fn max_overflow_recoveries(&self) -> usize { self.max_overflow_recoveries }

    /// 单轮任务:传入用户提示,返回最终文本、本次累计 token 用量与执行轨迹。
    pub async fn run_once(
        &self,
        user_input: &str,
    ) -> Result<(String, Usage, ExecutionTrace)> {
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
        let meta: RequestMeta = session.meta();
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

        for iter in 0..self.max_iterations {
            trace.iterations = iter + 1;
            // 迭代边界:取消检查(轻量 is_cancelled,热路径零 await 开销)
            if let Some(token) = cancel {
                if token.is_cancelled() {
                    backfill_cancelled_tool_results(session.context_mut());
                    return Err(AgentError::Cancelled);
                }
            }
            info!(iteration = iter, "agent step");
            // 上下文溢出自动恢复(L1038/L1044,2026-09-09 第 06 轮):
            // LLM 调用命中 prompt-too-long 类溢出错误时,自动执行
            // 排水(Level 1)→ 折叠(Level 2)→ 重试;两轮无效则上抛(三级暴露)。
            let completion: Completion = self
                .complete_with_overflow_recovery(session, cancel, &tool_defs, &meta, &mut trace)
                .await?;

            // 累计 usage
            total_usage.input_tokens = total_usage.input_tokens.saturating_add(completion.usage.input_tokens);
            total_usage.output_tokens = total_usage.output_tokens.saturating_add(completion.usage.output_tokens);
            total_usage.cache_read_input_tokens =
                total_usage.cache_read_input_tokens.saturating_add(completion.usage.cache_read_input_tokens);
            total_usage.cache_creation_input_tokens =
                total_usage.cache_creation_input_tokens.saturating_add(completion.usage.cache_creation_input_tokens);

            if !completion.has_tool_calls() {
                // 累计文本(续接场景下可能多次进入此分支)
                if !completion.text.trim().is_empty() {
                    if !accumulated_text.is_empty() {
                        accumulated_text.push('\n');
                    }
                    accumulated_text.push_str(&completion.text);
                    session.context_mut().push(ChatMessage::assistant(vec![ContentBlock::text(
                        completion.text.clone(),
                    )]));
                }

                // 检测截断:输出被 token 上限截断时自动续接
                if is_truncation_stop_reason(completion.stop_reason.as_deref()) {
                    if truncation_resumes >= self.max_truncation_resume {
                        // 达到上限,返回已累计的文本(优雅降级)
                        warn!(
                            truncation_resumes = truncation_resumes,
                            "截断续接达到上限,返回已累计文本"
                        );
                        trace.truncation_resumes = truncation_resumes;
                        return Ok(Self::finalize_trace(
                            trace,
                            &accumulated_text,
                            total_usage,
                        ));
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
                    continue;  // 继续循环
                }

                // 非截断,正常返回
                info!("agent finished with text answer");
                return Ok(Self::finalize_trace(
                    trace,
                    &accumulated_text,
                    total_usage,
                ));
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
            session.context_mut().push(ChatMessage::assistant(assistant_blocks));

            // 关联报告: 2026-09-09_05 E-001 —— 快照本轮文本/工具状态供后续短路判断使用
            // (completion.text 与 completion.tool_calls 在下面的循环会被 move)
            let this_round_text_empty = completion.text.trim().is_empty();
            let this_round_had_tool_calls = !completion.tool_calls.is_empty();

            // 逐个执行工具并把结果回填上下文(失败也作为 tool_result,is_error=true)
            let mut any_success_this_round = false;
            for call in completion.tool_calls {
                let name = call.name.clone();
                let id = call.id.clone();
                let args = call.arguments;
                info!(tool = %name, "executing tool");

                // 工具执行取消:select 命中后工具 future 被 drop——Bash 工具的
                // `kill_on_drop` 会随之 SIGKILL 子进程、setsid+killpg 清理整组
                // (知识库第五轮 §6.1「取消必须级联到进程组」,bash.rs ef84cec 已就位)
                let executed = match self.profile.tools.get(&name) {
                    Ok(tool) => match cancel {
                        Some(token) => {
                            tokio::select! {
                                biased;
                                _ = token.cancelled() => None,
                                r = tool.execute(args.clone()) => Some(r),
                            }
                        }
                        None => Some(tool.execute(args.clone()).await),
                    },
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
                    let fail_key = format!(
                        "{}|{}",
                        name,
                        stable_json_string(&args)
                    );
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
                }
                trace.tool_calls += 1;
                session
                    .context_mut()
                    .push(ChatMessage::tool_result(id, output, is_error));
            }
            if any_success_this_round {
                // 任一成功调用重置失败计数
                last_fail_key = None;
                consecutive_failures = 0;
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
                // 注入 nudge 让 LLM 主动收敛,即便不返回最终答复也走 finalize 路径
                session.context_mut().push(ChatMessage::user(
                    "[系统提示] 你已连续多轮只调用工具未输出最终答复。\
                     请基于已收集到的工具结果直接给出结论性答复,不要再发起新的工具调用。"
                        .to_string(),
                ));
                // 合成兜底文本:基于工具执行情况聚合
                let fallback_text = format!(
                    "[SubAgent 已达无文本收敛上限 {} 轮,基于以下工具执行情况返回]\n\n\
                     - 共执行 {tc} 次(成功 {ok},失败 {err})\n\
                     - 详细工具结果请参考 Session 历史 context",
                    NO_TEXT_CONVERGE_THRESHOLD,
                    tc = trace.tool_calls,
                    ok = trace.tool_calls_ok,
                    err = trace.tool_calls_err,
                );
                return Ok(Self::finalize_trace(
                    trace,
                    &fallback_text,
                    total_usage,
                ));
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
    async fn complete_with_overflow_recovery(
        &self,
        session: &mut Session,
        cancel: Option<&CancelToken>,
        tool_defs: &[crate::llm::ToolDef],
        meta: &RequestMeta,
        trace: &mut ExecutionTrace,
    ) -> Result<Completion> {
        // 按当前 LLM 协议渲染系统提示词(支持多协议差异化)
        let system = self.profile.system_prompt.render(self.llm.protocol());
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
                    r = self.llm.complete(&system, session.context(), tool_defs, meta) => Some(r),
                },
                None => Some(
                    self.llm
                        .complete(&system, session.context(), tool_defs, meta)
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

    /// 同步填充 trace 的输出字节数与失败信号后返回 Ok 三元组
    /// (在循环正常结束后调用,异常路径由调用方继续包装)。
    fn finalize_trace(
        mut trace: ExecutionTrace,
        text: &str,
        total_usage: Usage,
    ) -> (String, Usage, ExecutionTrace) {
        trace.output_bytes = text.len();
        trace.collect_failure_signals(text);
        (text.to_string(), total_usage, trace)
    }
}

/// 判断 `stop_reason` 是否为截断(输出被 token 上限截断)。
///
/// - Anthropic:`"max_tokens"` 表示输出达到 `max_tokens` 上限被截断
/// - OpenAI:`"length"` 表示输出达到 `max_tokens` 上限被截断
fn is_truncation_stop_reason(stop_reason: Option<&str>) -> bool {
    matches!(stop_reason, Some("max_tokens") | Some("length"))
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
            agent.run_session_cancellable(&mut session, Some(&token)).await
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
            agent.run_session_cancellable(&mut session, Some(&token)).await
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
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
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
        session.context_mut().push(ChatMessage::user("已取消的任务"));
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
            let n = self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            session
                .context_mut()
                .insert(1, ChatMessage::user(format!("历史{i} {}", "话".repeat(400))));
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
}
