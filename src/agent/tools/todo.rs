//! TodoWrite 工具 —— 单工具 + `action` 枚举分发,管理 SubAgent / Main-Work 的 TODO 清单。
//!
//! 第二十轮候选 5(2026-09-21):填补知识库 L1931-L2410「TODO 任务清单系统」空缺。
//!
//! 设计:
//! - 单工具多 action(`create` / `update` / `read` / `clear`),对齐
//!   MCP_Window_Use / MCP_Web_Use 单工具多 action 风格
//! - 数据持久化放在 [`crate::agent::todo_state`] 的进程内 Mutex 中
//! - 不变量:`in_progress` 状态项 ≤ 1;ID 单调递增;status/priority 兼容别名
//! - 调用方:SubAgent-Work(`builtin_registry`)+ Main-Work(`main_work_registry`);
//!   Yolo / Plan / Quality / SessionContext / Debug / Compact 不暴露
//! - 工具实例通过 `TodoWriteTool::new(state)` 持有 [`TodoState`] 引用;默认无参时回退到全局单例

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tools::Tool;
use crate::agent::todo_state::{self, TodoItemInputRaw, TodoState};
use crate::error::{AgentError, Result};

/// TodoWrite 工具名(注册名 / wire schema 名)。与 deepseek `tool-todo/` 命名一致,
/// 便于 LLM 心智模型直接命中。
pub const TODO_WRITE: &str = "TodoWrite";

/// 单次 create/update 最多允许的 todo 数量(防止 LLM 一次性塞入过多导致上下文爆炸)。
const MAX_TODOS_PER_CALL: usize = 50;

pub struct TodoWriteTool {
    /// 显式持有的 TodoState(由调用方注入);None 时回退到进程内全局单例。
    state: Option<Arc<TodoState>>,
}

impl TodoWriteTool {
    /// 创建绑定到某个 TodoState 的工具实例(便于 TUI 会话级隔离 / 测试)。
    pub fn new(state: Arc<TodoState>) -> Self {
        Self { state: Some(state) }
    }

    /// 默认实例,回退到全局 TodoState(适用于 builtin_registry / main_work_registry 直接注册)。
    pub fn shared() -> Self {
        Self { state: None }
    }

    fn resolve_state(&self) -> Arc<TodoState> {
        if let Some(s) = &self.state {
            return s.clone();
        }
        todo_state::global_or_init("default-session")
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        TODO_WRITE
    }

    fn description(&self) -> &str {
        "维护当前 SubAgent 的 TODO 任务清单(规划型工具,非系统副作用)。\
         \n\naction 语义:\
         \n- `create`:全量替换为新的清单(传入 todos 数组);首次建立建议使用。空数组等价于 clear。\
         \n- `update`:按 id 增量更新 — 标记完成、改状态、调整优先级、改 content。\
         \n         不需要更新的项无需重复列出。content / status / priority 缺省时保留原值。\
         \n- `read`:仅返回当前清单,不修改状态。\
         \n- `clear`:清空所有 todo(重新规划时用)。\
         \n\n不变量:`in_progress` 项最多 1 个。出现冲突时自动保留第一个,其余降级 pending 并在返回中给出 warning。\
         \n\n使用建议:多步任务(>2 步)开始时 `create` 全量规划;每完成一步 `update` 标记 completed;失败时改为 cancelled。\
         \n工具返回值是渲染版清单,LLM 可继续在后续 round 中 `read` 取最新状态。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update", "read", "clear"],
                    "description": "操作类型:create=全量替换,update=按id增量更新,read=仅读取,clear=清空"
                },
                "todos": {
                    "type": "array",
                    "description": "条目列表。create/update 时必填;read/clear 时省略。",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer", "minimum": 1, "description": "条目 ID(update 时必填;create 可省略自动分配)" },
                            "content": { "type": "string", "minLength": 1, "description": "任务描述,如 '读取 Cargo.toml'" },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "任务状态"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["high", "medium", "low"],
                                "description": "优先级(可选,仅展示)"
                            }
                        },
                        "required": ["content", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let action = match args.get("action").and_then(Value::as_str) {
            Some(a) if !a.trim().is_empty() => a.trim().to_ascii_lowercase(),
            _ => {
                return Err(AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: "缺少必需参数 action(create/update/read/clear)".into(),
                });
            }
        };

        let state = self.resolve_state();
        match action.as_str() {
            "create" => self.handle_create(&args, &state),
            "update" => self.handle_update(&args, &state),
            "read" => Ok(state.render()),
            "clear" => {
                state.clear();
                Ok("Todo list cleared.".to_string())
            }
            other => Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("未知 action: {other} (允许: create/update/read/clear)"),
            }),
        }
    }
}

