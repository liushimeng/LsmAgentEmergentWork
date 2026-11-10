//! Windows 有头帧查看器:Win32(GDI `StretchDIBits` + 原生工具栏控件)。
//!
//! 只在 `laew __browser-view` 子进程内运行(与 `__hitl-dialog` 同一模式)。
//! 窗口客户区自上而下 = 一级菜单栏(58 号,DevTools 十按钮)+ 工具栏
//! (后退/前进/刷新/首页 + 地址栏)+ 视口画面 + DevTools 面板(展开时贴底,
//! 页面区收缩,Chrome dock-to-bottom 语义);
//! PNG 帧经 image crate 解码为 RGBA 后以自顶向下 32bpp DIB **1:1 绘制**
//! (不拉伸:宿主按内容区尺寸重渲染,新帧尺寸 == 内容区尺寸 → 铺满且不变形)。
//! 100ms `WM_TIMER` 轮询共享帧槽(与 macOS 侧同构)。
//!
//! **人工拖动窗口**(视口自适应):`WM_SIZE` → 读客户区尺寸 → 变了就重排工具栏
//! 子控件并向宿主上报 `{"cmd":"resize","w":..,"h":..}`(内容区扣掉三层 chrome)。
//!
//! 工具栏交互经 stdout 命令通道驱动宿主引擎(与 macOS 侧协议一致);
//! DevTools 面板状态/快照合并/过滤逻辑在 `view::devtools_common`(双平台共用)。

use std::sync::Mutex;

use windows::core::w;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetMonitorInfoW,
    InvalidateRect, MonitorFromWindow, ScreenToClient, SelectObject, SetBkMode, SetTextColor,
    StretchDIBits, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_QUALITY, DIB_RGB_COLORS, FF_DONTCARE, FW_NORMAL, HDC, HFONT, HGDIOBJ,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, GetKeyState, SetFocus};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetClientRect, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, LoadCursorW, MoveWindow, PostQuitMessage,
    RegisterClassW, SendMessageW, SetCursor, SetTimer, SetWindowTextW, SetWindowLongPtrW,
    ShowWindow, TrackPopupMenu, TranslateMessage, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX,
    BS_PUSHBUTTON, CS_DBLCLKS, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, ES_AUTOHSCROLL, GetDlgCtrlID, GWL_STYLE, HMENU, IDC_ARROW,
    IDC_HAND, IDC_IBEAM,
    MF_STRING, MSG, SW_HIDE, SW_SHOWNORMAL, TPM_RIGHTBUTTON, TPM_RETURNCMD, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_CHAR, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN,
    WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE, WM_SYSKEYDOWN, WM_TIMER, WM_XBUTTONDOWN, WNDCLASSW,
    WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
};
// EM_SETSEL 定义在 Controls 模块(不在 WindowsAndMessaging;39 号 P2b)。
use windows::Win32::UI::Controls::EM_SETSEL;

pub(super) mod devtools;
use devtools::*;

use super::devtools_common as dt;
use super::read_frame;

/// 解码后的帧(自顶向下 RGBA)。
struct Frame {
    rgba: Vec<u8>,
    width: i32,
    height: i32,
}

