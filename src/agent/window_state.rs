//! WindowUse Agent 的窗口会话状态管理。
//!
//! 提供跨用户轮次的窗口上下文持久化:
//! - 最后操作窗口 / 操作历史 / 已知窗口列表 / 用户别名 / 待处理数据
//! - 持久化到 SQLite `window_state` 表(每 session_id 一条,覆盖更新)
//! - 下一轮 WindowUse 单元启动时加载并注入 prompt,实现「无需重复描述窗口」的连续性
//!
//! 设计见 `docs/WindowUse多轮对话与Agent间通信增强设计/01-设计与解决方案.md` §3.1-3.2。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::session::now_readable;

/// 窗口操作历史单条记录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowActionRecord {
    pub timestamp: String,
    pub window_id: String,
    pub window_title: String,
    pub path: String,
    pub action: String,
    /// 操作结果摘要(≤100 字符)。
    pub result_summary: String,
}

/// 已知窗口快照(避免每轮重新枚举)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowSnapshot {
    pub window_id: String,
    pub title: String,
    pub process_name: String,
    pub pid: u32,
    /// 上次检视的根路径(如有)。
    pub last_inspect_path: Option<String>,
    /// 已发现的控件列表 (path, role)。
    pub known_controls: Vec<(String, String)>,
}

impl WindowSnapshot {
    /// 从窗口信息字段快速构造(known_controls 为空)。
    pub fn simple(window_id: &str, title: &str, process_name: &str, pid: u32) -> Self {
        Self {
            window_id: window_id.to_string(),
            title: title.to_string(),
            process_name: process_name.to_string(),
            pid,
            last_inspect_path: None,
            known_controls: Vec::new(),
        }
    }
}

/// 跨应用操作的中间数据(从 A 读到的文本,待写入 B)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingData {
    pub source_window_id: String,
    pub source_path: String,
    pub content: String,
    pub target_window_id: Option<String>,
    pub created_at: String,
}

/// 窗口会话状态 —— 核心结构,每 session_id 一条,跨轮持久化。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WindowSessionState {
    pub session_id: String,
    /// 最后操作的窗口(用户大概率继续操作它)。
    pub last_window: Option<WindowSnapshot>,
    /// 最近操作历史(保留最近 MAX_HISTORY 条,防止膨胀)。
    pub action_history: Vec<WindowActionRecord>,
    /// 已知窗口列表(最近一次 WindowList 结果缓存)。
    pub known_windows: Vec<WindowSnapshot>,
    /// 用户自定义别名 → window_id。
    pub window_aliases: HashMap<String, String>,
    /// 待处理数据(从某窗口读取,待写入另一窗口)。
    pub pending_data: Option<PendingData>,
    /// 状态版本(每次更新 +1)。
    pub version: u64,
    pub updated_at: String,
}

/// 操作历史最大保留条数。
const MAX_HISTORY: usize = 20;
/// 已知窗口列表最大保留条数。
const MAX_KNOWN_WINDOWS: usize = 30;
/// 单条 result_summary 最大字符数。
const MAX_RESULT_SUMMARY: usize = 100;

