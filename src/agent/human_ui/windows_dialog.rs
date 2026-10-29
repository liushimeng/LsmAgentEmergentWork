//! Windows 弹窗后端:Rust 直调 Win32 API(第 137 轮原生重构)。
//!
//! 设计要点见 `src/agent/human_ui/mod.rs` 模块文档头。要点:
//! - 只在 `laew __hitl-dialog` 子进程内运行;窗口类 + 标准控件(BUTTON/EDIT/STATIC)
//!   + `IsDialogMessageW` 消息循环(Tab 导航 / Enter=默认按钮 / Esc=取消);
//! - **WM_SIZE 全量重排** —— 最大化/还原/拖拽缩放共享一条布局路径(坐标由纯函数
//!   从「当前客户区尺寸」推导),这是 Windows 侧「最大化后无响应」的根治点
//!   (旧 ps1 靠 WinForms Anchor,不重算);
//! - 2026-10-13:验证码图片区随弹窗验证码功能移除(SS_BITMAP/STM_SETIMAGE/
//!   image crate 解码链整链删除,零外部进程路径保持);「⛶ 最大化」按钮同步移除
//!   (系统标题栏保留 WS_MAXIMIZEBOX,WM_SIZE 全量重排天然覆盖);
//! - 2026-10-14:**macOS 风格主题接入**(`windows_theme.rs` 唯一真源,设计见
//!   `src/agent/human_ui/windows_theme.rs`):窗口类背景/按钮
//!   owner-draw/WM_CTLCOLOR*/DWM chrome/每秒进程窗口自遍历全部经主题模块;
//!   多选项勾选态由 `WinState::opt_checked` 台账单一真源(不再依赖系统勾选态)。
//!   杀开关 `LAEW_WIN_THEME=off` 回退系统原生观感;
//! - DPI:保持系统默认虚拟化(布局常量与 macOS 侧同值,见 macos_dialog.rs)。

use std::sync::Mutex;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetSysColorBrush, HDC, HGDIOBJ, InvalidateRect,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, DEFAULT_CHARSET, DEFAULT_PITCH,
    FF_SWISS, FW_BOLD, FW_NORMAL, OUT_DEFAULT_PRECIS, HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
// 2026-10-14:owner-draw 自绘(WM_DRAWITEM 载荷 + ODS 状态位);多选项勾选态改由
// opt_checked 台账单一真源,IsDlgButtonChecked/BST_CHECKED 不再使用。
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FlashWindowEx,
    GetClientRect, GetDlgCtrlID, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HWND_TOPMOST,
    IsDialogMessageW, LoadCursorW, MoveWindow, PostQuitMessage, RegisterClassExW, SendMessageW,
    SetTimer, SetWindowPos, ShowWindow, TranslateMessage, BS_AUTOCHECKBOX,
    BS_DEFPUSHBUTTON, BS_OWNERDRAW, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, FLASHW_ALL, FLASHW_TIMERNOFG,
    FLASHWINFO, HMENU, IDC_ARROW, SW_SHOW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_CTLCOLOREDIT, WM_DESTROY, WM_DRAWITEM,
    WM_SETFONT, WM_SETTEXT, WM_SETTINGCHANGE, WM_SIZE, WM_TIMER, WNDCLASSEXW, WS_CHILD,
    WS_EX_TOPMOST, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_SYSMENU, WS_TABSTOP,
    WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};
// 2026-10-13(二):侵入式触控板的鼠标/焦点/显示 API(SetCapture/ReleaseCapture/
// GetFocus 属 KeyboardAndMouse 模块;窗口消息常量属 WindowsAndMessaging)。
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, ReleaseCapture, SetCapture, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    SW_HIDE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WS_BORDER,
};

use super::dialog_main::{countdown_text, DialogPayload};
// 2026-10-14:macOS 风格主题(观感唯一真源;窗口代码只按控件 ID 决定「画什么」)。
use super::windows_theme::{theme, BtnKind, Surface};

// ---------- 布局常量(与 macos_dialog.rs 保持同值;两平台观感一致) ----------
// 2026-10-13:底行六键位 = 取消/继续/⏱+2分钟/解除屏蔽·恢复屏蔽(互切)‖ 复制。

const W: i32 = 680;
const PAD: i32 = 16;
const TITLE_H: i32 = 22;
const LINE_H: i32 = 18;
const INPUT_H: i32 = 28;
const BTN_H: i32 = 30;
const GAP: i32 = 8;
const CANCEL_W: i32 = 84;
/// 主按钮「继续」(空输入=选项1交棒;有输入=把补充说明交给 Agent)。
const SUBMIT_W: i32 = 96;
const EXTEND_W: i32 = 88;
/// 2026-10-13:「解除屏蔽/恢复屏蔽」互切按钮。
const GUARD_W: i32 = 96;
/// 2026-10-13(二):「🎮 侵入式」互切按钮(与 macOS 侧同值;(三)88→100 防退出态标题截断)。
const INTRUSIVE_W: i32 = 100;
const COPY_W: i32 = 92;
const INNER: i32 = 8;

const MSG_H_MIN: f64 = 90.0;
const MSG_H_MAX: f64 = 240.0;

// 控件 ID(WM_COMMAND 的 wParam 低位;选项 = 100 + 下标,tag 语义与 macOS 对齐)
const IDC_TITLE: i32 = 10;
const IDC_MSG: i32 = 11;
const IDC_T1: i32 = 13;
const IDC_T2: i32 = 14;
const IDC_INPUT: i32 = 15;

/// 2026-10-12:输入框提示语。**绝不能**作为 EDIT 控件的初始窗口文本 —— 那样
/// `GetWindowText` 会把提示语读成人工答复(见 [`read_input`] 与 submit 分支)。
///
/// 实测事故(`llaew_20261012_215443.log`,文心一言登录 HITL):本框此前以提示语
/// 预填,导致输入框**永远不为空** → 无输入提交时
/// `if !typed.is_empty()` 恒命中 → 把提示语原文当 `human_response` 回填 →
/// SubAgent 读成「人工已完成登录操作,页面已解锁」,**凭一句 UI 提示编造了登录事实**。
/// 注释里写明的「空输入 = 选项1『我已完成』」双语义在 Windows 上根本不可达。
///
/// 改用 Win10+ 原生 `EM_SETCUEBANNER`(真占位,不属于控件文本,`GetWindowText` 读不到)。
/// macOS 侧本就用的 `setPlaceholderString:`(真占位),不受影响 —— 本条是 Windows 独有缺陷。
/// 2026-10-13:文案去验证码化(通用补充说明语义)。
const INPUT_CUE_BANNER: &str = "可选:给 Agent 的补充说明;留空点『继续』交还控制";

