//! 窗口检视与动作工具(2026-09-17 自 tools/window.rs 拆分)。
//!
//! `WindowInspectTool`(控件树枚举,双重截断防上下文爆炸)与
//! `WindowActionTool`(Invoke / Value / 滚动 / 按键等控件动作)。

use super::*;

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
         建议先检视再操作;树过大时输出会自动截断并标注。\n\
         \n\
         【filter 失败时常见同义词表(2026-09-16 第 65 轮 P0-B)】中文 UI 控件名常出现笔误或同义词:\n\
         - 通讯录 = 通信录 = 联系人 = Contacts = contactsList\n\
         - 消息 = 发送 = Send = submit\n\
         - 按钮 = Button\n\
         - 输入框 = 搜索 = Search = TextField = Edit\n\
         - 关闭 = X = close = 退出\n\
         - 设置 = 设置 = 设置 = Settings = Preferences\n\
         filter 失败时优先改用上表同义词重试,不要立即放弃或全量遍历。"
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
        driver_preflight(self.name()).await?;
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let tree = driver.inspect(&window_id, max_depth, filter.as_deref())?;
            // 2026-09-16 第 68 轮 P2-A:控件树为空/只有 Pane 时,自动追加视觉路线引导。
            // 背景:微信 4.x 等自绘 UI 控件树为空(只有 MMUIRenderSubWindow 等 Pane),
            // LLM 常反复重试 WindowInspect 浪费时间。检测到空树时立即引导切换到视觉路线。
            // 注意:count_nodes / has_actionable_controls 必须在 tree_to_json 之前调用,
            // 因为 tree_to_json 会 move tree(ControlNode 未实现 Copy)。
            let node_count = count_nodes(&tree);
            let has_meaningful_controls = has_actionable_controls(&tree);
            let json_str = tree_to_json(tree);
            if node_count <= 2 || !has_meaningful_controls {
                return Ok(format!(
                    "{json_str}\n\n[视觉路线引导]当前窗口控件树为空或无可操作控件(自绘 UI / Electron canvas),\
                     请立即切换视觉路线:\n\
                     1. WindowOCR(window_id=\"{window_id}\") 识别界面文字 + 坐标\n\
                     2. WindowAction(window_id=\"{window_id}\", path=\"/\", action=\"click_point\", \
                        x=screen_cx, y=screen_cy) 点击目标位置\n\
                     坐标取 WindowOCR 返回的 screen_cx/screen_y(词块中心)。不要重复调用 WindowInspect。"
                ));
            }
            Ok(json_str)
        })
        .await
    }
}

/// 2026-09-16 第 68 轮 P2-A:检测控件树是否有「可操作控件」。
/// 可操作 = role 不是 Window / Pane / Group / Unknown 等容器类,而是 Button /
/// Edit / Text / List / MenuItem 等可交互控件。
fn has_actionable_controls(node: &ControlNode) -> bool {
    const ACTIONABLE_ROLES: &[&str] = &[
        "Button", "Edit", "Text", "List", "ListItem", "MenuItem", "CheckBox",
        "RadioButton", "ComboBox", "Slider", "Tab", "TreeItem", "Hyperlink",
        "DataItem", "Custom",
    ];
    if ACTIONABLE_ROLES
        .iter()
        .any(|r| node.role.eq_ignore_ascii_case(r))
    {
        return true;
    }
    node.children.iter().any(has_actionable_controls)
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
        "对指定窗口执行一个操作 —— 双路线:\n\
         【A. 控件树路线(原生/标准 UI)】window_id + path 定位控件:\n\
         - path 用 WindowInspect 返回的控件路径(如 /0/2/1;\"/\" 表示窗口本身);\n\
         - action:click(点击) / invoke(同 click) / focus(聚焦) /\n\
           set_text(写入文本,需 text) / get_text(读取文本) /\n\
           send_keys(按键,text 传命名键或组合键:enter/ctrl+a/alt+f4) /\n\
           scroll(滚轮,text 传 \"down:3\"/\"up:5\" 缺省 3 行) /\n\
           scroll_to_visible(把控件滚动到可见)。\n\
         【B. 视觉/坐标路线(自绘 UI:微信 4.x、QQ、游戏等控件树为空的应用)】\n\
         - 坐标来自 WindowOCR 返回的 blocks(screen_x/screen_y 取中心):\n\
         - click_point / double_click_point / right_click_point(需 x,y:屏幕绝对坐标) /\n\
           scroll_point(需 x,y;text 传方向行数如 \"down:3\") /\n\
           type_text(向当前焦点真实键入 text,配合 click_point 先点输入框)。\n\
         坐标动作的 path 照传 \"/\" 即可。控件是否支持某动作参考 WindowInspect 的 actions;路径失效时重新 WindowInspect。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "WindowList/WindowOpen/WindowFind 返回的窗口 id" },
                "path": { "type": "string", "description": "控件路径,如 /0/2/1;\"/\" 表示窗口本身(坐标动作照传 \"/\")" },
                "action": {
                    "type": "string",
                    "enum": [
                        "click", "invoke", "focus", "set_text", "get_text", "send_keys", "scroll", "scroll_to_visible",
                        "click_point", "double_click_point", "right_click_point", "scroll_point", "type_text"
                    ],
                    "description": "要执行的动作(控件树路线 / 坐标视觉路线)"
                },
                "text": { "type": "string", "description": "set_text/send_keys/type_text 的文本;scroll/scroll_point 传方向行数(down:3)" },
                "x": { "type": "integer", "description": "坐标动作必填:屏幕绝对 X(取 WindowOCR 返回 screen_x 中心)" },
                "y": { "type": "integer", "description": "坐标动作必填:屏幕绝对 Y(取 WindowOCR 返回 screen_y 中心)" }
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
        // 2026-09-16 第 67 轮:坐标动作参数(x/y 屏幕绝对坐标,来自 WindowOCR)
        let x = args.get("x").and_then(Value::as_i64);
        let y = args.get("y").and_then(Value::as_i64);
        let action = ControlAction::parse_ext(action_name, text, x, y)?;
        driver_preflight(self.name()).await?;
        run_blocking(self.name(), move || {
            let driver = current_driver();
            driver.act(&window_id, &path, action)
        })
        .await
    }
}
