//! ReAct 循环守卫(第 120 轮,2026-09-23):无进展检测(doom_loop)+ 双阈值止损。
//!
//! # 为什么需要它
//!
//! `agent_loop.rs` 既有三个计数器各自覆盖一类跑飞:
//!
//! | 机制 | 触发条件 | 覆盖场景 |
//! |------|---------|---------|
//! | `REPEATED_FAILURE_THRESHOLD=3` | 连续相同 **(工具,参数)** 且**失败** | 幻觉路径反复 Read 不存在文件 |
//! | `NO_TEXT_CONVERGE_THRESHOLD=8` | 连续 8 轮「仅 tool_use 无文本」 | 不停探查从不收敛 |
//! | `NO_TOOL_USE_THRESHOLD=3` | 连续 3 轮无 tool_use | 空跑 / 引导失效 |
//!
//! 三者**全部漏掉**同一类最常见的浪费:工具调用**成功**、结果**一字不变**、
//! 模型却一轮一轮重复同一动作(反复 Read 同一个文件、反复 inspect 同一个页面、
//! 反复跑同一条「成功但无用」的命令)。失败计数被成功调用重置,无文本计数在
//! 16 轮预算里要等到第 8 轮才响 —— 预算烧光后被 `MaxIterationsExceeded` 硬杀,
//! QC 拿到的是「跑满上限 + 零产物」。
//!
//! # 与外部工程的关键差异:Observation 进入进展键
//!
//! | 工程 | 重复判定键 | 缺陷 |
//! |------|-----------|------|
//! | OpenCode `doom_loop` | 工具名 + 输入 hash | 轮询等待(同一 inspect 等页面变化)被误判为死循环 |
//! | DeepSeek `repeat-tool-reminder` | 工具名 + `canonicalize(arguments)` | 同上 |
//! | AtomCode `ToolLoopPolicy` | 工具调用签名 | 同上 |
//! | **laew `LoopGuard`** | 工具名 + 参数稳定 JSON + **结果摘要** | 「同动作 + **同结果**」才算无进展;轮询期间结果在变 → 判为有进展,零误伤 |
//!
//! 参数序列化复用既有 [`crate::agent::runtime_hints::stable_json_string`]
//! (对象 key 递归排序),正是 DeepSeek `canonicalize = deep key-sort JSON` 的
//! 等价实现 —— LLM 调换参数顺序不会被误判为「换了个新动作」。
//!
//! # 等待类调用透明化
//!
//! 有一类调用**天生**会被重复且输出恒定,重复它们是正确行为而非无进展:
//! `MCP_Web_Use(control_action=wait)` / `MCP_Window_Use(action=wait)` /
//! 纯 `sleep N` 的 Bash / `SubAgent(action=result|history)`(轮询子 Agent 结果)。
//! 这类调用经 [`wait_like`] 识别后**不计入也不打断**重复链(透明),
//! 于是 `Read(x) → wait → Read(x) → wait → Read(x)` 仍能正确判为 3 次无进展。
//!
//! 设计见 `docs/SubAgentWork执行层ReAct与连续工作模式/01-设计与解决方案.md` §3.3。

use serde_json::Value;

use super::runtime_hints::stable_json_string;

/// 软提醒阈值:连续第 N 次「同动作 + 同结果」→ 注入 nudge,**不终止**循环。
///
/// 对齐 AtomCode `REPEAT_NUDGE_AT=3` / OpenCode `DOOM_LOOP_THRESHOLD=3`,但 laew
/// 单元迭代预算只有 16 轮(外部工程 50~256 轮),阈值必须更紧,否则提醒还没
/// 生效预算就烧完了。
pub(crate) const NUDGE_AT: usize = 2;

/// 止损阈值:连续第 N 次 → 首次命中给**宽限轮**(强提醒但不停),第二次才硬终止。
pub(crate) const ABORT_AT: usize = 3;

