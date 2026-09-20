//! act() match 分发(2026-09-20 第 97 轮从 macos_legacy.rs 拆分)。
//!
//! `WindowDriver::act` 的完整 match 分发器:根据 `ControlAction` 枚举选择
//! AX 路线(原生控件)或 CGEvent 路线(坐标动作 / 自绘 UI)。
//!
//! 跨平台一致语义(2026-09-19 第 90 轮):
//!   控制类操作 **无障碍优先 / Windows 消息优先,物理鼠标键盘兜底**;
//!   自绘 UI(微信 4.x / Electron canvas / 游戏)天然走物理路线。

#![allow(non_snake_case)]

use core_foundation::base::{CFRelease, TCFType};
use core_foundation::boolean::CFBooleanRef;
use core_foundation::string::CFStringRef;

use super::{
    ax_error_text, ax_get_string, cfstring_new, cg_click_at, cg_click_at_ex, cg_drag, cg_mod_flags,
    cg_mod_note, cg_move_cursor, cg_scroll_lines, cg_send_key, cg_type_text, element_action_names,
    element_at_path, kAXConfirmAction, kAXFocusedAttribute, kAXOpenAction, kAXPickAction,
    kAXPressAction, kAXScrollToVisibleAction, kAXTitleAttribute, kAXValueAttribute,
    keycode_for_name, parse_key_combo, parse_window_id, platform_err, window_element, AXError,
    AXUIElementPerformAction, AXUIElementSetAttributeValue, CgMouseButton, ControlAction,
    MacOsDriver, K_AX_ERROR_ACTION_UNSUPPORTED, K_AX_ERROR_SUCCESS,
};
use crate::error::Result;

