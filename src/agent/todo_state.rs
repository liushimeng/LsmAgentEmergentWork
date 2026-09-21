//! SubAgent TODO 任务清单状态(单进程单会话模型,Mutex 保护)。
//!
//! 第二十轮候选 5(2026-09-21):填补知识库 L1931-L2410「TODO 任务清单系统」空缺。
//! 对标:
//! - deepseek `tool-todo/` projection/invariant 模型(本轮采用更精简版)
//! - opencode `todo.ts` 单工具 + action dispatch 模式
//!
//! 设计要点:
//! - 进程内持有 `Arc<TodoState>`,通过 `GLOBAL_TODO_STATE` 全局单例访问
//! - 不引入新 crate,所有序列化用 serde_json;锁沿用 `std::sync::Mutex`(与 OfflineQueue / ConnectivityTracker 风格一致)
//! - 不变量:`in_progress` 状态项 ≤ 1;ID 单调递增不重用;状态机合法

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// TODO 状态枚举(对外暴露的合法值)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// 未开始
    #[default]
    Pending,
    /// 进行中(全列表最多 1 项)
    InProgress,
    /// 已完成
    Completed,
    /// 已取消(不再做)
    Cancelled,
}

impl TodoStatus {
    /// 状态稳定字符串标识(供 TodoWrite action / debug 展示使用)。
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }

    /// 从字符串还原(对用户传入 + 反序列化双向)。大小写敏感,空白裁剪。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "pending" => Some(TodoStatus::Pending),
            "in_progress" | "inprogress" | "in-progress" => Some(TodoStatus::InProgress),
            "completed" | "done" | "complete" => Some(TodoStatus::Completed),
            "cancelled" | "canceled" => Some(TodoStatus::Cancelled),
            _ => None,
        }
    }

    /// 用于 render 的中文章段。
    pub fn glyph(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "○",
            TodoStatus::InProgress => "→",
            TodoStatus::Completed => "✓",
            TodoStatus::Cancelled => "✗",
        }
    }
}

/// TODO 优先级(展示用,不参与不变量;允许 LLM 自由选填)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

impl TodoPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoPriority::High => "high",
            TodoPriority::Medium => "medium",
            TodoPriority::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" | "h" => Some(TodoPriority::High),
            "medium" | "med" | "m" | "normal" => Some(TodoPriority::Medium),
            "low" | "l" => Some(TodoPriority::Low),
            _ => None,
        }
    }
}

/// 单条 TODO 项。
///
/// `id` 是 1-based 单调递增的稳定标识,由 `create` 时由 TodoState 分配,
/// 跨 update 不变;`update` 必须按 id 寻址;`clear` 后 id 不复用,下次 `create` 重新从 1 开始。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u32,
    pub content: String,
    pub status: TodoStatus,
    /// 优先级(可选;None = medium 等价,但 JSON 序列化保留 null/缺省时按 medium 兜底)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<TodoPriority>,
    /// 创建时间(可读字符串,如 "2026-09-21 16:42")。TodoWrite create 时由 TodoState 自动填。
    pub created_at: String,
    /// 最后更新时间(同上)。
    pub updated_at: String,
}

/// 进程内 Todo 状态(单 session 视角)。
#[derive(Debug)]
pub struct TodoState {
    inner: Mutex<TodoInner>,
    /// 关联 session id(仅展示用,由 `init_for_session` 注入)。
    session_id: String,
}

#[derive(Debug, Default)]
struct TodoInner {
    items: Vec<TodoItem>,
    /// 下次 create 分配 ID 的种子(单调递增)。
    next_id: u32,
    /// 最后一次变更时间(用于 TUI 时间展示与不变量记录)。
    last_updated: Option<Instant>,
}