impl WindowSessionState {
    /// 构造一个空的窗口状态。
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            last_window: None,
            action_history: Vec::new(),
            known_windows: Vec::new(),
            window_aliases: HashMap::new(),
            pending_data: None,
            version: 0,
            updated_at: now_readable(),
        }
    }

    /// 记录一次成功的窗口操作,追加到历史并更新 last_window。
    pub fn record_action(
        &mut self,
        window_id: &str,
        window_title: &str,
        process_name: &str,
        pid: u32,
        path: &str,
        action: &str,
        result_summary: &str,
    ) {
        let summary = if result_summary.chars().count() > MAX_RESULT_SUMMARY {
            result_summary
                .chars()
                .take(MAX_RESULT_SUMMARY.saturating_sub(3))
                .chain(['.', '.', '.'])
                .collect::<String>()
        } else {
            result_summary.to_string()
        };

        let record = WindowActionRecord {
            timestamp: now_readable(),
            window_id: window_id.to_string(),
            window_title: window_title.to_string(),
            path: path.to_string(),
            action: action.to_string(),
            result_summary: summary,
        };
        self.action_history.push(record);
        // 裁剪超出上限的旧记录
        if self.action_history.len() > MAX_HISTORY {
            let excess = self.action_history.len() - MAX_HISTORY;
            self.action_history.drain(0..excess);
        }

        // 更新 last_window
        self.last_window = Some(WindowSnapshot::simple(
            window_id,
            window_title,
            process_name,
            pid,
        ));

        self.updated_at = now_readable();
        self.version += 1;
    }

    /// 更新已知窗口列表(合并:已存在的更新,不存在的追加)。
    pub fn update_known_windows(&mut self, windows: Vec<WindowSnapshot>) {
        for w in windows {
            if let Some(existing) = self
                .known_windows
                .iter_mut()
                .find(|x| x.window_id == w.window_id)
            {
                *existing = w;
            } else {
                self.known_windows.push(w);
            }
        }
        // 裁剪
        if self.known_windows.len() > MAX_KNOWN_WINDOWS {
            let excess = self.known_windows.len() - MAX_KNOWN_WINDOWS;
            self.known_windows.drain(0..excess);
        }
        self.updated_at = now_readable();
    }

    /// 设置窗口别名。
    pub fn set_alias(&mut self, alias: &str, window_id: &str) {
        self.window_aliases
            .insert(alias.to_string(), window_id.to_string());
        self.updated_at = now_readable();
        self.version += 1;
    }

    /// 通过别名查找 window_id。
    pub fn resolve_alias(&self, alias: &str) -> Option<&str> {
        self.window_aliases.get(alias).map(|s| s.as_str())
    }

    /// 设置待处理数据。
    pub fn set_pending_data(&mut self, data: PendingData) {
        self.pending_data = Some(data);
        self.updated_at = now_readable();
        self.version += 1;
    }

    /// 消费(取出并清除)待处理数据。
    pub fn take_pending_data(&mut self) -> Option<PendingData> {
        self.pending_data.take()
    }

    /// 清空待处理数据。
    pub fn clear_pending_data(&mut self) {
        self.pending_data = None;
        self.updated_at = now_readable();
    }

    /// 状态是否为空(无任何有用信息)。
    pub fn is_empty(&self) -> bool {
        self.last_window.is_none()
            && self.action_history.is_empty()
            && self.known_windows.is_empty()
            && self.window_aliases.is_empty()
            && self.pending_data.is_none()
    }

    /// 序列化为 JSON 字符串。
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// 从 JSON 字符串反序列化。
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

/// 窗口状态管理器 —— 封装 DB 访问,供 WindowUseRunner 调用。
#[derive(Clone)]
pub struct WindowStateManager {
    db: Arc<Db>,
}

impl WindowStateManager {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// 加载指定 session 的窗口状态(不存在返回 None)。
    pub async fn load(&self, session_id: &str) -> Option<WindowSessionState> {
        self.db.load_window_state(session_id).ok().flatten()
    }

    /// 保存窗口状态(覆盖写入)。
    pub async fn save(&self, session_id: &str, state: &WindowSessionState) -> Result<()> {
        self.db
            .save_window_state(session_id, state)
            .map_err(AgentError::from)
    }

    /// 删除指定 session 的窗口状态(Session 结束时可选调用)。
    pub async fn delete(&self, session_id: &str) -> Result<()> {
        self.db
            .delete_window_state(session_id)
            .map_err(AgentError::from)
    }
}

