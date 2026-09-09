//! Anthropic Prompt Caching 自动注入(第十六轮 L1047)。
//!
//! 在每个 LLM 请求中按既定策略(默认 "auto":last tool + last system +
//! latest user message)打上 `cache_control` 断点,让 Anthropic 服务端缓存
//! 复用 system / tools / 历史消息前缀,典型多轮 Agent 循环可省 60-80%
//! 输入侧 token 计费(单次复用 5 分钟即回本:write 1.25x / read 0.1x)。
//!
//! 知识库出处:第十六轮 opencode `cache-policy.ts`(L1047)+ §2.2「断点
//! 放置算法」/「4 断点 cap」/「RESPECTS_INLINE_HINTS 白名单」。

use serde_json::{json, Value};

/// Anthropic `cache_control` TTL(对应 `{"type":"ephemeral","ttl":"5m"|"1h"}`)。
///
/// 仅暴露 `FiveMinutes`(默认)/`OneHour` 两档,与 opencode `CacheHint` 形态对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTtl {
    /// 5 分钟缓存(默认)。
    FiveMinutes,
    /// 1 小时缓存(开销更高,但复用窗口更大)。
    OneHour,
}

impl CacheTtl {
    /// 渲染为 Anthropic wire 字段值。`None` 表示走默认 5m(不写 `ttl` 子键)。
    pub fn to_wire(self) -> Option<&'static str> {
        match self {
            CacheTtl::FiveMinutes => None,
            CacheTtl::OneHour => Some("1h"),
        }
    }
}

/// 一个 cache_control 标记 hint(本期未开放给用户配置;留作扩展点)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheHint {
    pub ttl: Option<CacheTtl>,
}

impl CacheHint {
    pub const fn ephemeral_5m() -> Self { Self { ttl: None } }
    pub const fn ephemeral_1h() -> Self { Self { ttl: Some(CacheTtl::OneHour) } }
}

/// Cache Policy 策略(对齐 opencode `CachePolicy`)。
///
/// - `Auto`(默认):打 3 个断点 — last tool / last system / latest user message。
///   Anthropic 5m-cache write 1.25x、read 0.1x,单次复用即回本,推荐默认开启。
/// - `None`:完全不打(用户主动关闭或非 Anthropic 协议路径)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CachePolicy {
    #[default]
    Auto,
    None,
}

impl CachePolicy {
    pub fn is_enabled(self) -> bool {
        matches!(self, CachePolicy::Auto)
    }
}

/// 4 断点计数器(Anthropic API 硬上限)。
///
/// 命中一处扣 1,`remaining <= 0` 时 `try_consume` 返回 `false`,调用方
/// 应跳过打标并 `dropped += 1`,不抛错(对齐 opencode `anthropic-messages.ts:243`
/// 「超过 4 断点不抛错,只记 warning + 静默丢多余」)。
#[derive(Debug, Clone, Default)]
pub struct CacheBreakpoints {
    pub remaining: usize,
    pub dropped: usize,
}

impl CacheBreakpoints {
    pub fn new(cap: usize) -> Self {
        Self { remaining: cap, dropped: 0 }
    }

    /// 尝试占一个断点配额。返回 `true` 表示配额可用,`false` 表示已耗尽。
    pub fn try_consume(&mut self) -> bool {
        if self.remaining == 0 {
            self.dropped += 1;
            false
        } else {
            self.remaining -= 1;
            true
        }
    }
}

/// Anthropic 4 断点硬上限(opencode `ANTHROPIC_BREAKPOINT_CAP`)。
pub const ANTHROPIC_BREAKPOINT_CAP: usize = 4;

/// 把 `CacheHint` 渲染成 Anthropic wire `cache_control` JSON。
fn cache_control_value(hint: CacheHint) -> Value {
    let mut v = json!({ "type": "ephemeral" });
    if let Some(ttl) = hint.ttl.and_then(|t| t.to_wire()) {
        v["ttl"] = json!(ttl);
    }
    v
}

/// 默认 hint(5m)。`apply_cache_policy` 唯一使用的 hint 形态;
/// 保留参数入口以便将来支持 `CacheHint` 配置。
fn default_hint() -> CacheHint {
    CacheHint::ephemeral_5m()
}

/// 检查 object 是否已带 `cache_control`(无论 type / ttl)。
fn has_cache_control(obj: &Value) -> bool {
    obj.get("cache_control").map(|v| !v.is_null()).unwrap_or(false)
}

/// 在最后一个 tool 上打 `cache_control`。
fn mark_last_tool(tools: Vec<Value>, hint: CacheHint, bp: &mut CacheBreakpoints) -> Vec<Value> {
    if tools.is_empty() {
        return tools;
    }
    if !bp.try_consume() {
        return tools;
    }
    let last = tools.len() - 1;
    let mut next = tools;
    let last_val = &next[last];
    if !last_val.is_object() {
        return next;
    }
    if has_cache_control(last_val) {
        bp.remaining += 1;
        return next;
    }
    if let Some(obj) = next[last].as_object_mut() {
        obj.insert("cache_control".to_string(), cache_control_value(hint));
    }
    next
}