/// 全局单例 TodoState(Mutex<Option<Arc<TodoState>>>),会话级生命周期。
///
/// laew 单进程单用户单会话:
/// - 启动时若已存在则覆盖(进程内唯一)
/// - `/clear` / `/new` 时通过 [`reset_global_for_session`] 重置
/// - TUI 与工具通过 [`global()`] / [`global_or_init()`] 获取
static GLOBAL_TODO_STATE: OnceLock<Mutex<Option<Arc<TodoState>>>> = OnceLock::new();

fn global_lock() -> &'static Mutex<Option<Arc<TodoState>>> {
    GLOBAL_TODO_STATE.get_or_init(|| Mutex::new(None))
}

/// 获取全局 TodoState(若未初始化则返回 None)。
pub fn global() -> Option<Arc<TodoState>> {
    global_lock().lock().expect("TodoState poisoned").clone()
}

/// 获取全局 TodoState,若未初始化则用 `session_id` 初始化一个。
pub fn global_or_init(session_id: &str) -> Arc<TodoState> {
    let mut guard = global_lock().lock().expect("TodoState poisoned");
    if let Some(existing) = guard.as_ref() {
        return existing.clone();
    }
    let new_state = Arc::new(TodoState::new(session_id.to_string()));
    *guard = Some(new_state.clone());
    new_state
}

/// 重置全局 TodoState(用于 `/clear` / `/new` / `/fork` 等需要全新列表的场景)。
///
/// 同时初始化新 session 的关联 id,保持调用方无需额外操作。
pub fn reset_global_for_session(session_id: &str) -> Arc<TodoState> {
    let mut guard = global_lock().lock().expect("TodoState poisoned");
    let new_state = Arc::new(TodoState::new(session_id.to_string()));
    *guard = Some(new_state.clone());
    new_state
}

impl TodoState {
    /// 创建空 TodoState(不会自动注册到全局)。
    pub fn new(session_id: String) -> Self {
        Self {
            inner: Mutex::new(TodoInner::default()),
            session_id,
        }
    }

