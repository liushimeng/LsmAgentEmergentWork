//! 第 120 轮:SubAgent-Work 执行层 ReAct 强化与工具连续工作模式 —— Agent 循环侧用例。
//!
//! **为什么是同级新文件而不是把 `tests.rs` 转成 `tests/` 目录**:工程 1800 行拆分规范的
//! 默认做法是「单文件超线 → 转同名目录」,但 `.gitignore` 第 33 行的 `tests/` 规则匹配
//! **任意层级**的 tests 目录 —— `src/agent/tests/` 会被静默忽略,克隆后 `cargo test`
//! 直接编译失败(`mod react_continuous;` 找不到文件)。仓库既有约定因此是 `xxx/tests.rs`
//! **文件**(`main_work/tests.rs`、`orchestrator/tests.rs` 等均已入库)。本轮改为把新用例
//! 放到 `agent` 下的**同级兄弟文件**,`tests.rs` 保持 1244 行不动(仅改 1 处 fixture)。
//!
//! 覆盖四件事:
//! 1. 同批只读工具**并发执行**且 `tool_result` **保序回填**(协议配对不变量);
//! 2. 有副作用调用打断连续段,两侧落单只读调用不成批(零回归);
//! 3. 无进展止损(doom_loop)在烧光迭代预算前终止,并留下可对账的 trace 证据;
//! 4. 进度 / Thought / 收口三类 runtime hint 真的送达 SubAgent-Work 的 system
//!    (此前收口预告只有 Yolo 有),且按角色分叉不再把浏览器文案发给代码任务。
//!
//! 设计见 `docs/SubAgentWork执行层ReAct与连续工作模式/01-设计与解决方案.md`。

use super::*;
use serde_json::json;