/// 在最后一个 system 文本块上打 `cache_control`。
fn mark_last_system(system: Vec<Value>, hint: CacheHint, bp: &mut CacheBreakpoints) -> Vec<Value> {
    if system.is_empty() {
        return system;
    }
    if !bp.try_consume() {
        return system;
    }
    let last = system.len() - 1;
    let mut next = system;
    if !next[last].is_object() {
        return next;
    }
    if has_cache_control(&next[last]) {
        bp.remaining += 1;
        return next;
    }
    if let Some(obj) = next[last].as_object_mut() {
        obj.insert("cache_control".to_string(), cache_control_value(hint));
    }
    next
}

/// 在指定 index 的 message 的最后一个文本块上打 `cache_control`。
fn mark_message_at(messages: Vec<Value>, index: usize, hint: CacheHint, bp: &mut CacheBreakpoints) -> Vec<Value> {
    if index >= messages.len() {
        return messages;
    }
    if !bp.try_consume() {
        return messages;
    }
    let mut next = messages;
    let Some(content_arr) = next.get(index).and_then(|m| m.get("content")).and_then(|c| c.as_array()).cloned() else {
        bp.remaining += 1;
        return next;
    };
    if content_arr.is_empty() {
        bp.remaining += 1;
        return next;
    }
    let last_text_idx = content_arr.iter().rposition(|p| {
        p.get("type").and_then(|t| t.as_str()) == Some("text")
    });
    let mark_at = last_text_idx.unwrap_or_else(|| content_arr.len() - 1);

    let already_marked = content_arr.get(mark_at).map(has_cache_control).unwrap_or(false);
    if already_marked {
        bp.remaining += 1;
        return next;
    }

    let Some(msg) = next.get_mut(index) else { return next };
    let Some(content_mut) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
        return next;
    };
    if let Some(part) = content_mut.get_mut(mark_at) {
        if let Some(obj) = part.as_object_mut() {
            obj.insert("cache_control".to_string(), cache_control_value(hint));
        }
    }
    next
}

/// 找最后一条指定 role 的 message index。`None` 表示未找到。
fn last_index_of_role(messages: &[Value], role: &str) -> Option<usize> {
    messages.iter().rposition(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some(role)
    })
}