/// 构造注入到 prompt 的窗口状态摘要文本。
///
/// 返回空串表示无可用状态(首次使用或状态为空)。
pub fn build_window_state_prompt(state: &WindowSessionState) -> String {
    if state.is_empty() {
        return String::new();
    }

    let mut out = String::new();

    // 1. 最后操作窗口
    if let Some(w) = &state.last_window {
        out.push_str(&format!(
            "上次操作窗口: {} (id={}, 进程={}, PID={})\n",
            w.title, w.window_id, w.process_name, w.pid
        ));
        if let Some(p) = &w.last_inspect_path {
            out.push_str(&format!("上次检视路径根: {p}\n"));
        }
    }

    // 2026-09-16 第 61 轮:已知窗口是连续操控的核心状态。
    if !state.known_windows.is_empty() {
        out.push_str("已知窗口(最近在前,window_id 可能随窗口重开失效,操作失败时必须重新枚举):\n");
        for w in state.known_windows.iter().rev().take(10) {
            out.push_str(&format!(
                "  - id={} title={} process={} pid={}\n",
                w.window_id, w.title, w.process_name, w.pid
            ));
        }
    }

    // 2. 窗口别名
    if !state.window_aliases.is_empty() {
        out.push_str("窗口别名:\n");
        for (alias, wid) in &state.window_aliases {
            out.push_str(&format!("  - \"{alias}\" → id={wid}\n"));
        }
    }

    // 3. 最近操作历史(新→旧,最多 5 条)
    if !state.action_history.is_empty() {
        out.push_str("最近操作历史(新→旧):\n");
        let total = state.action_history.len();
        for (i, rec) in state.action_history.iter().rev().take(5).enumerate() {
            let seq = total - i;
            let ts_short = rec.timestamp.chars().skip(11).take(8).collect::<String>();
            out.push_str(&format!(
                "  {}. [{}] \"{}\" {} → path={} → {}\n",
                seq, ts_short, rec.window_title, rec.action, rec.path, rec.result_summary
            ));
        }
    }

    // 4. 待处理数据
    if let Some(pending) = &state.pending_data {
        out.push_str(&format!(
            "待处理数据: 已从窗口 {}({}) 读取 {} 字符内容,待写入目标{}\n",
            pending.source_window_id,
            pending.source_path,
            pending.content.len(),
            pending
                .target_window_id
                .as_ref()
                .map(|t| format!(",目标={t}"))
                .unwrap_or_default(),
        ));
    }

    out
}

/// 窗口状态注入标记(幂等探测锚点)。
pub const WINDOW_STATE_MARKER_START: &str = "<<<LAEW:WINDOW_STATE>>>";
pub const WINDOW_STATE_MARKER_END: &str = "<<<LAEW:WINDOW_STATE_END>>>";

/// 探测上下文中是否已注入窗口状态(幂等)。
pub fn is_window_state_injected(context: &[crate::llm::ChatMessage]) -> bool {
    context.iter().any(|m| {
        m.content.iter().any(|b| match b {
            crate::llm::ContentBlock::Text { text } => text.contains(WINDOW_STATE_MARKER_START),
            _ => false,
        })
    })
}

/// 构造带标记的窗口状态注入消息。
pub fn build_window_state_message(state: &WindowSessionState) -> Option<crate::llm::ChatMessage> {
    let body = build_window_state_prompt(state);
    if body.is_empty() {
        return None;
    }
    let mut text = format!("{WINDOW_STATE_MARKER_START}\n[窗口会话状态,系统注入,非用户输入]\n");
    text.push_str(&format!(
        "以下是上一轮窗口操作的会话状态,供本轮继续操作参考;\n\
         不要把它本身当作用户请求,用户本轮请求以本消息之后的用户消息为准。\n\
         --- 状态开始 ---\n{body}--- 状态结束 ---\n{WINDOW_STATE_MARKER_END}"
    ));
    Some(crate::llm::ChatMessage::user(text))
}