/// 一轮返回 N 个 Read(不同文件)、下一轮返回终答的 LLM。
struct ParallelReadLlm {
    calls: std::sync::atomic::AtomicUsize,
    paths: Vec<String>,
}
#[async_trait::async_trait]
impl crate::llm::LlmClient for ParallelReadLlm {
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
                tool_calls: self
                    .paths
                    .iter()
                    .enumerate()
                    .map(|(i, p)| crate::llm::ToolCallReq {
                        id: format!("call-r{i}"),
                        name: "Read".into(),
                        arguments: json!({"file_path": p}),
                    })
                    .collect(),
                usage: Usage::default(),
                stop_reason: None,
            })
        } else {
            Ok(Completion {
                text: "读完三个文件".into(),
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

/// 在临时目录写 count 个内容互异的文件,返回绝对路径列表。
fn scratch_files(tag: &str, bodies: &[&str]) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("laew_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    bodies
        .iter()
        .enumerate()
        .map(|(i, body)| {
            let p = dir.join(format!("f{i}.txt"));
            std::fs::write(&p, body).expect("写临时文件");
            p.to_string_lossy().to_string()
        })
        .collect()
}

/// 提取上下文里全部 tool_result 的正文(按出现顺序)。
fn tool_result_contents(session: &Session) -> Vec<String> {
    session
        .context()
        .iter()
        .filter_map(|m| {
            m.content.iter().find_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
        })
        .collect()
}

#[tokio::test]
async fn parallel_readonly_batch_preserves_order() {
    let paths = scratch_files("par_order", &["AAA", "BBB", "CCC"]);
    let agent = Agent::new(
        std::sync::Arc::new(ParallelReadLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            paths: paths.clone(),
        }),
        AgentProfile::sub_agent_work_profile(),
    );
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("读三个文件"));
    let (_text, _usage, trace) = agent.run_session(&mut session).await.unwrap();

    assert_eq!(
        trace.parallel_tool_batches, 1,
        "3 个连续只读调用应并为 1 个并发批"
    );
    assert_eq!(trace.parallel_tool_calls, 3);
    assert_eq!(trace.tool_calls, 3);
    assert_eq!(trace.tool_calls_ok, 3);

    // 核心不变量:并发只改「执行时序」,不改「上下文时序」。
    let results = tool_result_contents(&session);
    assert_eq!(results.len(), 3, "应回填 3 条 tool_result");
    assert!(
        results[0].contains("AAA")
            && results[1].contains("BBB")
            && results[2].contains("CCC"),
        "tool_result 必须按 tool_calls 原序回填(协议配对依赖此序),实际: {results:?}"
    );
}

/// 一轮返回 [Read, Bash, Read] 的 LLM:只读段各长 1,均不应成批。
struct MixedBatchLlm {
    calls: std::sync::atomic::AtomicUsize,
    path: String,
}
#[async_trait::async_trait]
impl crate::llm::LlmClient for MixedBatchLlm {
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
                tool_calls: vec![
                    crate::llm::ToolCallReq {
                        id: "c1".into(),
                        name: "Read".into(),
                        arguments: json!({"file_path": self.path}),
                    },
                    crate::llm::ToolCallReq {
                        id: "c2".into(),
                        name: "Bash".into(),
                        arguments: json!({"command": "echo mixed"}),
                    },
                    crate::llm::ToolCallReq {
                        id: "c3".into(),
                        name: "Read".into(),
                        arguments: json!({"file_path": self.path}),
                    },
                ],
                usage: Usage::default(),
                stop_reason: None,
            })
        } else {
            Ok(Completion {
                text: "混合批完成".into(),
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
async fn unsafe_call_splits_batch_and_stays_serial() {
    let paths = scratch_files("par_mixed", &["MMM"]);
    let agent = Agent::new(
        std::sync::Arc::new(MixedBatchLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            path: paths[0].clone(),
        }),
        AgentProfile::sub_agent_work_profile(),
    );
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("混合批"));
    let (_text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
    assert_eq!(
        trace.parallel_tool_batches, 0,
        "Bash 打断连续只读段,两侧各只剩单条,不应成批"
    );
    assert_eq!(trace.parallel_tool_calls, 0);
    assert_eq!(trace.tool_calls, 3, "三条调用仍须全部执行");
    // 回填顺序不变
    let results = tool_result_contents(&session);
    assert_eq!(results.len(), 3);
    assert!(results[0].contains("MMM"), "第 1 条应是 Read 结果");
    assert!(results[1].contains("mixed"), "第 2 条应是 Bash 结果");
    assert!(results[2].contains("MMM"), "第 3 条应是 Read 结果");
}

/// 每轮都返回**完全相同**的 Read(同参数、文件内容恒定)→ 同动作 + 同结果。
struct SameReadLlm {
    calls: std::sync::atomic::AtomicUsize,
    path: String,
}
#[async_trait::async_trait]
impl crate::llm::LlmClient for SameReadLlm {
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
                id: format!("call-same-{n}"),
                name: "Read".into(),
                arguments: json!({"file_path": self.path}),
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
async fn doom_loop_stops_before_burning_budget() {
    let paths = scratch_files("doom", &["SAME"]);
    let agent = Agent::new(
        std::sync::Arc::new(SameReadLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            path: paths[0].clone(),
        }),
        AgentProfile::sub_agent_work_profile(),
    )
    .with_max_iterations(16);
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("反复读同一个文件"));
    // 已有工具产出 → 优雅收尾交 QC(Ok),而非硬 Err
    let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();

    assert!(trace.early_terminated, "无进展应标记 early_terminated");
    assert!(
        trace.early_terminate_reason.starts_with("doom_loop_no_progress"),
        "早终止原因应为 doom_loop_no_progress,实际 {}",
        trace.early_terminate_reason
    );
    assert!(
        trace.iterations < 16,
        "应在跑满 16 轮预算前止损,实际跑了 {} 轮",
        trace.iterations
    );
    assert!(
        trace.doom_loop_repeats >= 4,
        "止损发生在第 4 次重复,实际 {}",
        trace.doom_loop_repeats
    );
    assert!(
        text.contains("doom_loop"),
        "兜底文本应带止损标识供 QC/用户对账,实际: {text}"
    );
    assert!(
        trace
            .failure_signals
            .iter()
            .any(|s| s.starts_with("early_terminate:doom_loop")),
        "失败信号应含 doom_loop,实际 {:?}",
        trace.failure_signals
    );
    assert!(
        trace.is_failed(),
        "无进展止损必须被 is_failed() 判为失败(供 QC / Yolo 回流)"
    );
}

/// 前 N 轮返回互不相同的 Bash 调用(避开 doom_loop)、之后返回终答,并捕获每轮 system。
struct SystemCaptureLlm {
    calls: std::sync::atomic::AtomicUsize,
    tool_rounds: usize,
    seen_system: std::sync::Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl crate::llm::LlmClient for SystemCaptureLlm {
    async fn complete(
        &self,
        system: &str,
        _messages: &[ChatMessage],
        _tools: &[crate::llm::ToolDef],
        _meta: &RequestMeta,
    ) -> Result<Completion> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.seen_system
            .lock()
            .expect("system capture")
            .push(system.to_string());
        if n < self.tool_rounds {
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![crate::llm::ToolCallReq {
                    id: format!("call-{n}"),
                    name: "Bash".into(),
                    // 每轮命令不同 → 不触发 doom_loop,专测 hint 送达
                    arguments: json!({"command": format!("echo round-{n}")}),
                }],
                usage: Usage::default(),
                stop_reason: None,
            })
        } else {
            Ok(Completion {
                text: "收口".into(),
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
async fn subagent_system_gets_progress_thought_and_deadline_hints() {
    let cap = std::sync::Arc::new(SystemCaptureLlm {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_rounds: 3,
        seen_system: std::sync::Mutex::new(Vec::new()),
    });
    let agent = Agent::new(cap.clone(), AgentProfile::sub_agent_work_profile())
        .with_max_iterations(4);
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("跑三轮"));
    agent.run_session(&mut session).await.unwrap();

    let seen = cap.seen_system.lock().expect("system capture");
    assert!(seen.len() >= 3, "应至少 3 轮 LLM 调用,实际 {}", seen.len());

    // ① 第 1 轮(iter=0):预算未过半、无静默轮、非倒数第二轮 → 三类 hint 都不该出现。
    // 注:收口预告的断言锚点用「迭代预算即将耗尽」而非「【收口提示】」—— 后者也出现在
    // SubAgent-Work 基础提示词的 ReAct 节里(向模型解释该标记的含义),会假阳性。
    assert!(!seen[0].contains("【进度】"), "首轮不该有进度 hint");
    assert!(
        !seen[0].contains("迭代预算即将耗尽"),
        "首轮不该有收口预告"
    );
    assert!(
        !seen[0].contains("只发工具调用"),
        "首轮不该有 Thought 提醒"
    );

    // ② iter=1:预算过半(iterations=2,max=4)→ 进度 hint 出现
    assert!(
        seen[1].contains("【进度】"),
        "预算过半应注入进度 hint,实际 system 尾部: {}",
        &seen[1][seen[1].len().saturating_sub(400)..]
    );

    // ③ iter=2:连续 2 轮「仅 tool_use 无文本」→ Thought 提醒;
    //    且 iter+2==max → 通用收口预告(此前只有 Yolo 有)
    assert!(
        seen[2].contains("只发工具调用"),
        "连续 2 轮静默应提醒补写 Thought"
    );
    assert!(
        seen[2].contains("Thought"),
        "Thought 提醒应点名 ReAct 三段"
    );
    // 锚点取收口预告独有的整句(「expected_output」也出现在基础提示词的输出格式节,
    // 单独断言它会假阳性)。
    assert!(
        seen[2].contains("迭代预算即将耗尽(下一轮为最终轮)。请立即停止新动作"),
        "无 emit 通道的执行层也应收到收口预告,实际尾部: {}",
        &seen[2][seen[2].len().saturating_sub(500)..]
    );
    assert!(
        seen[2].contains("实际产出与 expected_output 的对应关系"),
        "执行层收口预告应要求对齐 expected_output"
    );
    assert!(
        !seen[2].contains("submit_task_classification"),
        "执行层无 emit 通道,收口预告不得提 Yolo 的提交工具"
    );
    // hint 必须落在 system 末尾的 RUNTIME_HINTS 标记块内(不破坏 cache 前缀)
    assert!(
        seen[2].contains("<<<LAEW:RUNTIME_HINTS>>>"),
        "hint 应包裹在 RUNTIME_HINTS 标记内"
    );
}

#[tokio::test]
async fn yolo_deadline_hint_still_names_emit_tool() {
    // 回归保护:Yolo 的收口预告文案不变(仍指名 submit_task_classification)
    let cap = std::sync::Arc::new(SystemCaptureLlm {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_rounds: 3,
        seen_system: std::sync::Mutex::new(Vec::new()),
    });
    let agent = Agent::new(cap.clone(), AgentProfile::yolo_profile()).with_max_iterations(4);
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("分类"));
    let _ = agent.run_session(&mut session).await;
    let seen = cap.seen_system.lock().expect("system capture");
    assert!(seen.len() >= 3);
    // 锚点取 Yolo 版收口预告独有的整句:Yolo 基础提示词本身就多处提到
    // submit_task_classification,单独断言工具名会假阳性。
    assert!(
        seen[2].contains("迭代预算即将耗尽(下一轮为最终轮,系统将强制要求提交"),
        "Yolo 收口预告应保持第 119 轮文案(指名强制提交 emit),实际尾部: {}",
        &seen[2][seen[2].len().saturating_sub(500)..]
    );
    assert!(
        !seen[2].contains("实际产出与 expected_output 的对应关系"),
        "Yolo 不应收到执行层版收口文案"
    );
}

#[test]
fn runtime_hints_explore_text_is_role_specific() {
    use crate::agent::runtime_hints::HintRole;
    let mut t = ExecutionTrace::default();
    t.explore_budget_exhausted = true;
    let build = |role| {
        build_runtime_hints_with(&RuntimeHintCtx {
            trace: &t,
            consecutive_failures: 0,
            role,
            loop_nudge: None,
            silent_rounds: 0,
            deadline: None,
        })
    };
    // D5 根因:UI 文案发给代码任务是错误指令 → 现在按角色分叉
    let ui = build(HintRole::Ui);
    assert!(ui.contains("click") && ui.contains("inspect"), "UI 角色应保留浏览器文案");
    let exec = build(HintRole::Execute);
    assert!(
        exec.contains("Write") && exec.contains("Bash"),
        "执行角色应指向落地改动与验证"
    );
    assert!(
        !exec.contains("screenshot"),
        "执行角色不得收到浏览器文案,实际: {exec}"
    );
    let gather = build(HintRole::Gather);
    assert!(gather.contains("收口提交"), "信息收集角色应提示收口");
    // 判定层无探索/执行阶段之分 → 不注入
    assert!(
        build(HintRole::Judge).is_empty(),
        "Judge 角色不应注入执行期文案"
    );
}

#[test]
fn runtime_hints_progress_only_after_half_budget() {
    use crate::agent::runtime_hints::HintRole;
    let build = |iterations, max| {
        let t = ExecutionTrace {
            iterations,
            max_iterations: max,
            tool_calls: 3,
            tool_calls_ok: 2,
            tool_calls_err: 1,
            ..ExecutionTrace::default()
        };
        build_runtime_hints_with(&RuntimeHintCtx {
            trace: &t,
            consecutive_failures: 0,
            role: HintRole::Execute,
            loop_nudge: None,
            silent_rounds: 0,
            deadline: None,
        })
    };
    assert!(build(1, 16).is_empty(), "1/16 不该有进度 hint");
    assert!(build(7, 16).is_empty(), "7/16 未过半,不该有进度 hint");
    let half = build(8, 16);
    assert!(half.contains("【进度】第 8/16 轮"), "过半应注入进度: {half}");
    assert!(
        half.contains("成功 2 / 失败 1"),
        "进度 hint 应带工具成败计数: {half}"
    );
    // max_iterations=0(旧 trace 反序列化)不得触发,否则默认 trace 也会冒出 hint
    assert!(build(0, 0).is_empty(), "max=0 时不应注入进度 hint");
}

#[test]
fn runtime_hints_loop_nudge_and_thought_are_delivered() {
    use crate::agent::runtime_hints::HintRole;
    let t = ExecutionTrace::default();
    let h = build_runtime_hints_with(&RuntimeHintCtx {
        trace: &t,
        consecutive_failures: 0,
        role: HintRole::Execute,
        loop_nudge: Some("【无进展警告】自定义文案"),
        silent_rounds: crate::agent::runtime_hints::THOUGHT_NUDGE_AT,
        deadline: Some("【收口提示】自定义文案"),
    });
    assert!(h.contains("【无进展警告】自定义文案"));
    assert!(h.contains("只发工具调用"), "Thought 提醒应在阈值触发");
    assert!(h.contains("【收口提示】自定义文案"));
    // 低于阈值不触发
    let quiet = build_runtime_hints_with(&RuntimeHintCtx {
        trace: &t,
        consecutive_failures: 0,
        role: HintRole::Execute,
        loop_nudge: None,
        silent_rounds: crate::agent::runtime_hints::THOUGHT_NUDGE_AT - 1,
        deadline: None,
    });
    assert!(quiet.is_empty(), "全空信号应返回空串(零开销)");
}

#[test]
fn hint_role_derivation_uses_observed_tools_not_static_surface() {
    use crate::agent::runtime_hints::HintRole;
    let empty = ExecutionTrace::default();
    assert_eq!(
        AgentProfile::yolo_profile().hint_role(&empty),
        HintRole::Gather,
        "Yolo(延迟强制 + emit)= 信息收集"
    );
    assert_eq!(
        AgentProfile::quality_check_profile().hint_role(&empty),
        HintRole::Judge
    );
    assert_eq!(
        AgentProfile::session_context_profile().hint_role(&empty),
        HintRole::Judge
    );
    // 关键:builtin_registry() 无条件含 MCP_Web_Use,但**没真调用过** → Execute
    assert_eq!(
        AgentProfile::sub_agent_work_profile().hint_role(&empty),
        HintRole::Execute,
        "工具面含浏览器工具不等于本单元是浏览器任务"
    );
    assert_eq!(
        AgentProfile::main_work_profile().hint_role(&empty),
        HintRole::Execute
    );
    // 真调用过浏览器 / 桌面 → Ui
    let mut used_web = ExecutionTrace::default();
    used_web.record_tool_call("MCP_Web_Use", "{}", true, 8, 1, "", "ok");
    assert_eq!(
        AgentProfile::sub_agent_work_profile().hint_role(&used_web),
        HintRole::Ui
    );
    let mut used_window = ExecutionTrace::default();
    used_window.record_tool_call("MCP_Window_Use", "{}", true, 8, 1, "", "ok");
    assert_eq!(
        AgentProfile::sub_agent_work_profile().hint_role(&used_window),
        HintRole::Ui
    );
    // 回归保护:叶子动态子 Agent 的 spawn_policy 也是 Disabled,但它是**干实事的执行层**
    // (第 114 轮:去掉 SubAgent 工具后策略降为 Disabled),按策略判会把它归到判定层、
    // 拿不到执行期提示。角色判据必须是「有没有结构化结论出口 / 有没有工具」。
    let leaf = AgentProfile::dynamic_child(
        "leaf",
        "prompt".into(),
        crate::agent::tools::sub_agent_work_registry().subset(&[], &["SubAgent"]),
        crate::agent::self_awareness::SpawnPolicy::Disabled,
    );
    assert_eq!(
        leaf.hint_role(&empty),
        HintRole::Execute,
        "叶子动态子 Agent 是执行层,不是判定层"
    );
}

#[test]
fn sub_agent_prompt_declares_react_and_continuous_mode() {
    let p = AgentProfile::sub_agent_work_profile();
    let s = p
        .system_prompt
        .render(crate::config::Protocol::Anthropic);
    // ReAct 三段(D1:此前提示词零 ReAct 表述)
    assert!(s.contains("执行循环(ReAct 模式)"), "应声明 ReAct 执行循环");
    assert!(s.contains("Thought(推理)"), "应要求显式 Thought");
    assert!(s.contains("Action(行动)"), "应定义 Action 段");
    assert!(s.contains("Observation(观察)"), "应要求归纳 Observation");
    assert!(
        s.contains("禁止无 Thought 的盲调"),
        "应禁止盲调"
    );
    // 连续工作模式(D8:复合工具此前未与 ReAct 循环绑定)
    assert!(s.contains("连续工作模式"), "应声明连续工作模式");
    assert!(s.contains("input_batch"), "应点名 MCP_Window_Use 复合动作");
    assert!(s.contains("sequence"), "应点名 sequence 复合动作");
    assert!(s.contains("TodoWrite"), "应把 TodoWrite 绑进 ReAct 进度自证");
    // 无进展约束必须让模型知道阈值(否则它会以为可以无限重试)
    assert!(
        s.contains("无进展") && s.contains("止损"),
        "应告知无进展止损规则"
    );
    // D9:提示词承诺的并发必须与运行时一致(只读三件套并发,写类保序)
    assert!(
        s.contains("并发执行(上限 4 条)"),
        "应说明只读调用会被并发执行及其上限"
    );
    assert!(
        s.contains("保序串行"),
        "应说明有副作用调用会被保序串行"
    );
}

#[test]
fn trace_render_prompt_exposes_react_evidence() {
    // QC 必须能看到「并发批用没用起来 / 有没有原地打转」
    let mut t = ExecutionTrace::default();
    t.parallel_tool_batches = 2;
    t.parallel_tool_calls = 5;
    t.doom_loop_repeats = 3;
    let rendered = t.render_prompt();
    assert!(
        rendered.contains("parallel_batches=2"),
        "QC prompt 应含并发批计数: {rendered}"
    );
    assert!(rendered.contains("parallel_calls=5"));
    assert!(rendered.contains("no_progress_repeats=3"));
    t.collect_failure_signals("");
    assert!(
        t.failure_signals.iter().any(|s| s == "doom_loop:3x"),
        "重复 ≥ NUDGE_AT 应打 doom_loop 弱信号,实际 {:?}",
        t.failure_signals
    );
    // 零并发零重复时不占 QC prompt 行数
    let clean = ExecutionTrace::default();
    assert!(!clean.render_prompt().contains("react:"));
}

// ======================== 第 122 轮(2026-09-23)Main-Work ReAct 编排测试 ========================
// 编排层 ReAct 化补齐(对齐第 120 轮执行层 SubAgentWork)。验证:
// 1. Main-WorkRunner 编排时若连续 4 次完全相同的工具调用 → 触发 LoopGuard 止损
//    (与 SubAgentWork 共享同一份 LoopGuard 双阈值);
// 2. Main-Work 工具面含 MCP_Web_Use(用于编排前网页前提验证);
// 3. hint_role 派生:Main-Work 工具面无 MCP_Web_Use 调用 → HintRole::Execute;
//    若真动过 MCP_Web_Use → HintRole::Ui。
// 设计见 docs/Main-Work工具扩展与ReAct改造/01-设计与解决方案.md §3.4。

/// 一轮返回 Read 同一个文件(持续 4 轮)的 LLM —— 编排层 doom_loop 端到端。
struct MainWorkDoomLlm {
    calls: std::sync::atomic::AtomicUsize,
    path: String,
}
#[async_trait::async_trait]
impl crate::llm::LlmClient for MainWorkDoomLlm {
    async fn complete(
        &self,
        _system: &str,
        _messages: &[ChatMessage],
        _tools: &[crate::llm::ToolDef],
        _meta: &RequestMeta,
    ) -> Result<Completion> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Completion {
            text: format!("编排第 {n} 轮"),
            tool_calls: vec![crate::llm::ToolCallReq {
                id: format!("call-{n}"),
                name: "Read".into(),
                arguments: json!({"file_path": self.path}),
            }],
            usage: Usage::default(),
            stop_reason: None,
        })
    }
    fn protocol(&self) -> crate::config::Protocol {
        crate::config::Protocol::Anthropic
    }
}

