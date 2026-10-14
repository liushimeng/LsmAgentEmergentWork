//! Agent 活动台账（第 171 轮）：把「哪个 Agent 正在干什么、干到第几步」以
//! 进程内可读的形式暴露给注入 UI 层，供 `MCP_Web_Use` 的**屏蔽页面状态栏 /
//! 屏蔽页面进度栏**渲染。
//!
//! 背景：`browser_status.rs` 的两条栏要在页面上回答人工三个问题 ——「谁在控制
//! 这个窗口」「正在干什么」「进度到哪」。此前页面侧只有一句静态的「LAEW Agent
//! 控制中」，并行多窗口（天猫一窗 / 京东一窗）时人工完全无法判断归属。
//!
//! 载体选型：`tokio::task_local`（与 `dynamic_subagent::scope`、
//! `mcp_web_use::IN_SEQUENCE` 同款模式）。工具执行发生在 Agent 自己的任务树内，
//! task_local 天然随 `.await` 传递 —— 并行 SubAgent 各自读到**自己**的活动快照，
//! 而不是全局「最后一个写入者」（用全局 `Mutex` 会被并发 Agent 互相覆盖，
//! 实测过一次 A 窗口显示 B 子 Agent 名字就足以让人工误判）。
//!
//! 埋点（见 `agent_loop.rs::run_session_inner` 与 `tool_exec.rs::exec_tool_call`）：
//! 会话建立 → 每轮迭代 → 每次工具调用 → 收口。工具层只读不算，纯开销可忽略。

use std::sync::{Arc, Mutex};

/// 活动快照字段上限（全部按字符裁剪，进度栏是给人看的，不是数据通道）。
const MAX_TASK_CHARS: usize = 120;
const MAX_TOOL_CHARS: usize = 160;

/// 注入块标记：Session 上下文首条 user 消息可能是项目上下文 / @ 附件 / 工作区
/// 快照等自动注入内容，取任务摘要时按此跳过（否则进度栏第一行显示的是工程说明）。
const INJECT_MARKERS: [&str; 4] = [
    "<<<LAEW:PROJECT_CONTEXT>>>",
    "<<<LAEW:ATTACHMENTS>>>",
    "<<<LAEW:WORKSPACE>>>",
    "<<<LAEW:SESSION_HISTORY>>>",
];

/// 活动状态（`Activity` 内部共享的那一份）。
#[derive(Debug, Clone, Default)]
pub struct ActivityState {
    /// 具体 Agent 名（如 `LsmAgentEmergentWork-SubAgent-Work`）。
    pub agent: String,
    /// 会话 ID（短前缀展示即可）。
    pub session: String,
    /// 当前迭代（1 起）与迭代预算。
    pub iter: usize,
    pub max_iter: usize,
    /// 任务摘要（用户原文，截断）。
    pub task: String,
    /// 当前阶段文案（如「第 3/12 轮 · 模型思考」）。
    pub phase: String,
    /// 当前工具调用标签（如 `MCP_Web_Use open https://…`）。
    pub tool: String,
    /// 会话开始时刻（毫秒时间戳，供页面算耗时）。
    pub started_ms: u128,
}

/// 活动句柄：`Clone` 廉价（共享同一份 `Mutex`），供循环各埋点处随手取用。
#[derive(Debug, Clone, Default)]
pub struct Activity {
    inner: Arc<Mutex<ActivityState>>,
}

tokio::task_local! {
    /// 当前任务的 Agent 活动台账（作用域外 = 非 Agent 工具调用路径，`current()` 返回 None）。
    static CURRENT: Activity;
}

/// 建立一个新的活动台账。
pub fn begin(agent: &str, session: &str, task: &str, max_iter: usize) -> Activity {
    Activity {
        inner: Arc::new(Mutex::new(ActivityState {
            agent: agent.to_string(),
            session: session.to_string(),
            iter: 0,
            max_iter,
            task: clip_chars(task, MAX_TASK_CHARS),
            phase: "启动中".to_string(),
            tool: String::new(),
            started_ms: now_ms(),
        })),
    }
}

/// 在活动台账作用域内执行 future（工具调用链因此可读到本 Agent 的快照）。
pub async fn scope<T>(act: Activity, fut: impl std::future::Future<Output = T>) -> T {
    CURRENT.scope(act, fut).await
}

/// 取当前任务的活动台账（`None` = 不在 Agent 工具作用域内）。
pub fn current() -> Option<Activity> {
    CURRENT.try_with(|a| a.clone()).ok()
}

/// 当前活动快照（无台账时返回 `None`；上层据此回退到「LAEW Agent 控制中」静态文案）。
pub fn current_snapshot() -> Option<ActivityState> {
    current().map(|a| a.snapshot())
}