impl TodoWriteTool {
    fn handle_create(&self, args: &Value, state: &TodoState) -> Result<String> {
        let raw_inputs = parse_todos_field(args, "create")?;
        let inputs: Vec<_> = raw_inputs.into_iter().map(Into::into).collect();
        if inputs.len() > MAX_TODOS_PER_CALL {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!(
                    "create 一次性最多 {MAX_TODOS_PER_CALL} 条;当前 {} 条",
                    inputs.len()
                ),
            });
        }
        let (n, warnings) = state.create(inputs);
        let mut out = if n == 0 {
            "Todo list cleared (empty create).".to_string()
        } else {
            format!("Todo list created with {n} items:\n{}", state.render())
        };
        append_warnings(&mut out, warnings);
        Ok(out)
    }

    fn handle_update(&self, args: &Value, state: &TodoState) -> Result<String> {
        let raw_inputs = parse_todos_field(args, "update")?;
        let inputs: Vec<_> = raw_inputs.into_iter().map(Into::into).collect();
        if inputs.len() > MAX_TODOS_PER_CALL {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!(
                    "update 一次性最多 {MAX_TODOS_PER_CALL} 条;当前 {} 条",
                    inputs.len()
                ),
            });
        }
        let (n, missing, warnings) = state.update(inputs);
        let mut out = format!("Todo list updated ({n} items changed):\n{}", state.render());
        if !missing.is_empty() {
            out.push_str(&format!(
                "\n[warning] update 指定了不存在的 id: {:?}",
                missing
            ));
        }
        append_warnings(&mut out, warnings);
        Ok(out)
    }
}

/// 从 args.todos 数组中解析出原始输入列表。create/update 必填,空数组允许(等价 clear)。
fn parse_todos_field(args: &Value, action: &str) -> Result<Vec<TodoItemInputRaw>> {
    let todos_value = match args.get("todos") {
        Some(v) => v,
        None => {
            return Err(AgentError::ToolExecution {
                tool: TODO_WRITE.into(),
                reason: format!("action '{action}' 必须传 todos 数组(空数组允许)"),
            });
        }
    };

    let arr = todos_value.as_array().ok_or_else(|| AgentError::ToolExecution {
        tool: TODO_WRITE.into(),
        reason: format!("todos 必须是数组,而非 {}", type_name_of(todos_value)),
    })?;

    let mut result = Vec::with_capacity(arr.len());
    for (idx, item_val) in arr.iter().enumerate() {
        let parsed: TodoItemInputRaw = serde_json::from_value(item_val.clone()).map_err(|e| {
            AgentError::ToolExecution {
                tool: TODO_WRITE.into(),
                reason: format!("todos[{idx}] 解析失败: {e}"),
            }
        })?;
        result.push(parsed);
    }
    Ok(result)
}

fn type_name_of(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Object(_) => "object".to_string(),
    }
}