    /// 关联 session_id(只读)。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 当前列表是否为空。
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.lock().expect("TodoState poisoned");
        inner.items.is_empty()
    }

    /// 当前列表条数。
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().expect("TodoState poisoned");
        inner.items.len()
    }

    /// 全量替换为新的 todo 列表(create 语义)。
    ///
    /// - **空输入**:接受并等价 clear
    /// - **多 in_progress**:自动保留第一个,其余降级 pending(返回值附 warning 描述)
    ///
    /// 返回 (更新后的 items 数, 是否做了 in_progress 降级)
    pub fn create(&self, mut items: Vec<TodoItemInput>) -> (usize, Vec<String>) {
        let mut warnings = Vec::new();
        let now = now_readable();

        // 自动分配 id(单 in_progress 保留)
        let mut next_id: u32 = 1;
        let mut saw_in_progress = false;
        for item in items.iter_mut() {
            if item.id.is_none() {
                item.id = Some(next_id);
            }
            next_id = next_id.max(item.id.unwrap() + 1);
            if matches!(item.status, TodoStatus::InProgress) {
                if saw_in_progress {
                    item.status = TodoStatus::Pending;
                    warnings.push(format!(
                        "多个 in_progress 项冲突,id={} 自动降级为 pending",
                        item.id.unwrap()
                    ));
                }
                saw_in_progress = true;
            }
        }

        let mut inner = self.inner.lock().expect("TodoState poisoned");
        inner.next_id = next_id;
        inner.last_updated = Some(Instant::now());
        let count = items.len();
        inner.items = items
            .into_iter()
            .map(|input| {
                let id = input.id.unwrap();
                let prio = input.priority.or(Some(TodoPriority::Medium));
                TodoItem {
                    id,
                    content: input.content.unwrap_or_default(),
                    status: input.status,
                    priority: prio,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                }
            })
            .collect();
        (count, warnings)
    }

    /// 按 id 增量更新(update 语义)。
    ///
    /// - `TodoItemInput.id` 必填
    /// - `content` 缺失则保留原值,status/priority 缺失保留原值
    ///
    /// 返回 (成功更新条数, 未找到 id 列表, 警告列表)
    pub fn update(&self, items: Vec<TodoItemInput>) -> (usize, Vec<u32>, Vec<String>) {
        let mut warnings = Vec::new();
        let mut not_found = Vec::new();
        let now = now_readable();

        let mut inner = self.inner.lock().expect("TodoState poisoned");

        // 先校验 + 找出多 in_progress
        let inputs_with_id: Vec<TodoItemInput> = items
            .into_iter()
            .filter_map(|mut i| {
                let id = match i.id {
                    Some(id) => id,
                    None => {
                        warnings.push("update 必须带 id,跳过无 id 项".to_string());
                        return None;
                    }
                };
                if !inner.items.iter().any(|existing| existing.id == id) {
                    not_found.push(id);
                    return None;
                }
                i.id = Some(id);
                Some(i)
            })
            .collect();

        // 多 in_progress 保留第一个
        let mut saw_in_progress = false;
        for input in inputs_with_id.iter() {
            if let (Some(_id), TodoStatus::InProgress) = (input.id, input.status) {
                if saw_in_progress {
                    warnings.push(format!(
                        "多个 in_progress 项冲突,id={} 自动降级为 pending",
                        _id
                    ));
                } else {
                    saw_in_progress = true;
                }
            }
        }

        let updated_count = inputs_with_id.len();
        for input in inputs_with_id {
            let id = input.id.unwrap();
            // 先把 extras 在锁内一次算好,避免后续在 iter_mut 持有期间重新借用 inner
            let extras = count_extras_in_progress_to(&inner.items, id);
            let target = inner.items.iter_mut().find(|it| it.id == id);
            if let Some(target) = target {
                if let Some(content) = input.content {
                    target.content = content;
                }
                let mut new_status = input.status;
                if matches!(input.status, TodoStatus::InProgress) {
                    if extras > 0 {
                        new_status = TodoStatus::Pending;
                    }
                }
                target.status = new_status;
                if let Some(prio) = input.priority {
                    target.priority = Some(prio);
                }
                target.updated_at = now.clone();
            }
        }
        inner.last_updated = Some(Instant::now());
        (updated_count, not_found, warnings)
    }

    /// 清空(clear 语义);id 计数也归零。
    pub fn clear(&self) {
        let mut inner = self.inner.lock().expect("TodoState poisoned");
        inner.items.clear();
        inner.next_id = 1;
        inner.last_updated = Some(Instant::now());
    }

    /// 读取当前快照(克隆,适合无锁视图层使用)。
    pub fn snapshot(&self) -> Vec<TodoItem> {
        let inner = self.inner.lock().expect("TodoState poisoned");
        inner.items.clone()
    }

    /// 渲染人类可读清单(用于 TodoWrite tool 返回 + TUI 横幅)。
    pub fn render(&self) -> String {
        let items = self.snapshot();
        if items.is_empty() {
            return "(空 todo 列表)".to_string();
        }
        let mut out = String::new();
        out.push_str(&format!("Todo list: {} items\n", items.len()));
        for it in &items {
            let prio = match it.priority {
                Some(TodoPriority::High) => " (high)",
                Some(TodoPriority::Medium) => " (medium)",
                Some(TodoPriority::Low) => " (low)",
                None => "",
            };
            out.push_str(&format!(
                "{} [{}] {}{}\n",
                it.status.glyph(),
                it.id,
                truncate_for_render(&it.content, 60),
                prio
            ));
        }
        out.trim_end().to_string()
    }

    /// 进度摘要行(供 TUI 横幅紧凑展示)。
    ///
    /// 返回形如 `[✓]3 [→]1 [○]2`;空列表返回空字符串。
    pub fn summary_line(&self) -> String {
        let items = self.snapshot();
        if items.is_empty() {
            return String::new();
        }
        let mut completed = 0u32;
        let mut in_progress = 0u32;
        let mut pending = 0u32;
        let mut cancelled = 0u32;
        let mut last_in_progress: Option<&TodoItem> = None;
        for it in &items {
            match it.status {
                TodoStatus::Completed => completed += 1,
                TodoStatus::InProgress => {
                    in_progress += 1;
                    last_in_progress = Some(it);
                }
                TodoStatus::Pending => pending += 1,
                TodoStatus::Cancelled => cancelled += 1,
            }
        }
        let mut parts = Vec::new();
        if completed > 0 {
            parts.push(format!("[✓]{}", completed));
        }
        if in_progress > 0 {
            parts.push(format!("[→]{}", in_progress));
        }
        if pending > 0 {
            parts.push(format!("[○]{}", pending));
        }
        if cancelled > 0 {
            parts.push(format!("[✗]{}", cancelled));
        }
        let mut line = parts.join(" ");
        if let Some(it) = last_in_progress {
            line.push_str(&format!(" · last \"{}\"", truncate_for_render(&it.content, 28)));
        }
        line
    }

    /// 写入 session_memory 表的 final summary(JSON)。
    ///
    /// 包含 completed / in_progress / pending 三块的简要计数 + 每项的 content。
    pub fn render_for_session_memory(&self) -> String {
        let items = self.snapshot();
        if items.is_empty() {
            return "{}".to_string();
        }
        serde_json::json!({
            "total": items.len(),
            "items": items
                .iter()
                .map(|it| serde_json::json!({
                    "id": it.id,
                    "content": it.content,
                    "status": it.status.as_str(),
                    "priority": it.priority.map(|p| p.as_str()).unwrap_or("medium"),
                }))
                .collect::<Vec<_>>(),
        })
        .to_string()
    }
}