/// 从 ExecutionTrace 的工具调用中提取窗口状态变更(2026-09-16 第 56 轮)。
///
/// 实现要点:
/// - ExecutionTrace.tool_call_log 含每次工具调用的 (tool, args_json, ok, output_bytes),
///   从中识别 WindowList / WindowFind / WindowAction 三类调用,反序列化 args_json
///   拿到 window_id/path/action,更新到 WindowSessionState;
/// - WindowScreenshot 与窗口状态无关(只产 PNG 文件),跳过;
/// - 失败调用 (ok=false) 不写入状态(避免污染历史)。
/// - 当前 Runner 已直接在工具回调中维护 win_state;本函数作为「事后从 trace 恢复」
///   的兜底,主要用于:进程崩溃后从落库 trace 重建、或第三方工具触发。
pub fn extract_window_state_from_trace(
    state: &mut WindowSessionState,
    trace: &crate::agent::extrace::ExecutionTrace,
) {
    let mut latest_window: Option<WindowSnapshot> = None;

    for entry in &trace.tool_call_log {
        if !entry.ok {
            continue;
        }
        match entry.tool.as_str() {
            "WindowList" | "WindowFind" | "WindowOpen" => {
                let Some(found) = parse_window_snapshots(&entry.output_summary) else {
                    continue;
                };
                if found.is_empty() {
                    continue;
                }
                state.update_known_windows(found.clone());
                latest_window = found.into_iter().last();

                // WindowFind / WindowOpen 的 query 沉淀为别名,下一轮直接复用。
                if entry.tool != "WindowList" {
                    if let Ok(args) = serde_json::from_str::<serde_json::Value>(&entry.args_json) {
                        if let Some(query) = args.get("query").and_then(serde_json::Value::as_str) {
                            if let Some(w) = &latest_window {
                                state.set_alias(query, &w.window_id);
                            }
                        }
                    }
                }
            }
            "WindowAction" => {
                let Ok(args) = serde_json::from_str::<serde_json::Value>(&entry.args_json) else {
                    continue;
                };
                let Some(window_id) = args.get("window_id").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let path = args
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("/")
                    .to_string();
                let action = args
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let text = args
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let result_summary = if text.is_empty() {
                    format!("{action} {path}")
                } else {
                    format!("{action} {path} text={text}")
                };
                let known_meta = state
                    .known_windows
                    .iter()
                    .find(|w| w.window_id == window_id)
                    .cloned()
                    .unwrap_or_else(|| WindowSnapshot {
                        window_id: window_id.to_string(),
                        title: String::new(),
                        process_name: String::new(),
                        pid: 0,
                        last_inspect_path: None,
                        known_controls: Vec::new(),
                    });
                state.update_known_windows(vec![known_meta.clone()]);
                latest_window = Some(known_meta);
                state.action_history.push(WindowActionRecord {
                    timestamp: now_readable(),
                    window_id: window_id.to_string(),
                    window_title: String::new(),
                    path,
                    action,
                    result_summary,
                });
                if state.action_history.len() > MAX_HISTORY {
                    let excess = state.action_history.len() - MAX_HISTORY;
                    state.action_history.drain(0..excess);
                }
            }
            _ => {}
        }
    }
    if let Some(w) = latest_window {
        state.last_window = Some(w);
    }
    state.updated_at = now_readable();
}