/// `EM_SETCUEBANNER`(Win10 1809+)。置位 wParam=TRUE 显示占位符,
/// lParam = 指向宽字符串的指针;FALSE 则清除。
const EM_SETCUEBANNER: u32 = 0x1501;
/// 提交按钮 / Enter(IsDialogMessage 路由)。windows crate 的 IDOK 是
/// MESSAGEBOX_RESULT 类型,控件 ID 需 i32,取裸值。
const IDOK_ID: i32 = 1;
const IDCANCEL_ID: i32 = 2;
const IDC_OPT_BASE: i32 = 100;
/// 第 139 轮:「📋 复制」一键把完整信息写入剪贴板。
const IDC_COPY: i32 = 108;
/// 第 145 轮:「⏱ +2分钟」延长等待(stdout extend 中间行 → 父进程 hub 延长)。
const IDC_EXTEND: i32 = 109;
/// 2026-10-13:「解除屏蔽/恢复屏蔽」互切(stdout guard 中间行 → 父进程 hub 落页)。
const IDC_GUARD: i32 = 110;
/// 2026-10-13(二):「🎮 侵入式」互切按钮 / 触控板子窗口。
const IDC_INTRUSIVE: i32 = 111;
const IDC_PAD: i32 = 112;
/// 「✓ 已复制」反馈保持时长(秒);由既有 1s 定时器回滚。
const COPY_FEEDBACK_SECS: u64 = 2;
const COPY_BTN_TITLE: &str = "📋 复制";
const COPY_BTN_DONE: &str = "✓ 已复制";
const TIMER_ID: usize = 1;

const CLASS_NAME: &str = "LAEWHITLDIALOG";
/// 2026-10-13(二):侵入式触控板窗口类(独立窗口过程:鼠标按下/拖动/抬起 + 按键)。
const PAD_CLASS_NAME: &str = "LAEWHITLRELAYPAD";
const FACE: &str = "Microsoft YaHei UI";

// ---------- 布局纯函数(Windows 坐标:左上原点,y 向下) ----------

#[derive(Clone, Copy, Debug, Default)]
struct Geometry {
    msg_h: i32,
}

#[derive(Clone, Debug, Default)]
struct Frames {
    title: RECT,
    msg: RECT,
    t1: RECT,
    t2: RECT,
    input: RECT,
    opts: Vec<RECT>,
    cancel: RECT,
    submit: RECT,
    extend: RECT,
    guard: RECT,
    intrusive: RECT,
    copy: RECT,
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
    let t1_y = place(LINE_H);
    let t2_y = place(LINE_H);
    let input_y = place(INPUT_H);
    let opts = (0..opt_count).map(|_| rect(PAD, place(BTN_H), text_w, BTN_H)).collect();
    // 底部按钮行:左 取消/继续/⏱+2分钟/管控切换,右 复制;钉在底边
    // (任何 WM_SIZE 后重算)
    let row_y = (ch - PAD - BTN_H).max(y);
    Frames {
        title: rect(PAD, title_y, text_w, TITLE_H),
        msg: rect(PAD, msg_y, text_w, geo.msg_h),
        t1: rect(PAD, t1_y, text_w, LINE_H),
        t2: rect(PAD, t2_y, text_w, LINE_H),
        input: rect(PAD, input_y, text_w, INPUT_H),
        opts,
        cancel: rect(PAD, row_y, CANCEL_W, BTN_H),
        submit: rect(PAD + CANCEL_W + INNER, row_y, SUBMIT_W, BTN_H),
        extend: rect(PAD + CANCEL_W + INNER + SUBMIT_W + INNER, row_y, EXTEND_W, BTN_H),
        guard: rect(
            PAD + CANCEL_W + INNER + SUBMIT_W + INNER + EXTEND_W + INNER,
            row_y,
            GUARD_W,
            BTN_H,
        ),
        intrusive: rect(
            PAD + CANCEL_W + INNER + SUBMIT_W + INNER + EXTEND_W + INNER + GUARD_W + INNER,
            row_y,
            INTRUSIVE_W,
            BTN_H,
        ),
        copy: rect(cw - PAD - COPY_W, row_y, COPY_W, BTN_H),
    }
}

/// 2026-10-13(二):侵入式触控板矩形(纯函数,由 [`Frames`] 派生)——
/// 覆盖说明区 + 候选选项区;与 [`layout_frames`] 同源参数,不回读窗口尺寸。
fn pad_frame(f: &Frames, opt_count: usize) -> RECT {
    let top = f.msg.top;
    let mut bottom = f.msg.bottom;
    if opt_count > 0 {
        bottom = bottom.max(f.opts[opt_count - 1].bottom);
    }
    rect(
        f.msg.left,
        top,
        f.msg.right - f.msg.left,
        (bottom - top).max(f.msg.bottom - f.msg.top),
    )
}

/// 预算纯函数:由内容推出所需**客户区**高(与 [`layout_frames`] 消费序列逐项对应)。
fn content_height(geo: Geometry, opt_count: usize) -> i32 {
    let mut consumed = PAD + TITLE_H + GAP + geo.msg_h + GAP;
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
    t1: HWND,
    t2: HWND,
    input: HWND,
    opts: Vec<HWND>,
    cancel: HWND,
    submit: HWND,
    extend: HWND,
    guard: HWND,
    /// 2026-10-13(二):「🎮 侵入式」互切按钮(句柄;模式位另见 `intrusive: bool`,
    /// 与 macOS 版 views.intrusive / st.intrusive 的拆分对齐)。
    intrusive_btn: HWND,
    /// 2026-10-13(二):侵入式触控板子窗口(缺省隐藏)。
    pad: HWND,
    copy: HWND,
    options: Vec<String>,
    /// 「📋 复制」写入剪贴板的文本(第 139 轮)。
    copy_payload: String,
    /// 「✓ 已复制」反馈的回滚时刻(由 on_tick 消费)。
    copied_at: Option<Instant>,
    /// 第 145 轮:请求 id / 提出时刻毫秒 —— extend 中间行携带 id;「超时截止」
    /// 时间轴行随延长重写。
    payload_id: u64,
    started_at_ms: u64,
    /// 已点「⏱ +2分钟」次数(按钮标题 ×N 反馈)。
    extend_count: u32,
    /// 2026-10-13:页面当前是否处于放行态 —— 「解除屏蔽/恢复屏蔽」互切按钮的
    /// 当前标题侧(false=「🔓 解除屏蔽」,true=「🔒 恢复屏蔽」)。
    guard_unblocked: bool,
    /// 2026-10-13:选项是否多选(BS_AUTOCHECKBOX 复选框组);收口时收集勾选。
    multi_select: bool,
    /// 2026-10-14:多选项勾选台账(**单一真源**)。macOS 风格主题下选项是 owner-draw
    /// 开关(不持有系统勾选态),点击在 [`on_command`] 翻转本台账 + 重绘;杀开关 off
    /// 回退 BS_AUTOCHECKBOX(系统自翻转)时同样翻转本台账 —— 两种模式收口路径
    /// 完全一致(只读本台账),不存在台账与系统勾选态漂移。
    opt_checked: Vec<bool>,
    /// 2026-10-13(二):是否处于「侵入式」触控板模式。
    intrusive: bool,
    /// 2026-10-13(二):触控板内是否处于按下拖动中(WM_MOUSEMOVE 只在按下时中继)。
    pad_down: bool,
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
    t1: HWND,
    t2: HWND,
    input: HWND,
    opts: Vec<HWND>,
    cancel: HWND,
    submit: HWND,
    extend: HWND,
    guard: HWND,
    intrusive: HWND,
    pad: HWND,
    copy: HWND,
}

