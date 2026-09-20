//! CGEvent 输入底座(2026-09-20 第 97 轮从 macos_legacy.rs 拆分)。
//!
//! - 键码解析:`keycode_for_name` / `parse_key_combo` / `parse_modifier_flags`;
//! - 修饰键:`modifier_flag_for_name` / `cg_mod_flags` / `cg_mod_note`
//!   (CGEventFlags 位,cmd+f / ctrl+shift+t 等组合键);
//! - 输入原语:`cg_move_cursor` / `cg_scroll_lines` / `cg_send_key[_with_flags]` /
//!   `cg_click_at[_ex]` / `cg_drag` / `cg_type_text`(逐字符 CGEvent 注入)。
//!
//! 跨平台一致语义(2026-09-19 第 90 轮):
//!   控制类操作 **无障碍优先 / Windows 消息优先,物理鼠标键盘兜底**;
//!   自绘 UI(微信 4.x / Electron canvas / 游戏)天然走物理路线。

#![allow(non_snake_case)]

// 从父模块 mod.rs 取用 FFI 函数 / 常量 / 类型。
use super::{
    platform_err, CFRelease, CGEventCreateKeyboardEvent, CGEventCreateMouseEvent,
    CGEventCreateScrollWheelEvent, CGEventKeyboardSetUnicodeString, CGEventPost, CGEventSetFlags,
    CGEventSetIntegerValueField, CGPoint, K_CG_EVENT_LEFT_MOUSE_DOWN, K_CG_EVENT_LEFT_MOUSE_UP,
    K_CG_EVENT_MOUSE_MOVED, K_CG_EVENT_OTHER_MOUSE_DOWN, K_CG_EVENT_OTHER_MOUSE_UP,
    K_CG_EVENT_RIGHT_MOUSE_DOWN, K_CG_EVENT_RIGHT_MOUSE_UP, K_CG_HID_EVENT_TAP,
    K_CG_MOUSE_BUTTON_CENTER, K_CG_MOUSE_BUTTON_LEFT, K_CG_MOUSE_BUTTON_RIGHT,
    K_CG_MOUSE_EVENT_CLICK_STATE, K_CG_SCROLL_EVENT_UNIT_LINE,
};
use crate::error::AgentError;

/// 命名键 → macOS 虚拟键码(kVK_*,来源 HIToolbox/Events.h)。
pub(super) fn keycode_for_name(name: &str) -> Option<u16> {
    Some(match name.trim().to_lowercase().as_str() {
        "enter" | "return" | "回车" => 36,
        "tab" => 48,
        "esc" | "escape" => 53,
        "space" | "空格" => 49,
        "delete" | "backspace" | "退格" => 51,
        "forwarddelete" | "del" => 117,
        "up" | "arrowup" => 126,
        "down" | "arrowdown" => 125,
        "left" | "arrowleft" => 123,
        "right" | "arrowright" => 124,
        "pageup" => 116,
        "pagedown" => 121,
        "home" => 115,
        "end" => 119,
        // 2026-09-18 第 87 轮:字母 / 数字 ANSI 键码(kVK_ANSI_*),
        // 支撑 cmd+f(微信搜索)等修饰键组合。
        "a" => 0x00,
        "s" => 0x01,
        "d" => 0x02,
        "f" => 0x03,
        "h" => 0x04,
        "g" => 0x05,
        "z" => 0x06,
        "x" => 0x07,
        "c" => 0x08,
        "v" => 0x09,
        "b" => 0x0B,
        "q" => 0x0C,
        "w" => 0x0D,
        "e" => 0x0E,
        "r" => 0x0F,
        "y" => 0x10,
        "t" => 0x11,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "9" => 0x19,
        "7" => 0x1A,
        "8" => 0x1C,
        "0" => 0x1D,
        "o" => 0x1F,
        "u" => 0x20,
        "i" => 0x22,
        "p" => 0x23,
        "l" => 0x25,
        "j" => 0x26,
        "k" => 0x28,
        "n" => 0x2D,
        "m" => 0x2E,
        _ => return None,
    })
}