/// Observation 摘要取样长度(字符)。取头部即可判别「结果是否变化」,
/// 尾部差异(时间戳/耗时)不参与,避免把「同一结果不同耗时」误判为有进展。
const RESULT_DIGEST_CHARS: usize = 256;

/// 一次 [`LoopGuard::observe`] 的裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopVerdict {
    /// 有进展(首次出现该动作 / 动作或结果变了 / 守卫已关闭)。
    Progress,
    /// 无进展软提醒:文案暂存守卫,下一轮经 runtime hints 注入 system 末尾
    /// (不污染上下文、不破坏 `cache_control` 缓存前缀)。
    Nudge(String),
    /// 宽限轮强提醒:**立即**作为 user 消息推入上下文(保证送达 LLM),不终止。
    /// 对齐 `agent_loop.rs` 第 25 轮 F9「nudge 必须真的送达才有意义」的教训。
    Grace(String),
    /// 止损:终止本单元。
    Abort { tool: String, repeats: usize },
}

/// ReAct 循环守卫:按「工具 + 参数 + 结果」三元组追踪重复链。
///
/// 生命周期 = 一次 `run_session_body`(与 `ExecutionTrace` 同域),不跨单元复用。
#[derive(Debug, Clone)]
pub(crate) struct LoopGuard {
    last_key: Option<String>,
    repeats: usize,
    /// 当前重复链上软提醒是否已发过(避免同档位每轮刷屏)。
    nudged: bool,
    /// `ABORT_AT` 首次命中的宽限轮是否已用(全会话一次,对齐 F9 `no_text_grace_used`)。
    abort_grace_used: bool,
    enabled: bool,
    /// 本会话观测到的最大重复次数(供 `ExecutionTrace` / TUI / Debug Report 观测)。
    max_repeats: usize,
    /// 待下一轮注入的软提醒文案(`Nudge` 裁决时写入,消费后清空)。
    pending_nudge: Option<String>,
}

impl Default for LoopGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopGuard {
    /// 按环境开关构造(默认开启,`LAEW_REACT_GUARD=off` 关闭)。
    pub(crate) fn new() -> Self {
        Self::with_enabled(react_guard_enabled())
    }

    /// 显式指定开关(单测用)。关闭后 [`observe`](Self::observe) 恒返回
    /// [`LoopVerdict::Progress`],但 `max_repeats` 仍统计(可观测性不丢)。
    pub(crate) fn with_enabled(enabled: bool) -> Self {
        Self {
            last_key: None,
            repeats: 0,
            nudged: false,
            abort_grace_used: false,
            enabled,
            max_repeats: 0,
            pending_nudge: None,
        }
    }

    /// 本会话观测到的最大重复次数。
    pub(crate) fn max_repeats(&self) -> usize {
        self.max_repeats
    }

    /// 取走待注入的软提醒文案(消费后清空)。
    pub(crate) fn take_nudge(&mut self) -> Option<String> {
        self.pending_nudge.take()
    }

    /// 记录一次已执行的工具调用,返回裁决。
    ///
    /// 必须在工具**执行完成后**调用(需要 `is_error` 与 `output` 参与进展键);
    /// 等待类调用([`wait_like`])透明跳过 —— 既不计入也不打断重复链。
    pub(crate) fn observe(
        &mut self,
        tool: &str,
        args: &Value,
        is_error: bool,
        output: &str,
    ) -> LoopVerdict {
        if wait_like(tool, args) {
            return LoopVerdict::Progress;
        }
        let key = progress_key(tool, args, is_error, output);
        if self.last_key.as_deref() == Some(key.as_str()) {
            self.repeats += 1;
        } else {
            self.last_key = Some(key);
            self.repeats = 1;
            // 换动作/换结果 → 软提醒档位重置(宽限轮是全会话一次,不重置)
            self.nudged = false;
        }
        self.max_repeats = self.max_repeats.max(self.repeats);
        if !self.enabled {
            return LoopVerdict::Progress;
        }

        let args_brief = args_brief(args);
        if self.repeats >= ABORT_AT {
            if !self.abort_grace_used {
                self.abort_grace_used = true;
                return LoopVerdict::Grace(grace_text(tool, &args_brief, self.repeats));
            }
            return LoopVerdict::Abort {
                tool: tool.to_string(),
                repeats: self.repeats,
            };
        }
        if self.repeats >= NUDGE_AT && !self.nudged {
            self.nudged = true;
            let text = nudge_text(tool, &args_brief, self.repeats);
            self.pending_nudge = Some(text.clone());
            return LoopVerdict::Nudge(text);
        }
        LoopVerdict::Progress
    }
}