/// 验证:Main-WorkRunner 编排时连续 4 次相同 Read 触发 LoopGuard 止损。
///
/// Main-Work 是「思考轮」循环,虽然迭代预算较小(8 轮),但 doom_loop 阈值
/// (NUDGE_AT=2 / ABORT_AT=3 + 宽限轮)与 SubAgent-Work 共用同一份进展键与
/// 双阈值,本轮验证编排层接入后零回归。
#[tokio::test]
async fn main_work_runner_doom_loop_stops() {
    let paths = scratch_files("mainwork_doom", &["SAME"]);
    let agent = Agent::new(
        std::sync::Arc::new(MainWorkDoomLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            path: paths[0].clone(),
        }),
        AgentProfile::main_work_profile(),
    )
    .with_max_iterations(8); // 编排层默认 8 轮
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("编排盲拆"));
    let (_text, _usage, trace) = agent.run_session(&mut session).await.unwrap();

    assert!(
        trace.early_terminated,
        "Main-Work 编排层无进展应早终止"
    );
    assert!(
        trace.early_terminate_reason.starts_with("doom_loop_no_progress"),
        "早终止原因应为 doom_loop_no_progress,实际 {}",
        trace.early_terminate_reason
    );
    assert!(
        trace.iterations < 8,
        "应在跑满 8 轮预算前止损,实际跑了 {} 轮",
        trace.iterations
    );
    assert!(
        trace.doom_loop_repeats >= 4,
        "止损发生在第 4 次重复,实际 {}",
        trace.doom_loop_repeats
    );
    assert!(
        trace
            .failure_signals
            .iter()
            .any(|s| s.starts_with("early_terminate:doom_loop")),
        "Main-Work 编排层失败信号应含 doom_loop,实际 {:?}",
        trace.failure_signals
    );
}