/// 修饰键名 → CGEventFlags 位(2026-09-18 第 87 轮)。
///
/// 值来源 `<CoreGraphics/CGEventTypes.h>`:kCGEventFlagMaskShift/Control/Alternate/Command。
pub(super) fn modifier_flag_for_name(name: &str) -> Option<u64> {
    Some(match name.trim().to_lowercase().as_str() {
        "shift" | "⇧" => 0x0002_0000,
        "ctrl" | "control" | "⌃" => 0x0004_0000,
        "alt" | "option" | "opt" | "⌥" => 0x0008_0000,
        "cmd" | "command" | "meta" | "⌘" => 0x0010_0000,
        _ => return None,
    })
}

/// 解析按键表达式:`cmd+f` / `ctrl+shift+t` / `enter` 等。
///
/// `+` 分隔,末段为键名,前缀为修饰键(可多个);无修饰键时 flags=0。
/// 返回 `(keycode, flags)`;任何一段无法识别返回 None。
pub(super) fn parse_key_combo(expr: &str) -> Option<(u16, u64)> {
    let parts: Vec<&str> = expr
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let (key_part, mod_parts) = parts.split_last()?;
    let keycode = keycode_for_name(key_part)?;
    let mut flags = 0u64;
    for m in mod_parts {
        flags |= modifier_flag_for_name(m)?;
    }
    Some((keycode, flags))
}

/// 解析**纯修饰键**规格 → CGEventFlags(2026-09-19 第 90 轮,修饰键 + 鼠标操作)。
///
/// 与 `parse_key_combo` 的区别:每一段都必须是修饰键(ctrl/shift/alt/cmd 及别名),
/// 无主键;任一段非修饰键返回 None。macOS 修饰键以 flags 位形式与鼠标事件同时携带。
pub(super) fn parse_modifier_flags(spec: &str) -> Option<u64> {
    let parts: Vec<&str> = spec
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let mut flags = 0u64;
    for p in parts {
        flags |= modifier_flag_for_name(p)?;
    }
    Some(flags)
}

/// 修饰键规格 → CGEventFlags(空规格 = 0;非法段结构化报错,2026-09-19 第 90 轮)。
pub(super) fn cg_mod_flags(spec: Option<&str>) -> Result<u64, AgentError> {
    match spec.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => parse_modifier_flags(s).ok_or_else(|| {
            platform_err(
                "macos",
                format!(
                    "modifiers 规格非法: {s:?}(仅允许 ctrl/shift/alt/cmd(+组合),如 \"ctrl\" / \"cmd+shift\")"
                ),
            )
        }),
        None => Ok(0),
    }
}

/// 修饰键规格 → 返回文案后缀(无修饰键为空串,第 90 轮)。
pub(super) fn cg_mod_note(spec: &Option<String>) -> String {
    spec.as_deref()
        .map(|m| format!(" + 按住 {m}"))
        .unwrap_or_default()
}

/// 把光标移到屏幕坐标(x,y)(CGEvent mouse-moved,无点击)。
pub(super) unsafe fn cg_move_cursor(x: f64, y: f64) {
    let ev = CGEventCreateMouseEvent(
        std::ptr::null(),
        K_CG_EVENT_MOUSE_MOVED,
        CGPoint { x, y },
        0,
    );
    if !ev.is_null() {
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
    }
}