/// 把 `CachePolicy` 应用到 (system, tools, messages) 三元组上,返回新三元组与断点统计。
///
/// - `Auto`:依次尝试打 last tool / last system / latest user message(顺序按 opencode)
/// - `None`:原样返回
/// - **幂等**:目标位置已有 `cache_control` 时跳过(用户手注优先)
/// - **cap 防御**:断点超过 `ANTHROPIC_BREAKPOINT_CAP` 时静默丢多余,记到
///   `breakpoints.dropped`(不抛错,允许上游策略更激进)
pub fn apply_cache_policy(
    policy: CachePolicy,
    mut system: Vec<Value>,
    mut tools: Vec<Value>,
    mut messages: Vec<Value>,
) -> (Vec<Value>, Vec<Value>, Vec<Value>, CacheBreakpoints) {
    if matches!(policy, CachePolicy::None) {
        return (system, tools, messages, CacheBreakpoints::new(ANTHROPIC_BREAKPOINT_CAP));
    }
    let mut bp = CacheBreakpoints::new(ANTHROPIC_BREAKPOINT_CAP);
    let hint = default_hint();

    tools = mark_last_tool(tools, hint, &mut bp);
    system = mark_last_system(system, hint, &mut bp);
    if let Some(idx) = last_index_of_role(&messages, "user") {
        messages = mark_message_at(messages, idx, hint, &mut bp);
    }

    (system, tools, messages, bp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v_system(text: &str) -> Value { json!({ "type": "text", "text": text }) }
    fn v_tool(name: &str) -> Value { json!({ "name": name, "description": "d", "input_schema": {} }) }
    fn v_user_msg(text: &str) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": text }] })
    }
    fn v_assistant_msg(text: &str) -> Value {
        json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] })
    }

    #[test]
    fn cache_ttl_wire_matches_spec() {
        assert_eq!(CacheTtl::FiveMinutes.to_wire(), None);
        assert_eq!(CacheTtl::OneHour.to_wire(), Some("1h"));
    }

    #[test]
    fn apply_cache_policy_auto_marks_last_tool() {
        let tools = vec![v_tool("Read"), v_tool("Write"), v_tool("Bash")];
        let sys = vec![v_system("you are x")];
        let msgs = vec![v_user_msg("hi")];
        let (_, out_tools, _, bp) = apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        assert!(out_tools[2].get("cache_control").is_some(), "last tool should be marked");
        assert_eq!(bp.dropped, 0);
        // 1 tool + 1 system + 1 user = 3 断点 → remaining = 4 - 3 = 1
        assert_eq!(bp.remaining, ANTHROPIC_BREAKPOINT_CAP - 3);
    }

    #[test]
    fn apply_cache_policy_auto_marks_last_system_block() {
        let tools = vec![v_tool("Read")];
        let sys = vec![v_system("alpha"), v_system("beta")];
        let msgs = vec![v_user_msg("hi")];
        let (out_sys, _, _, _) = apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        assert!(out_sys[1].get("cache_control").is_some(), "last system block marked");
        assert!(out_sys[0].get("cache_control").is_none(), "first system block untouched");
    }

    #[test]
    fn apply_cache_policy_auto_marks_latest_user_message() {
        let tools = vec![v_tool("Read")];
        let sys = vec![v_system("s")];
        let msgs = vec![
            v_user_msg("first"),
            v_assistant_msg("ack"),
            v_user_msg("second"),
        ];
        let (_, _, out_msgs, _) = apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        let latest = &out_msgs[2];
        let content = latest.get("content").unwrap().as_array().unwrap();
        assert!(content[0].get("cache_control").is_some(), "latest user text block marked");
        let first = &out_msgs[0];
        let c0 = first.get("content").unwrap().as_array().unwrap();
        assert!(c0[0].get("cache_control").is_none());
    }

    #[test]
    fn apply_cache_policy_respects_existing_cache_control() {
        let tools = vec![v_tool("Read"), {
            let mut t = v_tool("Write");
            t.as_object_mut().unwrap().insert(
                "cache_control".to_string(),
                json!({ "type": "ephemeral", "ttl": "1h" }),
            );
            t
        }];
        let sys = vec![v_system("s")];
        let msgs = vec![v_user_msg("hi")];
        let (_, out_tools, _, bp) = apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        assert_eq!(
            out_tools[1].get("cache_control").unwrap().get("ttl").unwrap(),
            "1h"
        );
        assert!(out_tools[0].get("cache_control").is_none());
        assert_eq!(bp.dropped, 0);
    }

    #[test]
    fn apply_cache_policy_caps_at_four_breakpoints() {
        let tools: Vec<Value> = (0..5).map(|i| v_tool(&format!("T{i}"))).collect();
        let sys = vec![v_system("a"), v_system("b")];
        let msgs = vec![v_user_msg("hi")];
        let (_, _, _, bp) = apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        assert_eq!(bp.remaining, ANTHROPIC_BREAKPOINT_CAP - 3);
        assert_eq!(bp.dropped, 0);
    }

    #[test]
    fn apply_cache_policy_caps_drops_when_exhausted() {
        let mut bp = CacheBreakpoints::new(2);
        assert!(bp.try_consume());
        assert!(bp.try_consume());
        assert!(!bp.try_consume());
        assert_eq!(bp.dropped, 1);
        assert_eq!(bp.remaining, 0);
    }

    #[test]
    fn apply_cache_policy_none_is_noop() {
        let tools = vec![v_tool("Read"), v_tool("Write")];
        let sys = vec![v_system("s")];
        let msgs = vec![v_user_msg("hi")];
        let (out_sys, out_tools, out_msgs, bp) =
            apply_cache_policy(CachePolicy::None, sys.clone(), tools.clone(), msgs.clone());
        assert_eq!(out_sys, sys);
        assert_eq!(out_tools, tools);
        assert_eq!(out_msgs, msgs);
        assert_eq!(bp.remaining, ANTHROPIC_BREAKPOINT_CAP);
        assert_eq!(bp.dropped, 0);
    }

    #[test]
    fn apply_cache_policy_one_hour_ttl() {
        let tools = vec![v_tool("Read"), v_tool("Write")];
        let sys = vec![v_system("s")];
        let msgs = vec![v_user_msg("hi")];
        let hint = CacheHint::ephemeral_1h();
        let mut bp = CacheBreakpoints::new(ANTHROPIC_BREAKPOINT_CAP);
        let out = mark_last_tool(tools, hint, &mut bp);
        assert_eq!(
            out[1].get("cache_control").unwrap().get("ttl").unwrap(),
            "1h"
        );
    }

    #[test]
    fn apply_cache_policy_works_with_empty_tools_or_system() {
        let tools = vec![];
        let sys = vec![];
        let msgs = vec![v_user_msg("hi")];
        let (out_sys, out_tools, out_msgs, bp) =
            apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        assert!(out_sys.is_empty());
        assert!(out_tools.is_empty());
        assert!(out_msgs[0]
            .get("content")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("cache_control")
            .is_some());
        assert_eq!(bp.remaining, ANTHROPIC_BREAKPOINT_CAP - 1);
    }

    #[test]
    fn apply_cache_policy_picks_last_user_with_no_user_falls_back() {
        // 无 user 消息 → 不打 messages(配额只扣 tool + system)
        let tools = vec![v_tool("R")];
        let sys = vec![v_system("s")];
        let msgs = vec![v_assistant_msg("ack")];
        let (_, _, out_msgs, bp) = apply_cache_policy(CachePolicy::Auto, sys, tools, msgs);
        assert!(out_msgs[0]
            .get("content")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("cache_control")
            .is_none());
        assert_eq!(bp.remaining, ANTHROPIC_BREAKPOINT_CAP - 2);
    }
}
