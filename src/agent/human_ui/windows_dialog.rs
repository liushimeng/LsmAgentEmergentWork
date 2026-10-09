//! Windows 弹窗后端:Rust 直调 Win32 API(第 137 轮原生重构)。
//!
//! 设计见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §7。要点:
//! - 只在 `laew __hitl-dialog` 子进程内运行;窗口类 + 标准控件(BUTTON/EDIT/STATIC)
//!   + `IsDialogMessageW` 消息循环(Tab 导航 / Enter=默认按钮 / Esc=取消);
//! - **WM_SIZE 全量重排** —— 最大化/还原/拖拽缩放共享一条布局路径(坐标由纯函数
//!   从「当前客户区尺寸」推导),这是 Windows 侧「最大化后无响应」的根治点
//!   (旧 ps1 靠 WinForms Anchor,不重算);
//! - 图片:`image` crate 解码 PNG → RGBA 重采样 → BGRA → `CreateDIBSection`
//!   → `STM_SETIMAGE`,不引入 GDI+/WIC;
//! - DPI:保持系统默认虚拟化(布局常量与 macOS 侧同值,见 macos_dialog.rs)。

use std::sync::Mutex;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, CreateFontW, DeleteObject, GetSysColorBrush, HGDIOBJ, BITMAPINFO,
    BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, DIB_RGB_COLORS,
    DEFAULT_CHARSET, DEFAULT_PITCH, FF_SWISS, FW_BOLD, FW_NORMAL, OUT_DEFAULT_PRECIS, HBITMAP,
    HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW,
    LoadCursorW, MoveWindow, PostQuitMessage, RegisterClassExW, SendMessageW, SetForegroundWindow,
    SetTimer, ShowWindow, TranslateMessage, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, HMENU,
    IDC_ARROW, STM_SETIMAGE, SW_MAXIMIZE, SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WM_SETTEXT, WM_SIZE, WM_TIMER, WNDCLASSEXW,
    WS_CHILD, WS_EX_TOPMOST, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_SYSMENU,
    WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};

use super::dialog_main::{countdown_text, DialogPayload};

// ---------- 布局常量(与 macos_dialog.rs 保持同值;两平台观感一致) ----------

const W: i32 = 560;
const PAD: i32 = 16;
const TITLE_H: i32 = 22;
const LINE_H: i32 = 18;
const INPUT_H: i32 = 28;
const BTN_H: i32 = 30;
const GAP: i32 = 8;
const CANCEL_W: i32 = 84;
const SUBMIT_W: i32 = 84;
const ORIG_W: i32 = 110;
const ZOOM_W: i32 = 110;
const INNER: i32 = 8;
const IMG_HINT_H: i32 = 14;

const MSG_H_MIN: f64 = 90.0;
const MSG_H_MAX: f64 = 240.0;
const IMG_MAX_W: u32 = 528;
const IMG_MAX_H: u32 = 300;

/// SS_BITMAP(值 14;位于未启用的 SystemServices feature,裸值等价,避免开 feature)。
const SS_BITMAP_RAW: u32 = 14;

// 控件 ID(WM_COMMAND 的 wParam 低位;选项 = 100 + 下标,tag 语义与 macOS 对齐)
const IDC_TITLE: i32 = 10;
const IDC_MSG: i32 = 11;
const IDC_T1: i32 = 13;
const IDC_T2: i32 = 14;
const IDC_INPUT: i32 = 15;
/// 提交按钮 / Enter(IsDialogMessage 路由)。windows crate 的 IDOK 是
/// MESSAGEBOX_RESULT 类型,控件 ID 需 i32,取裸值。
const IDOK_ID: i32 = 1;
const IDCANCEL_ID: i32 = 2;
const IDC_OPT_BASE: i32 = 100;
const IDC_ZOOM: i32 = 106;
const IDC_ORIG: i32 = 107;
const TIMER_ID: usize = 1;

const CLASS_NAME: &str = "LAEWHITLDIALOG";
const FACE: &str = "Microsoft YaHei UI";

// ---------- 布局纯函数(Windows 坐标:左上原点,y 向下) ----------

#[derive(Clone, Copy, Debug, Default)]
struct Geometry {
    msg_h: i32,
    /// 图片控件高(位图在加载时已重采样定死,不随窗口拉伸;0 = 无图)。
    img_h: i32,
}

