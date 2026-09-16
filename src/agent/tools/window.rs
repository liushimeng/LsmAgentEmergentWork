//! 窗口操控工具(WindowUse Agent 专用):WindowList / WindowInspect / WindowAction。
//!
//! - 平台差异封闭在 `agent::window` 驱动层,本模块只做参数校验、
//!   `spawn_blocking` 包裹(UIA COM / AX IPC 可能阻塞)与输出截断;
//! - 控件树输出双重截断(节点数 ≤ 500,序列化 ≤ 32KB),防止 tool_result
//!   打爆上下文(对齐 overflow.rs 排水策略);
//! - 错误信息 LLM 可读:权限缺失给开权限步骤,控件未找到给重新检视提示。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.4。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::window::{current_driver, ControlAction, ControlNode};
use crate::error::{AgentError, Result};

/// 控件树节点数上限(超出截断)。
const MAX_TREE_NODES: usize = 500;
/// 控件树序列化字节上限(超出截断)。
const MAX_TREE_BYTES: usize = 32 * 1024;
/// 窗口列表条数上限。
const MAX_WINDOWS: usize = 200;

fn tool_err(tool: &str, reason: impl Into<String>) -> AgentError {
    AgentError::ToolExecution {
        tool: tool.into(),
        reason: reason.into(),
    }
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn require_str<'a>(args: &'a Value, key: &str, tool: &str) -> Result<&'a str> {
    get_str(args, key).ok_or_else(|| tool_err(tool, format!("缺少 string 类型参数 {key}")))
}

/// 统计控件树节点数。
fn count_nodes(node: &ControlNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

/// 深度优先裁剪子树,使总节点数 ≤ budget。
fn prune_nodes(node: &mut ControlNode, budget: &mut usize) {
    let mut kept = Vec::with_capacity(node.children.len());
    for mut child in std::mem::take(&mut node.children) {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        prune_nodes(&mut child, budget);
        kept.push(child);
    }
    node.children = kept;
}

/// 把控件树序列化为带截断标注的 JSON 字符串。
fn tree_to_json(mut root: ControlNode) -> String {
    let total = count_nodes(&root);
    let mut truncated = false;
    if total > MAX_TREE_NODES {
        let mut budget = MAX_TREE_NODES;
        prune_nodes(&mut root, &mut budget);
        truncated = true;
    }
    let mut s = serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".into());
    if s.len() > MAX_TREE_BYTES {
        s.truncate(MAX_TREE_BYTES);
        s.push_str("\n... [truncated: 输出超过 32KB]");
        truncated = true;
    }
    if truncated {
        s.push_str(&format!(
            "\n[truncated] 控件树共 {total} 个节点,仅展示前 {} 个;可用 max_depth 调小或 filter 过滤后重试",
            MAX_TREE_NODES.min(total)
        ));
    }
    s
}

/// 驱动不可用/缺权限时的前置检查:有 permission_hint 时优先返回引导文案。
fn driver_preflight(tool: &str) -> Result<()> {
    let driver = current_driver();
    if let Some(hint) = driver.permission_hint() {
        return Err(tool_err(tool, hint));
    }
    Ok(())
}

async fn run_blocking<F>(tool: &str, f: F) -> Result<String>
where
    F: FnOnce() -> Result<String> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| tool_err(tool, format!("窗口驱动任务 join 失败: {e}")))?
}

// ===================== WindowList =====================

/// 枚举可见顶层窗口。
pub struct WindowListTool;

#[async_trait]
impl Tool for WindowListTool {
    fn name(&self) -> &str {
        "WindowList"
    }

    fn description(&self) -> &str {
        "枚举当前桌面全部可见顶层窗口,返回 JSON 数组(id/title/进程名/PID/位置尺寸)。\n\
         - filter 可选:按窗口标题或进程名子串过滤(大小写不敏感)。\n\
         - 返回的 id 是不透明标识,仅供 WindowInspect/WindowAction 回传使用。\n\
         - macOS 需要「辅助功能」权限;未授权时返回可读的开权限引导。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "可选,窗口标题/进程名子串过滤" }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let filter = get_str(&args, "filter").map(str::to_string);
        // 2026-09-15 第 53 轮 P2 修复:WindowList 不调用 driver_preflight。
        // 原设计是「缺权限/缺依赖时前置报错」,但实际上:
        //   - Windows:WindowList 无需任何授权(UIA 仅 inspect/act 需权限);
        //   - macOS:WindowList 走 CoreGraphics(CGWindowListCopyWindowInfo),
        //     不需要 AX 无障碍权限(即使未授权也可枚举);
        //   - Fallback (Linux):缺 wmctrl 时由 list_windows 自身返回结构化错误。
        // 旧 preflight 曾误把 WindowList 也 fail-closed(源于「macOS 26 移除 AX」的误判),
        // 但 WindowList 完全可用 —— 改由 list_windows 内部自行处理平台差异。
        // WindowInspect / WindowAction 仍保留 driver_preflight(真需要权限)。
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let mut wins = driver.list_windows(filter.as_deref())?;
            let total = wins.len();
            if total > MAX_WINDOWS {
                wins.truncate(MAX_WINDOWS);
            }
            let mut out = serde_json::to_string_pretty(&wins).unwrap_or_else(|_| "[]".into());
            if total > MAX_WINDOWS {
                out.push_str(&format!(
                    "\n[truncated] 共 {total} 个窗口,仅展示前 {MAX_WINDOWS} 个;请用 filter 缩小范围"
                ));
            }
            if wins.is_empty() {
                out.push_str("\n(无匹配窗口;若预期有窗口,请确认目标应用已启动且未最小化)");
            }
            Ok(out)
        })
        .await
    }
}