impl Controls {
    fn from(st: &WinState) -> Self {
        Self {
            title: st.title,
            msg: st.msg,
            t1: st.t1,
            t2: st.t2,
            input: st.input,
            opts: st.opts.clone(),
            cancel: st.cancel,
            submit: st.submit,
            extend: st.extend,
            guard: st.guard,
            intrusive: st.intrusive_btn,
            pad: st.pad,
            copy: st.copy,
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
            // 2026-10-14:每秒自遍历本进程顶层窗口补挂 chrome(复用既有 1s 定时器,
            // 零新定时器)—— 日后新增窗口即使创建路径忘了接入,最多 1s 后被兜底。
            super::windows_theme::sweep_process_windows();
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            on_command(id);
            LRESULT(0)
        }
        // 2026-10-14:owner-draw 自绘入口(所有 BS_OWNERDRAW 控件的 macOS 风格绘制)。
        WM_DRAWITEM => {
            on_draw_item(lparam);
            LRESULT(1) // 已处理(Win32 约定:处理 WM_DRAWITEM 返回 TRUE)
        }
        // 2026-10-14:文本类控件配色(标题/时间轴/说明区/输入框);主题关闭时回退
        // 系统默认(DefWindowProc)。lParam = 子控件句柄,wParam = 其 DC。
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            if theme().enabled {
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let child = HWND(lparam.0 as *mut core::ffi::c_void);
                let ctrl_id = GetDlgCtrlID(child);
                let surface = if msg == WM_CTLCOLOREDIT {
                    Surface::Card // 输入框:卡片底 + 主文本色
                } else {
                    match ctrl_id {
                        IDC_TITLE => Surface::Title,
                        IDC_T1 | IDC_T2 => Surface::Timeline,
                        IDC_MSG => Surface::Card, // 只读 EDIT 说明区走 WM_CTLCOLORSTATIC
                        _ => Surface::Timeline,
                    }
                };
                let brush = super::windows_theme::ctl_color(hdc, surface);
                LRESULT(brush.0 as isize)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        // 2026-10-14:系统外观/设置变更广播 → 重申本进程全部窗口 chrome。
        WM_SETTINGCHANGE => {
            super::windows_theme::sweep_process_windows();
            DefWindowProcW(hwnd, msg, wparam, lparam)
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
    let (hwnd, controls, opt_count, intrusive) = {
        let guard = state();
        let Some(st) = guard.as_ref() else { return };
        (st.hwnd, Controls::from(st), st.options.len(), st.intrusive)
    };
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let cw = rc.right - rc.left;
    let ch = rc.bottom - rc.top;
    let geo = Geometry { msg_h: msg_h_for(ch) };
    let frames = layout_frames(cw, ch, geo, opt_count);
    unsafe { apply_frames(&controls, &frames) };
    // 2026-10-13(二):侵入式触控板矩形按同一布局结果派生(msg + 选项区)。
    if intrusive && !controls.pad.0.is_null() {
        let pf = pad_frame(&frames, opt_count);
        unsafe {
            let _ = MoveWindow(
                controls.pad,
                pf.left,
                pf.top,
                pf.right - pf.left,
                pf.bottom - pf.top,
                true,
            );
        }
    }
}

unsafe fn apply_frames(c: &Controls, f: &Frames) {
    let mv = |hwnd: HWND, r: RECT| {
        if !hwnd.0.is_null() {
            let _ = MoveWindow(hwnd, r.left, r.top, r.right - r.left, r.bottom - r.top, true);
        }
    };
    mv(c.title, f.title);
    mv(c.msg, f.msg);
    mv(c.t1, f.t1);
    mv(c.t2, f.t2);
    mv(c.input, f.input);
    for (hwnd, r) in c.opts.iter().zip(f.opts.iter()) {
        mv(*hwnd, *r);
    }
    mv(c.cancel, f.cancel);
    mv(c.submit, f.submit);
    mv(c.extend, f.extend);
    mv(c.guard, f.guard);
    mv(c.intrusive, f.intrusive);
    mv(c.copy, f.copy);
}

// ---------- 2026-10-13(二):侵入式触控板(独立窗口类 + 独立窗口过程) ----------

/// 触控板事件 → relay 中间行(归一化坐标 0..1,页面 y 轴向下 = Windows 客户区
/// 方向一致,无需翻转)。非侵入式/收口后丢弃。
fn pad_relay_at(lparam: LPARAM, kind: &str) {
    let (id, hwnd, down) = {
        let guard = state();
        match guard.as_ref() {
            Some(st) if st.intrusive && !st.done => (st.payload_id, st.pad, st.pad_down),
            _ => return,
        }
    };
    if hwnd.0.is_null() {
        return;
    }
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let w = (rc.right - rc.left).max(1) as f64;
    let h = (rc.bottom - rc.top).max(1) as f64;
    let x = (lparam.0 & 0xFFFF) as i16 as f64;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
    let _ = down;
    let nx = (x / w).clamp(0.0, 1.0);
    let ny = (y / h).clamp(0.0, 1.0);
    super::dialog_main::emit_line_or_exit(&super::dialog_main::relay_line(id, kind, nx, ny, ""));
}

unsafe extern "system" fn pad_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            let _ = SetFocus(hwnd);
            let _ = SetCapture(hwnd);
            {
                let mut guard = state();
                if let Some(st) = guard.as_mut() {
                    st.pad_down = true;
                }
            }
            pad_relay_at(lparam, "down");
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let dragging = {
                let guard = state();
                guard.as_ref().map(|st| st.pad_down).unwrap_or(false)
            };
            if dragging {
                pad_relay_at(lparam, "move");
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            {
                let mut guard = state();
                if let Some(st) = guard.as_mut() {
                    st.pad_down = false;
                }
            }
            let _ = ReleaseCapture();
            pad_relay_at(lparam, "up");
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u32;
            let name = match vk {
                0x0D => "enter",
                0x1B => "esc",
                0x09 => "tab",
                0x20 => "space",
                0x08 => "backspace",
                0x2E => "delete",
                0x25 => "left",
                0x26 => "up",
                0x27 => "right",
                0x28 => "down",
                _ => "",
            };
            if name.is_empty() {
                return LRESULT(0);
            }
            let id = {
                let guard = state();
                match guard.as_ref() {
                    Some(st) if st.intrusive && !st.done => st.payload_id,
                    _ => return LRESULT(0),
                }
            };
            super::dialog_main::emit_line_or_exit(&super::dialog_main::relay_line(id, "key", 0.0, 0.0, name));
            LRESULT(0)
        }
        // 2026-10-14:触控板提示 STATIC 的父窗是触控板本身,配色在此处理
        // (次要文本色 + 卡片底;主题关闭回退系统默认)。
        WM_CTLCOLORSTATIC => {
            if theme().enabled {
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let brush = super::windows_theme::ctl_color(hdc, Surface::CardSecondary);
                LRESULT(brush.0 as isize)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 2026-10-13(二):「🎮 侵入式」互切 —— 进入时隐藏说明区/选项区/输入框、显示
/// 触控板并打 `session_start`;退出时还原并打 `session_end`。
fn relay_toggle() {
    let (line, title, btn, pad, msg, input, opts, now_on) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        st.intrusive = !st.intrusive;
        st.pad_down = false;
        let now_on = st.intrusive;
        let line = if now_on {
            super::dialog_main::relay_line(st.payload_id, "session_start", 0.0, 0.0, "")
        } else {
            super::dialog_main::relay_line(st.payload_id, "session_end", 0.0, 0.0, "")
        };
        let title = if now_on {
            super::dialog_main::INTRUSIVE_BTN_EXIT_TITLE
        } else {
            super::dialog_main::INTRUSIVE_BTN_TITLE
        };
        (line, title, st.intrusive_btn, st.pad, st.msg, st.input, st.opts.clone(), now_on)
    };
    super::dialog_main::emit_line_or_exit(&line);
    unsafe {
        // 进入:隐藏说明区/选项区/输入框,显示触控板并聚焦;退出:反向还原
        // (触控板矩形由 relayout 按 pad_frame 重算)。
        let _ = ShowWindow(msg, if now_on { SW_HIDE } else { SW_SHOW });
        for o in &opts {
            let _ = ShowWindow(*o, if now_on { SW_HIDE } else { SW_SHOW });
        }
        let _ = ShowWindow(input, if now_on { SW_HIDE } else { SW_SHOW });
        if !pad.0.is_null() {
            let _ = ShowWindow(pad, if now_on { SW_SHOW } else { SW_HIDE });
            if now_on {
                let _ = SetFocus(pad);
            }
        }
        if !btn.0.is_null() {
            let t = to_wide(title);
            SendMessageW(btn, WM_SETTEXT, WPARAM(0), LPARAM(t.as_ptr() as isize));
        }
    }
    relayout();
}

// ---------- 事件处理 ----------

fn on_tick() {
    // 先锁定取出所需并解锁,再触碰 UI(finish 会再次锁定 STATE)。
    let (t2, text, timed_out, rollback_copy) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        let elapsed = st.started.elapsed().as_millis() as u64;
        let remain = st.timeout_ms.saturating_sub(elapsed);
        // 第 139 轮:「✓ 已复制」反馈到期回滚标题(不额外起定时器,复用 1s tick)。
        let rollback = match st.copied_at {
            Some(at) if at.elapsed().as_secs() >= COPY_FEEDBACK_SECS => {
                st.copied_at = None;
                Some(st.copy)
            }
            _ => None,
        };
        (
            st.t2,
            countdown_text(elapsed, remain),
            remain == 0,
            rollback,
        )
    };
    let wide = to_wide(&text);
    unsafe {
        SendMessageW(t2, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
    }
    if let Some(btn) = rollback_copy {
        let wide = to_wide(COPY_BTN_TITLE);
        unsafe {
            SendMessageW(btn, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
    }
    if timed_out {
        finish("timeout", -1);
    }
}

/// 第 139 轮:「📋 复制」—— 完整信息写入系统剪贴板。
///
/// 说明区虽是 `ES_READONLY` 的 EDIT(可 Ctrl-A/Ctrl-C),但标题/时间轴/倒计时都是
/// `STATIC`(不可选中),倒计时行每秒刷新又会冲掉拖选 —— 一键复制是确定性最高的路径。
fn copy_info() {
    let (payload, btn) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        st.copied_at = Some(Instant::now());
        (st.copy_payload.clone(), st.copy)
    };
    let ok = unsafe { set_clipboard_text(&payload) };
    let wide = to_wide(if ok { COPY_BTN_DONE } else { "⚠ 复制失败" });
    unsafe {
        SendMessageW(btn, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
    }
}

/// `CF_UNICODETEXT` = 13(数值来自 SDK 头文件 windows.h,勿改);只为一个常量
/// 多开 `Win32_System_Ole` feature 不值,故用字面量。
const CF_UNICODETEXT: u16 = 13;

/// UTF-16 文本 → 系统剪贴板(fail-open:失败只反映在按钮标题上)。
unsafe fn set_clipboard_text(text: &str) -> bool {
    if OpenClipboard(HWND::default()).is_err() {
        return false;
    }
    let mut ok = false;
    if EmptyClipboard().is_ok() {
        let wide = text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let bytes = wide.len() * std::mem::size_of::<u16>();
        // GMEM_MOVEABLE:剪贴板接管内存所有权,LocalFree 由系统负责。
        if let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let p = GlobalLock(handle);
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), p.cast::<u8>(), bytes);
                let _ = GlobalUnlock(handle);
                ok = SetClipboardData(CF_UNICODETEXT as u32, HANDLE(handle.0)).is_ok();
            }
        }
    }
    let _ = CloseClipboard();
    ok
}

fn on_command(id: i32) {
    match id {
        IDOK_ID => finish("answer", -1),     // 继续按钮 / Enter(默认按钮)
        IDCANCEL_ID => finish("cancel", -1), // 取消按钮 / Esc
        IDC_EXTEND => extend_wait(),
        IDC_GUARD => guard_toggle(),
        // 2026-10-13(二):「🎮 侵入式/⏹ 退出侵入式」互切(触控板物理输入中继)。
        IDC_INTRUSIVE => relay_toggle(),
        IDC_COPY => copy_info(),
        // 2026-10-13:多选模式选项点击只翻转勾选态(不收口,收口动作 = 「继续」收集
        // 勾选);单选保持 push button 点击即应答。
        // 2026-10-14:勾选态由 opt_checked 台账单一真源 —— 主题开启时选项是
        // owner-draw macOS 开关(点击翻转台账 + 重绘);杀开关 off 时系统
        // BS_AUTOCHECKBOX 自翻转,台账同步翻转,收口路径两种模式完全一致。
        _ if id >= IDC_OPT_BASE => {
            let (is_multi, flip_target) = {
                let mut guard = state();
                let Some(st) = guard.as_mut() else { return };
                if st.done {
                    return;
                }
                let mut flip_target = None;
                if st.multi_select {
                    let idx = (id - IDC_OPT_BASE) as usize;
                    if idx < st.opts.len() && idx < st.opt_checked.len() {
                        st.opt_checked[idx] = !st.opt_checked[idx];
                        flip_target = Some(st.opts[idx]);
                    }
                }
                (st.multi_select, flip_target)
            };
            if is_multi {
                if theme().enabled {
                    if let Some(option_hwnd) = flip_target {
                        // SAFETY:option_hwnd 是本弹窗子控件句柄,STATE 生命周期内有效。
                        let _ = unsafe { InvalidateRect(option_hwnd, None, true) };
                    }
                }
            } else {
                finish("answer", (id - IDC_OPT_BASE) as i64);
            }
        }
        _ => {}
    }
}

/// 2026-10-14:WM_DRAWITEM 分发 —— 所有 owner-draw 控件的 macOS 风格自绘入口。
/// 图元绘制/配色在 [`super::windows_theme`];此处只按控件 ID 决定「画什么」:
/// 继续=主按钮,取消/延长/管控/侵入式/复制=次按钮,单选选项=次按钮整行,
/// 多选选项=macOS 开关行(勾选态读台账)。
unsafe fn on_draw_item(lparam: LPARAM) {
    if lparam.0 == 0 {
        return;
    }
    let dis = &*(lparam.0 as *const DRAWITEMSTRUCT);
    if dis.hDC.0.is_null() || dis.hwndItem.0.is_null() {
        return;
    }
    // ODS_* 是 ODS_FLAGS(u32 newtype)——按裸值比较,避免类型包装噪声。
    let pressed = dis.itemState.0 & ODS_SELECTED.0 != 0;
    let hot = dis.itemState.0 & ODS_HOTLIGHT.0 != 0;
    let focused = dis.itemState.0 & ODS_FOCUS.0 != 0;
    let text = window_text(dis.hwndItem);
    let ctrl_id = dis.CtlID as i32;
    match ctrl_id {
        IDOK_ID => super::windows_theme::draw_button(
            dis.hDC,
            &dis.rcItem,
            &text,
            BtnKind::Primary,
            pressed,
            hot,
            focused,
        ),
        IDCANCEL_ID | IDC_EXTEND | IDC_GUARD | IDC_INTRUSIVE | IDC_COPY => {
            super::windows_theme::draw_button(
                dis.hDC,
                &dis.rcItem,
                &text,
                BtnKind::Secondary,
                pressed,
                hot,
                focused,
            )
        }
        other if other >= IDC_OPT_BASE => {
            let (multi_select, checked) = {
                let guard = state();
                let idx = (other - IDC_OPT_BASE) as usize;
                guard
                    .as_ref()
                    .map(|st| {
                        (st.multi_select, st.opt_checked.get(idx).copied().unwrap_or(false))
                    })
                    .unwrap_or((false, false))
            };
            if multi_select {
                super::windows_theme::draw_switch_row(dis.hDC, &dis.rcItem, &text, checked, focused);
            } else {
                super::windows_theme::draw_button(
                    dis.hDC,
                    &dis.rcItem,
                    &text,
                    BtnKind::Secondary,
                    pressed,
                    hot,
                    focused,
                );
            }
        }
        _ => {}
    }
}

/// 控件当前窗口文本(自绘用快照;与 [`read_input`] 无关)。
unsafe fn window_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 第 145 轮:「⏱ +2分钟」延长等待 —— 本地 `timeout_ms += EXTEND_STEP_MS`(总等待
/// 封顶 MAX_TOTAL_WAIT_MS,倒计时与「超时截止」行立即受益),同时向 stdout 打
/// extend **中间行**(进程不退出),父进程逐行读并接线 hub 推迟截止时刻。
fn extend_wait() {
    let (line, title, extend, t1, timeline) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        let cap = super::dialog_main::MAX_TOTAL_WAIT_MS.saturating_sub(st.timeout_ms);
        let add = super::dialog_main::EXTEND_STEP_MS.min(cap);
        if add == 0 {
            return; // 已达 30 分钟总上限:不再累计、不打印中间行
        }
        st.timeout_ms += add;
        st.extend_count += 1;
        let clock = |ms: u64| {
            super::fmt_local_ms(ms).split(' ').nth(1).unwrap_or("").to_string()
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let base = if st.started_at_ms > 0 { st.started_at_ms } else { now_ms };
        let timeline = format!(
            "提出时间: {}   超时截止: {}",
            clock(base),
            clock(base.saturating_add(st.timeout_ms))
        );
        (
            super::dialog_main::extend_line(st.payload_id, add),
            format!("{} ×{}", super::dialog_main::EXTEND_BTN_TITLE, st.extend_count),
            st.extend,
            st.t1,
            timeline,
        )
    };
    super::dialog_main::emit_line_or_exit(&line); // 行缓冲:换行即 flush,父进程逐行读
    let title_w = to_wide(&title);
    let timeline_w = to_wide(&timeline);
    unsafe {
        SendMessageW(extend, WM_SETTEXT, WPARAM(0), LPARAM(title_w.as_ptr() as isize));
        SendMessageW(t1, WM_SETTEXT, WPARAM(0), LPARAM(timeline_w.as_ptr() as isize));
    }
}

/// 2026-10-13:「解除屏蔽/恢复屏蔽」互切 —— 与 macOS 侧 `on_guard_toggle` 同构:
/// 点击即翻转本地态 + 向 stdout 打 guard **中间行**(进程不退出;父进程逐行读并
/// 接线 `HumanAssistHub::guard_toggle_from_ui` → 对关联 page_id 应用整页 open /
/// 恢复解锁前快照),按钮标题同步切换。
fn guard_toggle() {
    let (line, title, btn) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        let next = !st.guard_unblocked;
        st.guard_unblocked = next;
        (
            super::dialog_main::guard_line(st.payload_id, next),
            if next {
                super::dialog_main::GUARD_BTN_LOCK_TITLE
            } else {
                super::dialog_main::GUARD_BTN_UNLOCK_TITLE
            },
            st.guard,
        )
    };
    super::dialog_main::emit_line_or_exit(&line); // 行缓冲:换行即 flush,父进程逐行读
    let wide = to_wide(title);
    unsafe {
        SendMessageW(btn, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
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
        } else if kind == "closed" {
            ("closed".to_string(), String::new())
        } else if kind == "cancel" {
            ("cancel".to_string(), String::new())
        } else {
            let typed = unsafe { read_input(st.input) };
            if !typed.is_empty() {
                ("answer".to_string(), typed)
            } else if st.multi_select {
                // 2026-10-13:多选收口 —— 收集全部勾选(空勾选回退选项 1 交棒)。
                // 2026-10-14:勾选态读 opt_checked 台账(单一真源;主题开启的
                // owner-draw 开关与杀开关 off 的 BS_AUTOCHECKBOX 两种模式同源)。
                let picked: Vec<usize> = st
                    .opt_checked
                    .iter()
                    .enumerate()
                    .filter(|(_, &checked)| checked)
                    .map(|(idx, _)| idx)
                    .collect();
                let joined = super::dialog_main::join_multi_answer(&st.options, &picked);
                (
                    "answer".to_string(),
                    joined.unwrap_or_else(|| format!("1. {}", st.options[0])),
                )
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
        // 2026-10-13(二):侵入式会话随收口一并结束(父进程据此恢复页面管控;
        // 收口路径本身也会恢复,幂等双保险)。
        if st.intrusive && st.payload_id > 0 {
            super::dialog_main::emit_line_or_exit(&super::dialog_main::relay_line(
                st.payload_id,
                "session_end",
                0.0,
                0.0,
                "",
            ));
        }
        st.hwnd
    };
    unsafe {
        // DestroyWindow → WM_DESTROY → PostQuitMessage → 消息循环退出
        let _ = DestroyWindow(hwnd);
    }
}

/// 给 EDIT 控件设置原生占位提示(Win10 1809+)。
///
/// 用 `EM_SETCUEBANNER` 而非预填控件文本 —— 后者会被 `GetWindowText` 读回,
/// 把提示语当成人工答复(2026-10-12 实测事故)。宽字符串需在调用期间保持有效,
/// 故在此栈上持有。
unsafe fn set_cue_banner(edit: HWND, text: &str) {
    let wide = to_wide(text);
    SendMessageW(
        edit,
        EM_SETCUEBANNER,
        windows::Win32::Foundation::WPARAM(1),
        windows::Win32::Foundation::LPARAM(wide.as_ptr() as isize),
    );
}

unsafe fn read_input(input: HWND) -> String {
    let len = GetWindowTextLengthW(input);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(input, &mut buf);
    let typed = String::from_utf16_lossy(&buf[..n.max(0) as usize])
        .trim()
        .to_string();
    // 2026-10-12 防御性剥离:老系统(<Win10 1809)不支持 EM_SETCUEBANNER 时,
    // 占位符不可用(框为空) —— 不会产生误判;而若将来任何路径又把提示语写进
    // 控件文本,这里兜底保证提示语绝不会作为人工答复外泄。
    if typed == INPUT_CUE_BANNER {
        return String::new();
    }
    typed
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
    // 2026-10-14:主题在类注册前初始化(背景刷取自主题;进程生命周期常驻)。
    let ui_theme = theme();
    let class = to_wide(CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst,
        lpszClassName: PCWSTR(class.as_ptr()),
        hCursor: LoadCursorW(windows::Win32::Foundation::HINSTANCE::default(), IDC_ARROW)
            .unwrap_or_default(),
        // 2026-10-14:macOS 窗口灰底(浅 #F2F2F7 / 深 #1E1E20);杀开关 off 回退系统色。
        hbrBackground: if ui_theme.enabled {
            ui_theme.bg_brush
        } else {
            GetSysColorBrush(COLOR_WINDOW)
        },
        ..Default::default()
    };
    if RegisterClassExW(&wc) == 0 {
        return ("error".into(), "窗口类注册失败".into());
    }

    // 2026-10-13:验证码图片区已移除,无位图预解码。
    let geo = Geometry { msg_h: MSG_H_MIN as i32 };
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
    // 2026-10-14:窗口级 chrome(标题栏与客户区同色/沉浸式深色/Win11 圆角;
    // 每秒 WM_TIMER 的自遍历 sweep 兜底重申,幂等)。
    super::windows_theme::apply_window_chrome(hwnd);

    // ---- 字体与控件 ----
    let font_normal = make_font(FW_NORMAL.0 as i32);
    let font_bold = make_font(FW_BOLD.0 as i32);

    let title = create_control(hwnd, hinst, "STATIC", &p.kind_label, style_static(), IDC_TITLE, font_bold);
    let msg = create_control(hwnd, hinst, "EDIT", &p.info_text(), style_msg_edit(), IDC_MSG, font_normal);
    let t1 = create_control(
        hwnd, hinst, "STATIC",
        &format!("提出时间: {}   超时截止: {}", p.started_clock(), p.deadline_clock()),
        style_static(), IDC_T1, font_normal,
    );
    let t2 = create_control(
        hwnd, hinst, "STATIC", &countdown_text(0, p.timeout_ms),
        style_static(), IDC_T2, font_normal,
    );
    // 2026-10-12:输入框以**空文本**创建,提示语走原生 EM_SETCUEBANNER。
    // 此前把提示语预填为控件文本,导致无输入提交会把提示语当人工答复回填
    // (详见 INPUT_CUE_BANNER 文档与 read_input 的防御性剥离)。
    let input = create_control(
        hwnd, hinst, "EDIT", "",
        style_input_edit(), IDC_INPUT, font_normal,
    );
    unsafe { set_cue_banner(input, INPUT_CUE_BANNER) };
    let mut opts = Vec::new();
    for (i, opt) in p.options.iter().enumerate() {
        // 2026-10-13:多选 = BS_AUTOCHECKBOX 复选框(点击系统自翻转,收口动作是
        // 「继续」收集勾选);单选 = push button 点击即应答(现状)。
        let style = if p.multi_select {
            style_check_option()
        } else {
            style_button(false)
        };
        opts.push(create_control(
            hwnd, hinst, "BUTTON", opt,
            style, IDC_OPT_BASE + i as i32, font_normal,
        ));
    }
    let cancel = create_control(hwnd, hinst, "BUTTON", "取消", style_button(false), IDCANCEL_ID, font_normal);
    // 2026-10-13:主按钮「继续」 —— 人工操作完成后点击,把流程与控制权交还 Agent
    // (空输入=选项1「我已完成人工操作,继续」;有输入=把补充说明交给 Agent)。
    // 2026-10-16:去掉 BS_DEFPUSHBUTTON —— 实测弹窗抢键盘焦点后,用户在其它窗口
    // 敲 Enter 被 IsDialogMessageW 路由到默认按钮,20~48s 内产生「假人工应答」
    // (选项1空输入交棒),Agent 复验不过→再弹窗→再误触,人机协作闭环被机械绕过。
    // 提交必须鼠标点击;「输入框持有焦点时的 Enter」由消息循环显式转发(真交互)。
    let submit = create_control(hwnd, hinst, "BUTTON", "继续", style_button(false), IDOK_ID, font_normal);
    // 第 145 轮:「⏱ +2分钟」延长等待。
    let extend = create_control(hwnd, hinst, "BUTTON", super::dialog_main::EXTEND_BTN_TITLE, style_button(false), IDC_EXTEND, font_normal);
    // 2026-10-13:「解除屏蔽/恢复屏蔽」互切(初始标题按页面实际放行态)。
    let guard = create_control(
        hwnd,
        hinst,
        "BUTTON",
        if p.guard_unlocked {
            super::dialog_main::GUARD_BTN_LOCK_TITLE
        } else {
            // (三)文案保持原文案:「🔓 解除屏蔽」即侵入式语境的「非侵入式」入口
            // (整页开放,人工直接操作;实测改名会造成「文案跑到按钮位置上」的错位观感)。
            super::dialog_main::GUARD_BTN_UNLOCK_TITLE
        },
        style_button(false),
        IDC_GUARD,
        font_normal,
    );
    // 2026-10-13(二):「🎮 侵入式」互切按钮 + 触控板子窗口(缺省隐藏;事件经
    // pad_wnd_proc 归一化后以 relay 中间行回放为宿主可信物理输入)。
    let intrusive_btn = create_control(
        hwnd,
        hinst,
        "BUTTON",
        super::dialog_main::INTRUSIVE_BTN_TITLE,
        style_button(false),
        IDC_INTRUSIVE,
        font_normal,
    );
    // 第 139 轮:一键复制(STATIC 标签天生不可选中,拖选复制不可靠)
    let copy = create_control(hwnd, hinst, "BUTTON", COPY_BTN_TITLE, style_button(false), IDC_COPY, font_normal);

    // ---- 2026-10-13(二):侵入式触控板(独立窗口类;缺省隐藏,进入侵入式才显示) ----
    // 2026-10-14:主题启用时触控板 = 卡片观感(卡片底刷 + 无边框,与说明区一致);
    // 杀开关 off 回退系统白底 + WS_BORDER。
    let pad_class = to_wide(PAD_CLASS_NAME);
    let pad_wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(pad_wnd_proc),
        hInstance: hinst,
        lpszClassName: PCWSTR(pad_class.as_ptr()),
        hCursor: LoadCursorW(windows::Win32::Foundation::HINSTANCE::default(), IDC_ARROW)
            .unwrap_or_default(),
        hbrBackground: if ui_theme.enabled {
            ui_theme.card_brush
        } else {
            GetSysColorBrush(COLOR_WINDOW)
        },
        ..Default::default()
    };
    let _ = RegisterClassExW(&pad_wc);
    let pad_style = if ui_theme.enabled {
        WINDOW_STYLE(WS_CHILD.0)
    } else {
        WINDOW_STYLE((WS_CHILD.0 | WS_BORDER.0) as u32)
    };
    let pad = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(pad_class.as_ptr()),
        PCWSTR::null(),
        pad_style,
        0,
        0,
        10,
        10,
        hwnd,
        HMENU(IDC_PAD as isize as *mut _),
        hinst,
        None,
    )
    .unwrap_or_default();
    if !pad.0.is_null() {
        // 提示文字(可选中复制;居中偏上,不挡操作)。
        let hint = create_control(
            pad,
            hinst,
            "STATIC",
            super::dialog_main::RELAY_PAD_HINT,
            style_static(),
            IDC_PAD + 1,
            font_normal,
        );
        unsafe {
            let _ = MoveWindow(hint, 10, 8, 620, 40, true);
        }
    }

    // ---- 状态入位 + 显示 + 初始布局 ----
    *state() = Some(WinState {
        hwnd,
        title,
        msg,
        t1,
        t2,
        input,
        opts,
        cancel,
        submit,
        extend,
        guard,
        intrusive_btn,
        pad,
        copy,
        options: p.options.clone(),
        copy_payload: p.copy_text(),
        copied_at: None,
        payload_id: p.id,
        started_at_ms: p.started_at_ms,
        extend_count: 0,
        guard_unblocked: p.guard_unlocked,
        intrusive: false,
        pad_down: false,
        multi_select: p.multi_select,
        opt_checked: vec![false; p.options.len()],
        started: Instant::now(),
        timeout_ms: p.timeout_ms,
        done: false,
        result: None,
    });
    relayout();

    SetTimer(hwnd, TIMER_ID, 1000, None);
    let _ = ShowWindow(hwnd, SW_SHOW);
    // 第 154 轮:强制置 WS_VISIBLE 位。若本进程被以 STARTF_USESHOWWINDOW + SW_HIDE
    // 启动(实测 PowerShell `Start-Process -WindowStyle Hidden` / 部分 agent harness
    // 调度形态),**首次** ShowWindow(SW_SHOW) 会被 STARTUPINFO 的 wShowWindow 压制,
    // 窗口创建成功却不可见(本机探针复现:IsWindowVisible=False)。SWP_SHOWWINDOW
    // 直接改窗口样式位、不经首次 ShowWindow 通道,不受启动参数影响;TOPMOST 重申
    // 置顶(创建时已 WS_EX_TOPMOST,此处幂等)。
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
    // 2026-10-16:不再抢键盘焦点 —— 删除 SetForegroundWindow + 创建期 SetFocus。
    // 实测事故(BOSS 直聘任务):弹窗弹出瞬间抢占前台,用户正在其它窗口打字,
    // 下一次敲 Enter 落进弹窗触发默认按钮提交 →「假人工已响应」(选项1空输入
    // 交棒),复验不过→再弹窗→再误触,20~48s 一轮空转。与「焦点租约」治理的
    // 浏览器窗口抢焦是同一类病。现改:置顶显示(不激活) + 闪烁任务栏提示;人工
    // 点击弹窗任意位置后键盘焦点自然进入,输入框 Enter 提交由消息循环显式
    // 转发(见下方 WM_KEYDOWN 拦截)。
    let flash = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 4,
        dwTimeout: 0,
    };
    let _ = FlashWindowEx(&flash);
    // 2026-10-14:显示后自遍历本进程全部顶层窗口补挂 chrome(创建点单点已挂,
    // 此处为统一入口的幂等重申;此后每秒 WM_TIMER 持续兜底)。
    super::windows_theme::sweep_process_windows();

    // ---- 消息循环(IsDialogMessage:Tab/Enter/Esc 标准路由) ----
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    loop {
        let r = GetMessageW(&mut msg, HWND::default(), 0, 0);
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        // 2026-10-16:输入框持有键盘焦点时的 Enter = 提交(人工真在打字的信号)。
        // 主按钮已无 BS_DEFPUSHBUTTON,IsDialogMessageW 不再把 Enter 路由到提交;
        // 这里显式转发,保住「打完字按回车」的自然交互。焦点在其它控件/触控板
        // 时 Enter 无操作(防误触:焦点不在弹窗内的键盘消息本就到不了这里)。
        if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_RETURN.0 as usize {
            let f = GetFocus();
            let input_focused = {
                let guard = state();
                guard.as_ref().map(|st| st.input == f).unwrap_or(false)
            };
            if input_focused {
                let _ = SendMessageW(
                    hwnd,
                    WM_COMMAND,
                    WPARAM(IDOK_ID as usize),
                    LPARAM(0),
                );
                continue;
            }
        }
        // 2026-10-13(二):触控板持有焦点时跳过对话框键导航 —— 否则 Enter/Esc
        // 会被 IsDialogMessageW 当作「默认按钮/取消」吃掉,无法回放给页面
        // (滑块挑战常需 Enter 提交)。
        let focused_pad = {
            let f = GetFocus();
            !f.0.is_null() && {
                let guard = state();
                guard.as_ref().map(|st| st.pad == f).unwrap_or(false)
            }
        };
        if focused_pad || !IsDialogMessageW(hwnd, &msg).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // ---- 收尾(进程即将退出,清理以防御性为主) ----
    // 意外返回不得报「人工取消」(会误杀整个任务);兜底为 closed。
    let result = {
        let mut guard = state();
        guard
            .as_mut()
            .and_then(|st| st.result.take())
            .unwrap_or_else(|| ("closed".to_string(), String::new()))
    };
    let font_handles = (font_normal, font_bold);
    *state() = None;
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
/// 按钮样式:2026-10-14 主题启用时追加 `BS_OWNERDRAW`(macOS 风格自绘,
/// WM_DRAWITEM → windows_theme::draw_button);杀开关 off 保持系统原生按钮。
fn style_button(default: bool) -> WINDOW_STYLE {
    let mut base = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32);
    if default {
        base |= WINDOW_STYLE(BS_DEFPUSHBUTTON as u32);
    }
    if theme().enabled {
        base |= WINDOW_STYLE(BS_OWNERDRAW as u32);
    }
    base
}

/// 多选模式选项样式:2026-10-14 主题启用时为 owner-draw macOS 开关(勾选态由
/// opt_checked 台账记账);杀开关 off 回退 BS_AUTOCHECKBOX(系统自翻转)。
fn style_check_option() -> WINDOW_STYLE {
    let base = WS_CHILD | WS_VISIBLE | WS_TABSTOP;
    if theme().enabled {
        base | WINDOW_STYLE(BS_OWNERDRAW as u32)
    } else {
        base | WINDOW_STYLE(BS_AUTOCHECKBOX as u32)
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

// ---------- 布局单测(纯函数) ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_bottom_row_pinned_and_in_bounds() {
        for opt_count in [1usize, 2, 6] {
            for geo in [
                Geometry { msg_h: 90 },
                Geometry { msg_h: 240 },
            ] {
                let ch = content_height(geo, opt_count);
                let f = layout_frames(W, ch, geo, opt_count);
                // 底部按钮行钉在底边
                let row = f.cancel.top;
                assert_eq!(f.submit.top, row);
                assert_eq!(f.extend.top, row);
                assert_eq!(f.guard.top, row);
                assert_eq!(f.intrusive.top, row);
                assert_eq!(f.copy.top, row);
                assert_eq!(f.cancel.bottom, ch - PAD);
                // 2026-10-13:六键位(移除「⛶ 最大化」)后底部按钮行不得重叠
                // 2026-10-13(二):guard(非侵入式)与 copy 之间插入「侵入式」按钮。
                assert!(f.submit.right <= f.extend.left, "submit 与 extend 重叠");
                assert!(f.extend.right <= f.guard.left, "extend 与 guard 重叠");
                assert!(f.guard.right <= f.intrusive.left, "guard 与 intrusive 重叠");
                assert!(f.intrusive.right <= f.copy.left, "intrusive 与 copy 重叠");
                // 全控件在界内
                let mut all = vec![
                    ("title", f.title), ("msg", f.msg), ("t1", f.t1), ("t2", f.t2),
                    ("input", f.input), ("cancel", f.cancel), ("submit", f.submit),
                    ("extend", f.extend), ("guard", f.guard),
                    ("intrusive", f.intrusive), ("copy", f.copy),
                ];
                for (i, r) in f.opts.iter().enumerate() {
                    all.push((Box::leak(format!("opt{i}").into_boxed_str()) as &str, *r));
                }
                for (name, r) in all {
                    assert!(r.left >= 0 && r.top >= 0, "{name} 负坐标 {r:?}");
                    assert!(r.right <= W && r.bottom <= ch, "{name} 越界 {r:?} (W={W}, ch={ch})");
                }
                // 最大化形态(大客户区)也在界内且底行仍钉底
                let big_geo = Geometry { msg_h: msg_h_for(1200) };
                let big = layout_frames(1400, 1200, big_geo, opt_count);
                assert_eq!(big.cancel.bottom, 1200 - PAD);
                assert!(big.copy.right <= 1400);
                assert!(big.copy.left > big.submit.right, "右组与左组不应重叠");
            }
        }
    }

    #[test]
    fn msg_h_adapts_but_bounded() {
        assert_eq!(msg_h_for(300), 90);
        assert_eq!(msg_h_for(1090), 240);
        assert_eq!(msg_h_for(2000), 240);
    }

    #[test]
    fn pad_frame_covers_msg_and_opts() {
        // 2026-10-13(二):侵入式触控板 = 说明区 + 候选选项区(与布局同源派生)。
        let geo = Geometry { msg_h: 90 };
        let ch = content_height(geo, 3);
        let f = layout_frames(W, ch, geo, 3);
        let pf = pad_frame(&f, 3);
        assert_eq!(pf.left, f.msg.left);
        assert_eq!(pf.top, f.msg.top);
        assert_eq!(pf.right, f.msg.right);
        assert!(pf.bottom >= f.msg.bottom, "触控板至少覆盖说明区");
        assert!(
            pf.bottom >= f.opts[2].bottom,
            "触控板应覆盖到最后一个选项:{pf:?} vs {:?}",
            f.opts[2]
        );
        // 无选项时 = 说明区
        let f0 = layout_frames(W, ch, geo, 0);
        let pf0 = pad_frame(&f0, 0);
        assert_eq!(pf0.bottom, f0.msg.bottom);
    }
}