/// 验证:Main-Work 工具面含 MCP_Web_Use,与 SubAgent-Work 的全集合保持兼容。
#[test]
fn main_work_profile_tool_names_mcp_web_use() {
    let profile = AgentProfile::main_work_profile();
    let names: Vec<&str> = profile.tools.names();
    assert!(
        names.contains(&"MCP_Web_Use"),
        "Main-Work 编排层应持 MCP_Web_Use(信息收集),实际: {names:?}"
    );
    assert!(names.contains(&"Bash"));
    assert!(names.contains(&"Read"));
    assert!(names.contains(&"Glob"));
    assert!(names.contains(&"Grep"));
    assert!(names.contains(&"TodoWrite"));
    assert!(names.contains(&"SubAgent"));
    // 仍不持 Write / Edit / MCP_Window_Use
    assert!(!names.contains(&"Write"));
    assert!(!names.contains(&"Edit"));
}

/// 验证:Main-Work hint_role 派生 —— 未用 MCP_Web_Use → HintRole::Execute
/// (与 SubAgent-Work 代码类单元一致)。
#[tokio::test]
async fn main_work_hint_role_execute_when_no_ui_tool_used() {
    let paths = scratch_files("mainwork_role", &["x"]);
    let agent = Agent::new(
        std::sync::Arc::new(MainWorkDoomLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            path: paths[0].clone(),
        }),
        AgentProfile::main_work_profile(),
    )
    .with_max_iterations(8);
    let mut session = Session::new();
    session.context_mut().push(ChatMessage::user("编排"));
    let (_text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
    // trace.tool_call_log 应只含 Read,无 MCP_Web_Use → HintRole::Execute
    let used_ui = trace.tool_call_log.iter().any(|e| {
        e.tool == crate::agent::tools::mcp_web_use::MCP_WEB_USE_TOOL_NAME
            || e.tool == crate::agent::tools::mcp_window_use::MCP_WINDOW_USE_TOOL_NAME
    });
    assert!(!used_ui, "本用例不应触发 Ui 角色(只用 Read)");
}