#[derive(Clone, Debug, Default)]
struct Frames {
    title: RECT,
    msg: RECT,
    image: RECT,
    img_hint: RECT,
    t1: RECT,
    t2: RECT,
    input: RECT,
    opts: Vec<RECT>,
    cancel: RECT,
    submit: RECT,
    orig: RECT,
    zoom: RECT,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

/// 说明区高:按客户区高度自适应(正常窗 ≈90,最大化时放大到 ≤240)。
fn msg_h_for(client_h: i32) -> i32 {
    ((client_h as f64) * 0.22).round().clamp(MSG_H_MIN, MSG_H_MAX) as i32
}

/// 排布纯函数:顶部内容顺序排,底部按钮行钉在客户区底边。
fn layout_frames(cw: i32, ch: i32, geo: Geometry, opt_count: usize) -> Frames {
    let text_w = cw - PAD * 2;
    let mut y = PAD;
    let mut place = |h: i32| -> i32 {
        let top = y;
        y += h + GAP;
        top
    };
    let title_y = place(TITLE_H);
    let msg_y = place(geo.msg_h);
    let (img_y, hint_y) = if geo.img_h > 0 {
        let iy = place(geo.img_h);
        let hy = place(IMG_HINT_H);
        (iy, hy)
    } else {
        (0, 0)
    };
    let t1_y = place(LINE_H);
    let t2_y = place(LINE_H);
    let input_y = place(INPUT_H);
    let opts = (0..opt_count).map(|_| rect(PAD, place(BTN_H), text_w, BTN_H)).collect();
    // 底部按钮行:左 取消/提交,右 查看原图/⛶ 最大化;钉在底边(任何 WM_SIZE 后重算)
    let row_y = (ch - PAD - BTN_H).max(y);
    Frames {
        title: rect(PAD, title_y, text_w, TITLE_H),
        msg: rect(PAD, msg_y, text_w, geo.msg_h),
        image: rect(PAD, img_y, text_w, geo.img_h),
        img_hint: rect(PAD, hint_y, text_w, IMG_HINT_H),
        t1: rect(PAD, t1_y, text_w, LINE_H),
        t2: rect(PAD, t2_y, text_w, LINE_H),
        input: rect(PAD, input_y, text_w, INPUT_H),
        opts,
        cancel: rect(PAD, row_y, CANCEL_W, BTN_H),
        submit: rect(PAD + CANCEL_W + INNER, row_y, SUBMIT_W, BTN_H),
        orig: rect(cw - PAD - ZOOM_W - INNER - ORIG_W, row_y, ORIG_W, BTN_H),
        zoom: rect(cw - PAD - ZOOM_W, row_y, ZOOM_W, BTN_H),
    }
}

/// 预算纯函数:由内容推出所需**客户区**高(与 [`layout_frames`] 消费序列逐项对应)。
fn content_height(geo: Geometry, opt_count: usize) -> i32 {
    let mut consumed = PAD + TITLE_H + GAP + geo.msg_h + GAP;
    if geo.img_h > 0 {
        consumed += geo.img_h + GAP + IMG_HINT_H + GAP;
    }
    consumed += (LINE_H + GAP) * 2;
    consumed += INPUT_H + GAP;
    consumed += opt_count as i32 * (BTN_H + GAP);
    consumed += BTN_H + PAD;
    consumed
}

// ---------- 运行时状态(单弹窗进程,UI 线程独占) ----------

struct WinState {
    hwnd: HWND,
    title: HWND,
    msg: HWND,
    image: Option<HWND>,
    img_hint: Option<HWND>,
    t1: HWND,
    t2: HWND,
    input: HWND,
    opts: Vec<HWND>,
    cancel: HWND,
    submit: HWND,
    orig: HWND,
    zoom: HWND,
    options: Vec<String>,
    image_open_path: Option<String>,
    /// 图片显示高(像素;0 = 无图)。
    img_h: i32,
    /// 图片位图(收尾 DeleteObject)。
    image_bitmap: Option<HBITMAP>,
    zoomed: bool,
    started: Instant,
    timeout_ms: u64,
    done: bool,
    result: Option<(String, String)>,
}

static STATE: Mutex<Option<WinState>> = Mutex::new(None);

// HANDLE(裸指针)默认 !Send,static Mutex 要求 Sync。本模块只在
// `__hitl-dialog` 子进程的 UI 线程创建/触碰 STATE(窗口过程同线程分发)。
unsafe impl Send for WinState {}

fn state() -> std::sync::MutexGuard<'static, Option<WinState>> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 控件快照(脱离 STATE 锁后使用;窗口销毁前恒有效)。
struct Controls {
    title: HWND,
    msg: HWND,
    image: Option<HWND>,
    img_hint: Option<HWND>,
    t1: HWND,
    t2: HWND,
    input: HWND,
    opts: Vec<HWND>,
    cancel: HWND,
    submit: HWND,
    orig: HWND,
    zoom: HWND,
}

impl Controls {
    fn from(st: &WinState) -> Self {
        Self {
            title: st.title,
            msg: st.msg,
            image: st.image,
            img_hint: st.img_hint,
            t1: st.t1,
            t2: st.t2,
            input: st.input,
            opts: st.opts.clone(),
            cancel: st.cancel,
            submit: st.submit,
            orig: st.orig,
            zoom: st.zoom,
        }
    }
}

// ---------- 窗口过程 ----------

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            relayout();
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 as usize == TIMER_ID {
                on_tick();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            on_command(id);
            LRESULT(0)
        }
        WM_CLOSE => {
            // 系统菜单关闭按钮 = 人工取消(与 Esc/「取消」同语义)
            finish("cancel", -1);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// WM_SIZE 全量重排:唯一布局入口(创建后/拖拽/最大化/还原都经过这里)。
fn relayout() {
    let (hwnd, controls, img_h, opt_count) = {
        let guard = state();
        let Some(st) = guard.as_ref() else { return };
        (st.hwnd, Controls::from(st), st.img_display_h(), st.options.len())
    };
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let cw = rc.right - rc.left;
    let ch = rc.bottom - rc.top;
    let geo = Geometry { msg_h: msg_h_for(ch), img_h };
    let frames = layout_frames(cw, ch, geo, opt_count);
    unsafe { apply_frames(&controls, &frames, geo) };
}

impl WinState {
    fn img_display_h(&self) -> i32 {
        self.img_h
    }
}
unsafe fn apply_frames(c: &Controls, f: &Frames, geo: Geometry) {
    let mv = |hwnd: HWND, r: RECT| {
        if !hwnd.0.is_null() {
            let _ = MoveWindow(hwnd, r.left, r.top, r.right - r.left, r.bottom - r.top, true);
        }
    };
    mv(c.title, f.title);
    mv(c.msg, f.msg);
    if geo.img_h > 0 {
        if let Some(hwnd) = c.image {
            mv(hwnd, f.image);
        }
        if let Some(hwnd) = c.img_hint {
            mv(hwnd, f.img_hint);
        }
    }
    mv(c.t1, f.t1);
    mv(c.t2, f.t2);
    mv(c.input, f.input);
    for (hwnd, r) in c.opts.iter().zip(f.opts.iter()) {
        mv(*hwnd, *r);
    }
    mv(c.cancel, f.cancel);
    mv(c.submit, f.submit);
    mv(c.orig, f.orig);
    mv(c.zoom, f.zoom);
}

// ---------- 事件处理 ----------

fn on_tick() {
    // 先锁定取出所需并解锁,再触碰 UI(finish 会再次锁定 STATE)。
    let (t2, text, timed_out) = {
        let guard = state();
        let Some(st) = guard.as_ref() else { return };
        if st.done {
            return;
        }
        let elapsed = st.started.elapsed().as_millis() as u64;
        let remain = st.timeout_ms.saturating_sub(elapsed);
        (st.t2, countdown_text(elapsed, remain), remain == 0)
    };
    let wide = to_wide(&text);
    unsafe {
        SendMessageW(t2, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
    }
    if timed_out {
        finish("timeout", -1);
    }
}

fn on_command(id: i32) {
    match id {
        IDOK_ID => finish("answer", -1),     // 提交按钮 / Enter(默认按钮)
        IDCANCEL_ID => finish("cancel", -1), // 取消按钮 / Esc
        IDC_ZOOM => toggle_zoom(),
        IDC_ORIG => open_image(),
        _ if id >= IDC_OPT_BASE => finish("answer", (id - IDC_OPT_BASE) as i64),
        _ => {}
    }
}

fn toggle_zoom() {
    let (hwnd, zoomed) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        st.zoomed = !st.zoomed;
        let title = if st.zoomed { "⛶ 还原" } else { "⛶ 最大化" };
        let wide = to_wide(title);
        unsafe {
            SendMessageW(st.zoom, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
        (st.hwnd, st.zoomed)
    };
    unsafe {
        // ShowWindow 触发 WM_SIZE → relayout(全量重排)
        let _ = ShowWindow(hwnd, if zoomed { SW_MAXIMIZE } else { SW_RESTORE });
    }
}

fn open_image() {
    let path = {
        let guard = state();
        guard.as_ref().and_then(|st| st.image_open_path.clone())
    };
    let Some(path) = path else { return };
    // 系统默认查看器打开;失败静默(纯附加能力)
    let verb = to_wide("open");
    let file = to_wide(&path);
    unsafe {
        let _ = ShellExecuteW(
            HWND::default(),
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOW,
        );
    }
}

// ---------- 收口 ----------

fn finish(kind: &str, tag: i64) {
    let hwnd = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        st.done = true;
        let result = if kind == "timeout" {
            ("timeout".to_string(), String::new())
        } else if kind == "cancel" {
            ("cancel".to_string(), String::new())
        } else {
            let typed = unsafe { read_input(st.input) };
            if !typed.is_empty() {
                ("answer".to_string(), typed)
            } else if tag >= 0 && (tag as usize) < st.options.len() {
                (
                    "answer".to_string(),
                    format!("{}. {}", tag + 1, st.options[tag as usize]),
                )
            } else {
                ("answer".to_string(), format!("1. {}", st.options[0]))
            }
        };
        st.result = Some(result);
        st.hwnd
    };
    unsafe {
        // DestroyWindow → WM_DESTROY → PostQuitMessage → 消息循环退出
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn read_input(input: HWND) -> String {
    let len = GetWindowTextLengthW(input);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(input, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string()
}

// ---------- 主流程 ----------

/// 弹出原生弹窗并阻塞至收口;返回 (status, text) 契约 JSON 字段。
pub(super) fn run_dialog(p: &DialogPayload) -> (String, String) {
    unsafe { run_win32(p) }
}

unsafe fn run_win32(p: &DialogPayload) -> (String, String) {
    let hinst = match GetModuleHandleW(PCWSTR::null()) {
        Ok(h) => windows::Win32::Foundation::HINSTANCE(h.0),
        Err(_) => return ("error".into(), "GetModuleHandleW 失败".into()),
    };
    let class = to_wide(CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst,
        lpszClassName: PCWSTR(class.as_ptr()),
        hCursor: LoadCursorW(windows::Win32::Foundation::HINSTANCE::default(), IDC_ARROW)
            .unwrap_or_default(),
        hbrBackground: GetSysColorBrush(COLOR_WINDOW),
        ..Default::default()
    };
    if RegisterClassExW(&wc) == 0 {
        return ("error".into(), "窗口类注册失败".into());
    }

    // 图片预解码(失败降级无图,fail-open)
    let (bitmap, img_disp_h, image_open_path) = load_image(p);
    let geo = Geometry { msg_h: MSG_H_MIN as i32, img_h: img_disp_h };
    let client_h = content_height(geo, p.options.len());

    // 允许最大化/还原/拖拽缩放:任何 WM_SIZE 都全量重排(§7 根治点)
    let style = WS_OVERLAPPEDWINDOW | WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_MINIMIZEBOX;
    let mut rc = RECT { left: 0, top: 0, right: W, bottom: client_h };
    if AdjustWindowRect(&mut rc, style, false).is_err() {
        return ("error".into(), "AdjustWindowRect 失败".into());
    }
    let title_text = to_wide(&format!("🔐 人工介入 · {}", p.kind_label));
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOPMOST,
        PCWSTR(class.as_ptr()),
        PCWSTR(title_text.as_ptr()),
        style,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        rc.right - rc.left,
        rc.bottom - rc.top,
        HWND::default(),
        HMENU(std::ptr::null_mut()),
        hinst,
        None,
    ) else {
        return ("error".into(), "窗口创建失败(可能无桌面会话)".into());
    };

    // ---- 字体与控件 ----
    let font_normal = make_font(FW_NORMAL.0 as i32);
    let font_bold = make_font(FW_BOLD.0 as i32);

    let title = create_control(hwnd, hinst, "STATIC", &p.kind_label, style_static(), IDC_TITLE, font_bold);
    let msg = create_control(hwnd, hinst, "EDIT", &p.info_text(), style_msg_edit(), IDC_MSG, font_normal);
    let bitmap_keep = bitmap;
    let (image, img_hint) = if let Some(bmp) = bitmap_keep {
        let ctl = create_control(
            hwnd, hinst, "STATIC", "",
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_BITMAP_RAW),
            12, font_normal,
        );
        // STM_SETIMAGE:WPARAM=IMAGE_BITMAP(0),LPARAM=HBITMAP;返回旧位图(首次为 0)
        let old = SendMessageW(ctl, STM_SETIMAGE, WPARAM(0), LPARAM(bmp.0 as isize)).0;
        if old != 0 {
            let _ = DeleteObject(HGDIOBJ(old as *mut _));
        }
        let hint_text = format!("弹窗内显示已缩放到 ≤{}×{} px;「查看原图」可用系统查看器放大", IMG_MAX_W, IMG_MAX_H);
        let hint = create_control(hwnd, hinst, "STATIC", &hint_text, style_static(), IDC_MSG + 1, font_normal);
        (Some(ctl), Some(hint))
    } else {
        (None, None)
    };
    let t1 = create_control(
        hwnd, hinst, "STATIC",
        &format!("提出时间: {}   超时截止: {}", p.started_clock(), p.deadline_clock()),
        style_static(), IDC_T1, font_normal,
    );
    let t2 = create_control(
        hwnd, hinst, "STATIC", &countdown_text(0, p.timeout_ms),
        style_static(), IDC_T2, font_normal,
    );
    let input = create_control(
        hwnd, hinst, "EDIT", "可输入验证码/动态码/说明;留空点选项或提交",
        style_input_edit(), IDC_INPUT, font_normal,
    );
    let mut opts = Vec::new();
    for (i, opt) in p.options.iter().enumerate() {
        opts.push(create_control(
            hwnd, hinst, "BUTTON", opt,
            style_button(false), IDC_OPT_BASE + i as i32, font_normal,
        ));
    }
    let cancel = create_control(hwnd, hinst, "BUTTON", "取消", style_button(false), IDCANCEL_ID, font_normal);
    let submit = create_control(hwnd, hinst, "BUTTON", "提交", style_button(true), IDOK_ID, font_normal);
    let zoom = create_control(hwnd, hinst, "BUTTON", "⛶ 最大化", style_button(false), IDC_ZOOM, font_normal);
    let orig = create_control(hwnd, hinst, "BUTTON", "查看原图", style_button(false), IDC_ORIG, font_normal);

    // ---- 状态入位 + 显示 + 初始布局 ----
    *state() = Some(WinState {
        hwnd,
        title,
        msg,
        image,
        img_hint,
        t1,
        t2,
        input,
        opts,
        cancel,
        submit,
        orig,
        zoom,
        options: p.options.clone(),
        image_open_path,
        img_h: img_disp_h,
        image_bitmap: bitmap_keep,
        zoomed: false,
        started: Instant::now(),
        timeout_ms: p.timeout_ms,
        done: false,
        result: None,
    });
    relayout();

    SetTimer(hwnd, TIMER_ID, 1000, None);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(input); // 输入框初始焦点

    // ---- 消息循环(IsDialogMessage:Tab/Enter/Esc 标准路由) ----
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    loop {
        let r = GetMessageW(&mut msg, HWND::default(), 0, 0);
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        if !IsDialogMessageW(hwnd, &msg).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // ---- 收尾(进程即将退出,清理以防御性为主) ----
    let result = {
        let mut guard = state();
        guard
            .as_mut()
            .and_then(|st| st.result.take())
            .unwrap_or_else(|| ("cancel".to_string(), String::new()))
    };
    let (bmp_handle, font_handles) = {
        let guard = state();
        let st = guard.as_ref();
        (
            st.and_then(|st| st.image_bitmap),
            (font_normal, font_bold),
        )
    };
    *state() = None;
    if let Some(bmp) = bmp_handle {
        let _ = DeleteObject(HGDIOBJ(bmp.0));
    }
    let _ = DeleteObject(HGDIOBJ(font_handles.0 .0));
    let _ = DeleteObject(HGDIOBJ(font_handles.1 .0));
    result
}

fn style_static() -> WINDOW_STYLE {
    WS_CHILD | WS_VISIBLE
}
fn style_msg_edit() -> WINDOW_STYLE {
    WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE as u32) | WINDOW_STYLE(ES_READONLY as u32) | WINDOW_STYLE(ES_AUTOVSCROLL as u32) | WS_VSCROLL
}
fn style_input_edit() -> WINDOW_STYLE {
    WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32)
}
fn style_button(default: bool) -> WINDOW_STYLE {
    let base = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32);
    if default {
        base | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32)
    } else {
        base
    }
}

unsafe fn create_control(
    parent: HWND,
    hinst: windows::Win32::Foundation::HINSTANCE,
    class: &str,
    text: &str,
    style: WINDOW_STYLE,
    id: i32,
    font: HFONT,
) -> HWND {
    let cls = to_wide(class);
    let txt = to_wide(text);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(cls.as_ptr()),
        PCWSTR(txt.as_ptr()),
        style,
        0,
        0,
        10,
        10,
        parent,
        HMENU(id as isize as *mut _),
        hinst,
        None,
    )
    .unwrap_or_default();
    if !hwnd.0.is_null() {
        SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
    hwnd
}

unsafe fn make_font(weight: i32) -> HFONT {
    let face = to_wide(FACE);
    CreateFontW(
        -12, // 9pt @ 96DPI
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
        PCWSTR(face.as_ptr()),
    )
}

/// PNG → RGBA 等比重采样(≤IMG_MAX_W×IMG_MAX_H)→ BGRA → DIB Section 位图。
/// 返回 (位图, 显示高, 原图路径);失败 (None, 0, None) fail-open。
unsafe fn load_image(p: &DialogPayload) -> (Option<HBITMAP>, i32, Option<String>) {
    if p.image_path.is_empty() {
        return (None, 0, None);
    }
    let Ok(reader) = image::ImageReader::open(&p.image_path) else {
        return (None, 0, None);
    };
    let Ok(img) = reader.decode() else {
        return (None, 0, None);
    };
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return (None, 0, None);
    }
    let scale = (IMG_MAX_W as f64 / w as f64).min(IMG_MAX_H as f64 / h as f64).min(1.0);
    let (dw, dh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let rgba_img = img.to_rgba8();
    let resized = image::imageops::resize(&rgba_img, dw, dh, image::imageops::FilterType::Lanczos3);
    let (tw, th) = resized.dimensions();
    let mut bgra: Vec<u8> = resized.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2); // RGBA → BGRA
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: tw as i32,
            biHeight: -(th as i32), // 负高 = top-down,与屏幕行序一致
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let Ok(bmp) = CreateDIBSection(
        windows::Win32::Graphics::Gdi::HDC::default(),
        &bmi,
        DIB_RGB_COLORS,
        &mut bits,
        windows::Win32::Foundation::HANDLE::default(),
        0,
    ) else {
        return (None, 0, None);
    };
    if bits.is_null() {
        return (None, 0, None);
    }
    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len());
    (Some(bmp), th as i32, Some(p.image_path.clone()))
}

