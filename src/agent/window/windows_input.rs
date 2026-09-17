//! Windows 物理输入底座(2026-09-16 第 67 轮):SendInput 鼠标 / 键盘注入。
//!
//! 背景:微信 4.x 等自绘 UI(MMUIRenderSubWindow)没有 UIA 控件,Invoke/ValuePattern/
//! WM_SETTEXT 全部无效;WM_MOUSEWHEEL 消息滚动也常被忽略。唯一可靠路径是
//! **模拟真实硬件输入**:`SetCursorPos` + `SendInput`(鼠标事件 / Unicode 键入)。
//!
//! 设计要点:
//! - 全部 Win32 OS API(`Win32_UI_Input_KeyboardAndMouse`),零 PowerShell / 零外部命令;
//! - **前台化三保险**:`IsIconic → SW_RESTORE` → `SetForegroundWindow`,失败时
//!   ALT 键预按 + `AttachThreadInput`(后台进程抢前台的经典解法);
//! - **DPI 感知**:`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` 一次初始化,
//!   保证 GetWindowRect / SetCursorPos / GDI 截图三者坐标同源(物理像素);
//! - 键入节拍:逐字符 5ms,防止自绘 UI 输入队列丢字;点击/键入之间留 40-80ms
//!   让目标应用处理焦点切换。

#![cfg(windows)]

use std::sync::OnceLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F10, VK_F11, VK_F12,
    VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_LEFT, VK_LMENU,
    VK_LWIN, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SetCursorPos, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

use super::platform_err;
use crate::error::Result;

/// 滚轮 1 行 = 120(WHEEL_DELTA)。
const WHEEL_DELTA: i32 = 120;

/// 进程级 DPI 感知初始化(一次;失败静默 —— 旧系统回退系统默认行为)。
pub(crate) fn ensure_dpi_aware() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // SAFETY:进程级一次性设置(OnceLock 保证无并发);失败不影响主流程。
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
    });
}

/// 把窗口带到前台:恢复最小化 + 三保险激活。
///
/// 返回 Ok 时窗口应已获得焦点;失败返回带引导的错误(调用方可降级为仅激活不报错)。
pub fn force_foreground(hwnd: HWND) -> Result<()> {
    ensure_dpi_aware();
    // SAFETY:以下均为按 HWND 的幂等 UI 调用;句柄非法时各 API 返回失败值,不崩溃。
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        if SetForegroundWindow(hwnd).as_bool() {
            std::thread::sleep(std::time::Duration::from_millis(60));
            return Ok(());
        }
        // 保险 1:ALT 键预按(让系统认为本进程收到过输入,放开 SetForegroundWindow 限制)
        send_key_raw(VK_LMENU, false);
        let ok = SetForegroundWindow(hwnd).as_bool();
        send_key_raw(VK_LMENU, true);
        if ok {
            std::thread::sleep(std::time::Duration::from_millis(60));
            return Ok(());
        }
        // 保险 2:AttachThreadInput(把本线程挂到前台窗口线程的输入队列)
        let fg = GetForegroundWindow();
        if !fg.is_invalid() {
            let mut fg_tid: u32 = 0;
            let mut target_tid_of_hwnd: u32 = 0;
            let _ = GetWindowThreadProcessId(fg, Some(&mut fg_tid));
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut target_tid_of_hwnd));
            let this_tid = windows::Win32::System::Threading::GetCurrentThreadId();
            let target_tid = if fg_tid != 0 { fg_tid } else { target_tid_of_hwnd };
            if target_tid != 0 && target_tid != this_tid {
                let attached = windows::Win32::System::Threading::AttachThreadInput(
                    this_tid,
                    target_tid,
                    true,
                )
                .as_bool();
                let ok2 = SetForegroundWindow(hwnd).as_bool();
                if attached {
                    let _ = windows::Win32::System::Threading::AttachThreadInput(
                        this_tid,
                        target_tid,
                        false,
                    );
                }
                if ok2 {
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    return Ok(());
                }
            }
        }
        Err(platform_err(
            "windows",
            "无法把窗口带到前台(SetForegroundWindow 三保险均失败);窗口可能被系统置顶锁/全屏应用占用",
        ))
    }
}

/// 发送单键(系统级,wVk 直填)。
///
/// # Safety
/// `vk` 必须是合法虚拟键码常量(本模块内部只传 VK_* 常量或解析后的单字母/数字)。
unsafe fn send_key_raw(vk: VIRTUAL_KEY, up: bool) {
    let flags: KEYBD_EVENT_FLAGS = if up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS::default()
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    if sent != 1 {
        tracing::warn!(?vk, up, "SendInput 键事件注入失败");
    }
}