/// 进展键 = `工具名 # 参数稳定 JSON # 结果摘要`。
///
/// 结果摘要含 `is_error` + 输出字节长度 + 头部 256 字符的哈希:
/// 前 256 字符相同但总长不同(例:列表在增长)也算**有进展**。
pub(crate) fn progress_key(tool: &str, args: &Value, is_error: bool, output: &str) -> String {
    format!(
        "{tool}#{}#{}",
        stable_json_string(args),
        result_digest(is_error, output)
    )
}

/// 结果摘要:`ok|err : 字节长度 : 头部哈希`。
fn result_digest(is_error: bool, output: &str) -> String {
    use std::hash::{Hash, Hasher};
    let head: String = output.chars().take(RESULT_DIGEST_CHARS).collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    head.hash(&mut hasher);
    format!(
        "{}:{}:{:x}",
        if is_error { "err" } else { "ok" },
        output.len(),
        hasher.finish()
    )
}

/// 参数摘要(供 nudge 文案展示,截断到 120 字符)。
fn args_brief(args: &Value) -> String {
    let s = stable_json_string(args);
    let brief: String = s.chars().take(120).collect();
    if brief.len() < s.len() {
        format!("{brief}…")
    } else {
        brief
    }
}

/// 等待类调用判定:天生会被重复且输出恒定,重复它们是**正确行为**。
///
/// 命中则透明跳过(不计入、不打断重复链),于是
/// `Read(x) → wait → Read(x) → wait → Read(x)` 仍正确判为 3 次无进展。
///
/// | 工具 | 命中条件 |
/// |------|---------|
/// | `MCP_Web_Use` | `action == "wait"` 或 `(params.)control_action == "wait"` |
/// | `MCP_Window_Use` | `action == "wait"` 或 `(params.)control_action == "wait"` |
/// | `Bash` | 命令是纯 `sleep N`(可带 `&&` 前后空白的整条只有 sleep) |
/// | `SubAgent` | `action == "result"` / `"history"`(轮询子 Agent 运行结果) |
pub(crate) fn wait_like(tool: &str, args: &Value) -> bool {
    // 收集顶层 + 嵌套 `params` 里某个 key 的字符串取值(MCP_* 工具两种入参形状都吃)。
    // **不能用「找到第一个就返回」**:典型形状是 `{"action":"control","params":{"control_action":"wait"}}`
    // —— 顶层 `action` 先命中会掩盖嵌套里的 `wait`。
    let strs_at = |key: &str| -> Vec<String> {
        let mut out = Vec::new();
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            out.push(v.to_string());
        }
        if let Some(v) = args
            .get("params")
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
        {
            out.push(v.to_string());
        }
        out
    };
    match tool {
        "MCP_Web_Use" | "MCP_Window_Use" => strs_at("action")
            .iter()
            .chain(strs_at("control_action").iter())
            .any(|v| v == "wait"),
        "SubAgent" => strs_at("action")
            .iter()
            .any(|v| v == "result" || v == "history"),
        "Bash" => strs_at("command").iter().any(|c| {
            let c = c.trim();
            c.starts_with("sleep ") && c["sleep ".len()..].trim().parse::<f64>().is_ok()
        }),
        _ => false,
    }
}