impl Activity {
    /// 只读快照。
    pub fn snapshot(&self) -> ActivityState {
        self.inner.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// 逐字段更新（`None` 字段保持原值），埋点处按需传。
    pub fn update(&self, f: impl FnOnce(&mut ActivityState)) {
        if let Ok(mut s) = self.inner.lock() {
            f(&mut s);
        }
    }

    /// 迭代推进 + 阶段文案。
    pub fn set_iter(&self, iter: usize, phase: &str) {
        self.update(|s| {
            s.iter = iter;
            s.phase = phase.to_string();
        });
    }

    /// 工具调用标签（工具名 + 已裁剪的参数摘要）。
    pub fn set_tool(&self, label: &str) {
        let label = clip_chars(label, MAX_TOOL_CHARS);
        self.update(|s| s.tool = label);
    }
}

/// 毫秒时间戳（与页面 `Date.now()` 同一量纲）。
pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// 按字符裁剪并补省略号（按字符边界，不切碎多字节字符）。
pub fn clip_chars(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 从 Session 上下文取**任务摘要**：第一条非注入块 user 消息（跳过工程说明 /
/// @ 附件 / 工作区快照等自动注入内容）。
pub fn task_summary_from_context(messages: &[crate::llm::ChatMessage]) -> String {
    for m in messages {
        if !matches!(m.role, crate::llm::Role::User) {
            continue;
        }
        let text = m
            .content
            .iter()
            .filter_map(|b| match b {
                crate::llm::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.trim().is_empty() {
            continue;
        }
        if INJECT_MARKERS.iter().any(|mk| text.contains(mk)) {
            continue;
        }
        return clip_chars(&text, MAX_TASK_CHARS);
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    fn msg(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    #[test]
    fn 活动台账字段随埋点更新() {
        let a = begin("LsmAgentEmergentWork-SubAgent-Work", "sess-1234", "调研双站价格", 12);
        a.set_iter(3, "第 3/12 轮 · 模型思考");
        a.set_tool("MCP_Web_Use open https://a.example");
        let s = a.snapshot();
        assert_eq!(s.agent, "LsmAgentEmergentWork-SubAgent-Work");
        assert_eq!(s.session, "sess-1234");
        assert_eq!(s.iter, 3);
        assert_eq!(s.max_iter, 12);
        assert_eq!(s.task, "调研双站价格");
        assert_eq!(s.phase, "第 3/12 轮 · 模型思考");
        assert_eq!(s.tool, "MCP_Web_Use open https://a.example");
        assert!(s.started_ms > 0, "会话开始时刻应已记录");
    }

    #[test]
    fn 任务摘要跳过注入块取首条用户原文() {
        let ctx = vec![
            msg("<<<LAEW:PROJECT_CONTEXT>>>\n# 工程说明 …"),
            msg("<<<LAEW:WORKSPACE>>>\n工作区快照"),
            msg("  调研天猫京东双站 20 款笔记本电脑价格  "),
        ];
        assert_eq!(task_summary_from_context(&ctx), "调研天猫京东双站 20 款笔记本电脑价格");
    }

    #[test]
    fn 任务摘要超长按字符裁剪不切碎多字节() {
        let long = "价".repeat(MAX_TASK_CHARS + 40);
        let out = task_summary_from_context(&[msg(&long)]);
        assert_eq!(out.chars().count(), MAX_TASK_CHARS);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn 任务摘要无有效用户消息时为空() {
        let ctx = vec![
            msg("<<<LAEW:ATTACHMENTS>>>\n@README.md"),
            msg("   "),
            ChatMessage::assistant(vec![crate::llm::ContentBlock::text("助手回复")]),
        ];
        assert_eq!(task_summary_from_context(&ctx), "");
    }

    #[tokio::test]
    async fn 作用域内外可读性() {
        assert!(current().is_none(), "作用域外应无台账");
        let a = begin("LsmAgentEmergentWork-Yolo", "s1", "分类", 8);
        let got = scope(a, async {
            let s = current_snapshot().expect("作用域内应可读");
            assert_eq!(s.agent, "LsmAgentEmergentWork-Yolo");
            s.agent
        })
        .await;
        assert_eq!(got, "LsmAgentEmergentWork-Yolo");
        assert!(current().is_none(), "作用域退出后应不可读");
    }

    #[test]
    fn 工具标签按上限裁剪() {
        let a = begin("A", "s", "t", 1);
        a.set_tool(&"x".repeat(MAX_TOOL_CHARS + 10));
        assert_eq!(a.snapshot().tool.chars().count(), MAX_TOOL_CHARS);
    }
}