// ---------- 布局单测(纯函数) ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_bottom_row_pinned_and_in_bounds() {
        for opt_count in [1usize, 2, 6] {
            for geo in [
                Geometry { msg_h: 90, img_h: 0 },
                Geometry { msg_h: 90, img_h: 60 },
                Geometry { msg_h: 240, img_h: 300 },
            ] {
                let ch = content_height(geo, opt_count);
                let f = layout_frames(W, ch, geo, opt_count);
                // 底部按钮行钉在底边
                let row = f.cancel.top;
                assert_eq!(f.submit.top, row);
                assert_eq!(f.orig.top, row);
                assert_eq!(f.zoom.top, row);
                assert_eq!(f.cancel.bottom, ch - PAD);
                // 全控件在界内
                let mut all = vec![
                    ("title", f.title), ("msg", f.msg), ("t1", f.t1), ("t2", f.t2),
                    ("input", f.input), ("cancel", f.cancel), ("submit", f.submit),
                    ("orig", f.orig), ("zoom", f.zoom),
                ];
                for (i, r) in f.opts.iter().enumerate() {
                    all.push((Box::leak(format!("opt{i}").into_boxed_str()) as &str, *r));
                }
                for (name, r) in all {
                    assert!(r.left >= 0 && r.top >= 0, "{name} 负坐标 {r:?}");
                    assert!(r.right <= W && r.bottom <= ch, "{name} 越界 {r:?} (W={W}, ch={ch})");
                }
                // 最大化形态(大客户区)也在界内且底行仍钉底
                let big_geo = Geometry { msg_h: msg_h_for(1200), img_h: geo.img_h };
                let big = layout_frames(1400, 1200, big_geo, opt_count);
                assert_eq!(big.cancel.bottom, 1200 - PAD);
                assert!(big.zoom.right <= 1400);
                assert!(big.zoom.left > big.submit.right, "右组与左组不应重叠");
            }
        }
    }

    #[test]
    fn msg_h_adapts_but_bounded() {
        assert_eq!(msg_h_for(300), 90);
        assert_eq!(msg_h_for(1090), 240);
        assert_eq!(msg_h_for(2000), 240);
    }
}
