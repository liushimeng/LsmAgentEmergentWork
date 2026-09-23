//! 工具调用执行底座(第 120 轮「工具连续工作模式」自 `agent_loop.rs` 抽出)。
//!
//! 承载三件事:
//! 1. **单条执行归一**([`Agent::exec_tool_call`]):Schema 预校验 → 取消竞争 →
//!    `execute` → 错误归一(含 `ToolNotFound` 的可用工具边界提示)。自 agent_loop
//!    主循环**逐行机械抽取**,判定分支一字未改,让并发批与串行路径共用同一实现。
//! 2. **并发批规划**([`Agent::plan_parallel_batches`]):把同一 LLM 响应里
//!    **连续的** `parallel_safe` 调用切成段,段长 ≥ 2 才进批(Claude Code
//!    `partitionToolCalls` 语义)。
//! 3. **并发批执行**([`Agent::run_parallel_batches`]):`join_all` + `Semaphore`
//!    限流,返回 `call index → 结果` 映射;**回填顺序由调用方按原序保证**
//!    (并发只改执行时序,不改上下文时序)。
//!
//! 设计见 `docs/SubAgentWork执行层ReAct与连续工作模式/01-设计与解决方案.md` §3.2。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Semaphore;

use super::*;
use crate::agent::tools::{max_parallel_tools, parallel_tools_enabled};
use crate::llm::ToolCallReq;

/// 单次工具调用的执行结果(并发批与串行路径共用)。
pub(crate) struct ToolExec {
    /// 工具输出(失败时为归一后的 `[工具执行失败] …` 文本)。
    pub output: String,
    /// 是否失败(回填 `tool_result` 的 `is_error`)。
    pub is_error: bool,
    /// 失败摘要(≤120 字符,写入 `ToolCallLogEntry.error_summary`)。
    pub error_summary: String,
    /// 墙钟耗时(毫秒)。**必须在 `execute()` 之前采样计时起点**,否则恒为 0
    /// (第 82 轮 P0-1 的教训,见 `tmpPlan/2026-09-17_13` §3.1)。
    pub elapsed_ms: u64,
    /// 命中取消(`select` 竞争失败)。调用方负责 `backfill_cancelled_tool_results`
    /// + 返回 `Err(AgentError::Cancelled)`。
    pub cancelled: bool,
}

impl ToolExec {
    /// 取消占位结果(不进上下文,仅用于向上传递取消信号)。
    fn cancelled(elapsed_ms: u64) -> Self {
        Self {
            output: String::new(),
            is_error: false,
            error_summary: String::new(),
            elapsed_ms,
            cancelled: true,
        }
    }
}

/// 收尾一个并发段:`[start, end)` 段长 ≥ 2 才记为一批。
///
/// 段长 1 不进批 —— 单条调用走原串行路径,零额外开销、行为与第 119 轮逐字一致。
fn flush_batch(start: &mut Option<usize>, end: usize, out: &mut Vec<(usize, usize)>) {
    if let Some(s) = start.take() {
        if end.saturating_sub(s) >= 2 {
            out.push((s, end));
        }
    }
}