fn nudge_text(tool: &str, args_brief: &str, repeats: usize) -> String {
    format!(
        "【无进展警告】你已连续 {repeats} 次发起完全相同的工具调用并得到完全相同的结果\
         ({tool} {args_brief})。重复同一动作不会产生新信息。请立即改变策略:\
         (1) 换工具或换参数(不同路径 / 不同 selector / 不同命令);\
         (2) 若目标是等待外部状态变化,改用带 wait/timeout 的调用而非反复轮询;\
         (3) 若已确认无法推进,直接收口并如实说明卡点。"
    )
}

fn grace_text(tool: &str, args_brief: &str, repeats: usize) -> String {
    format!(
        "[ReAct 无进展止损宽限轮 · 第 120 轮]\n\
         你已连续 {repeats} 次执行完全相同的调用并拿到完全相同的结果:\n\
         \x20 {tool} {args_brief}\n\
         下一次再出现同样的「动作 + 结果」,本单元将被强制终止并判为无进展失败。\n\
         请立刻做以下之一:\n\
         \x20 (1) 换一个完全不同的动作(换工具 / 换参数 / 换路径);\n\
         \x20 (2) 若已拿到足够信息,直接输出终答收口(完成情况 + 实际产出与 \
         expected_output 的对应关系);\n\
         \x20 (3) 若确实无法推进,明确说明卡点与已尝试过的方法。"
    )
}

/// 止损终止时的兜底叙事文本(交给 QC / 用户层)。
pub(crate) fn abort_fallback_text(tool: &str, repeats: usize, history_block: &str) -> String {
    format!(
        "[SubAgent 无进展止损(doom_loop)]\n\
         - 触发原因:连续 {repeats} 次执行完全相同的调用并得到完全相同的结果({tool})\n\
         - 判定依据:进展键 = 工具名 + 参数稳定 JSON + 结果摘要,三者全同即无进展\n\
         - 最近工具调用摘要:\n{history_block}\n\
         请上游据此换路径重试(不要重复同一动作),或确认该单元目标是否可达。"
    )
}

/// ReAct 循环守卫总开关(第 120 轮)。
///
/// 环境变量 `LAEW_REACT_GUARD=off|0|false|no` 关闭无进展软提醒与止损
/// (对齐 `LAEW_FORCED_TOOLS` / `LAEW_PARALLEL_TOOLS` 惯例);默认开启。
/// 关闭后 `max_repeats` 仍统计,可观测性不丢。
pub(crate) fn react_guard_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        react_guard_enabled_from(std::env::var("LAEW_REACT_GUARD").unwrap_or_default())
    })
}