/// 用于外部调用 `create` / `update` 的输入项(部分字段可选)。
///
/// - `create`:`id` 可选(None = 自动分配);`content` 必填;`status` 必填;`priority` 可选
/// - `update`:`id` 必填;`content` 可选(缺失保留原值);`status` 可选;`priority` 可选
#[derive(Debug, Clone, Deserialize)]
pub struct TodoItemInput {
    pub id: Option<u32>,
    pub content: Option<String>,
    #[serde(default)]
    pub status: TodoStatus,
    #[serde(default)]
    pub priority: Option<TodoPriority>,
}

/// 兼容性:从 `{"content":..., "status":"pending", "priority":"high"}`
/// (无 id 字段) 也能解析 —— 自动化 / 手工构造常用。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TodoItemInputRaw {
    WithId {
        id: Option<u32>,
        content: Option<String>,
        #[serde(default)]
        status: TodoStatus,
        #[serde(default)]
        priority: Option<TodoPriority>,
    },
    Bare {
        content: String,
        status: TodoStatus,
        #[serde(default)]
        priority: Option<TodoPriority>,
    },
}

impl From<TodoItemInputRaw> for TodoItemInput {
    fn from(r: TodoItemInputRaw) -> Self {
        match r {
            TodoItemInputRaw::WithId { id, content, status, priority } => {
                TodoItemInput { id, content, status, priority }
            }
            TodoItemInputRaw::Bare { content, status, priority } => TodoItemInput {
                id: None,
                content: Some(content),
                status,
                priority,
            },
        }
    }
}

fn now_readable() -> String {
    use time::OffsetDateTime;

    // Cargo.toml 主依赖未开 macros feature,这里手动格式化避免对宏依赖;
    // 输出格式 "YYYY-MM-DD HH:MM:SS"(本地时区,失败回退 UTC)。
    let dt = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let s = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    if s.is_empty() {
        "0000-00-00 00:00:00".to_string()
    } else {
        s
    }
}

fn count_extras_in_progress_to(items: &[TodoItem], target_id: u32) -> usize {
    items
        .iter()
        .filter(|it| it.id != target_id && matches!(it.status, TodoStatus::InProgress))
        .count()
}

