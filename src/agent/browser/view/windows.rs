//! Windows 有头帧查看器:Win32(GDI `StretchDIBits`)。
//!
//! 只在 `laew __browser-view` 子进程内运行(与 `__hitl-dialog` 同一模式)。
//! 窗口客户区尺寸 = argv 传入的视口尺寸;PNG 帧经 image crate 解码为 RGBA 后
//! 以自顶向下 32bpp DIB 绘制。100ms `WM_TIMER` 轮询共享帧槽(与 macOS 侧同构)。

use std::sync::Mutex;

use windows::core::w;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect, StretchDIBits,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetClientRect, LoadCursorW, PostQuitMessage, RegisterClassW, SetTimer, SetWindowTextW,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, IDC_ARROW, MSG,
    SW_SHOWNORMAL, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use super::read_frame;

/// 解码后的帧(自顶向下 RGBA)。
struct Frame {
    rgba: Vec<u8>,
    width: i32,
    height: i32,
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
static PENDING_TITLE: Mutex<Option<String>> = Mutex::new(None);
static CLOSE_FLAG: Mutex<bool> = Mutex::new(false);
/// 帧槽:读线程写 PNG 原始字节,WM_TIMER 解码。
static TAKE_IMAGE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 100;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            let mut changed = false;
            if let Some(t) = PENDING_TITLE.lock().unwrap().take() {
                SetWindowTextW(hwnd, PCWSTR(to_wide(&t).as_ptr()));
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
                InvalidateRect(Some(hwnd), None, false);
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

unsafe fn paint(hdc: HDC, hwnd: HWND) {
    // 客户区底色(无帧/帧较小时可见)。
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
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
            0,
            rc.right,
            rc.bottom,
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
        let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        windows::Win32::Graphics::Gdi::FillRect(hdc, &rc, brush);
        DeleteObject(brush.into());
    }
}

/// 查看器主入口:阻塞运行至 stdin EOF / 'c' 帧 / 用户关窗。
pub fn run(width: u32, height: u32, title: &str) -> i32 {
    unsafe {
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => {
                drain_stdin();
                return 1;
            }
        };
        let class_name = w!("LsmBrowserViewWnd");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW),
            hbrBackground: unsafe {
                CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FFFFFF))
            },
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            drain_stdin();
            return 1;
        }

        let w = width.clamp(320, 3840);
        let h = height.clamp(240, 2160);
        let title_w = to_wide(title);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            w as i32,
            h as i32,
            None,
            None,
            Some(hinstance.into()),
            None,
        );
        if hwnd.0 == 0 {
            drain_stdin();
            return 1;
        }
        let _ = SetTimer(Some(hwnd), TIMER_ID, TIMER_MS, None);
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