/// 开关取值解析(独立出来便于单测,惯例同 `forced_tools_enabled_from`)。
pub(crate) fn react_guard_enabled_from(raw: String) -> bool {
    !matches!(
        raw.trim().to_lowercase().as_str(),
        "off" | "0" | "false" | "no"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guard() -> LoopGuard {
        LoopGuard::with_enabled(true)
    }

    #[test]
    fn same_action_same_result_counts_repeat() {
        let mut g = guard();
        let args = json!({"file_path": "a.txt"});
        // 第 1 次:首次出现 → 有进展
        assert_eq!(g.observe("Read", &args, false, "hello"), LoopVerdict::Progress);
        // 第 2 次:同动作 + 同结果 → 软提醒
        match g.observe("Read", &args, false, "hello") {
            LoopVerdict::Nudge(t) => assert!(t.contains("无进展警告"), "{t}"),
            other => panic!("期望 Nudge,实得 {other:?}"),
        }
        // 第 3 次:命中 ABORT_AT → 宽限轮强提醒(不停)
        match g.observe("Read", &args, false, "hello") {
            LoopVerdict::Grace(t) => assert!(t.contains("宽限轮"), "{t}"),
            other => panic!("期望 Grace,实得 {other:?}"),
        }
        // 第 4 次:宽限已用 → 止损
        match g.observe("Read", &args, false, "hello") {
            LoopVerdict::Abort { tool, repeats } => {
                assert_eq!(tool, "Read");
                assert_eq!(repeats, 4);
            }
            other => panic!("期望 Abort,实得 {other:?}"),
        }
        assert_eq!(g.max_repeats(), 4);
    }

    #[test]
    fn same_action_different_result_is_progress() {
        // 轮询等待场景:同参数但结果在变 → 恒有进展,零误伤。
        let mut g = guard();
        let args = json!({"action": "inspect", "info": "elements"});
        assert_eq!(g.observe("MCP_Web_Use", &args, false, "loading"), LoopVerdict::Progress);
        assert_eq!(g.observe("MCP_Web_Use", &args, false, "loading."), LoopVerdict::Progress);
        assert_eq!(g.observe("MCP_Web_Use", &args, false, "loaded"), LoopVerdict::Progress);
        assert_eq!(g.max_repeats(), 1);
    }

    #[test]
    fn same_text_different_length_is_progress() {
        // 前 256 字符相同但总长不同(列表在增长)→ 有进展。
        let mut g = guard();
        let args = json!({"pattern": "*.rs"});
        let base = "x".repeat(RESULT_DIGEST_CHARS);
        assert_eq!(g.observe("Grep", &args, false, &base), LoopVerdict::Progress);
        assert_eq!(
            g.observe("Grep", &args, false, &format!("{base}y")),
            LoopVerdict::Progress
        );
        assert_eq!(g.max_repeats(), 1);
    }

    #[test]
    fn error_and_success_are_different_keys() {
        let mut g = guard();
        let args = json!({"command": "ls /nope"});
        assert_eq!(g.observe("Bash", &args, true, "no such file"), LoopVerdict::Progress);
        assert_eq!(g.observe("Bash", &args, false, "no such file"), LoopVerdict::Progress);
        assert_eq!(g.max_repeats(), 1);
    }

    #[test]
    fn arg_order_does_not_break_key() {
        // stable_json_string 递归排序 key:LLM 调换参数顺序仍是同一动作。
        let mut g = guard();
        let a = json!({"path": "src", "pattern": "*.rs"});
        let b = json!({"pattern": "*.rs", "path": "src"});
        assert_eq!(progress_key("Glob", &a, false, "r"), progress_key("Glob", &b, false, "r"));
        assert_eq!(g.observe("Glob", &a, false, "r"), LoopVerdict::Progress);
        match g.observe("Glob", &b, false, "r") {
            LoopVerdict::Nudge(_) => {}
            other => panic!("期望 Nudge,实得 {other:?}"),
        }
    }

    #[test]
    fn different_tool_resets_chain() {
        let mut g = guard();
        let args = json!({"file_path": "a.txt"});
        assert_eq!(g.observe("Read", &args, false, "x"), LoopVerdict::Progress);
        assert_eq!(g.observe("Grep", &args, false, "x"), LoopVerdict::Progress);
        assert_eq!(g.observe("Read", &args, false, "x"), LoopVerdict::Progress);
        assert_eq!(g.max_repeats(), 1);
    }

    #[test]
    fn nudge_fires_once_per_chain() {
        // 同一档位不刷屏:第 2 次 Nudge 后,第 3 次直接进 Grace(而非再 Nudge)。
        let mut g = guard();
        let args = json!({"file_path": "a.txt"});
        let _ = g.observe("Read", &args, false, "x");
        assert!(matches!(g.observe("Read", &args, false, "x"), LoopVerdict::Nudge(_)));
        assert!(matches!(g.observe("Read", &args, false, "x"), LoopVerdict::Grace(_)));
    }

    #[test]
    fn pending_nudge_is_consumed_once() {
        let mut g = guard();
        let args = json!({"file_path": "a.txt"});
        let _ = g.observe("Read", &args, false, "x");
        let _ = g.observe("Read", &args, false, "x");
        assert!(g.take_nudge().is_some());
        assert!(g.take_nudge().is_none(), "软提醒只注入一次");
    }

    #[test]
    fn disabled_guard_never_intervenes() {
        let mut g = LoopGuard::with_enabled(false);
        let args = json!({"file_path": "a.txt"});
        for _ in 0..8 {
            assert_eq!(g.observe("Read", &args, false, "x"), LoopVerdict::Progress);
        }
        // 关闭后仍统计,可观测性不丢
        assert_eq!(g.max_repeats(), 8);
        assert!(g.take_nudge().is_none());
    }

    #[test]
    fn wait_like_calls_are_transparent() {
        // 等待类调用既不计入也不打断重复链。
        let mut g = guard();
        let args = json!({"file_path": "a.txt"});
        let wait = json!({"action": "control", "params": {"control_action": "wait"}});
        assert_eq!(g.observe("Read", &args, false, "x"), LoopVerdict::Progress);
        assert_eq!(g.observe("MCP_Web_Use", &wait, false, "waited"), LoopVerdict::Progress);
        assert_eq!(g.observe("MCP_Web_Use", &wait, false, "waited"), LoopVerdict::Progress);
        match g.observe("Read", &args, false, "x") {
            LoopVerdict::Nudge(_) => {}
            other => panic!("wait 应保持重复链不断,实得 {other:?}"),
        }
    }

    #[test]
    fn wait_like_matrix() {
        assert!(wait_like(
            "MCP_Web_Use",
            &json!({"action": "control", "params": {"control_action": "wait"}})
        ));
        assert!(wait_like("MCP_Window_Use", &json!({"action": "wait"})));
        assert!(wait_like("Bash", &json!({"command": "  sleep 5 "})));
        assert!(wait_like("Bash", &json!({"command": "sleep 0.5"})));
        assert!(wait_like("SubAgent", &json!({"action": "result", "run_id": "r1"})));
        assert!(wait_like("SubAgent", &json!({"action": "history"})));
        // 反例
        assert!(!wait_like("Bash", &json!({"command": "sleep 5 && ls"})));
        assert!(!wait_like("Bash", &json!({"command": "ls"})));
        assert!(!wait_like("MCP_Web_Use", &json!({"action": "inspect"})));
        assert!(!wait_like("SubAgent", &json!({"action": "launch"})));
        assert!(!wait_like("Read", &json!({"file_path": "a"})));
        assert!(!wait_like("Write", &json!({"file_path": "a"})));
    }

    #[test]
    fn react_guard_switch_parsing() {
        for off in ["off", "OFF", "0", "false", "False", "no", " no "] {
            assert!(!react_guard_enabled_from(off.to_string()), "{off} 应关闭");
        }
        for on in ["", "on", "1", "true", "yes", "whatever"] {
            assert!(react_guard_enabled_from(on.to_string()), "{on} 应开启");
        }
    }

    #[test]
    fn thresholds_are_ordered_and_tight() {
        // laew 单元预算 16 轮,阈值必须显著紧于外部工程(AtomCode 3/6、DeepSeek 3/5/8)。
        assert!(NUDGE_AT >= 2, "第 1 次就提醒会误伤正常的二次确认");
        assert!(ABORT_AT > NUDGE_AT);
        assert!(ABORT_AT <= 4, "预算只有 16 轮,止损不能晚于第 4 次重复");
    }

    #[test]
    fn abort_fallback_text_carries_evidence() {
        let t = abort_fallback_text("Read", 4, "  [ 1] 成功 Read a.txt");
        assert!(t.contains("doom_loop"));
        assert!(t.contains("Read"));
        assert!(t.contains("4"));
        assert!(t.contains("a.txt"));
    }
}