/// 在光标当前位置注入滚轮事件:lines>0 向上,lines<0 向下(单位:行)。
/// 分片投递(每片 ≤3 行 + 30ms 间隔),避免一次性大 delta 被应用按「甩尾」处理跳屏。
pub(super) unsafe fn cg_scroll_lines(lines: i32) {
    let mut remaining = lines;
    while remaining != 0 {
        let step = remaining.clamp(-3, 3);
        let ev =
            CGEventCreateScrollWheelEvent(std::ptr::null(), K_CG_SCROLL_EVENT_UNIT_LINE, 1, step);
        if ev.is_null() {
            break;
        }
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
        remaining -= step;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

/// 注入一次按键(keydown + keyup,间隔 20ms)。
pub(super) unsafe fn cg_send_key(keycode: u16) {
    cg_send_key_with_flags(keycode, 0);
}

/// 注入一次带修饰键的按键(2026-09-18 第 87 轮):
/// keydown/keyup 事件均 `CGEventSetFlags(flags)`,支撑 cmd+f 等组合键。
pub(super) unsafe fn cg_send_key_with_flags(keycode: u16, flags: u64) {
    for keydown in [true, false] {
        let ev = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, keydown);
        if ev.is_null() {
            return;
        }
        if flags != 0 {
            CGEventSetFlags(ev, flags);
        }
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
        if keydown {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// 在屏幕坐标 (x,y) 注入物理鼠标点击(2026-09-17 第 70 轮,坐标动作视觉路线)。
/// 先移光标到位再按压;双击时第二次点击 clickState=2(应用按该字段判定双击语义)。
pub(super) unsafe fn cg_click_at(x: f64, y: f64, right: bool, double: bool) {
    cg_click_at_ex(
        x,
        y,
        if right {
            CgMouseButton::Right
        } else {
            CgMouseButton::Left
        },
        if double { 2 } else { 1 },
        0,
    );
}

/// CGEvent 鼠标按键(2026-09-19 第 90 轮,cg_click_at_ex 参数)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CgMouseButton {
    Left,
    Right,
    Middle,
}

/// 任意按键 + 1~2 次点击 + 修饰键的完整鼠标点击原语(2026-09-19 第 90 轮)。
///
/// `flags` 为 CGEventFlags 修饰键位(`parse_modifier_flags` 产出),按下/抬起
/// 事件均携带 —— 实现 ctrl+点击 / shift+点击等「键盘修饰键 + 鼠标」同时操作。
pub(super) unsafe fn cg_click_at_ex(x: f64, y: f64, button: CgMouseButton, clicks: u8, flags: u64) {
    let clicks = clicks.clamp(1, 2);
    let (down, up, btn) = match button {
        CgMouseButton::Left => (
            K_CG_EVENT_LEFT_MOUSE_DOWN,
            K_CG_EVENT_LEFT_MOUSE_UP,
            K_CG_MOUSE_BUTTON_LEFT,
        ),
        CgMouseButton::Right => (
            K_CG_EVENT_RIGHT_MOUSE_DOWN,
            K_CG_EVENT_RIGHT_MOUSE_UP,
            K_CG_MOUSE_BUTTON_RIGHT,
        ),
        CgMouseButton::Middle => (
            K_CG_EVENT_OTHER_MOUSE_DOWN,
            K_CG_EVENT_OTHER_MOUSE_UP,
            K_CG_MOUSE_BUTTON_CENTER,
        ),
    };
    cg_move_cursor(x, y);
    std::thread::sleep(std::time::Duration::from_millis(60));
    for seq in 1..=clicks {
        for &mouse_type in &[down, up] {
            let ev = CGEventCreateMouseEvent(std::ptr::null(), mouse_type, CGPoint { x, y }, btn);
            if ev.is_null() {
                return;
            }
            if clicks > 1 {
                CGEventSetIntegerValueField(ev, K_CG_MOUSE_EVENT_CLICK_STATE, seq as i64);
            }
            if flags != 0 {
                CGEventSetFlags(ev, flags);
            }
            CGEventPost(K_CG_HID_EVENT_TAP, ev);
            CFRelease(ev);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if clicks > 1 {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
}

/// 从 (fx,fy) 按住左键拖拽到 (tx,ty)(2026-09-19 第 90 轮),可选修饰键。
///
/// 实现:移到起点 → 左键按下(携带修饰 flags)→ **10 步线性插值 mouseMoved**
/// (每步 15ms,拖拽启动阈值依赖移动序列)→ 左键抬起。macOS 修饰键以 flags
/// 形式挂在每个鼠标事件上(与 cliclick 同款做法)。
pub(super) unsafe fn cg_drag(fx: f64, fy: f64, tx: f64, ty: f64, flags: u64) {
    const STEPS: i64 = 10;
    cg_move_cursor(fx, fy);
    std::thread::sleep(std::time::Duration::from_millis(60));
    // 按下(带修饰键)
    let ev = CGEventCreateMouseEvent(
        std::ptr::null(),
        K_CG_EVENT_LEFT_MOUSE_DOWN,
        CGPoint { x: fx, y: fy },
        K_CG_MOUSE_BUTTON_LEFT,
    );
    if !ev.is_null() {
        if flags != 0 {
            CGEventSetFlags(ev, flags);
        }
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    // 插值移动(kCGEventMouseMoved,保持修饰 flags)
    for i in 1..=STEPS {
        let nx = fx + (tx - fx) * (i as f64) / (STEPS as f64);
        let ny = fy + (ty - fy) * (i as f64) / (STEPS as f64);
        let mv = CGEventCreateMouseEvent(
            std::ptr::null(),
            K_CG_EVENT_MOUSE_MOVED,
            CGPoint { x: nx, y: ny },
            0,
        );
        if !mv.is_null() {
            if flags != 0 {
                CGEventSetFlags(mv, flags);
            }
            CGEventPost(K_CG_HID_EVENT_TAP, mv);
            CFRelease(mv);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
    // 抬起(修饰键随事件释放)
    let up = CGEventCreateMouseEvent(
        std::ptr::null(),
        K_CG_EVENT_LEFT_MOUSE_UP,
        CGPoint { x: tx, y: ty },
        K_CG_MOUSE_BUTTON_LEFT,
    );
    if !up.is_null() {
        CGEventPost(K_CG_HID_EVENT_TAP, up);
        CFRelease(up);
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
}

/// 向当前焦点控件真实键入文本(2026-09-17 第 70 轮;第 81 轮改**逐字符**注入)。
///
/// 第 81 轮根因修复(用户实测「只输入了 i 字符」):旧实现一次
/// `CGEventKeyboardSetUnicodeString` 携带 20 个 UTF-16 单元,微信 macOS 等
/// 自绘输入框对单个 keyDown 事件**只消费首个字符**,后续字符全部丢失。
/// 逐字符注入(每字符一对 keyDown/keyUp)是 cliclick / Appium mac2 driver
/// 的同款稳健路径,任何按 `NSEvent.characters` 消费的应用都能完整接收。
///
/// - 每字符一个事件对,字间隔 10ms(200 字 ≈ 2.4s,正确性优先);
/// - 尾部保留 80ms 消化等待(第 77 轮 P1-3 修复,避免与后续 Enter 竞争)。
pub(super) unsafe fn cg_type_text(text: &str) {
    for ch in text.chars() {
        let mut units = [0u16; 2];
        let len = ch.encode_utf16(&mut units).len();
        for &keydown in &[true, false] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), 0, keydown);
            if ev.is_null() {
                return;
            }
            CGEventKeyboardSetUnicodeString(ev, len as isize, units.as_ptr());
            CGEventPost(K_CG_HID_EVENT_TAP, ev);
            CFRelease(ev);
            if keydown {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    // 全部字符注入完成后等待 80ms,让目标应用的输入事件循环完整消化,
    // 避免后续 send_keys("enter") 与残余字符竞争导致输入不完整。
    // (经验值:微信 macOS 输入框事件循环周期约 50ms,80ms 留 30ms 余量)
    std::thread::sleep(std::time::Duration::from_millis(80));
}
