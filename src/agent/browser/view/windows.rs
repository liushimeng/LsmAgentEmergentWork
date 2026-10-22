//! Windows 有头帧查看器:Win32(GDI `StretchDIBits` + 原生工具栏控件)。
//!
//! 只在 `laew __browser-view` 子进程内运行(与 `__hitl-dialog` 同一模式)。
//! 窗口客户区 = 工具栏(后退/前进/刷新/首页 + 地址栏)+ 视口画面;
//! PNG 帧经 image crate 解码为 RGBA 后以自顶向下 32bpp DIB 绘制。
//! 100ms `WM_TIMER` 轮询共享帧槽(与 macOS 侧同构)。
//!
//! 工具栏交互经 stdout 命令通道驱动宿主引擎(与 macOS 侧协议一致)。

use std::sync::Mutex;

use windows::core::w;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, StretchDIBits,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, GetWindowTextW, LoadCursorW, PostQuitMessage, RegisterClassW, SetTimer,
    SetWindowTextW, ShowWindow, TranslateMessage, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, ES_AUTOHSCROLL, HMENU, IDC_ARROW, MSG, SW_SHOWNORMAL, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

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
static ADDR_EDIT: Mutex<Option<HwndCell>> = Mutex::new(None);
static BACK_BTN: Mutex<Option<HwndCell>> = Mutex::new(None);
static FWD_BTN: Mutex<Option<HwndCell>> = Mutex::new(None);

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

/// 地址栏编辑框子类:回车 → 导航命令。
unsafe extern "system" fn edit_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _ref: usize,
) -> LRESULT {
    if msg == WM_KEYDOWN && (wp.0 & 0xFFFF) as u32 == 0x0D {
        let text = address_text(hwnd).trim().to_string();
        if !text.is_empty() {
            let esc = text.replace('\\', "\\\\").replace('"', "\\\"");
            send_cmd(&format!("{{\"cmd\":\"navigate\",\"url\":\"{esc}\"}}"));
        }
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            let mut changed = false;
            if let Some(t) = PENDING_TITLE.lock().unwrap().take() {
                let _ = SetWindowTextW(hwnd, PCWSTR(to_wide(&t).as_ptr()));
            }
            if let Some((url, can_back, can_forward)) = PENDING_URL.lock().unwrap().take() {
                apply_urlbar(&url, can_back, can_forward);
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
            if *CLOSE_FLAG.lock().unwrap() {
                DestroyWindow(hwnd);
            } else if changed {
                InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            // 子控件通知:BN_CLICKED(高 16 位 = 0)→ 工具栏按钮。
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
        WM_CLOSE => {
            DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
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
            EnableWindow(back, can_back);
        }
        if let Some(fwd) = FWD_BTN.lock().unwrap().map(|c| c.0) {
            EnableWindow(fwd, can_forward);
        }
    }
}

unsafe fn paint(hdc: HDC, hwnd: HWND) {
    // 客户区底色(无帧/帧较小时可见)。
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    // 工具栏条:浅灰底。
    let bar = RECT {
        left: 0,
        top: 0,
        right: rc.right,
        bottom: TOOLBAR_H,
    };
    let brush = CreateSolidBrush(COLORREF(0x00F0F0F0));
    FillRect(hdc, &bar, brush);
    DeleteObject(brush);
    // 画面区:工具栏之下。
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
        StretchDIBits(
            hdc,
            0,
            TOOLBAR_H,
            rc.right,
            rc.bottom - TOOLBAR_H,
            0,
            0,
            f.width,
            f.height,
            Some(f.rgba.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    } else {
        let area = RECT {
            left: 0,
            top: TOOLBAR_H,
            right: rc.right,
            bottom: rc.bottom,
        };
        let brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
        FillRect(hdc, &area, brush);
        DeleteObject(brush);
    }
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
        (TOOLBAR_H - BTN_H) / 2,
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
            style: CS_HREDRAW | CS_VREDRAW,
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
        let title_w = to_wide(title);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            w,
            h + TOOLBAR_H,
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
        let _reload = make_button(hwnd, hinstance, x, "刷新", ID_RELOAD);
        x += BTN_W + BTN_GAP;
        let _home = make_button(hwnd, hinstance, x, "首页", ID_HOME);
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
            (TOOLBAR_H - BTN_H) / 2,
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

        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

        // 读线程:stdin → 帧槽。
        std::thread::spawn(|| {
            use std::io::Read as _;
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
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 4096];
    while let Ok(n) = stdin.read(&mut buf) {
        if n == 0 {
            break;
        }
    }
}