/// 从 WindowList / WindowFind / WindowOpen 的成功输出摘要恢复窗口快照。
fn parse_window_snapshots(output: &str) -> Option<Vec<WindowSnapshot>> {
    let value: serde_json::Value = serde_json::from_str(output.trim()).ok()?;
    let arr = if let serde_json::Value::Array(items) = &value {
        items.clone()
    } else if let serde_json::Value::Object(obj) = &value {
        if let Some(items) = obj.get("windows").and_then(|v| v.as_array()) {
            items.clone()
        } else if obj.contains_key("window_id") {
            vec![value.clone()]
        } else {
            return None;
        }
    } else {
        return None;
    };
    let mut out = Vec::new();
    for item in arr {
        let Some(window_id) = item
            .get("window_id")
            .or_else(|| item.get("id"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        out.push(WindowSnapshot::simple(
            window_id,
            item.get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            item.get("process_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            item.get("pid").and_then(|v| v.as_u64()).unwrap_or_default() as u32,
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_action_trims_long_summary() {
        let mut s = WindowSessionState::new("s1");
        let long = "x".repeat(200);
        s.record_action("w1", "title", "proc", 123, "/0/1", "click", &long);
        let rec = &s.action_history[0];
        assert_eq!(rec.result_summary.chars().count(), MAX_RESULT_SUMMARY);
        assert!(rec.result_summary.ends_with("..."));
    }

    #[test]
    fn record_action_caps_history() {
        let mut s = WindowSessionState::new("s1");
        for i in 0..(MAX_HISTORY + 5) {
            s.record_action(&format!("w{i}"), "t", "p", i as u32, "/0", "click", "ok");
        }
        assert_eq!(s.action_history.len(), MAX_HISTORY);
        // 最新的应在末尾
        assert_eq!(s.action_history.last().unwrap().window_id, "w24");
    }

    #[test]
    fn is_empty_default() {
        let s = WindowSessionState::new("s1");
        assert!(s.is_empty());
    }

    #[test]
    fn is_empty_after_action() {
        let mut s = WindowSessionState::new("s1");
        s.record_action("w", "t", "p", 1, "/0", "click", "ok");
        assert!(!s.is_empty());
    }

    #[test]
    fn alias_set_and_resolve() {
        let mut s = WindowSessionState::new("s1");
        s.set_alias("我的记事本", "w-123");
        assert_eq!(s.resolve_alias("我的记事本"), Some("w-123"));
        assert_eq!(s.resolve_alias("不存在的"), None);
    }

    #[test]
    fn pending_data_take() {
        let mut s = WindowSessionState::new("s1");
        assert!(s.take_pending_data().is_none());
        s.set_pending_data(PendingData {
            source_window_id: "a".into(),
            source_path: "/0".into(),
            content: "hello".into(),
            target_window_id: None,
            created_at: now_readable(),
        });
        assert!(s.take_pending_data().is_some());
        assert!(s.pending_data.is_none());
    }

    #[test]
    fn build_prompt_empty_when_no_state() {
        let s = WindowSessionState::new("s1");
        assert!(build_window_state_prompt(&s).is_empty());
    }

    #[test]
    fn build_prompt_includes_last_window_and_history() {
        let mut s = WindowSessionState::new("s1");
        s.record_action(
            "w1",
            "记事本",
            "notepad.exe",
            1234,
            "/0/1",
            "set_text",
            "输入hello",
        );
        s.update_known_windows(vec![WindowSnapshot::simple(
            "w1",
            "记事本",
            "notepad.exe",
            1234,
        )]);
        s.set_alias("我的记事本", "w1");
        let prompt = build_window_state_prompt(&s);
        assert!(prompt.contains("记事本"));
        assert!(prompt.contains("set_text"));
        assert!(prompt.contains("我的记事本"));
        assert!(prompt.contains("w1"));
        assert!(prompt.contains("已知窗口"));
        assert!(prompt.contains("输入hello"));
    }

    #[test]
    fn json_roundtrip() {
        let mut s = WindowSessionState::new("s1");
        s.record_action("w1", "t", "p", 1, "/0", "click", "ok");
        s.set_alias("a", "w1");
        let json = s.to_json().unwrap();
        let restored = WindowSessionState::from_json(&json).unwrap();
        assert_eq!(s, restored);
    }

    #[test]
    fn marker_detection() {
        use crate::llm::{ChatMessage, ContentBlock};
        let ctx = vec![ChatMessage::user(&format!(
            "{WINDOW_STATE_MARKER_START} state {WINDOW_STATE_MARKER_END}"
        ))];
        assert!(is_window_state_injected(&ctx));

        let ctx_clean = vec![ChatMessage::user("hello")];
        assert!(!is_window_state_injected(&ctx_clean));
    }

    #[test]
    fn build_message_none_when_empty() {
        let s = WindowSessionState::new("s1");
        assert!(build_window_state_message(&s).is_none());
    }

    #[test]
    fn build_message_has_markers() {
        let mut s = WindowSessionState::new("s1");
        s.record_action("w1", "记事本", "notepad.exe", 1234, "/0", "click", "ok");
        let msg = build_window_state_message(&s).unwrap();
        let text = match &msg.content[0] {
            crate::llm::ContentBlock::Text { text } => text.clone(),
            _ => panic!("应为文本块"),
        };
        assert!(text.contains(WINDOW_STATE_MARKER_START));
        assert!(text.contains(WINDOW_STATE_MARKER_END));
        assert!(text.contains("记事本"));
    }

    // ===== 2026-09-16 第 56 轮:extract_window_state_from_trace 测试 =====

    #[test]
    fn extract_window_state_from_empty_trace() {
        let mut state = WindowSessionState::new("s1");
        let trace = crate::agent::extrace::ExecutionTrace::default();
        extract_window_state_from_trace(&mut state, &trace);
        assert!(state.action_history.is_empty());
        assert!(state.known_windows.is_empty());
    }

    #[test]
    fn extract_window_state_from_window_action_log() {
        let mut state = WindowSessionState::new("s1");
        let mut trace = crate::agent::extrace::ExecutionTrace::default();
        // 模拟一次 WindowAction 成功调用 + 一次失败
        let args_ok = serde_json::json!({
            "window_id": "w-123",
            "path": "/0/2",
            "action": "click",
            "text": ""
        })
        .to_string();
        trace.record_tool_call("WindowAction", &args_ok, true, 256, 100, "", "");

        let args_fail = serde_json::json!({
            "window_id": "w-456",
            "path": "/",
            "action": "click"
        })
        .to_string();
        trace.record_tool_call(
            "WindowAction",
            &args_fail,
            false,
            128,
            50,
            "PathInvalid: / 越界",
            "",
        );

        extract_window_state_from_trace(&mut state, &trace);
        // 仅成功调用写入 action_history
        assert_eq!(state.action_history.len(), 1);
        assert_eq!(state.action_history[0].window_id, "w-123");
        assert_eq!(state.action_history[0].action, "click");
    }

    #[test]
    fn extract_window_state_records_list_known_windows() {
        let mut state = WindowSessionState::new("s1");
        let mut trace = crate::agent::extrace::ExecutionTrace::default();
        // WindowList 调用 + WindowAction 调用 → 应同时填充 known_windows
        trace.record_tool_call("WindowList", "{}", true, 1024, 50, "", "[]");
        let args = serde_json::json!({
            "window_id": "w-789",
            "path": "/0",
            "action": "focus"
        })
        .to_string();
        trace.record_tool_call("WindowAction", &args, true, 64, 100, "", "");

        extract_window_state_from_trace(&mut state, &trace);
        assert_eq!(state.known_windows.len(), 1);
        assert_eq!(state.known_windows[0].window_id, "w-789");
        assert_eq!(state.action_history.len(), 1);
    }

    #[test]
    fn extract_window_state_recovers_window_find_output_and_alias() {
        let mut state = WindowSessionState::new("s1");
        let mut trace = crate::agent::extrace::ExecutionTrace::default();
        let args = r#"{"query":"WeChat","match_mode":"contains"}"#;
        let output = r#"{"window_id":"1551:0","title":"微信","process_name":"微信","pid":1551}"#;
        trace.record_tool_call("WindowFind", args, true, output.len(), 12, "", output);

        extract_window_state_from_trace(&mut state, &trace);
        assert_eq!(state.last_window.as_ref().unwrap().window_id, "1551:0");
        assert_eq!(state.known_windows[0].process_name, "微信");
        assert_eq!(state.resolve_alias("WeChat"), Some("1551:0"));
    }

    #[test]
    fn record_tool_call_respects_max_log_len() {
        use crate::agent::extrace::MAX_TOOL_CALL_LOG;
        let mut trace = crate::agent::extrace::ExecutionTrace::default();
        // 写入 MAX_TOOL_CALL_LOG + 5 条,验证 FIFO 截断
        for i in 0..(MAX_TOOL_CALL_LOG + 5) {
            trace.record_tool_call("Test", &format!("{{\"i\":{i}}}"), true, 0, 0, "", "");
        }
        assert_eq!(trace.tool_call_log.len(), MAX_TOOL_CALL_LOG);
        // 最早的 5 条应被截断,留下的应是 i=5..MAX
        let first = &trace.tool_call_log[0];
        assert!(first.args_json.contains("\"i\":5"));
        let last = trace.tool_call_log.last().unwrap();
        assert!(last
            .args_json
            .contains(&format!("\"i\":{}", MAX_TOOL_CALL_LOG + 4)));
    }
}