// ===================== WindowInspect =====================

/// 枚举指定窗口的控件树。
pub struct WindowInspectTool;

#[async_trait]
impl Tool for WindowInspectTool {
    fn name(&self) -> &str {
        "WindowInspect"
    }

    fn description(&self) -> &str {
        "枚举指定窗口的控件树(Windows 走 UI Automation,macOS 走 Accessibility),\n\
         返回 JSON 树:每个控件含 path(如 /0/2/1,供 WindowAction 定位)、role、name、value、\n\
         bounds、actions(支持的动作列表)、children。\n\
         - window_id 必填:WindowList 返回的窗口 id。\n\
         - max_depth 可选:遍历深度,默认 3,范围 1-12。控件很多时先用小深度+filter。\n\
         - filter 可选:按控件名/角色子串过滤,只保留命中控件及其祖先链。\n\
         建议先检视再操作;树过大时输出会自动截断并标注。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "WindowList 返回的窗口 id" },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 12, "description": "遍历深度,默认 3" },
                "filter": { "type": "string", "description": "可选,控件名/角色子串过滤" }
            },
            "required": ["window_id"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let window_id = require_str(&args, "window_id", self.name())?.to_string();
        let max_depth = args
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .clamp(1, 12) as usize;
        let filter = get_str(&args, "filter").map(str::to_string);
        driver_preflight(self.name())?;
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let tree = driver.inspect(&window_id, max_depth, filter.as_deref())?;
            Ok(tree_to_json(tree))
        })
        .await
    }
}

// ===================== WindowAction =====================

/// 对控件执行操作。
pub struct WindowActionTool;

#[async_trait]
impl Tool for WindowActionTool {
    fn name(&self) -> &str {
        "WindowAction"
    }

    fn description(&self) -> &str {
        "对指定窗口内指定控件执行一个操作。\n\
         - window_id 必填:WindowList 返回的窗口 id。\n\
         - path 必填:WindowInspect 返回的控件路径(如 /0/2/1;\"/\" 表示窗口本身)。\n\
         - action 必填:click(点击按钮等) / invoke(同 click) / focus(聚焦) /\n\
           set_text(写入文本,需 text 参数) / get_text(读取文本) / send_keys(按键,平台支持有限)。\n\
         - text 可选:set_text/send_keys 的文本内容。\n\
         控件是否支持某动作请参考 WindowInspect 返回的 actions 列表;路径失效时重新 WindowInspect。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "WindowList 返回的窗口 id" },
                "path": { "type": "string", "description": "控件路径,如 /0/2/1;\"/\" 表示窗口本身" },
                "action": {
                    "type": "string",
                    "enum": ["click", "invoke", "focus", "set_text", "get_text", "send_keys"],
                    "description": "要执行的动作"
                },
                "text": { "type": "string", "description": "set_text/send_keys 的文本内容" }
            },
            "required": ["window_id", "path", "action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let window_id = require_str(&args, "window_id", self.name())?.to_string();
        let path = require_str(&args, "path", self.name())?.to_string();
        let action_name = require_str(&args, "action", self.name())?;
        let text = get_str(&args, "text").map(str::to_string);
        let action = ControlAction::parse(action_name, text)?;
        driver_preflight(self.name())?;
        run_blocking(self.name(), move || {
            let driver = current_driver();
            driver.act(&window_id, &path, action)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deep_tree(depth: usize) -> ControlNode {
        let mut node = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        let mut cur = &mut node;
        for i in 0..depth {
            cur.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Pane".into(),
                name: format!("n{i}"),
                ..Default::default()
            });
            cur = cur.children.last_mut().unwrap();
        }
        node
    }

    #[test]
    fn count_nodes_counts_all() {
        let t = deep_tree(5);
        assert_eq!(count_nodes(&t), 6);
    }

    #[test]
    fn prune_nodes_respects_budget() {
        let mut t = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        for i in 0..10 {
            t.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Button".into(),
                name: format!("b{i}"),
                ..Default::default()
            });
        }
        let mut budget = 4;
        prune_nodes(&mut t, &mut budget);
        assert_eq!(count_nodes(&t), 5); // root + 4
    }

    #[test]
    fn tree_to_json_truncates_huge_tree() {
        // 构造超过节点上限的树
        let mut root = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        for i in 0..600 {
            root.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Button".into(),
                name: format!("btn-{i}"),
                ..Default::default()
            });
        }
        let s = tree_to_json(root);
        assert!(s.contains("[truncated]"), "应含截断标注");
    }

    #[tokio::test]
    async fn window_action_validates_params() {
        let t = WindowActionTool;
        // 缺 path
        let err = t
            .execute(json!({"window_id": "1", "action": "click"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("path"));
        // set_text 缺 text
        let err = t
            .execute(json!({"window_id": "1", "path": "/", "action": "set_text"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("text"));
        // 未知 action
        let err = t
            .execute(json!({"window_id": "1", "path": "/", "action": "explode"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("未知 action"));
    }

    #[tokio::test]
    async fn window_inspect_validates_window_id() {
        let t = WindowInspectTool;
        let err = t.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("window_id"));
    }
}