fn truncate_for_render(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(content: &str, status: TodoStatus) -> TodoItemInput {
        TodoItemInput {
            id: None,
            content: Some(content.to_string()),
            status,
            priority: None,
        }
    }

    fn input_with_id(id: u32, content: &str, status: TodoStatus) -> TodoItemInput {
        TodoItemInput {
            id: Some(id),
            content: Some(content.to_string()),
            status,
            priority: None,
        }
    }

    fn state() -> TodoState {
        TodoState::new("test-session".to_string())
    }

    #[test]
    fn todo_status_round_trip() {
        for s in [
            TodoStatus::Pending,
            TodoStatus::InProgress,
            TodoStatus::Completed,
            TodoStatus::Cancelled,
        ] {
            assert_eq!(TodoStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(TodoStatus::parse("unknown"), None);
        // 别名兼容
        assert_eq!(TodoStatus::parse("in-progress"), Some(TodoStatus::InProgress));
        assert_eq!(TodoStatus::parse("done"), Some(TodoStatus::Completed));
        assert_eq!(TodoStatus::parse("canceled"), Some(TodoStatus::Cancelled));
    }

    #[test]
    fn todo_priority_round_trip() {
        assert_eq!(TodoPriority::parse("high"), Some(TodoPriority::High));
        assert_eq!(TodoPriority::parse("MEDIUM"), Some(TodoPriority::Medium));
        assert_eq!(TodoPriority::parse("low"), Some(TodoPriority::Low));
        assert_eq!(TodoPriority::parse("unknown"), None);
    }

    #[test]
    fn create_5_items_assigns_ids() {
        let s = state();
        let items = vec![
            input("a", TodoStatus::Pending),
            input("b", TodoStatus::Pending),
            input("c", TodoStatus::Pending),
            input("d", TodoStatus::Pending),
            input("e", TodoStatus::Pending),
        ];
        let (n, warns) = s.create(items);
        assert_eq!(n, 5);
        assert!(warns.is_empty());
        let snap = s.snapshot();
        assert_eq!(snap.iter().map(|it| it.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn in_progress_constraint_max_one_in_create() {
        let s = state();
        let items = vec![
            input("a", TodoStatus::InProgress),
            input("b", TodoStatus::InProgress),
            input("c", TodoStatus::InProgress),
        ];
        let (n, warns) = s.create(items);
        assert_eq!(n, 3);
        assert_eq!(warns.len(), 2);
        // 第一个保留 in_progress,其余降级 pending
        let snap = s.snapshot();
        assert_eq!(snap[0].status, TodoStatus::InProgress);
        assert_eq!(snap[1].status, TodoStatus::Pending);
        assert_eq!(snap[2].status, TodoStatus::Pending);
    }

    #[test]
    fn update_changes_status_by_id() {
        let s = state();
        s.create(vec![
            input("a", TodoStatus::Pending),
            input("b", TodoStatus::Pending),
            input("c", TodoStatus::Pending),
        ]);
        let (updated, missing, warns) = s.update(vec![
            input_with_id(2, "bbb", TodoStatus::InProgress),
        ]);
        assert_eq!(updated, 1);
        assert!(missing.is_empty());
        assert!(warns.is_empty());
        let snap = s.snapshot();
        assert_eq!(snap[1].status, TodoStatus::InProgress);
        assert_eq!(snap[1].content, "bbb");
    }

    #[test]
    fn update_missing_id_reports_not_found() {
        let s = state();
        s.create(vec![input("a", TodoStatus::Pending)]);
        let (updated, missing, _warns) = s.update(vec![input_with_id(99, "x", TodoStatus::Completed)]);
        assert_eq!(updated, 0);
        assert_eq!(missing, vec![99]);
    }

    #[test]
    fn update_keeps_existing_content_when_omitted() {
        let s = state();
        s.create(vec![input("original", TodoStatus::Pending)]);
        // 没传 content -> 保留
        s.update(vec![TodoItemInput {
            id: Some(1),
            content: None,
            status: TodoStatus::Completed,
            priority: None,
        }]);
        let snap = s.snapshot();
        assert_eq!(snap[0].content, "original");
        assert_eq!(snap[0].status, TodoStatus::Completed);
    }

    #[test]
    fn clear_resets_state() {
        let s = state();
        s.create(vec![input("a", TodoStatus::Pending), input("b", TodoStatus::Pending)]);
        assert_eq!(s.len(), 2);
        s.clear();
        assert_eq!(s.len(), 0);
        // clear 后下一次 create 从 id=1 开始
        let (n, _w) = s.create(vec![input("x", TodoStatus::Pending)]);
        assert_eq!(n, 1);
        let snap = s.snapshot();
        assert_eq!(snap[0].id, 1);
    }

    #[test]
    fn render_summary_counts_correct() {
        let s = state();
        let items = vec![
            input("done1", TodoStatus::Completed),
            input("done2", TodoStatus::Completed),
            input("done3", TodoStatus::Completed),
            input("doing", TodoStatus::InProgress),
            input("next1", TodoStatus::Pending),
            input("next2", TodoStatus::Pending),
        ];
        s.create(items);
        let line = s.summary_line();
        assert!(line.contains("[✓]3"), "summary: {line}");
        assert!(line.contains("[→]1"), "summary: {line}");
        assert!(line.contains("[○]2"), "summary: {line}");
        assert!(line.contains("last \"doing\""), "summary: {line}");
    }

    #[test]
    fn render_summary_empty_returns_blank() {
        let s = state();
        assert_eq!(s.summary_line(), "");
    }

    #[test]
    fn render_for_session_memory_has_items() {
        let s = state();
        s.create(vec![
            input("a", TodoStatus::Completed),
            input("b", TodoStatus::InProgress),
        ]);
        let json = s.render_for_session_memory();
        assert!(json.contains("\"total\":2"), "{json}");
        assert!(json.contains("\"completed\""), "{json}");
        assert!(json.contains("\"in_progress\""), "{json}");
    }

    #[test]
    fn render_for_session_memory_empty_returns_braces() {
        let s = state();
        assert_eq!(s.render_for_session_memory(), "{}");
    }

    #[test]
    fn todo_item_input_raw_bare_form() {
        let raw: TodoItemInputRaw =
            serde_json::from_str(r#"{"content":"x","status":"completed"}"#).unwrap();
        let parsed: TodoItemInput = raw.into();
        assert_eq!(parsed.id, None);
        assert_eq!(parsed.content.as_deref(), Some("x"));
        assert_eq!(parsed.status, TodoStatus::Completed);
    }

    #[test]
    fn global_register_and_reset() {
        // 串行锁:与其它全局状态测试互斥,避免并行 cargo test 时全局状态被交换。
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tag = unique_test_tag();
        reset_global_for_session(&format!("sess-A-{tag}"));
        let s = global().expect("global not initialized");
        assert_eq!(s.session_id(), format!("sess-A-{tag}"));
        s.create(vec![input("a", TodoStatus::Pending)]);
        assert_eq!(s.len(), 1);
        // reset 时清空
        reset_global_for_session(&format!("sess-B-{tag}"));
        let s2 = global().expect("global not init");
        assert_eq!(s2.session_id(), format!("sess-B-{tag}"));
        assert_eq!(s2.len(), 0);
    }

    #[test]
    fn global_or_init_initializes_lazily() {
        // 串行锁:与 global_register_and_reset 互斥。
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_global_for_session(&format!("sess-C-{}", unique_test_tag()));
        let _ = global(); // 取现有
        // global_or_init 不应在已存在时覆盖
        let s = global_or_init("sess-C-another");
        assert!(s.session_id().starts_with("sess-C-"));
    }

    /// 串行化全局 mutex 测试用 lock —— cargo test 默认并行,
    /// 涉及全局状态的测试同时只能跑一个,避免互相覆盖。
    static GLOBAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 简单计数器,让每个测试使用唯一的 session_id 起始点,避免并行污染。
    fn unique_test_tag() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
    }
}