/// 发送鼠标输入(系统级)。
///
/// # Safety
/// `mi` 的 dwFlags 必须是本模块定义的合法鼠标事件组合。
unsafe fn send_mouse(mi: MOUSEINPUT) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi },
    };
    let sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    if sent != 1 {
        tracing::warn!("SendInput 鼠标事件注入失败");
    }
}

/// 光标移到屏幕绝对坐标 (x,y)(物理像素,DPI 感知后与 OCR/GDI 坐标同源)。
pub fn move_cursor(x: i64, y: i64) -> Result<()> {
    ensure_dpi_aware();
    // SAFETY:简单坐标设置;失败(越界/安全策略)转结构化错误。
    unsafe {
        if SetCursorPos(x as i32, y as i32).is_ok() {
            Ok(())
        } else {
            Err(platform_err(
                "windows",
                format!("SetCursorPos({x},{y}) 失败(坐标可能越界)"),
            ))
        }
    }
}

/// 在 (x,y) 执行物理鼠标点击;`double` 双击 / `right` 右键。
///
/// 点击前会把光标移到目标点;按下/抬起间隔 30ms,双击间隔 80ms(贴近真人节拍,
/// 自绘 UI 依赖消息时序区分单击/双击)。
pub fn click_point(x: i64, y: i64, double: bool, right: bool) -> Result<()> {
    move_cursor(x, y)?;
    std::thread::sleep(std::time::Duration::from_millis(40));
    let (down, up) = if right {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };
    // SAFETY:固定合法鼠标事件标志组合。
    unsafe {
        for round in 0..(if double { 2 } else { 1 }) {
            send_mouse(MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: down,
                time: 0,
                dwExtraInfo: 0,
            });
            std::thread::sleep(std::time::Duration::from_millis(30));
            send_mouse(MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: up,
                time: 0,
                dwExtraInfo: 0,
            });
            if round == 0 && double {
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    Ok(())
}

/// 在 (x,y) 滚动滚轮:lines>0 向上,lines<0 向下。
pub fn wheel_at(x: i64, y: i64, lines: i32) -> Result<()> {
    move_cursor(x, y)?;
    std::thread::sleep(std::time::Duration::from_millis(30));
    let delta = lines.saturating_mul(WHEEL_DELTA);
    // SAFETY:MOUSEEVENTF_WHEEL + 有符号 delta(mouseData 按有符号解释)。
    unsafe {
        send_mouse(MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: delta as u32,
            dwFlags: MOUSEEVENTF_WHEEL,
            time: 0,
            dwExtraInfo: 0,
        });
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    Ok(())
}

/// 向当前焦点控件真实键入文本(KEYEVENTF_UNICODE,绕过 IME,中文直入)。
///
/// 逐字符 5ms 节拍 + 每字符 keydown/keyup 成对;上限 2000 字符防失控。
///
/// 2026-09-17 第 77 轮 P1-3:全部字符注入完成后等待 80ms,确保应用消费字符;
/// 否则后续立即调用的 `send_keys("enter")` 等操作可能在输入框还没接收完整
/// 文本时就把当前片段发送出去。LLM 调用范式:
/// `click_point(输入框) → type_text(完整消息) → send_keys("enter")`。
pub fn type_text(text: &str) -> Result<()> {
    let chars: Vec<char> = text.chars().take(2000).collect();
    if chars.is_empty() {
        return Ok(());
    }
    // SAFETY:Unicode 键入事件,scan 为合法 UTF-16 单元。
    unsafe {
        for c in chars {
            send_unicode_char(c as u16, false);
            send_unicode_char(c as u16, true);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // 2026-09-17 第 77 轮 P1-3:80ms 等待(对齐 macOS CGEvent 注入的等待时长),
        // 让目标应用输入事件循环完整消化字符,避免后续 send_keys 与残余字符竞争。
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    Ok(())
}

/// Unicode 单字符事件(压栈/弹起)。
///
/// # Safety
/// `scan` 必须是有效 UTF-16 码元。
unsafe fn send_unicode_char(scan: u16, up: bool) {
    let flags = if up {
        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
    } else {
        KEYEVENTF_UNICODE
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    if sent != 1 {
        tracing::warn!(scan, up, "SendInput Unicode 键入失败");
    }
}

/// 解析命名键 / 组合键规格 → (修饰键序列, 主键)。
///
/// 支持:
/// - 单键:`enter` / `esc` / `a` / `9` / `f5`;
/// - 组合键:`ctrl+enter` / `ctrl+a` / `alt+f4` / `ctrl+shift+s` / `win+d`
///   (修饰键按下 → 主键 → 逆序释放)。
pub fn parse_key_spec(spec: &str) -> Result<(Vec<VIRTUAL_KEY>, VIRTUAL_KEY)> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    if parts.len() > 5 {
        return Err(platform_err(
            "windows",
            format!("send_keys 组合键段数过多({}),最多 5 段(如 ctrl+shift+s)", parts.len()),
        ));
    }
    let mut modifiers = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            let main = key_vk(p).ok_or_else(|| {
                platform_err(
                    "windows",
                    format!(
                        "send_keys 无法识别按键名: {p:?}(支持 enter/tab/esc/space/delete/backspace/\
                         up/down/left/right/pageup/pagedown/home/end/f1-f12/a-z/0-9 与组合键 ctrl+enter 等)"
                    ),
                )
            })?;
            return Ok((modifiers, main));
        }
        modifiers.push(key_vk(p).ok_or_else(|| {
            platform_err("windows", format!("send_keys 组合键段无法识别: {p:?}"))
        })?);
    }
    Err(platform_err(
        "windows",
        "send_keys 规格为空(应形如 \"enter\" / \"ctrl+a\")",
    ))
}

/// 命名键 → 虚拟键码。
fn key_vk(name: &str) -> Option<VIRTUAL_KEY> {
    let l = name.trim().to_lowercase();
    Some(match l.as_str() {
        "enter" | "return" | "回车" => VK_RETURN,
        "tab" => VK_TAB,
        "esc" | "escape" => VK_ESCAPE,
        "space" | "空格" => VK_SPACE,
        "delete" | "del" => VK_DELETE,
        "backspace" | "bs" => VK_BACK,
        "up" | "arrowup" | "上" => VK_UP,
        "down" | "arrowdown" | "下" => VK_DOWN,
        "left" | "arrowleft" | "左" => VK_LEFT,
        "right" | "arrowright" | "右" => VK_RIGHT,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "home" => VK_HOME,
        "end" => VK_END,
        "win" | "meta" | "cmd" => VK_LWIN,
        "ctrl" | "control" => VK_CONTROL,
        "alt" | "option" => VK_LMENU,
        "shift" => VK_SHIFT,
        _ => {
            // F1-F12
            if let Some(num) = l.strip_prefix('f') {
                if let Ok(n) = num.parse::<u8>() {
                    return Some(match n {
                        1 => VK_F1,
                        2 => VK_F2,
                        3 => VK_F3,
                        4 => VK_F4,
                        5 => VK_F5,
                        6 => VK_F6,
                        7 => VK_F7,
                        8 => VK_F8,
                        9 => VK_F9,
                        10 => VK_F10,
                        11 => VK_F11,
                        12 => VK_F12,
                        _ => return None,
                    });
                }
                return None;
            }
            // 单字母 / 单数字
            let mut chars = l.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_lowercase() {
                    return Some(VIRTUAL_KEY(c as u16));
                }
                if c.is_ascii_digit() {
                    return Some(VIRTUAL_KEY(0x30 + (c as u16 - '0' as u16)));
                }
            }
            return None;
        }
    })
}