/// HWND 是裸指针,不能跨线程共享;这里只做容器传递(实际读写都在窗口线程)。
#[derive(Clone, Copy)]
struct HwndCell(HWND);
unsafe impl Send for HwndCell {
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
static PENDING_TITLE: Mutex<Option<String>> = Mutex::new(None);
static CLOSE_FLAG: Mutex<bool> = Mutex::new(false);
/// 帧槽:读线程写 PNG 原始字节,WM_TIMER 解码。
static TAKE_IMAGE: Mutex<Option<Vec<u8>>> = Mutex::new(None);
/// 地址栏状态(URL + 前进/后退可用性)。
static PENDING_URL: Mutex<Option<(String, bool, bool)>> = Mutex::new(None);
/// ⌘/Ctrl+L 聚焦地址栏请求(39 号 P2b)。
static FOCUS_ADDRBAR_FLAG: Mutex<bool> = Mutex::new(false);
/// P2c:光标形状(`"default"|"pointer"|"text"`),WM_SETCURSOR 读取。
// 修复(2026-11-06):静态初始化不能用非 const 的 `to_string()`——Windows
// 目标下此前根本编不过(P2c 轮引入,本机 macOS 从不编译该文件所以未暴露)。
// `LazyLock` 用法与 `Mutex` 直连完全一致(Deref 透传)。
static CURSOR_KIND: std::sync::LazyLock<Mutex<String>> =
    std::sync::LazyLock::new(|| Mutex::new("default".to_string()));
static ADDR_EDIT: Mutex<Option<HwndCell>> = Mutex::new(None);
static BACK_BTN: Mutex<Option<HwndCell>> = Mutex::new(None);
static FWD_BTN: Mutex<Option<HwndCell>> = Mutex::new(None);
/// 刷新/首页按钮(人工拖动窗口后需按新宽度重排)。
static RELOAD_BTN: Mutex<Option<HwndCell>> = Mutex::new(None);
static HOME_BTN: Mutex<Option<HwndCell>> = Mutex::new(None);
/// 上一次已知的客户区尺寸(像素);变了才重排 + 上报宿主。
static LAST_SIZE: Mutex<Option<(i32, i32)>> = Mutex::new(None);
/// P2e:F11 全屏前的窗口样式与位置(还原用;None = 当前非全屏)。
static FULLSCREEN_SAVE: Mutex<Option<(u32, RECT)>> = Mutex::new(None);
/// 本查看器认知里的视口尺寸(键鼠坐标换算基线;初始来自 argv,随 resize 同步)。
static VIEWPORT: Mutex<(i32, i32)> = Mutex::new((0, 0));
/// 上次已上报的 move(节流:≥30ms 且位移 ≥1px 才再发)。
static LAST_MOVE: Mutex<Option<(std::time::Instant, f64, f64)>> = Mutex::new(None);
/// P2d:人工缩放系数(Ctrl + =/-/0;查看器侧生效,宿主只记账)。
static ZOOM: Mutex<f64> = Mutex::new(1.0);
/// P2d:宿主最近下发的原始标题(缩放后缀叠加其上)。
static LAST_TITLE: Mutex<String> = Mutex::new(String::new());
/// 右键原生菜单是否关闭(`LAEW_HUMAN_RMENU=off` → 回退 contextmenu 事件路径)。
static RMENU_OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
/// 右键菜单是否接管(接管期间 up 不再上报 contextmenu)。
static RMENU_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn rmenu_disabled() -> bool {
    *RMENU_OFF.get_or_init(|| {
        matches!(
            std::env::var("LAEW_HUMAN_RMENU")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

/// 右键原生菜单(后退/前进/刷新/首页/复制/粘贴),选择经命令通道驱动引擎。
unsafe fn show_context_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    // (id, 标题, 命令)
    let items: [(usize, &str, &str); 6] = [
        (101, "后退", "{\"cmd\":\"back\"}"),
        (102, "前进", "{\"cmd\":\"forward\"}"),
        (103, "刷新", "{\"cmd\":\"reload\"}"),
        (104, "首页", "{\"cmd\":\"home\"}"),
        (105, "复制", "{\"cmd\":\"clip\",\"act\":\"copy\"}"),
        (106, "粘贴", "{\"cmd\":\"clip\",\"act\":\"paste\"}"),
    ];
    for (id, title, _) in items {
        let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(to_wide(title).as_ptr()));
    }
    let mut pt = windows::Win32::Foundation::POINT::default();
    if GetCursorPos(&mut pt).is_ok() {
        let picked = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        if let Some((_, _, cmd)) = items.iter().find(|(id, _, _)| *id == picked.0 as usize) {
            send_cmd(cmd);
        }
    }
    let _ = DestroyMenu(menu);
}

const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 100;

/// 工具栏高度(像素)。
const TOOLBAR_H: i32 = 38;
/// 按钮高度。
const BTN_H: i32 = 24;
/// 按钮宽度。
const BTN_W: i32 = 52;
/// 按钮间距。
const BTN_GAP: i32 = 6;
/// 工具栏左右留白。
const PAD: i32 = 8;

const ID_BACK: i32 = 101;
const ID_FORWARD: i32 = 102;
const ID_RELOAD: i32 = 103;
const ID_HOME: i32 = 104;
const ID_EDIT: i32 = 110;



fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 向宿主发送一条命令(查看器 → 引擎)。
fn send_cmd(line: &str) {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
    let _ = lock.flush();
}

/// 读取地址栏文本。
fn address_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 1024];
    unsafe {
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

// ---------- 人类键鼠输入(39 号设计文档,与 macOS 侧同协议) ----------

/// 输入上报是否被 `LAEW_HUMAN_INPUT=off` 关闭。
fn input_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        matches!(
            std::env::var("LAEW_HUMAN_INPUT")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

/// 修饰键状态(GetKeyState 高位 = 按下)。
unsafe fn mods_state() -> (bool, bool, bool, bool) {
    let down = |vk: i32| GetKeyState(vk) as u16 & 0x8000 != 0;
    (down(0x10), down(0x11), down(0x12), false) // shift/ctrl/alt;Windows 无 meta
}

/// 客户区坐标 → 视口坐标(画面 1:1 居中,与 paint 同一换算;越界返回 None)。
fn client_to_page(hwnd: HWND, x: i32, y: i32) -> Option<(f64, f64)> {
    let (cw, ch) = unsafe { client_size(hwnd) }?;
    // 页面区 = 客户区 - 一级菜单栏 - 工具栏 - DevTools 面板(58 号)。
    let view_h = ch - MENU_H - TOOLBAR_H - panel_h();
    if view_h <= 0 {
        return None;
    }
    // 帧尺寸优先(当前真画面),缺帧时回落认知视口。
    let (fw, fh) = {
        let frame = FRAME.lock().unwrap();
        match frame.as_ref() {
            Some(f) => (f.width, f.height),
            None => {
                let (w, h) = *VIEWPORT.lock().unwrap();
                if w <= 0 || h <= 0 {
                    return None;
                }
                (w, h)
            }
        }
    };
    // P2d:帧等比缩放居中(缩放系数体现在帧尺寸 vs 内容区尺寸的比例),
    // 坐标按同一变换逆映射到帧(页面)坐标系。
    if fw <= 0 || fh <= 0 {
        return None;
    }
    let scale = (cw as f64 / fw as f64).min(view_h as f64 / fh as f64);
    if scale <= 0.0 {
        return None;
    }
    let dw = fw as f64 * scale;
    let dh = fh as f64 * scale;
    let dx = (cw as f64 - dw) / 2.0;
    let dy = (MENU_H + TOOLBAR_H) as f64 + (view_h as f64 - dh) / 2.0;
    let px = (x as f64 - dx) / scale;
    let py = (y as f64 - dy) / scale;
    if px < 0.0 || py < 0.0 || px >= fw as f64 || py >= fh as f64 {
        return None;
    }
    Some((px, py))
}

fn move_allowed(x: f64, y: f64) -> bool {
    let mut last = LAST_MOVE.lock().unwrap();
    match *last {
        Some((at, lx, ly)) => {
            let due =
                at.elapsed().as_millis() >= 30 && ((lx - x).abs() >= 1.0 || (ly - y).abs() >= 1.0);
            if due {
                *last = Some((std::time::Instant::now(), x, y));
            }
            due
        }
        None => {
            *last = Some((std::time::Instant::now(), x, y));
            true
        }
    }
}

/// 指针事件公共出口。
fn forward_pointer(hwnd: HWND, x: i32, y: i32, act: &str, btn: Option<&str>, is_move: bool) {
    if input_disabled() {
        return;
    }
    let Some((px, py)) = client_to_page(hwnd, x, y) else {
        return;
    };
    if is_move && !move_allowed(px, py) {
        return;
    }
    let line = match btn {
        Some(b) => {
            format!("{{\"cmd\":\"input\",\"act\":\"{act}\",\"x\":{px:.0},\"y\":{py:.0},\"btn\":\"{b}\"}}")
        }
        None => format!("{{\"cmd\":\"input\",\"act\":\"{act}\",\"x\":{px:.0},\"y\":{py:.0}}}"),
    };
    send_cmd(&line);
}

/// lParam 的客户区坐标(带符号扩展,多显示器负坐标正确)。
fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    let raw = lp.0 as u64;
    let x = (raw & 0xFFFF) as u16 as i16 as i32;
    let y = ((raw >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// 虚拟键码 → 浏览器键名(特殊键);可打印字符走 WM_CHAR。
fn vk_special(vk: u32) -> Option<&'static str> {
    Some(match vk {
        0x0D => "Enter",
        0x09 => "Tab",
        0x1B => "Escape",
        0x08 => "Backspace",
        0x2E => "Delete",
        0x25 => "ArrowLeft",
        0x26 => "ArrowUp",
        0x27 => "ArrowRight",
        0x28 => "ArrowDown",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x20 => "Space",
        _ => return None,
    })
}

fn send_key_json(key: &str, mods: (bool, bool, bool, bool)) {
    let line = format!(
        "{{\"cmd\":\"input\",\"act\":\"key\",\"key\":\"{key}\",\"mods\":{{\"shift\":{},\"ctrl\":{},\"alt\":{},\"meta\":false}}}}",
        mods.0, mods.1, mods.2
    );
    send_cmd(&line);
}

/// 键按下:浏览器级快捷键本地消化(Ctrl+R/F5 刷新、标签页三键、剪贴板三键
/// 转发);特殊键转发;可打印键等 WM_CHAR(避免 KEYDOWN+CHAR 双发)。
/// `sys` = Alt 组合(Alt+←/→ 前进后退)。
unsafe fn forward_key(hwnd: HWND, wp: WPARAM, sys: bool) {
    if input_disabled() {
        return;
    }
    let vk = (wp.0 & 0xFFFF) as u32;
    let (shift, ctrl, alt, _) = mods_state();
    if ctrl && vk == 0x52 || vk == 0x74 {
        send_cmd("{\"cmd\":\"reload\"}");
        return;
    }
    if sys {
        if alt && vk == 0x25 {
            send_cmd("{\"cmd\":\"back\"}");
        } else if alt && vk == 0x27 {
            send_cmd("{\"cmd\":\"forward\"}");
        }
        return;
    }
    // 标签页快捷键(39 号设计文档阶段二)。
    if ctrl && vk == 0x54 {
        send_cmd("{\"cmd\":\"new_tab\"}");
        return;
    }
    if ctrl && vk == 0x57 {
        send_cmd("{\"cmd\":\"close_tab\"}");
        return;
    }
    if ctrl && vk == 0x09 {
        send_cmd("{\"cmd\":\"next_tab\"}");
        return;
    }
    // ⌘/Ctrl+L:聚焦地址栏(39 号 P2b;Windows 无 meta)。
    if ctrl && vk == 0x4C {
        send_cmd("{\"cmd\":\"focus_addrbar\"}");
        return;
    }
    // Ctrl+F:页面查找(39 号 P2c;浏览器级快捷键,不进页面)。
    if ctrl && vk == 0x46 {
        send_cmd("{\"cmd\":\"find_toggle\"}");
        return;
    }
    // P2d:浏览器级缩放 —— Ctrl + =/数字键盘+ 放大(0xBB/0x6B)、
    // -/数字键盘- 缩小(0xBD/0x6D)、0 复位(0x30);Chrome 语义。
    if ctrl && matches!(vk, 0xBB | 0x6B | 0xBD | 0x6D | 0x30) {
        let dir = match vk {
            0xBD | 0x6D => -1,
            0x30 => 0,
            _ => 1,
        };
        apply_zoom(dir, hwnd);
        return;
    }
    // P2e:F11(VK_F11 = 0x7A)全屏切换 —— 无边框铺满所在显示器,再按还原;
    // 浏览器级快捷键不进页面(与 Chrome 行为一致)。
    if vk == 0x7A {
        toggle_fullscreen(hwnd);
        return;
    }
    if ctrl || alt {
        // 组合键只转发剪贴板三键(宿主侧按聚焦控件整值处理)。
        if ctrl && (vk == 0x43 || vk == 0x56 || vk == 0x58) {
            let k = match vk {
                0x43 => "c",
                0x56 => "v",
                _ => "x",
            };
            send_key_json(k, (shift, ctrl, alt, false));
        }
        return; // 其余组合键不产生页面文本
    }
    if let Some(k) = vk_special(vk) {
        send_key_json(k, (shift, ctrl, alt, false));
    }
    // 可打印键:WM_CHAR 负责发送(WM_KEYDOWN 不发,防双发)。
}

/// WM_CHAR:可打印字符转发(跳过控制字符/代理对;组合键已在 KEYDOWN 过滤)。
unsafe fn forward_char(wp: WPARAM) {
    if input_disabled() {
        return;
    }
    let unit = (wp.0 & 0xFFFF) as u32;
    // 控制字符与 UTF-16 代理对(IME 组合输入属阶段二)不转发。
    if unit < 0x20 || unit == 0x7F || (0xD800..=0xDFFF).contains(&unit) {
        return;
    }
    let Some(c) = char::from_u32(unit) else { return };
    let (shift, ctrl, alt, _) = mods_state();
    if ctrl || alt {
        return;
    }
    // 键内容只可能是简单 BMP 可打印字符,无需复杂转义;保守处理引号/反斜杠。
    let escaped = match c {
        '"' => "\\\"".to_string(),
        '\\' => "\\\\".to_string(),
        c => c.to_string(),
    };
    send_key_json(&escaped, (shift, ctrl, alt, false));
}

/// 滚轮:wParam 高 16 位 = 档位(±120/档);lParam 为**屏幕**坐标需转客户区。
unsafe fn forward_wheel(hwnd: HWND, wp: WPARAM, lp: LPARAM, horizontal: bool) {
    if input_disabled() {
        return;
    }
    let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
    if delta == 0 {
        return;
    }
    // P2e:Shift+纵滚 = 横向滚动(Chrome 语义:上滚 = 向左滚,即 dx 取反号)。
    // 系统对触控板横滚已走 WM_MOUSEHWHEEL 不经此分支;经典鼠标 Shift+滚轮
    // 在 Windows 原生仍是纵向 WM_MOUSEWHEEL,这里按 MK_SHIFT(0x0004)转横。
    let shift = (wp.0 & 0xFFFF) & 0x0004 != 0;
    let horizontal = horizontal || shift;
    let (sx, sy) = lparam_xy(lp);
    let mut pt = windows::Win32::Foundation::POINT { x: sx, y: sy };
    if !ScreenToClient(hwnd, &mut pt).as_bool() {
        return;
    }
    let Some((px, py)) = client_to_page(hwnd, pt.x, pt.y) else {
        // 坐标越界仍允许滚动(滚轮作用整页;宿主按视口滚动兜底)。
        return;
    };
    // P2d:滚轮档位 ÷ 缩放(视觉滚动距离与缩放无关)。
    let z = (*ZOOM.lock().unwrap()).max(0.05);
    let (dx, dy) = if horizontal {
        let d = if shift { -(delta as f64) } else { delta as f64 };
        (d / 120.0 / z, 0.0)
    } else {
        (0.0, delta as f64 / 120.0 / z)
    };
    let line = format!(
        "{{\"cmd\":\"input\",\"act\":\"wheel\",\"x\":{px:.0},\"y\":{py:.0},\"dx\":{dx:.3},\"dy\":{dy:.3}}}"
    );
    send_cmd(&line);
}

/// P2e:F11 全屏切换。进入:记录原窗口样式与位置 → `WS_POPUP` 无边框铺满
/// 窗口所在显示器;再次按下还原记录值。客户区变化经既有 `WM_SIZE` 链
/// 重排工具栏并上报宿主新视口(与人工拖动窗口同一条路径)。
unsafe fn toggle_fullscreen(hwnd: HWND) {
    let mut saved = FULLSCREEN_SAVE.lock().unwrap();
    if let Some((style, rect)) = *saved {
        // 还原:先回样式再回位置(顺序错了边框会被裁)。
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        let _ = MoveWindow(hwnd, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, true);
        *saved = None;
    } else {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        *saved = Some((style, rect));
        SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP.0 as isize);
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            let r = info.rcMonitor;
            let _ = MoveWindow(hwnd, r.left, r.top, r.right - r.left, r.bottom - r.top, true);
        }
    }
    let _ = InvalidateRect(hwnd, None, true);
}

/// 地址栏编辑框子类:回车 → 导航命令。
///
/// 外壳只做 FFI 回调 panic 防火墙(`extern "system"` 同为 nounwind 边界,
/// panic 展开会 abort 整个查看器,见 `crate::ffi_guard`);逻辑在 `_inner`。
unsafe extern "system" fn edit_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _ref: usize,
) -> LRESULT {
    crate::ffi_guard::guard_ret("view::edit_subclass", LRESULT(0), || unsafe {
        edit_subclass_inner(hwnd, msg, wp, lp, _id, _ref)
    })
}

unsafe fn edit_subclass_inner(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _ref: usize,
) -> LRESULT {
    let ctrl_id = GetDlgCtrlID(hwnd);
    if msg == WM_KEYDOWN && (wp.0 & 0xFFFF) as u32 == 0x0D {
        match ctrl_id {
            // Console eval 输入(58 号):表达式送宿主页面全局求值,清空输入框。
            ID_EVAL => {
                if let Some(text) = hwnd_text(hwnd) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        send_cmd(&format!(
                            "{{\"cmd\":\"console_eval\",\"expr\":{}}}",
                            dt::json_string(trimmed)
                        ));
                        let _ = SetWindowTextW(hwnd, PCWSTR::null());
                    }
                }
                return LRESULT(0);
            }
            // 搜索框(58 号):回车按关键词重绘日志区。
            ID_SEARCH => {
                if let Some(text) = hwnd_text(hwnd) {
                    if let Ok(mut f) = dt::FILTER.lock() {
                        f.search = text.trim().to_string();
                    }
                }
                return LRESULT(0);
            }
            // 地址栏(既有语义)。
            _ => {
                let text = address_text(hwnd).trim().to_string();
                if !text.is_empty() {
                    let esc = text.replace('\\', "\\\\").replace('"', "\\\"");
                    send_cmd(&format!("{{\"cmd\":\"navigate\",\"url\":\"{esc}\"}}"));
                }
                return LRESULT(0);
            }
        }
    }
    if msg == WM_KILLFOCUS && ctrl_id == ID_EDIT {
        // 失焦:回填宿主 URL(清掉误入的外来输入,与 macOS 侧同语义;仅地址栏)。
        if let Some((url, _, _)) = PENDING_URL.lock().unwrap().clone() {
            let _ = SetWindowTextW(hwnd, PCWSTR(to_wide(&url).as_ptr()));
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// 主窗口过程(panic 防火墙外壳,逻辑在 `_inner`)。
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::ffi_guard::guard_ret("view::wnd_proc", LRESULT(0), || unsafe {
        wnd_proc_inner(hwnd, msg, wp, lp)
    })
}

unsafe fn wnd_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            let mut changed = false;
            if let Some(t) = PENDING_TITLE.lock().unwrap().take() {
                // P2d:原始标题入账 + 叠加缩放后缀(≠100% 时可见)。
                *LAST_TITLE.lock().unwrap() = t.clone();
                let z = *ZOOM.lock().unwrap();
                let _ = SetWindowTextW(hwnd, PCWSTR(to_wide(&zoom_title(&t, z)).as_ptr()));
            }
            if let Some((url, can_back, can_forward)) = PENDING_URL.lock().unwrap().take() {
                apply_urlbar(&url, can_back, can_forward);
            }
            // ⌘/Ctrl+L:聚焦地址栏并全选(与 macOS 同语义)。
            // EM_SETSEL 的 lParam = MAKELONG(0, -1):起点 0、终点 -1 = 全选。
            // 修复(2026-11-06):`MutexGuard<bool>` 无 `.take()`,改经解引用取值。
            if std::mem::take(&mut *FOCUS_ADDRBAR_FLAG.lock().unwrap()) {
                if let Some(edit) = ADDR_EDIT.lock().unwrap().map(|c| c.0) {
                    let _ = SetFocus(edit);
                    let _ = SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
                }
            }
            if let Some(png) = TAKE_IMAGE.lock().unwrap().take() {
                if let Ok(img) = image::load_from_memory(&png) {
                    let rgba = img.to_rgba8();
                    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
                    *FRAME.lock().unwrap() = Some(Frame {
                        rgba: rgba.into_raw(),
                        width: w,
                        height: h,
                    });
                    changed = true;
                }
            }
            // 58 号:消费 DevTools 快照槽 → 合并进日志缓冲 → 重绘面板。
            if dt::take_slot_and_merge() {
                changed = true;
            }
            if *CLOSE_FLAG.lock().unwrap() {
                let _ = DestroyWindow(hwnd);
            } else if changed {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            forward_pointer(hwnd, x, y, "move", None, true);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = lparam_xy(lp);
            forward_pointer(hwnd, x, y, "down", Some("left"), false);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            forward_pointer(hwnd, x, y, "up", Some("left"), false);
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => {
            if input_disabled() || rmenu_disabled() {
                RMENU_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
                let (x, y) = lparam_xy(lp);
                forward_pointer(hwnd, x, y, "down", Some("right"), false);
            } else {
                // 原生菜单接管(阻塞至选择/点空白),down/up 都不再走事件路径。
                show_context_menu(hwnd);
                RMENU_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if RMENU_ACTIVE.swap(false, std::sync::atomic::Ordering::Relaxed) {
                return LRESULT(0);
            }
            let (x, y) = lparam_xy(lp);
            forward_pointer(hwnd, x, y, "up", Some("right"), false);
            LRESULT(0)
        }
        WM_MBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            forward_pointer(hwnd, x, y, "down", Some("middle"), false);
            LRESULT(0)
        }
        WM_MBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            forward_pointer(hwnd, x, y, "up", Some("middle"), false);
            LRESULT(0)
        }
        // P2c:鼠标侧键前进/后退(XBUTTON1=后退、XBUTTON2=前进)。
        WM_XBUTTONDOWN => {
            let xbtn = ((wp.0 >> 16) & 0xFFFF) as u16;
            if xbtn == 0x0001 {
                send_cmd("{\"cmd\":\"back\"}");
            } else if xbtn == 0x0002 {
                send_cmd("{\"cmd\":\"forward\"}");
            }
            LRESULT(0)
        }
        // P2c:光标形状跟随(手型/文本 I 型/箭头),仅客户区生效。
        WM_SETCURSOR => {
            let hit = (lp.0 & 0xFFFF) as u16;
            if hit == 1 {
                // HTCLIENT
                let kind = CURSOR_KIND.lock().unwrap().clone();
                let idc = match kind.as_str() {
                    "pointer" => IDC_HAND,
                    "text" => IDC_IBEAM,
                    _ => IDC_ARROW,
                };
                if let Ok(h) = LoadCursorW(None, idc) {
                    SetCursor(h);
                }
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_MOUSEWHEEL => {
            // 58 号:光标在 DevTools 面板区时滚轮翻日志(不进页面)。
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if scroll_console(hwnd, delta) {
                return LRESULT(0);
            }
            forward_wheel(hwnd, wp, lp, false);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_MOUSEHWHEEL => {
            forward_wheel(hwnd, wp, lp, true);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_KEYDOWN => {
            // 地址栏聚焦时不进页面(其子类自会处理编辑键)。
            let focused = GetFocus();
            let edit = ADDR_EDIT.lock().unwrap().map(|c| c.0);
            if edit == Some(focused) {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            forward_key(hwnd, wp, false);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_SYSKEYDOWN => {
            let focused = GetFocus();
            let edit = ADDR_EDIT.lock().unwrap().map(|c| c.0);
            if edit == Some(focused) {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            forward_key(hwnd, wp, true);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CHAR => {
            let focused = GetFocus();
            let edit = ADDR_EDIT.lock().unwrap().map(|c| c.0);
            if edit == Some(focused) {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            forward_char(wp);
            LRESULT(0)
        }
        WM_COMMAND => {
            // 子控件通知:BN_CLICKED(高 16 位 = 0)→ 工具栏/DevTools 按钮。
            let id = (wp.0 & 0xFFFF) as i32;
            let code = ((wp.0 >> 16) & 0xFFFF) as u32;
            if code == 0 {
                let cmd = match id {
                    ID_BACK => Some("back"),
                    ID_FORWARD => Some("forward"),
                    ID_RELOAD => Some("reload"),
                    ID_HOME => Some("home"),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    send_cmd(&format!("{{\"cmd\":\"{cmd}\"}}"));
                }
                // DevTools(58 号):菜单/过滤/清空/关闭经 devtools 模块分发。
                let _ = devtools::on_command(hwnd, id);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            unsafe { paint(hdc, hwnd) };
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_SIZE => {
            // 人工拖动窗口 → 客户区尺寸变化 → 重排工具栏 + 上报宿主。
            unsafe { handle_resize(hwnd) };
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// 地址栏同步:焦点不在地址栏时才覆盖文本;按钮按可用性禁用。
fn apply_urlbar(url: &str, can_back: bool, can_forward: bool) {
    unsafe {
        let focused = GetFocus();
        let edit = ADDR_EDIT.lock().unwrap().map(|c| c.0);
        if let Some(edit) = edit {
            if edit != focused {
                let _ = SetWindowTextW(edit, PCWSTR(to_wide(url).as_ptr()));
            }
        }
        if let Some(back) = BACK_BTN.lock().unwrap().map(|c| c.0) {
            let _ = EnableWindow(back, can_back);
        }
        if let Some(fwd) = FWD_BTN.lock().unwrap().map(|c| c.0) {
            let _ = EnableWindow(fwd, can_forward);
        }
    }
}

unsafe fn paint(hdc: HDC, hwnd: HWND) {
    // 客户区底色(无帧/帧较小时可见)。
    let mut rc = RECT::default();
    if GetClientRect(hwnd, &mut rc).is_err() {
        return;
    }
    let ch = rc.bottom - rc.top;
    // 一级菜单栏条:浅灰底(58 号,最顶一行)。
    let menu_bar = RECT {
        left: 0,
        top: 0,
        right: rc.right,
        bottom: MENU_H,
    };
    let brush = CreateSolidBrush(COLORREF(0x00F0F0F0));
    FillRect(hdc, &menu_bar, brush);
    let _ = DeleteObject(brush);
    // 工具栏条:浅灰底(菜单栏之下)。
    let bar = RECT {
        left: 0,
        top: MENU_H,
        right: rc.right,
        bottom: MENU_H + TOOLBAR_H,
    };
    let brush = CreateSolidBrush(COLORREF(0x00F0F0F0));
    FillRect(hdc, &bar, brush);
    let _ = DeleteObject(brush);
    // 画面区:工具栏之下、面板之上,先铺白底(帧比内容区小时露白边)。
    let ph = panel_h();
    let area = RECT {
        left: 0,
        top: MENU_H + TOOLBAR_H,
        right: rc.right,
        bottom: ch - ph,
    };
    let brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
    FillRect(hdc, &area, brush);
    let _ = DeleteObject(brush);
    // 画面 1:1 绘制(**不拉伸**):宿主按内容区尺寸重渲染,新帧尺寸 == 内容区
    // 尺寸,于是天然铺满且不变形;尺寸暂时不一致时居中摆放(仍不缩放)。
    let frame = FRAME.lock().unwrap();
    if let Some(f) = frame.as_ref() {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: f.width,
                biHeight: -f.height, // 自顶向下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let view_w = rc.right - rc.left;
        let view_h = ch - MENU_H - TOOLBAR_H - ph;
        // P2d:帧等比缩放铺满画面区(缩放 = 帧尺寸 ÷ 内容区尺寸的反比;
        // z=1 且帧 == 内容区时退化为既有 1:1 绘制)。尺寸暂时不一致的
        // 过渡帧也按比例居中,不再溢出/露边。
        let mut dx = (view_w - f.width) / 2;
        let mut dy = MENU_H + TOOLBAR_H + (view_h - f.height) / 2;
        let (mut dw, mut dh) = (f.width, f.height);
        if view_w > 0 && view_h > 0 && f.width > 0 && f.height > 0 {
            let scale = (view_w as f64 / f.width as f64).min(view_h as f64 / f.height as f64);
            dw = (f.width as f64 * scale).round() as i32;
            dh = (f.height as f64 * scale).round() as i32;
            dx = (view_w - dw) / 2;
            dy = MENU_H + TOOLBAR_H + (view_h - dh) / 2;
        }
        let _ = StretchDIBits(
            hdc,
            dx,
            dy,
            dw,
            dh,
            0,
            0,
            f.width,
            f.height,
            Some(f.rgba.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
    drop(frame);
    // DevTools 面板(58 号):展开时贴底绘制日志区。
    if ph > 0 {
        paint_console(hdc, rc.right, ch);
    }
}

/// 读子控件文本(空/失败返回 None)。
unsafe fn hwnd_text(hwnd: HWND) -> Option<String> {
    if hwnd.0.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 4096];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

/// 客户区尺寸(像素);取不到返回 None。
unsafe fn client_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut rc = RECT::default();
    if GetClientRect(hwnd, &mut rc).is_err() {
        return None;
    }
    Some((rc.right - rc.left, rc.bottom - rc.top))
}

/// P2d:标题叠加缩放后缀(≠100% 时 `[125%]`;复位时用原始标题)。
fn zoom_title(raw: &str, z: f64) -> String {
    if (z - 1.0).abs() < 1e-9 {
        raw.to_string()
    } else {
        format!("{} [{}%]", raw, (z * 100.0).round() as i64)
    }
}

/// P2d:调整浏览器级缩放并立即生效(视口按新系数换算重报 + 宿主记账 +
/// 标题后缀 + 整窗重绘)。`dir > 0` 放大、`dir < 0` 缩小、`0` 复位。
unsafe fn apply_zoom(dir: i32, hwnd: HWND) {
    let next = if dir == 0 {
        1.0
    } else {
        super::zoom_step(*ZOOM.lock().unwrap(), dir)
    };
    *ZOOM.lock().unwrap() = next;
    send_cmd(&format!("{{\"cmd\":\"zoom\",\"z\":{next}}}"));
    // 视口按新缩放换算重报(置 None 强制变化检测命中)。
    *LAST_SIZE.lock().unwrap() = None;
    handle_resize(hwnd);
    let t = LAST_TITLE.lock().unwrap().clone();
    let _ = SetWindowTextW(hwnd, PCWSTR(to_wide(&zoom_title(&t, next)).as_ptr()));
    let _ = InvalidateRect(hwnd, None, true);
}

/// 人工拖动窗口 → 客户区尺寸变化 → 重排工具栏 + 上报宿主。
///
/// * 工具栏在**上**边(与既有视觉一致),画面区 = 客户区扣掉工具栏;
/// * 尺寸不变不重复上报;窗口创建过程中触发的 WM_SIZE 用初始尺寸兜底;
/// * 上报的是内容区尺寸(已扣工具栏)= 宿主侧的新视口。
unsafe fn handle_resize(hwnd: HWND) {
    let Some((cw, ch)) = client_size(hwnd) else {
        return;
    };
    if cw <= 0 || ch <= MENU_H + TOOLBAR_H {
        return;
    }
    {
        let mut last = LAST_SIZE.lock().unwrap();
        if *last == Some((cw, ch)) {
            return;
        }
        *last = Some((cw, ch));
    }
    // 工具栏子控件重排(按钮宽度固定,地址栏吃掉剩余宽度;菜单栏之下)。
    let mut x = PAD;
    let bar_y = MENU_H + (TOOLBAR_H - BTN_H) / 2;
    let back = BACK_BTN.lock().unwrap().map(|c| c.0);
    let fwd = FWD_BTN.lock().unwrap().map(|c| c.0);
    let reload = RELOAD_BTN.lock().unwrap().map(|c| c.0);
    let home = HOME_BTN.lock().unwrap().map(|c| c.0);
    for b in [back, fwd, reload, home] {
        if let Some(b) = b {
            let _ = MoveWindow(b, x, bar_y, BTN_W, BTN_H, true);
        }
        x += BTN_W + BTN_GAP;
    }
    if let Some(edit) = ADDR_EDIT.lock().unwrap().map(|c| c.0) {
        let edit_w = (cw - x - PAD).max(60);
        let _ = MoveWindow(edit, x, bar_y, edit_w, BTN_H, true);
    }
    // DevTools 菜单栏 + 面板控件重排(58 号)。
    layout_controls(cw, ch);
    // 上报宿主:内容区宽 = 客户区宽;内容区高扣掉菜单栏 + 工具栏 + 面板 = 视口高
    //(P2d:按缩放系数换算 —— Chrome 语义,可视 CSS 视口随缩放反比变化)。
    let z = (*ZOOM.lock().unwrap()).max(0.05);
    let vw = ((cw as f64 / z).round() as i32).clamp(1, 7680) as u32;
    let vh = (((ch - MENU_H - TOOLBAR_H - panel_h()) as f64 / z).round() as i32)
        .clamp(1, 4320) as u32;
    // 记录本查看器认知里的视口尺寸(键鼠坐标换算基线)。
    *VIEWPORT.lock().unwrap() = (vw as i32, vh as i32);
    send_cmd(&super::resize::resize_cmd_json(vw, vh));
}

/// 创建工具栏按钮(子控件)。
unsafe fn make_button(parent: HWND, hinst: HINSTANCE, x: i32, text: &str, id: i32) -> HWND {
    let cls = to_wide("BUTTON");
    let txt = to_wide(text);
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(cls.as_ptr()),
        PCWSTR(txt.as_ptr()),
        WINDOW_STYLE((WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32) as u32),
        x,
        MENU_H + (TOOLBAR_H - BTN_H) / 2,
        BTN_W,
        BTN_H,
        parent,
        HMENU(id as isize as *mut _),
        hinst,
        None,
    )
    .unwrap_or_default()
}

/// 查看器主入口:阻塞运行至 stdin EOF / 'c' 帧 / 用户关窗。
pub fn run(width: u32, height: u32, title: &str) -> i32 {
    unsafe {
        let hmodule = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => {
                drain_stdin();
                return 1;
            }
        };
        // GetModuleHandleW 返回 HMODULE;Win32 的 hinstance 形参是 HINSTANCE。
        let hinstance: HINSTANCE = hmodule.into();
        let class_name = w!("LsmBrowserViewWnd");
        let wc = WNDCLASSW {
            // CS_DBLCLKS:第二次同位点击产生 WM_LBUTTONDBLCLK(转发为按下,
            // 宿主按两次抬起间隔合成 dblclick)。
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(0x00FFFFFF)),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            drain_stdin();
            return 1;
        }

        let w = width.clamp(320, 3840) as i32;
        let h = height.clamp(240, 2160) as i32;
        // 键鼠坐标换算基线:初始视口 = argv(后续随 resize 上报同步)。
        *VIEWPORT.lock().unwrap() = (w, h);
        // 客户区尺寸 = 视口 + 工具栏 + 一级菜单栏(58 号);用 AdjustWindowRectEx
        // 反算外框尺寸,保证客户区**正好**是 w × (h + TOOLBAR_H + MENU_H)
        //(否则画面区比视口小,1:1 绘制会在右侧/底部露白边)。
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h + TOOLBAR_H + MENU_H,
        };
        let _ = AdjustWindowRectEx(&mut wr, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
        let outer_w = (wr.right - wr.left).max(w);
        let outer_h = (wr.bottom - wr.top).max(h + TOOLBAR_H + MENU_H);
        // 初始客户区尺寸登记为基线:创建过程中的 WM_SIZE 不重复上报。
        *LAST_SIZE.lock().unwrap() = Some((w, h + TOOLBAR_H + MENU_H));
        let title_w = to_wide(title);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer_w,
            outer_h,
            None,
            HMENU(std::ptr::null_mut()),
            hinstance,
            None,
        ) {
            Ok(h) => h,
            Err(_) => {
                drain_stdin();
                return 1;
            }
        };

        // 工具栏:后退/前进/刷新/首页 + 地址栏。
        let mut x = PAD;
        let back = make_button(hwnd, hinstance, x, "后退", ID_BACK);
        x += BTN_W + BTN_GAP;
        let forward = make_button(hwnd, hinstance, x, "前进", ID_FORWARD);
        x += BTN_W + BTN_GAP;
        let reload = make_button(hwnd, hinstance, x, "刷新", ID_RELOAD);
        x += BTN_W + BTN_GAP;
        let home = make_button(hwnd, hinstance, x, "首页", ID_HOME);
        x += BTN_W + BTN_GAP;
        let edit_w = (w - x - PAD).max(60);
        let edit_cls = to_wide("EDIT");
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(edit_cls.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(
                (WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_BORDER.0
                    | WS_TABSTOP.0
                    | ES_AUTOHSCROLL as u32) as u32,
            ),
            x,
            MENU_H + (TOOLBAR_H - BTN_H) / 2,
            edit_w,
            BTN_H,
            hwnd,
            HMENU(ID_EDIT as isize as *mut _),
            hinstance,
            None,
        )
        .unwrap_or_default();
        if !edit.0.is_null() {
            let _ = SetWindowSubclass(edit, Some(edit_subclass), 1, 0);
        }

        *ADDR_EDIT.lock().unwrap() = Some(HwndCell(edit));
        *BACK_BTN.lock().unwrap() = Some(HwndCell(back));
        *FWD_BTN.lock().unwrap() = Some(HwndCell(forward));
        *RELOAD_BTN.lock().unwrap() = Some(HwndCell(reload));
        *HOME_BTN.lock().unwrap() = Some(HwndCell(home));
        // 58 号:一级菜单栏 + Console/占位面板控件(初始隐藏)。
        build_ui(hwnd, hinstance, w, h + TOOLBAR_H + MENU_H);
        // 测试钩子:LAEW_DEVTOOLS_AUTO=<panel> 启动即展开面板(与 macOS 同语义)。
        maybe_auto_panel(hwnd);

        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        // 初始焦点给主窗口:地址栏编辑框不自动抢焦点(否则外部误入的按键
        // 会写进地址栏,把宿主下发的 URL 顶掉)。
        let _ = SetFocus(hwnd);

        // 读线程:stdin → 帧槽。
        std::thread::spawn(|| {
            let mut stdin = std::io::stdin().lock();
            loop {
                match read_frame(&mut stdin) {
                    Some((b'i', png)) => {
                        *TAKE_IMAGE.lock().unwrap() = Some(png);
                    }
                    Some((b't', payload)) => {
                        if let Some(t) = serde_json::from_slice::<serde_json::Value>(&payload)
                            .ok()
                            .and_then(|v| v.get("title").and_then(|t| t.as_str().map(str::to_string)))
                        {
                            *PENDING_TITLE.lock().unwrap() = Some(t);
                        }
                    }
                    Some((b'u', payload)) => {
                        let v = serde_json::from_slice::<serde_json::Value>(&payload).ok();
                        let url = v
                            .as_ref()
                            .and_then(|v| v.get("url"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                        let can_back = v
                            .as_ref()
                            .and_then(|v| v.get("can_back"))
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false);
                        let can_forward = v
                            .as_ref()
                            .and_then(|v| v.get("can_forward"))
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false);
                        *PENDING_URL.lock().unwrap() = Some((url, can_back, can_forward));
                    }
                    // ⌘/Ctrl+L:聚焦地址栏(39 号 P2b)。
                    Some((b'f', _)) => {
                        *FOCUS_ADDRBAR_FLAG.lock().unwrap() = true;
                    }
                    // P2c:光标形状跟随(WM_SETCURSOR 消费)。
                    Some((b'k', payload)) => {
                        if let Ok(kind) = std::str::from_utf8(&payload) {
                            *CURSOR_KIND.lock().unwrap() = kind.trim().to_string();
                        }
                    }
                    // 58 号:DevTools 状态帧(console 台账快照;latest-wins 入槽)。
                    Some((b'd', payload)) => {
                        dt::push_payload(&payload);
                    }
                    Some((b'c', _)) | None => {
                        *CLOSE_FLAG.lock().unwrap() = true;
                        return;
                    }
                    Some(_) => {}
                }
            }
        });

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        0
    }
}

/// 消费 stdin 至 EOF(创建失败的兜底,防父进程写管道阻塞)。同步阻塞。
fn drain_stdin() {
    use std::io::Read as _;
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 4096];
    while let Ok(n) = stdin.read(&mut buf) {
        if n == 0 {
            break;
        }
    }
}