fn append_warnings(out: &mut String, warnings: Vec<String>) {
    if warnings.is_empty() {
        return;
    }
    out.push_str("\n[warning] ");
    out.push_str(&warnings.join("; "));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::todo_state::{self, TodoItemInput, TodoState};

    /// 每个测试都创建独立 TodoState(不依赖全局)避免并行污染。
    fn fresh_state() -> std::sync::Arc<TodoState> {
        std::sync::Arc::new(TodoState::new("isolated-test".to_string()))
    }

    fn tool_with(state: std::sync::Arc<TodoState>) -> TodoWriteTool {
        TodoWriteTool::new(state)
    }

    #[tokio::test]
    async fn todowrite_create_with_full_list() {
        let state = fresh_state();
        let tool = tool_with(state.clone());
        let args = json!({
            "action": "create",
            "todos": [
                {"content": "读取文件", "status": "in_progress", "priority": "high"},
                {"content": "解析", "status": "pending"},
                {"content": "写入", "status": "pending"},
            ]
        });
        let r = tool.execute(args).await.unwrap();
        assert!(r.contains("created with 3 items"), "{r}");
        assert!(r.contains("读取文件"), "{r}");
        assert!(r.contains("→"), "{r}"); // in_progress glyph
        let snap = state.snapshot();
        assert_eq!(snap[0].id, 1);
        assert_eq!(snap[0].status, todo_state::TodoStatus::InProgress);
        assert_eq!(snap[1].status, todo_state::TodoStatus::Pending);
        assert_eq!(snap[2].status, todo_state::TodoStatus::Pending);
    }

    #[tokio::test]
    async fn todowrite_update_marks_complete() {
        let state = fresh_state();
        // 手工塞入 seed 项目
        state.create(vec![
            TodoItemInput {
                id: None,
                content: Some("seed-a".into()),
                status: todo_state::TodoStatus::Pending,
                priority: None,
            },
            TodoItemInput {
                id: None,
                content: Some("seed-b".into()),
                status: todo_state::TodoStatus::Pending,
                priority: None,
            },
        ]);
        let tool = tool_with(state.clone());
        let args = json!({
            "action": "update",
            "todos": [
                {"id": 1, "status": "completed"},
                {"id": 2, "status": "in_progress"}
            ]
        });
        let r = tool.execute(args).await.unwrap();
        assert!(r.contains("updated (2 items changed)"), "{r}");
        let snap = state.snapshot();
        assert_eq!(snap[0].status, todo_state::TodoStatus::Completed);
        assert_eq!(snap[1].status, todo_state::TodoStatus::InProgress);
    }

    #[tokio::test]
    async fn todowrite_read_no_modification() {
        let state = fresh_state();
        state.create(vec![
            TodoItemInput {
                id: None,
                content: Some("seed-a".into()),
                status: todo_state::TodoStatus::Pending,
                priority: None,
            },
        ]);
        let tool = tool_with(state.clone());
        let s_before = state.snapshot();
        let r = tool.execute(json!({"action": "read"})).await.unwrap();
        let s_after = state.snapshot();
        assert_eq!(s_before, s_after);
        assert!(r.contains("Todo list:"), "{r}");
        assert!(r.contains("seed-a"), "{r}");
    }

    #[tokio::test]
    async fn todowrite_clear_empties_list() {
        let state = fresh_state();
        state.create(vec![
            TodoItemInput {
                id: None,
                content: Some("a".into()),
                status: todo_state::TodoStatus::Pending,
                priority: None,
            },
        ]);
        let tool = tool_with(state.clone());
        let r = tool.execute(json!({"action": "clear"})).await.unwrap();
        assert!(r.contains("cleared"), "{r}");
        assert_eq!(state.len(), 0);
    }

    #[tokio::test]
    async fn todowrite_create_empty_array_acts_as_clear() {
        let state = fresh_state();
        state.create(vec![TodoItemInput {
            id: None,
            content: Some("a".into()),
            status: todo_state::TodoStatus::Pending,
            priority: None,
        }]);
        let tool = tool_with(state.clone());
        assert_eq!(state.len(), 1);
        let r = tool
            .execute(json!({"action": "create", "todos": []}))
            .await
            .unwrap();
        assert!(r.contains("cleared"), "{r}");
        assert_eq!(state.len(), 0);
    }

    #[tokio::test]
    async fn todowrite_create_warns_multi_in_progress() {
        let state = fresh_state();
        let tool = tool_with(state.clone());
        let args = json!({
            "action": "create",
            "todos": [
                {"content": "a", "status": "in_progress"},
                {"content": "b", "status": "in_progress"},
            ]
        });
        let r = tool.execute(args).await.unwrap();
        assert!(r.contains("[warning]"), "{r}");
        let snap = state.snapshot();
        assert_eq!(snap[0].status, todo_state::TodoStatus::InProgress);
        assert_eq!(snap[1].status, todo_state::TodoStatus::Pending);
    }

    #[tokio::test]
    async fn todowrite_update_missing_id_warns() {
        let state = fresh_state();
        state.create(vec![TodoItemInput {
            id: None,
            content: Some("a".into()),
            status: todo_state::TodoStatus::Pending,
            priority: None,
        }]);
        let tool = tool_with(state.clone());
        let args = json!({
            "action": "update",
            "todos": [
                {"id": 99, "status": "completed"}
            ]
        });
        let r = tool.execute(args).await.unwrap();
        assert!(r.contains("不存在的 id"), "{r}");
        assert!(r.contains("99"), "{r}");
    }

    #[tokio::test]
    async fn todowrite_missing_action_is_error() {
        let tool = TodoWriteTool::shared();
        let err = tool.execute(json!({})).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("action"), "{msg}");
    }

    #[tokio::test]
    async fn todowrite_create_without_todos_is_error() {
        let tool = TodoWriteTool::shared();
        let err = tool.execute(json!({"action": "create"})).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("todos"), "{msg}");
    }

    #[tokio::test]
    async fn todowrite_unknown_action_is_error() {
        let tool = TodoWriteTool::shared();
        let err = tool
            .execute(json!({"action": "bogus"}))
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("bogus"), "{msg}");
    }

    #[tokio::test]
    async fn todowrite_description_mentions_actions() {
        let tool = TodoWriteTool::shared();
        let d = tool.description();
        for needle in ["create", "update", "read", "clear"] {
            assert!(d.contains(needle), "description 缺少 {needle}: {d}");
        }
    }

    #[tokio::test]
    async fn todowrite_schema_has_action_and_todos() {
        let tool = TodoWriteTool::shared();
        let schema = tool.parameters();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["action"].is_object());
        let allowed = schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = allowed.iter().map(|v| v.as_str().unwrap()).collect();
        for n in ["create", "update", "read", "clear"] {
            assert!(names.contains(&n), "schema enum 缺 {n}: {names:?}");
        }
        assert!(schema["properties"]["todos"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "action"));
    }

    // 工具名常量 + JSON schema 解析回路
    #[test]
    fn todo_item_input_raw_round_trip() {
        // 带 id + priority
        let v: TodoItemInputRaw = serde_json::from_str(
            r#"{"id":3,"content":"x","status":"completed","priority":"low"}"#,
        )
        .unwrap();
        let parsed: todo_state::TodoItemInput = v.into();
        assert_eq!(parsed.id, Some(3));
        assert_eq!(parsed.content.as_deref(), Some("x"));
        assert_eq!(parsed.priority, Some(todo_state::TodoPriority::Low));

        // 不带 id 也能解析(bare)
        let v2: TodoItemInputRaw = serde_json::from_str(
            r#"{"content":"y","status":"cancelled"}"#,
        )
        .unwrap();
        let parsed2: todo_state::TodoItemInput = v2.into();
        assert_eq!(parsed2.id, None);
        assert_eq!(parsed2.content.as_deref(), Some("y"));
        assert_eq!(parsed2.status, todo_state::TodoStatus::Cancelled);
    }
}