/// WindowDriver::act 的实际实现。逐行与拆分前一致。
pub(super) fn dispatch_act(
    _driver: &MacOsDriver,
    window_id: &str,
    path: &str,
    action: ControlAction,
) -> Result<String> {
    // 2026-09-16 第 62 轮:同 inspect,未授权时持续等待。
    let prompt = std::env::var("LAEW_AX_PROMPT")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(true);
    let (granted, waited) =
        MacOsDriver::is_ax_trusted_with_retry(prompt, 120, |elapsed, granted| {
            tracing::debug!(
                elapsed_secs = elapsed,
                granted = granted,
                "MCP_Window_Use(action=control) 等待 macOS 辅助功能授权"
            );
        });
    if !granted {
        return Err(platform_err(
            "macos",
            format!(
                "辅助功能未授权;已等待 {waited}s。请到「系统设置→隐私与安全性→辅助功能」勾选当前终端后重试"
            ),
        ));
    }
    let (pid, idx) = parse_window_id(window_id)?;
    unsafe {
        let win = window_element(pid, idx)?;
        let el = element_at_path(win, path)?;
        CFRelease(win);
        let result = match &action {
            ControlAction::Click | ControlAction::Invoke => {
                // 第 80 轮:按控件真实 action 能力选择,而不是硬编码 AXPress。
                let ax_actions = element_action_names(el);
                let candidates: &[(&str, CFStringRef)] = &[
                    ("AXPress", kAXPressAction()),
                    ("AXPick", kAXPickAction()),
                    ("AXConfirm", kAXConfirmAction()),
                    ("AXOpen", kAXOpenAction()),
                ];
                let mut selected: Option<(String, AXError)> = None;
                for (name, action) in candidates {
                    if ax_actions.is_empty() || ax_actions.iter().any(|s| s == name) {
                        let err = AXUIElementPerformAction(el, *action);
                        if err == K_AX_ERROR_SUCCESS {
                            selected = Some(((*name).to_string(), err));
                            break;
                        }
                        selected = Some(((*name).to_string(), err));
                        if err != K_AX_ERROR_ACTION_UNSUPPORTED {
                            break;
                        }
                    }
                }
                match selected {
                    Some((name, K_AX_ERROR_SUCCESS)) => {
                        Ok(format!("已对 {window_id}{path} 执行点击({name})"))
                    }
                    Some((name, err)) => Err(platform_err(
                        "macos",
                        format!("AX {name} 失败: {}", ax_error_text(err)),
                    )),
                    None => Err(platform_err(
                        "macos",
                        "控件未暴露任何可点击 AX action;请重新 MCP_Window_Use(action=inspect) 确认 path",
                    )),
                }
            }
            ControlAction::Focus => {
                let true_v: CFBooleanRef =
                    core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
                let err = AXUIElementSetAttributeValue(el, kAXFocusedAttribute(), true_v.cast());
                if err == K_AX_ERROR_SUCCESS {
                    Ok(format!("已聚焦 {window_id}{path}"))
                } else {
                    Err(platform_err("macos", ax_error_text(err)))
                }
            }
            ControlAction::SetText(text) => {
                let cf = cfstring_new(text);
                let err = AXUIElementSetAttributeValue(el, kAXValueAttribute(), cf.cast());
                CFRelease(cf.cast());
                if err == K_AX_ERROR_SUCCESS {
                    Ok(format!(
                        "已向 {window_id}{path} 写入文本({} 字符)",
                        text.chars().count()
                    ))
                } else {
                    Err(platform_err("macos", ax_error_text(err)))
                }
            }
            ControlAction::GetText => {
                let v = ax_get_string(el, kAXValueAttribute());
                let t = ax_get_string(el, kAXTitleAttribute());
                Ok(if v.is_empty() { t } else { v })
            }
            // 2026-09-16 第 66 轮:CGEvent 按键注入(此前直接报「暂不支持」,
            // 微信 Enter 发送 / PageDown 翻页链路无原语)。
            // 2026-09-18 第 87 轮:支持修饰键组合(cmd+f 微信搜索联系人)。
            ControlAction::SendKeys(expr) => {
                if let Some((keycode, flags)) = parse_key_combo(expr) {
                    super::cg_send_key_with_flags(keycode, flags);
                    Ok(format!(
                        "已通过 CGEvent 注入按键 \"{expr}\"(route=physical{})",
                        if flags != 0 { " + 修饰键" } else { "" }
                    ))
                } else {
                    Err(platform_err(
                        "macos",
                        format!("send_keys 表达式无法解析: {expr:?}(形如 \"enter\" / \"cmd+f\" / \"ctrl+shift+t\")"),
                    ))
                }
            }
            ControlAction::Scroll { lines } => {
                // 第 66 轮:CGEvent 滚轮。macOS 滚轮事件天然作用在光标下窗口,
                // 不需要先聚焦目标控件(应用惯例);但部分自绘控件要求先 hover,
                // 此处保持简单:直接滚。
                cg_scroll_lines(*lines);
                Ok(format!(
                    "已滚动 {} 行({})",
                    lines.abs(),
                    if *lines > 0 { "向上" } else { "向下" }
                ))
            }
            ControlAction::ScrollToVisible => {
                let err = AXUIElementPerformAction(el, kAXScrollToVisibleAction());
                if err == K_AX_ERROR_SUCCESS {
                    Ok(format!("已把 {window_id}{path} 滚动到可见区域"))
                } else {
                    Err(platform_err("macos", ax_error_text(err)))
                }
            }
            // ===== 第 67 轮:坐标动作(自绘 UI 视觉路线)=====
            ControlAction::ClickPoint { x, y, modifiers } => {
                let flags = cg_mod_flags(modifiers.as_deref())?;
                cg_click_at_ex(*x as f64, *y as f64, CgMouseButton::Left, 1, flags);
                Ok(format!(
                    "已在屏幕坐标 ({x},{y}) 执行物理左键单击{}(CGEvent,route=physical)",
                    cg_mod_note(modifiers)
                ))
            }
            ControlAction::DoubleClickPoint { x, y, modifiers } => {
                let flags = cg_mod_flags(modifiers.as_deref())?;
                cg_click_at_ex(*x as f64, *y as f64, CgMouseButton::Left, 2, flags);
                Ok(format!(
                    "已在屏幕坐标 ({x},{y}) 执行物理双击{}(CGEvent,clickState=2,route=physical)",
                    cg_mod_note(modifiers)
                ))
            }
            ControlAction::RightClickPoint { x, y, modifiers } => {
                let flags = cg_mod_flags(modifiers.as_deref())?;
                cg_click_at_ex(*x as f64, *y as f64, CgMouseButton::Right, 1, flags);
                Ok(format!(
                    "已在屏幕坐标 ({x},{y}) 执行物理右键单击{}(CGEvent,route=physical)",
                    cg_mod_note(modifiers)
                ))
            }
            // ===== 2026-09-19 第 90 轮:鼠标键盘原子能力 =====
            ControlAction::MovePoint { x, y } => {
                cg_move_cursor(*x as f64, *y as f64);
                Ok(format!("已把光标移动到 ({x},{y})(悬停,route=physical)"))
            }
            ControlAction::MiddleClickPoint { x, y, modifiers } => {
                let flags = cg_mod_flags(modifiers.as_deref())?;
                cg_click_at_ex(*x as f64, *y as f64, CgMouseButton::Middle, 1, flags);
                Ok(format!(
                    "已在屏幕坐标 ({x},{y}) 执行物理中键单击{}(CGEvent,route=physical)",
                    cg_mod_note(modifiers)
                ))
            }
            ControlAction::DragPoint {
                x,
                y,
                x2,
                y2,
                modifiers,
            } => {
                let flags = cg_mod_flags(modifiers.as_deref())?;
                cg_drag(*x as f64, *y as f64, *x2 as f64, *y2 as f64, flags);
                Ok(format!(
                    "已从 ({x},{y}) 拖拽到 ({x2},{y2}){}(CGEvent 插值,route=physical)",
                    cg_mod_note(modifiers)
                ))
            }
            ControlAction::ScrollPoint { x, y, lines } => {
                cg_move_cursor(*x as f64, *y as f64);
                std::thread::sleep(std::time::Duration::from_millis(60));
                cg_scroll_lines(*lines);
                Ok(format!(
                    "已在 ({x},{y}) 滚动 {} 行({})",
                    lines.abs(),
                    if *lines > 0 { "向上" } else { "向下" }
                ))
            }
            ControlAction::TypeText(text) => {
                cg_type_text(text);
                Ok(format!(
                    "已向当前焦点控件真实键入 {} 字符(CGEvent Unicode)",
                    text.chars().count()
                ))
            }
            // 第 80 轮:原子发送链路。可选坐标点击 → 完整 Unicode 键入 →
            // 等待 App 消费 → Enter,消除三次工具调用之间的焦点竞态。
            ControlAction::TypeTextSubmit { text, x, y } => {
                if let (Some(x), Some(y)) = (x, y) {
                    cg_click_at(*x as f64, *y as f64, false, false);
                } else {
                    // 控件树路径操作时先聚焦,保证 CGEvent 投递到目标控件。
                    let true_v: CFBooleanRef =
                        core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
                    let _ = AXUIElementSetAttributeValue(el, kAXFocusedAttribute(), true_v.cast());
                }
                cg_type_text(text);
                // WeChat/Electron 在收到 Unicode key-up 后才把文本入输入模型;
                // 120ms 足以吸收一次 App 内部 runloop,又几乎不感知延迟。
                std::thread::sleep(std::time::Duration::from_millis(120));
                cg_send_key(keycode_for_name("enter").unwrap_or(36));
                Ok(format!(
                    "已键入 {} 字符并提交(type_text_submit{})",
                    text.chars().count(),
                    match (x, y) {
                        (Some(x), Some(y)) => format!(", 坐标=({x},{y})"),
                        _ => String::new(),
                    }
                ))
            }
        };
        CFRelease(el);
        result
    }
}
