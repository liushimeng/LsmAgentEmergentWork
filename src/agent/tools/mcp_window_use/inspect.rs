//! MCP_Window_Use 检视与操作类 action(2026-09-18 第 84 轮自 tools/window/inspect.rs
//! 迁入):`inspect`(控件树枚举,双重截断防上下文爆炸)与 `control`
//! (Invoke / Value / 滚动 / 按键 / 坐标动作等控件操作)。

use super::*;
use crate::agent::window::{ControlAction, ControlNode};

// ===================== action=inspect =====================

/// 枚举指定窗口的控件树。
///
/// filter 失败时常见同义词表:中文 UI 控件名常出现笔误或同义词——
/// 通讯录 = 通信录 = 联系人 = Contacts;消息 = 发送 = Send = submit;
/// 按钮 = Button;输入框 = 搜索 = Search = TextField = Edit;
/// 关闭 = X = close = 退出;设置 = Settings = Preferences。
/// filter 失败时优先改用同义词重试,不要立即放弃或全量遍历。
pub(super) async fn run_inspect(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let max_depth = args
        .get("max_depth")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .clamp(1, 12) as usize;
    let filter = get_str(&args, "filter").map(str::to_string);
    driver_preflight(MCP_WINDOW_USE_TOOL_NAME).await?;
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let driver = current_driver();
        let tree = driver.inspect(&window_id, max_depth, filter.as_deref())?;
        // 控件树为空/只有 Pane 时,自动追加视觉路线引导。
        // 背景:微信 4.x 等自绘 UI 控件树为空(只有 MMUIRenderSubWindow 等 Pane),
        // LLM 常反复重试 inspect 浪费时间。检测到空树时立即引导切换路线。
        // 注意:count_nodes / has_actionable_controls 必须在 tree_to_json 之前调用,
        // 因为 tree_to_json 会 move tree(ControlNode 未实现 Copy)。
        let node_count = count_nodes(&tree);
        let has_meaningful_controls = has_actionable_controls(&tree);
        let json_str = tree_to_json(tree);
        if node_count <= 2 || !has_meaningful_controls {
            // 引导按权限分派 —— 屏幕录制缺失时推 OCR 是把 LLM 送进必败路线,
            // 改推 AX 深挖 + bounds 比例估坐标 + 键盘路线;屏幕录制可用时保留
            // 原视觉路线引导。
            let perm = crate::agent::window::check_platform_permissions();
            let guide = if perm.screen_recording {
                format!(
                    "[视觉路线引导]当前窗口控件树为空或无可操作控件(自绘 UI / Electron canvas),\
                     请立即切换视觉路线:\n\
                     1. MCP_Window_Use(action=ocr, window_id=\"{window_id}\") 识别界面文字 + 坐标\n\
                     2. MCP_Window_Use(action=control, window_id=\"{window_id}\", path=\"/\", \
                        control_action=\"click_point\", x=screen_cx, y=screen_cy) 点击目标位置\n\
                     坐标取 ocr 返回的 screen_cx/screen_cy(词块中心)。不要重复调用 inspect。"
                )
            } else {
                format!(
                    "[无 OCR 引导]控件树为空/浅树,且屏幕录制未授权 —— action=ocr / action=screenshot / \
                     screencapture 均不可用,不要尝试。替代路线:\n\
                     1. 已自动 AXEnhancedUserInterface 建树等待,可带 filter 或加大 max_depth(6-8)再 inspect 一次;\n\
                     2. 仍为空:按窗口 bounds **比例估算坐标**直接操作(click_point 不依赖 OCR:\n\
                        输入框≈底部 85% 高度、搜索框≈顶部 5%、导航栏≈左侧 3-8% 宽度),\
                        操作后用 control(control_action=get_text) 验证;\n\
                     3. 键盘路线:control(control_action=\"type_text_submit\", text=内容) 直接向焦点控件键入并提交;\n\
                     4. System Events keystroke / pbcopy + cmd+v 走 Bash(辅助功能已授权时可用)。"
                )
            };
            return Ok(format!("{json_str}\n\n{guide}"));
        }
        Ok(json_str)
    })
    .await
}

/// 检测控件树是否有「可操作控件」。
/// 可操作 = role 不是 Window / Pane / Group / Unknown 等容器类,而是 Button /
/// Edit / Text / List / MenuItem 等可交互控件。
///
/// macOS AX 角色带 `AX` 前缀(AXButton / AXTextField),需剥离 AX 前缀后做子串
/// 匹配,覆盖 AXStaticText / AXRows 等变体;Windows UIA 角色(Button / Edit)不变。
fn has_actionable_controls(node: &ControlNode) -> bool {
    const ACTIONABLE_ROLES: &[&str] = &[
        "button",
        "edit",
        "text",
        "list",
        "listitem",
        "menuitem",
        "menu",
        "checkbox",
        "radiobutton",
        "combo",
        "slider",
        "tab",
        "treeitem",
        "tree",
        "hyperlink",
        "link",
        "dataitem",
        "custom",
        "row",
        "cell",
        "outline",
        "image",
        "scrollbar",
    ];
    fn role_matches(node_role: &str) -> bool {
        let r = node_role.trim().to_ascii_lowercase();
        let r = r.strip_prefix("ax").unwrap_or(&r);
        ACTIONABLE_ROLES.iter().any(|k| r.contains(k))
    }
    if role_matches(&node.role) {
        return true;
    }
    node.children.iter().any(has_actionable_controls)
}

// ===================== action=control =====================

/// 对指定窗口执行一个操作 —— 双路线:
/// 【A. 控件树路线(原生/标准 UI)】window_id + path 定位控件,control_action=
/// click/invoke/focus/set_text/get_text/send_keys/scroll/scroll_to_visible;
/// 【B. 视觉/坐标路线(自绘 UI:微信 4.x、QQ、游戏等控件树为空的应用)】
/// control_action=click_point/double_click_point/right_click_point/scroll_point/
/// type_text/type_text_submit,x/y 传屏幕绝对坐标(取 ocr 返回 blocks 的
/// screen_cx/screen_cy 中心),path 照传 "/"。
pub(super) async fn run_control(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let path = require_str(&args, "path", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let control_action_name = require_str(&args, "control_action", MCP_WINDOW_USE_TOOL_NAME)?;
    let text = get_str(&args, "text").map(str::to_string);
    // 坐标动作参数(x/y 屏幕绝对坐标,来自 action=ocr;第 90 轮:x2/y2 = drag_point
    // 终点,modifiers = 修饰键规格,作用于 click_point 系 / drag_point)
    let x = args.get("x").and_then(Value::as_i64);
    let y = args.get("y").and_then(Value::as_i64);
    let x2 = args.get("x2").and_then(Value::as_i64);
    let y2 = args.get("y2").and_then(Value::as_i64);
    let modifiers = get_str(&args, "modifiers").map(str::to_string);
    let action =
        ControlAction::parse_ext(control_action_name, text, x, y, x2, y2, modifiers)?;
    driver_preflight(MCP_WINDOW_USE_TOOL_NAME).await?;
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let driver = current_driver();
        driver.act(&window_id, &path, action)
    })
    .await
}