/// 发送按键规格(支持组合键);调用方应先 `force_foreground` 确保焦点在目标窗口。
pub fn send_keys_spec(spec: &str) -> Result<()> {
    let (modifiers, main) = parse_key_spec(spec)?;
    // SAFETY:全部为合法 VK 常量序列。
    unsafe {
        for m in &modifiers {
            send_key_raw(*m, false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        send_key_raw(main, false);
        std::thread::sleep(std::time::Duration::from_millis(20));
        send_key_raw(main, true);
        for m in modifiers.iter().rev() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            send_key_raw(*m, true);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_spec_single_named_keys() {
        assert_eq!(parse_key_spec("enter").unwrap().1, VK_RETURN);
        assert_eq!(parse_key_spec("ESC").unwrap().1, VK_ESCAPE);
        assert_eq!(parse_key_spec("f5").unwrap().1, VK_F5);
        assert_eq!(parse_key_spec("a").unwrap().1, VIRTUAL_KEY('a' as u16));
        assert_eq!(parse_key_spec("7").unwrap().1, VIRTUAL_KEY(0x37));
        assert!(parse_key_spec("enter").unwrap().0.is_empty());
    }

    #[test]
    fn parse_key_spec_combos() {
        let (mods, main) = parse_key_spec("ctrl+enter").unwrap();
        assert_eq!(mods, vec![VK_CONTROL]);
        assert_eq!(main, VK_RETURN);

        let (mods, main) = parse_key_spec("Ctrl+Shift+S").unwrap();
        assert_eq!(mods, vec![VK_CONTROL, VK_SHIFT]);
        assert_eq!(main, VIRTUAL_KEY('s' as u16));

        let (mods, main) = parse_key_spec("alt+f4").unwrap();
        assert_eq!(mods, vec![VK_LMENU]);
        assert_eq!(main, VK_F4);
    }

    #[test]
    fn parse_key_spec_rejects_unknown() {
        assert!(parse_key_spec("notakey").is_err());
        assert!(parse_key_spec("f13").is_err());
        assert!(parse_key_spec("multiword").is_err());
        // 段数上限
        assert!(parse_key_spec("a+b+c+d+e+f").is_err());
    }
}