impl Agent {
    /// 执行单条工具调用(第 120 轮自 agent_loop 主循环机械抽取,语义零变化)。
    pub(crate) async fn exec_tool_call(
        &self,
        name: &str,
        args: serde_json::Value,
        cancel: Option<&CancelToken>,
    ) -> ToolExec {
        // ★ 计时起点必须在 tool.execute() 之前(第 82 轮 P0-1)。
        let tool_call_started = std::time::Instant::now();
        let executed = match self.profile.tools.get(name) {
            Ok(tool) => {
                // Schema 预校验(校验失败 → 返回错误,不执行工具)
                if let Err(e) = crate::agent::tool_schema_validator::validate_tool_args(
                    name,
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
        debug_assert!(tool_call_started.elapsed().as_nanos() > 0);
        let elapsed_ms = tool_call_started.elapsed().as_millis() as u64;
        match executed {
            Some(Ok(out)) => ToolExec {
                output: out,
                is_error: false,
                error_summary: String::new(),
                elapsed_ms,
                cancelled: false,
            },
            Some(Err(e)) => {
                warn!(tool = %name, error = %e, "tool failed");
                // F1(2026-09-14 第 51 轮):工具不存在时,回填文本明示可用工具边界——
                // auto tool_choice 降级后模型可能尝试越权工具(如 Yolo 调 Bash),
                // 裸错误「工具不存在」不足以让模型收敛,导致同一轮内反复试错浪费迭代。
                let brief = format!("{e}");
                let brief_short: String = brief.chars().take(120).collect();
                if matches!(e, AgentError::ToolNotFound(_)) {
                    let avail = self.profile.tools.names().join(", ");
                    ToolExec {
                        output: format!(
                            "[工具执行失败] {name}: {e}。你当前可用的工具仅有: [{avail}]。\
                             禁止再次调用 {name};请立即改用上述可用工具完成任务,或直接给出最终回答。"
                        ),
                        is_error: true,
                        error_summary: format!("ToolNotFound: {brief_short}"),
                        elapsed_ms,
                        cancelled: false,
                    }
                } else {
                    ToolExec {
                        output: format!("[工具执行失败] {}: {}", name, e),
                        is_error: true,
                        error_summary: format!("{name}: {brief_short}"),
                        elapsed_ms,
                        cancelled: false,
                    }
                }
            }
            // 取消:本条 + 本轮剩余未执行的 tool_use 由调用方 backfill 统一补全
            None => ToolExec::cancelled(elapsed_ms),
        }
    }

    /// 规划本轮 `tool_calls` 的并发批(第 120 轮)。
    ///
    /// 返回 `Vec<(start, end)>` 半开区间,每个区间是**连续的** `parallel_safe` 段
    /// 且段长 ≥ 2。`limit_to` = emit 短路点下标:该下标及其后的调用一律不进批
    /// (emit 命中后本轮剩余调用只回填不执行,并发执行它们会白烧资源)。
    ///
    /// `LAEW_PARALLEL_TOOLS=off` 或本轮调用数 < 2 时返回空(全串行,零回归)。
    pub(crate) fn plan_parallel_batches(
        &self,
        calls: &[ToolCallReq],
        limit_to: usize,
    ) -> Vec<(usize, usize)> {
        if !parallel_tools_enabled() || calls.len() < 2 {
            return Vec::new();
        }
        let emit = self.profile.emit_tool.as_deref();
        let n = limit_to.min(calls.len());
        let mut out: Vec<(usize, usize)> = Vec::new();
        let mut start: Option<usize> = None;
        for (i, call) in calls.iter().enumerate().take(n) {
            if emit == Some(call.name.as_str()) {
                flush_batch(&mut start, i, &mut out);
                return out;
            }
            // 工具不在注册表里(`ToolNotFound` 路径)→ 不可并行,交给串行路径
            // 生成带「可用工具边界」的归一错误文本。
            let safe = self
                .profile
                .tools
                .get(&call.name)
                .map(|t| t.parallel_safe(&call.arguments))
                .unwrap_or(false);
            if safe {
                if start.is_none() {
                    start = Some(i);
                }
            } else {
                flush_batch(&mut start, i, &mut out);
            }
        }
        flush_batch(&mut start, n, &mut out);
        out
    }

    /// 并发执行规划出的批次,返回 `(call index → 结果, 是否命中取消)`。
    ///
    /// - 限流:`Semaphore(LAEW_MAX_PARALLEL_TOOLS)`,默认 4(对齐 AtomCode
    ///   `ATOMCODE_MAX_PARALLEL_TOOLS`;Claude Code 用 10,laew 单元预算小取保守值);
    /// - 取消:每条 future 内部仍 `select! biased` 竞争 cancel token;任一条命中
    ///   即停止后续批次(已完成的保留,由调用方 backfill 剩余 tool_use)。
    pub(crate) async fn run_parallel_batches(
        &self,
        calls: &[ToolCallReq],
        batches: &[(usize, usize)],
        cancel: Option<&CancelToken>,
    ) -> (HashMap<usize, ToolExec>, bool) {
        if batches.is_empty() {
            return (HashMap::new(), false);
        }
        let sem = Arc::new(Semaphore::new(max_parallel_tools()));
        let mut out: HashMap<usize, ToolExec> = HashMap::new();
        let mut cancelled = false;
        for &(start, end) in batches {
            let futs = (start..end).map(|i| {
                let sem = sem.clone();
                let call = &calls[i];
                async move {
                    // 限流许可在 future 内获取:批大小 > 上限时自动排队,
                    // 不需要调用方二次切批。
                    let _permit = sem.acquire().await.ok();
                    let r = self
                        .exec_tool_call(&call.name, call.arguments.clone(), cancel)
                        .await;
                    (i, r)
                }
            });
            for (i, r) in futures::future::join_all(futs).await {
                if r.cancelled {
                    cancelled = true;
                }
                out.insert(i, r);
            }
            if cancelled {
                break;
            }
        }
        (out, cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profile::AgentProfile;
    use crate::agent::tools::Tool;
    use crate::llm::ToolDef;
    use serde_json::json;

    /// 本模块只测「批规划 + 批执行」,不发 LLM 请求 —— 用一个永不返回的桩客户端即可。
    struct UnusedLlm;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for UnusedLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            std::future::pending().await
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCallReq {
        ToolCallReq {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    fn agent_with(reg: crate::agent::tools::ToolRegistry) -> Agent {
        Agent::new(
            Arc::new(UnusedLlm),
            AgentProfile {
                name: "test".to_string(),
                system_prompt: crate::agent::system_prompt::SystemPrompt::sub_agent_work(),
                tools: reg,
                emit_tool: None,
                spawn_policy: crate::agent::self_awareness::SpawnPolicy::Disabled,
                defer_emit_force: false,
            },
        )
    }

    /// 只读工具必须声明可并发;有副作用 / 独占物理资源的工具必须保持串行。
    #[test]
    fn parallel_safe_classification() {
        let arg = json!({});
        // 正例:纯只读、无副作用、无跨调用共享状态
        assert!(
            crate::agent::tools::read::ReadTool.parallel_safe(&arg),
            "Read 应可并发"
        );
        assert!(
            crate::agent::tools::glob::GlobTool.parallel_safe(&arg),
            "Glob 应可并发"
        );
        assert!(
            crate::agent::tools::grep::GrepTool.parallel_safe(&arg),
            "Grep 应可并发"
        );
        // 反例:Bash(子进程副作用)/ TodoWrite(共享状态)/ MCP_Web_Use(独占浏览器)
        // Round 124:BashTool 加 BashMode 字段后需走实例方法,`::new()` 默认 ReadWrite。
        assert!(
            !crate::agent::tools::bash::BashTool::new().parallel_safe(&arg),
            "Bash 不得并发"
        );
        assert!(
            !crate::agent::tools::todo::TodoWriteTool::shared().parallel_safe(&arg),
            "TodoWrite 不得并发"
        );
        assert!(
            !crate::agent::tools::mcp_web_use::McpWebUseTool.parallel_safe(&arg),
            "MCP_Web_Use 独占浏览器,不得并发"
        );
    }

    /// 全量注册表里「可并发」的必须恰好是只读三件套(防止后续新增工具误开并发)。
    #[test]
    fn builtin_registry_parallel_surface_is_readonly_trio() {
        let reg = crate::agent::tools::builtin_registry();
        let arg = json!({});
        let mut safe = Vec::new();
        for name in reg.names() {
            if let Ok(t) = reg.get(name) {
                if t.parallel_safe(&arg) {
                    safe.push(name.to_string());
                }
            }
        }
        safe.sort();
        assert_eq!(safe, vec!["Glob", "Grep", "Read"]);
    }

    #[test]
    fn readonly_run_of_two_or_more_forms_batch() {
        let agent = agent_with(crate::agent::tools::builtin_registry());
        let calls = vec![
            call("1", "Read", json!({"file_path": "a"})),
            call("2", "Grep", json!({"pattern": "x"})),
            call("3", "Glob", json!({"pattern": "*.rs"})),
        ];
        assert_eq!(agent.plan_parallel_batches(&calls, calls.len()), vec![(0, 3)]);
    }

    #[test]
    fn unsafe_call_splits_runs() {
        // [Read, Grep, Bash, Read, Glob] → 两段:(0,2) 与 (3,5);Bash 单独不成批
        let agent = agent_with(crate::agent::tools::builtin_registry());
        let calls = vec![
            call("1", "Read", json!({"file_path": "a"})),
            call("2", "Grep", json!({"pattern": "x"})),
            call("3", "Bash", json!({"command": "ls"})),
            call("4", "Read", json!({"file_path": "b"})),
            call("5", "Glob", json!({"pattern": "*.rs"})),
        ];
        assert_eq!(
            agent.plan_parallel_batches(&calls, calls.len()),
            vec![(0, 2), (3, 5)]
        );
    }

    #[test]
    fn single_call_never_batches() {
        let agent = agent_with(crate::agent::tools::builtin_registry());
        let one = vec![call("1", "Read", json!({"file_path": "a"}))];
        assert!(agent.plan_parallel_batches(&one, 1).is_empty());
        // 混合批里落单的只读调用也不成批:[Read, Bash] → 空
        let mixed = vec![
            call("1", "Read", json!({"file_path": "a"})),
            call("2", "Bash", json!({"command": "ls"})),
        ];
        assert!(agent.plan_parallel_batches(&mixed, 2).is_empty());
    }

    #[test]
    fn emit_short_circuit_caps_batch() {
        // emit 之前的连续只读段可成批;emit 及其后一律不进批
        let mut profile = AgentProfile::yolo_profile();
        profile.defer_emit_force = true;
        let agent = Agent::new(Arc::new(UnusedLlm), profile);
        let calls = vec![
            call("1", "Read", json!({"file_path": "a"})),
            call("2", "Grep", json!({"pattern": "x"})),
            call("3", "submit_task_classification", json!({"task_level": "simple"})),
            call("4", "Read", json!({"file_path": "b"})),
            call("5", "Glob", json!({"pattern": "*.rs"})),
        ];
        assert_eq!(agent.plan_parallel_batches(&calls, calls.len()), vec![(0, 2)]);
    }

    #[test]
    fn unknown_tool_is_not_parallel() {
        let agent = agent_with(crate::agent::tools::builtin_registry());
        let calls = vec![
            call("1", "Read", json!({"file_path": "a"})),
            call("2", "NoSuchTool", json!({})),
            call("3", "Grep", json!({"pattern": "x"})),
        ];
        // 未注册工具打断连续段 → 两侧都只剩单条,不成批
        assert!(agent.plan_parallel_batches(&calls, calls.len()).is_empty());
    }

    #[tokio::test]
    async fn parallel_batch_returns_results_by_index() {
        let agent = agent_with(crate::agent::tools::builtin_registry());
        let dir = std::env::temp_dir().join(format!("laew_toolexec_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (n, body) in [("a.txt", "AAA"), ("b.txt", "BBB")] {
            std::fs::write(dir.join(n), body).unwrap();
        }
        let calls = vec![
            call("1", "Read", json!({"file_path": dir.join("a.txt").to_string_lossy()})),
            call("2", "Read", json!({"file_path": dir.join("b.txt").to_string_lossy()})),
        ];
        let batches = agent.plan_parallel_batches(&calls, calls.len());
        assert_eq!(batches, vec![(0, 2)]);
        let (out, cancelled) = agent.run_parallel_batches(&calls, &batches, None).await;
        assert!(!cancelled);
        assert_eq!(out.len(), 2);
        assert!(out[&0].output.contains("AAA"), "index 0 应是 a.txt");
        assert!(out[&1].output.contains("BBB"), "index 1 应是 b.txt");
        assert!(!out[&0].is_error && !out[&1].is_error);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_batches_are_noop() {
        let agent = agent_with(crate::agent::tools::builtin_registry());
        let calls = vec![call("1", "Read", json!({"file_path": "a"}))];
        let (out, cancelled) = agent.run_parallel_batches(&calls, &[], None).await;
        assert!(out.is_empty());
        assert!(!cancelled);
    }

    #[test]
    fn max_parallel_tools_parsing() {
        use crate::agent::tools::max_parallel_tools_from;
        assert_eq!(max_parallel_tools_from("".into()), 4);
        assert_eq!(max_parallel_tools_from("0".into()), 4);
        assert_eq!(max_parallel_tools_from("abc".into()), 4);
        assert_eq!(max_parallel_tools_from("-3".into()), 4);
        assert_eq!(max_parallel_tools_from("8".into()), 8);
        assert_eq!(max_parallel_tools_from(" 6 ".into()), 6);
    }

    /// `ToolDef` 未因新增 trait 方法而漂移(默认实现不改变 wire 工具定义)。
    #[test]
    fn parallel_safe_does_not_change_tool_def() {
        let t = crate::agent::tools::read::ReadTool;
        let d: ToolDef = t.def();
        assert_eq!(d.name, "Read");
        assert!(!d.description.is_empty());
    }
}
