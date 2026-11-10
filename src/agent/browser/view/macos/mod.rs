//! macOS 有头帧查看器:AppKit FFI(NSWindow + 工具栏 + NSImageView + NSTimer 轮询)。
//!
//! 只在 `laew __browser-view` 子进程内运行(与 `__hitl-dialog` 同一模式),
//! 事件循环跑在子进程主线程。初始布局单一真源:内容区高度 = argv 传入的
//! 视口高度 + 工具栏高度。
//!
//! **人工拖动窗口**(视口自适应):100ms 一拍读回内容区真实尺寸,与上次记录
//! 比较;变了就重排控件(画面铺满 + 工具栏按新宽度重算)并向宿主上报
//! `{"cmd":"resize","w":..,"h":..}`(内容区扣掉工具栏 = 新视口),
//! 宿主按新视口重排重渲染,新帧 1:1 铺满内容区(不缺区域、不变形)。
//! 读取内容区尺寸与 `human_ui` 的「永不回读窗口 frame」铁律**刻意不同**:
//! 那条铁律针对 HITL 弹窗自身布局的回环,详见 `content_size` 的注释。
//!
//! 窗口形态 = 真浏览器:
//! * 顶部工具栏:后退 / 前进 / 刷新 / 首页 四个按钮 + 地址栏;
//! * 下方内容区:视口渲染帧;
//! * 按钮与地址栏经 stdout 命令通道驱动宿主引擎(后退/前进/刷新/导航),
//!   地址栏内容由宿主按当前页面 URL 同步(见 `view::ViewSink::send_urlbar`)。

use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use super::read_frame;

type Id = *mut c_void;
type Sel = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

fn ns_rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect {
        origin: NSPoint { x, y },
        size: NSSize { width: w, height: h },
    }
}

extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra_bytes: usize) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_registerClassPair(cls: Id);
    fn objc_msgSend();
}

const NS_APPLICATION_ACTIVATION_POLICY_REGULAR: i64 = 0;
/// Titled|Closable|Miniaturizable|Resizable。
const NS_WINDOW_STYLE_ALL: u64 = 1 | 2 | 4 | 8;
const NS_BACKING_STORE_BUFFERED: i64 = 2;
/// NSImageScaleProportionallyUpOrDown。
const NS_IMAGE_SCALE_PROPORTIONALLY: i64 = 3;
/// NSBezelStyleRounded。
const NS_BEZEL_ROUNDED: i64 = 1;

macro_rules! msg {
    ($name:ident, ($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        unsafe fn $name(target: Id, cmd: Sel $(, $arg: $ty)*) -> $ret {
            #[allow(clippy::missing_transmute_annotations)]
            let f: unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $ret =
                std::mem::transmute(objc_msgSend as *const std::ffi::c_void);
            f(target, cmd $(, $arg)*)
        }
    };
}

msg!(send_id, () -> Id);
msg!(send_id_id, (arg: Id) -> Id);
msg!(send_id_id_id_id, (a: Id, b: Id, c: Id) -> Id);
msg!(send_id_cstr, (arg: *const c_char) -> Id);
msg!(send_id_ptr_usize, (ptr: *const c_void, len: usize) -> Id);
msg!(send_id_ptr, () -> *const c_char);
msg!(send_id_rect, (rect: NSRect) -> Id);
msg!(send_id_rect_u64_i64_bool, (rect: NSRect, mask: u64, backing: i64, defer: bool) -> Id);
msg!(send_id_f64_id_sel_id_bool, (t: f64, tgt: Id, act: Sel, info: Id, repeats: bool) -> Id);
msg!(send_id_f64, (size: f64) -> Id);
msg!(send_i64, () -> i64);
msg!(send_void, () -> ());
msg!(send_void_id, (arg: Id) -> ());
msg!(send_void_bool, (arg: bool) -> ());
msg!(send_void_i64, (arg: i64) -> ());
msg!(send_void_rect, (rect: NSRect) -> ());
msg!(send_void_sel, (arg: Sel) -> ());
// 返回 NSRect 的消息(结构体返回值)。
//
// NSRect 是 4 个 f64 的聚合体,macOS(x86_64 SysV / arm64 Darwin HFA)的 C
// ABI 都按浮点寄存器返回,与 Rust `extern "C"` 的聚合体返回约定一致;
// 已用探针实测取到正确尺寸。取不到合理值时上层按「读不到」处理。
msg!(send_rect, () -> NSRect);
// NSEvent 字段读取(39 号设计文档:人类键鼠输入)。
// NSPoint(2×f64)按浮点寄存器传参/返回,与 send_rect 同一 ABI 依据。
msg!(send_point, () -> NSPoint);
msg!(send_point_point_id, (p: NSPoint, from: Id) -> NSPoint);
msg!(send_u64, () -> u64);
msg!(send_f64, () -> f64);
// `respondsToSelector:` / `instancesRespondToSelector:`(选择器存在性守卫)。
msg!(send_i64_sel, (arg: Sel) -> i64);
// NSTrackingArea initWithRect:options:owner:userInfo:
msg!(send_id_rect_u64_id_id, (rect: NSRect, opts: u64, owner: Id, info: Id) -> Id);
// NSView addCursorRect:cursor:(P2c 光标形状跟随)。
msg!(send_void_rect_id, (rect: NSRect, cursor: Id) -> ());
// NSMenu popUpMenuPositioningItem:atLocation:inView:(右键原生菜单,阶段二)。
msg!(send_i8_id_point_id, (item: Id, loc: NSPoint, view: Id) -> i8);

fn cls(name: &'static [u8]) -> Id {
    unsafe { objc_getClass(name.as_ptr().cast()) }
}

/// 选择器存在性守卫(实例):发送前先 `respondsToSelector:`。
///
/// 发不存在的选择器会抛 NSException(foreign exception)——它**不是** Rust panic,
/// `catch_unwind` 拦不住,展开穿越 nounwind 回调直接 abort 整个查看器
/// (2026-11-04 `hasPreciseDeltas` 误拼事故,详见 `crate::ffi_guard`)。
/// 事件子类型相关的选择器(滚轮/中键等)一律先验后发,缺失时优雅降级。
unsafe fn respond(obj: Id, name: &'static [u8]) -> bool {
    !obj.is_null() && send_i64_sel(obj, sel(b"respondsToSelector:\0"), sel(name)) != 0
}

fn sel(name: &'static [u8]) -> Sel {
    unsafe { sel_registerName(name.as_ptr().cast()) }
}

unsafe fn ns_str(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_default();
    send_id_cstr(cls(b"NSString\0"), sel(b"stringWithUTF8String:\0"), c.as_ptr())
}

/// 帧槽:读线程写、定时器读。seq 单调递增避免重复 setImage。
struct FrameSlot {
    image: Option<Vec<u8>>,
    title: Option<String>,
    /// 地址栏状态(URL + 前进/后退可用性)。
    urlbar: Option<(String, bool, bool)>,
    /// ⌘/Ctrl+L 聚焦地址栏请求(39 号 P2b)。
    focus_addrbar: bool,
    /// P2c:光标形状(`"default"|"pointer"|"text"`),读线程写、定时器消费。
    cursor: Option<String>,
    close: bool,
    seq: u64,
    shown_seq: u64,
}

static SLOT: once_cell::sync::Lazy<Mutex<FrameSlot>> =
    once_cell::sync::Lazy::new(|| Mutex::new(FrameSlot {
        image: None,
        title: None,
        urlbar: None,
        focus_addrbar: false,
        cursor: None,
        close: false,
        seq: 0,
        shown_seq: 0,
    }));

/// 当前光标形状(`resetCursorRects` 读取;主 RunLoop 线程写)。
static CURSOR_KIND: once_cell::sync::Lazy<Mutex<String>> =
    once_cell::sync::Lazy::new(|| Mutex::new("default".to_string()));

/// ObjC 对象指针的跨线程容器(只在主 RunLoop 线程读写,static 存储需要 Send)。
#[derive(Clone, Copy)]
struct IdCell(Id);
unsafe impl Send for IdCell {
}
static IMAGE_VIEW: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
static WINDOW: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
/// 地址栏输入框。
static ADDRESS_FIELD: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
/// 后退/前进按钮(用于按可用性置灰)。
static BACK_BTN: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
static FWD_BTN: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
/// 刷新/首页按钮(人工拖动窗口后需按新宽度重排)。
static RELOAD_BTN: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
static HOME_BTN: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
/// 上一次已知的窗口内容区尺寸(点);变了才重排 + 上报宿主。
static CONTENT_SIZE: once_cell::sync::Lazy<Mutex<(f64, f64)>> =
    once_cell::sync::Lazy::new(|| Mutex::new((0.0, 0.0)));
/// 用户是否正在地址栏里编辑(编辑中不覆盖用户输入)。
static EDITING: AtomicBool = AtomicBool::new(false);
/// 宿主最后下发的 URL(地址栏失焦时回填,避免残留外来输入)。
static LAST_URL: once_cell::sync::Lazy<Mutex<String>> =
    once_cell::sync::Lazy::new(|| Mutex::new(String::new()));

// ---------- 工具栏常量 ----------
/// 工具栏高度(点)。
const TOOLBAR_H: f64 = 38.0;
/// 按钮高度。
const BTN_H: f64 = 24.0;
/// 按钮间距。
const BTN_GAP: f64 = 6.0;
/// 工具栏左右留白。
const PAD: f64 = 8.0;

/// 向宿主发送一条命令(查看器 → 引擎)。
fn send_cmd(line: &str) {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
    let _ = lock.flush();
}

/// 读取地址栏当前文本。
unsafe fn address_text() -> String {
    let field = ADDRESS_FIELD.lock().unwrap().0;
    if field.is_null() {
        return String::new();
    }
    let str_obj = send_id(field, sel(b"stringValue\0"));
    if str_obj.is_null() {
        return String::new();
    }
    let ptr = send_id_ptr(str_obj, sel(b"UTF8String\0"));
    if ptr.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string()
}

/// 构造命令 JSON(避免引入 serde 依赖到 FFI 层)。
fn cmd_json(cmd: &str, url: Option<&str>) -> String {
    match url {
        Some(u) => {
            let esc = u.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{{\"cmd\":\"{cmd}\",\"url\":\"{esc}\"}}")
        }
        None => format!("{{\"cmd\":\"{cmd}\"}}"),
    }
}

// ---------- 模块拆分(58 号设计文档):职责子模块经 glob 再导出互见 ----------
// `msg!` 生成的消息发送助手定义在上方,子模块声明必须在其后(宏文本作用域)。
pub(super) mod chrome;
pub(super) mod devtools;
pub(super) mod input;
#[cfg(test)]
mod tests;
use chrome::*;
use input::*;

/// 读回窗口**内容区**尺寸(点)。
///
/// 与 `human_ui` 的「永不回读窗口 frame」铁律**刻意不同**:那条铁律针对
/// HITL 弹窗自身的布局回环(弹窗按自身设定值重排,回读会自激);此处读回
/// 内容区尺寸是「人工拖动窗口 → 视口自适应」的必需输入——拿不到真实尺寸
/// 就无从知道新视口该是多少,只能停在初始视口(拖动后大片空白)。
unsafe fn content_size(window: Id) -> Option<(f64, f64)> {
    if window.is_null() {
        return None;
    }
    let content = send_id(window, sel(b"contentView\0"));
    if content.is_null() {
        return None;
    }
    let r = send_rect(content, sel(b"frame\0"));
    let (w, h) = (r.size.width, r.size.height);
    // 合理性闸:结构体返回值若因 ABI 不符取到垃圾值,宁可当作「读不到」。
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 || w > 20000.0 || h > 20000.0 {
        return None;
    }
    Some((w, h))
}

/// 按新的内容区尺寸重排控件:画面铺满下半部,工具栏按新宽度重算。
///
/// AppKit 原点在左下,自上而下四层(58 号设计文档):
/// 一级菜单栏 `[ch-MENU_H, ch]` → 工具栏 `[panel_h+view_h, …]` →
/// 页面画面 `[panel_h, panel_h+view_h]` → DevTools 面板 `[0, panel_h]`。
unsafe fn reflow_controls(cw: f64, ch: f64) {
    let panel_h = devtools::panel_extra_h();
    let view_h = (ch - TOOLBAR_H - devtools::MENU_H - panel_h).max(1.0);
    let iv = IMAGE_VIEW.lock().unwrap().0;
    if !iv.is_null() {
        send_void_rect(iv, sel(b"setFrame:\0"), ns_rect(0.0, panel_h, cw, view_h));
    }
    let btn_w = if cw >= 640.0 { 46.0 } else { 30.0 };
    let btn_y = panel_h + view_h + (TOOLBAR_H - BTN_H) / 2.0;
    let mut x = PAD;
    let back = BACK_BTN.lock().unwrap().0;
    let fwd = FWD_BTN.lock().unwrap().0;
    let reload = RELOAD_BTN.lock().unwrap().0;
    let home = HOME_BTN.lock().unwrap().0;
    for b in [back, fwd, reload, home] {
        if !b.is_null() {
            send_void_rect(b, sel(b"setFrame:\0"), ns_rect(x, btn_y, btn_w, BTN_H));
        }
        x += btn_w + BTN_GAP;
    }
    let field = ADDRESS_FIELD.lock().unwrap().0;
    if !field.is_null() {
        let addr_w = (cw - x - PAD).max(60.0);
        send_void_rect(field, sel(b"setFrame:\0"), ns_rect(x, btn_y, addr_w, BTN_H));
    }
    // 一级菜单栏 + DevTools 面板(58 号)。
    devtools::reflow(cw, ch, view_h);
}

/// 定时器一拍:同步窗口内容区尺寸——变了就重排控件并上报宿主。
///
/// 宿主收到后按新视口重排重渲染,新帧按 1:1 铺满内容区(不变形)。
/// P2d:上报视口 = 内容区 ÷ 缩放系数(Chrome 缩放语义:CSS 像素变大、
/// 可视 CSS 视口变小,媒体查询随之重估);帧等比居中绘制天然完成放大。
unsafe fn sync_content_size() {
    let window = WINDOW.lock().unwrap().0;
    if window.is_null() {
        return;
    }
    let Some((cw, ch)) = content_size(window) else {
        return;
    };
    {
        let mut last = CONTENT_SIZE.lock().unwrap();
        if (cw - last.0).abs() < 0.5 && (ch - last.1).abs() < 0.5 {
            return;
        }
        *last = (cw, ch);
    }
    reflow_controls(cw, ch);
    let z = (*ZOOM.lock().unwrap()).max(0.05);
    // 上报宿主:内容区宽 = 视口宽;内容区高扣掉工具栏 + 一级菜单栏 + DevTools
    // 面板 = 视口高(四舍五入到整像素;P2d 起按缩放系数换算)。
    let vw = (cw / z).round().clamp(1.0, 7680.0) as u32;
    let vh = ((ch - TOOLBAR_H - devtools::MENU_H - devtools::panel_extra_h()) / z)
        .round()
        .clamp(1.0, 4320.0) as u32;
    // 记录本查看器认知里的视口尺寸(键鼠坐标换算用;帧按此等比居中)。
    *VIEWPORT.lock().unwrap() = (vw as f64, vh as f64);
    send_cmd(&super::resize::resize_cmd_json(vw, vh));
}

/// 宿主侧当前视口尺寸(= 最近一次上报的 resize;初始来自 argv)。
/// 帧画面按此尺寸与内容区做等比居中换算(稳态 1:1)。
static VIEWPORT: once_cell::sync::Lazy<Mutex<(f64, f64)>> =
    once_cell::sync::Lazy::new(|| Mutex::new((0.0, 0.0)));
/// P2d:人工缩放系数(⌘/Ctrl + =/-/0;查看器侧生效,宿主只记账)。
static ZOOM: once_cell::sync::Lazy<Mutex<f64>> =
    once_cell::sync::Lazy::new(|| Mutex::new(1.0));
/// P2d:宿主最近下发的原始标题(缩放后缀叠加其上,换页/缩放时重设窗口标题)。
static LAST_TITLE: once_cell::sync::Lazy<Mutex<String>> =
    once_cell::sync::Lazy::new(|| Mutex::new(String::new()));
/// 上次已上报的 move(节流:≥30ms 且位移 ≥1px 才再发)。
static LAST_MOVE: once_cell::sync::Lazy<Mutex<Option<(std::time::Instant, f64, f64)>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));
/// 页面视图类(`NSImageView` 子类;`ensure_page_view_class` 注册)。
static PAGE_VIEW_CLASS: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));
/// 右键原生菜单是否关闭(`LAEW_HUMAN_RMENU=off` → 回退 contextmenu 事件路径)。
static RMENU_OFF: once_cell::sync::Lazy<bool> = once_cell::sync::Lazy::new(|| {
    matches!(
        std::env::var("LAEW_HUMAN_RMENU")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
});

/// 右键菜单是否接管(接管期间 down/up 都不再走 contextmenu 事件路径)。
static RMENU_ACTIVE: AtomicBool = AtomicBool::new(false);
/// 控制器实例(右键菜单的 target;`run()` 创建后登记)。
static CTRL: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));

/// 定时器回调:取新帧 → NSImage initWithData → setImage;标题/地址栏/关闭同理。
extern "C" fn on_tick(_self: Id, _cmd: Sel, _timer: Id) {
    crate::ffi_guard::guard("on_tick", || unsafe {
        let (img, title, urlbar, focus_addrbar, cursor, close, seq) = {
            let mut s = SLOT.lock().unwrap();
            let img = s.image.take();
            let title = s.title.take();
            let urlbar = s.urlbar.take();
            let focus_addrbar = std::mem::take(&mut s.focus_addrbar);
            let cursor = s.cursor.take();
            (img, title, urlbar, focus_addrbar, cursor, s.close, s.seq)
        };
        if close {
            let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
            send_void_id(app, sel(b"terminate:\0"), std::ptr::null_mut());
            return;
        }
        // 人工拖动窗口 → 内容区尺寸变化 → 重排控件 + 上报宿主(视口自适应)。
        sync_content_size();
        if let Some(t) = title {
            // P2d:原始标题入账 + 叠加缩放后缀(≠100% 时可见)。
            *LAST_TITLE.lock().unwrap() = t.clone();
            let z = *ZOOM.lock().unwrap();
            let w = WINDOW.lock().unwrap().0;
            if !w.is_null() {
                send_void_id(w, sel(b"setTitle:\0"), ns_str(&zoom_title(&t, z)));
            }
        }
        if let Some((url, can_back, can_forward)) = urlbar {
            apply_urlbar(&url, can_back, can_forward);
        }
        // ⌘/Ctrl+L:把窗口焦点交给地址栏并全选(浏览器语义)。
        if focus_addrbar {
            unsafe {
                let win = WINDOW.lock().unwrap().0;
                let field = ADDRESS_FIELD.lock().unwrap().0;
                if !win.is_null() && !field.is_null() {
                    send_void_id(win, sel(b"makeFirstResponder:\0"), field);
                    // 全选当前 URL(真实浏览器 ⌘L 即全选,便于直接覆盖输入)。
                    send_void_id(field, sel(b"selectAll:\0"), std::ptr::null_mut());
                }
            }
        }
        // P2c:应用光标形状变化(主线程;cursor rect 重挂)。
        if let Some(kind) = cursor {
            *CURSOR_KIND.lock().unwrap() = kind;
            let win = WINDOW.lock().unwrap().0;
            let iv = IMAGE_VIEW.lock().unwrap().0;
            // `invalidateCursorRectsForView:` 缺失时守卫降级(不做光标跟随)。
            if !win.is_null() && !iv.is_null() && respond(win, b"invalidateCursorRectsForView:\0") {
                send_void_id(win, sel(b"invalidateCursorRectsForView:\0"), iv);
            }
        }
        if let Some(png) = img {
            let iv = IMAGE_VIEW.lock().unwrap().0;
            if !iv.is_null() {
                let data = send_id_ptr_usize(
                    cls(b"NSData\0"),
                    sel(b"dataWithBytes:length:\0"),
                    png.as_ptr().cast(),
                    png.len(),
                );
                let alloc = send_id(cls(b"NSImage\0"), sel(b"alloc\0"));
                let image = send_id_id(alloc, sel(b"initWithData:\0"), data);
                if !image.is_null() {
                    send_void_id(iv, sel(b"setImage:\0"), image);
                }
            }
            if let Ok(mut s) = SLOT.lock() {
                s.shown_seq = seq;
            }
        }
        // 58 号:消费 DevTools 快照槽 → Console 面板合并重绘(主线程)。
        devtools::tick_apply();
    });
}

/// 窗口关闭 → 先告知宿主(人工关闭,宿主勿自动重生窗口),再退出查看器
/// (读线程随进程一起结束)。
extern "C" fn on_window_close(_self: Id, _cmd: Sel, _note: Id) {
    crate::ffi_guard::guard("on_window_close", || {
        send_cmd("{\"cmd\":\"viewer_closed\"}");
        unsafe {
            let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
            send_void_id(app, sel(b"terminate:\0"), std::ptr::null_mut());
        }
    });
}

unsafe fn ensure_frameworks() -> bool {
    const PATHS: [&[u8]; 2] = [
        b"/System/Library/Frameworks/Foundation.framework/Foundation\0",
        b"/System/Library/Frameworks/AppKit.framework/AppKit\0",
    ];
    for path in PATHS {
        let handle = libc::dlopen(path.as_ptr().cast(), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
        if handle.is_null() {
            return false;
        }
    }
    true
}

/// 查看器主入口:阻塞运行至 stdin EOF / 'c' 帧 / 用户关窗。
pub fn run(width: u32, height: u32, title: &str) -> i32 {
    unsafe {
        if !ensure_frameworks() {
            // 无桌面会话:消费 stdin 防父进程写管道阻塞,静默退出。
            drain_stdin();
            return 0;
        }
        let nsapp = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
        if nsapp.is_null() {
            drain_stdin();
            return 0;
        }
        send_void_i64(
            nsapp,
            sel(b"setActivationPolicy:\0"),
            NS_APPLICATION_ACTIVATION_POLICY_REGULAR,
        );
        // 主菜单:地址栏 ⌘C/⌘V/⌘X/⌘A 的系统路径(无菜单条这些快捷键不可用)。
        install_main_menu();
        let pool = {
            let a = send_id(cls(b"NSAutoreleasePool\0"), sel(b"alloc\0"));
            send_id(a, sel(b"init\0"))
        };

        // 控制器类:tick: 驱帧 + windowWillClose: 退出 + 工具栏动作 + 地址栏委托。
        let ctrl_cls = objc_allocateClassPair(
            cls(b"NSObject\0"),
            b"LsmBrowserViewCtrl\0".as_ptr().cast(),
            0,
        );
        if ctrl_cls.is_null() {
            if !pool.is_null() {
                send_void(pool, sel(b"drain\0"));
            }
            drain_stdin();
            return 1;
        }
        let ok_tick = class_addMethod(
            ctrl_cls,
            sel(b"tick:\0"),
            on_tick as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_close = class_addMethod(
            ctrl_cls,
            sel(b"windowWillClose:\0"),
            on_window_close as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_back = class_addMethod(
            ctrl_cls,
            sel(b"on_back:\0"),
            on_back as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_fwd = class_addMethod(
            ctrl_cls,
            sel(b"on_forward:\0"),
            on_forward as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_reload = class_addMethod(
            ctrl_cls,
            sel(b"on_reload:\0"),
            on_reload as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_home = class_addMethod(
            ctrl_cls,
            sel(b"on_home:\0"),
            on_home as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_clip_copy = class_addMethod(
            ctrl_cls,
            sel(b"on_clip_copy:\0"),
            on_clip_copy as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_clip_paste = class_addMethod(
            ctrl_cls,
            sel(b"on_clip_paste:\0"),
            on_clip_paste as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_text_cmd = class_addMethod(
            ctrl_cls,
            sel(b"control:textView:doCommandBySelector:\0"),
            on_text_command as extern "C" fn(Id, Sel, Id, Id, Sel) -> i64 as *const c_void,
            b"B@:@@@\0".as_ptr().cast(),
        );
        let ok_text_begin = class_addMethod(
            ctrl_cls,
            sel(b"controlTextDidBeginEditing:\0"),
            on_text_begin as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_text_end = class_addMethod(
            ctrl_cls,
            sel(b"controlTextDidEndEditing:\0"),
            on_text_end as extern "C" fn(Id, Sel, Id) as *const c_void,
            b"v@:@\0".as_ptr().cast(),
        );
        // 58 号:DevTools 一级菜单栏 + Console 面板的六个动作。
        let ok_dt = devtools::register_methods(ctrl_cls);
        objc_registerClassPair(ctrl_cls);
        if ok_tick == 0
            || ok_close == 0
            || ok_back == 0
            || ok_fwd == 0
            || ok_reload == 0
            || ok_home == 0
            || ok_clip_copy == 0
            || ok_clip_paste == 0
            || ok_text_cmd == 0
            || ok_text_begin == 0
            || ok_text_end == 0
            || ok_dt == 0
        {
            if !pool.is_null() {
                send_void(pool, sel(b"drain\0"));
            }
            drain_stdin();
            return 1;
        }
        let ctrl = send_id(send_id(ctrl_cls, sel(b"alloc\0")), sel(b"init\0"));
        *CTRL.lock().unwrap() = IdCell(ctrl);

        // 窗口:内容区 = 视口 + 工具栏 + 一级菜单栏(单一真源,不回读;
        // DevTools 面板初始隐藏,展开时页面区收缩经 resize 上报)。
        let w = width.clamp(320, 3840) as f64;
        let h = height.clamp(240, 2160) as f64;
        let content_h = h + TOOLBAR_H + devtools::MENU_H;
        let window = send_id_rect_u64_i64_bool(
            send_id(cls(b"NSWindow\0"), sel(b"alloc\0")),
            sel(b"initWithContentRect:styleMask:backing:defer:\0"),
            ns_rect(60.0, 60.0, w, content_h),
            NS_WINDOW_STYLE_ALL,
            NS_BACKING_STORE_BUFFERED,
            false,
        );
        if window.is_null() {
            if !pool.is_null() {
                send_void(pool, sel(b"drain\0"));
            }
            drain_stdin();
            return 1;
        }
        send_void_id(window, sel(b"setTitle:\0"), ns_str(title));
        send_void_id(window, sel(b"setDelegate:\0"), ctrl);
        send_void_bool(window, sel(b"setReleasedWhenClosed:\0"), false);

        // 内容:页面视图(NSImageView 子类,叠加键鼠捕获)铺满内容区下半部
        // + 等比缩放。初始第一响应者 = 页面视图:键盘事件进页面而非地址栏
        //(地址栏被点选时自动接管第一响应者)。
        let content = send_id(window, sel(b"contentView\0"));
        let pv_cls = ensure_page_view_class();
        let image_view = send_id_rect(
            send_id(pv_cls, sel(b"alloc\0")),
            sel(b"initWithFrame:\0"),
            ns_rect(0.0, 0.0, w, h),
        );
        send_void_i64(image_view, sel(b"setImageScaling:\0"), NS_IMAGE_SCALE_PROPORTIONALLY);
        // 跟踪区:mouseMoved 需要(NSTrackingInVisibleRect 自动跟随窗口拖动)。
        let tracking = send_id_rect_u64_id_id(
            send_id(cls(b"NSTrackingArea\0"), sel(b"alloc\0")),
            sel(b"initWithRect:options:owner:userInfo:\0"),
            ns_rect(0.0, 0.0, w, h),
            TRACKING_OPTS,
            image_view,
            std::ptr::null_mut(),
        );
        if !tracking.is_null() {
            send_void_id(image_view, sel(b"addTrackingArea:\0"), tracking);
        }
        send_void_id(content, sel(b"addSubview:\0"), image_view);
        send_void_id(window, sel(b"setInitialFirstResponder:\0"), image_view);
        // 键鼠坐标换算基线:初始视口 = argv(后续随 resize 上报同步)。
        *VIEWPORT.lock().unwrap() = (w, h);

        // 工具栏:后退/前进/刷新/首页 + 地址栏。
        let btn_w = if w >= 640.0 { 46.0 } else { 30.0 };
        let btn_y = h + (TOOLBAR_H - BTN_H) / 2.0;
        let mut x = PAD;
        // 工具栏按钮必须挂到 contentView 上才可见(NSButton 默认无父视图不绘制)。
        let back = make_button(
            ctrl,
            ns_rect(x, btn_y, btn_w, BTN_H),
            "后退",
            sel(b"on_back:\0"),
            "后退",
        );
        send_void_id(content, sel(b"addSubview:\0"), back);
        x += btn_w + BTN_GAP;
        let fwd = make_button(
            ctrl,
            ns_rect(x, btn_y, btn_w, BTN_H),
            "前进",
            sel(b"on_forward:\0"),
            "前进",
        );
        send_void_id(content, sel(b"addSubview:\0"), fwd);
        x += btn_w + BTN_GAP;
        let reload = make_button(
            ctrl,
            ns_rect(x, btn_y, btn_w, BTN_H),
            "刷新",
            sel(b"on_reload:\0"),
            "刷新当前页",
        );
        send_void_id(content, sel(b"addSubview:\0"), reload);
        x += btn_w + BTN_GAP;
        let home = make_button(
            ctrl,
            ns_rect(x, btn_y, btn_w, BTN_H),
            "首页",
            sel(b"on_home:\0"),
            "回到空白页",
        );
        send_void_id(content, sel(b"addSubview:\0"), home);
        x += btn_w + BTN_GAP;
        let addr_w = (w - x - PAD).max(60.0);
        let field = send_id_rect(
            send_id(cls(b"NSTextField\0"), sel(b"alloc\0")),
            sel(b"initWithFrame:\0"),
            ns_rect(x, btn_y, addr_w, BTN_H),
        );
        if !field.is_null() {
            send_void_id(field, sel(b"setPlaceholderString:\0"), ns_str("输入网址后回车"));
            send_void_id(field, sel(b"setDelegate:\0"), ctrl);
            send_void_bool(field, sel(b"setBezeled:\0"), true);
            send_void_bool(field, sel(b"setBordered:\0"), true);
            send_void_bool(field, sel(b"setEditable:\0"), true);
            send_void_bool(field, sel(b"setSelectable:\0"), true);
            send_void_bool(field, sel(b"setDrawsBackground:\0"), true);
            let font = send_id_f64(cls(b"NSFont\0"), sel(b"systemFontOfSize:\0"), 13.0);
            if !font.is_null() {
                send_void_id(field, sel(b"setFont:\0"), font);
            }
            send_void_id(content, sel(b"addSubview:\0"), field);
        }

        *IMAGE_VIEW.lock().unwrap() = IdCell(image_view);
        *WINDOW.lock().unwrap() = IdCell(window);
        *ADDRESS_FIELD.lock().unwrap() = IdCell(field);
        *BACK_BTN.lock().unwrap() = IdCell(back);
        *FWD_BTN.lock().unwrap() = IdCell(fwd);
        *RELOAD_BTN.lock().unwrap() = IdCell(reload);
        *HOME_BTN.lock().unwrap() = IdCell(home);
        // 58 号:一级菜单栏 + Console/占位面板(初始隐藏,贴窗口底部)。
        devtools::build_ui(content, ctrl, w, h);
        // 测试钩子:LAEW_DEVTOOLS_AUTO=<panel> 启动即展开面板(真机验证用)。
        devtools::maybe_auto_panel();
        // 初始内容区尺寸登记为基线:启动后不重复上报(只有人工拖动才上报)。
        if let Some((cw, ch)) = content_size(window) {
            *CONTENT_SIZE.lock().unwrap() = (cw, ch);
        }

        // 读线程:stdin → 帧槽。
        std::thread::spawn(|| {
            let mut stdin = std::io::stdin().lock();
            loop {
                match read_frame(&mut stdin) {
                    Some((b'i', png)) => {
                        if let Ok(mut s) = SLOT.lock() {
                            s.image = Some(png);
                            s.seq += 1;
                        }
                    }
                    Some((b't', payload)) => {
                        let t = serde_json::from_slice::<serde_json::Value>(&payload)
                            .ok()
                            .and_then(|v| v.get("title").and_then(|t| t.as_str().map(str::to_string)));
                        if let (Some(t), Ok(mut s)) = (t, SLOT.lock()) {
                            s.title = Some(t);
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
                        if let Ok(mut s) = SLOT.lock() {
                            s.urlbar = Some((url, can_back, can_forward));
                        }
                    }
                    // ⌘/Ctrl+L:聚焦地址栏(39 号 P2b)。
                    Some((b'f', _)) => {
                        if let Ok(mut s) = SLOT.lock() {
                            s.focus_addrbar = true;
                        }
                    }
                    // P2c:光标形状跟随(定时器在主线程应用 cursor rect)。
                    Some((b'k', payload)) => {
                        let kind = String::from_utf8_lossy(&payload).trim().to_string();
                        if let Ok(mut s) = SLOT.lock() {
                            s.cursor = Some(kind);
                        }
                    }
                    // 58 号:DevTools 状态帧(console 台账快照;latest-wins 入槽)。
                    Some((b'd', payload)) => {
                        super::devtools_common::push_payload(&payload);
                    }
                    Some((b'c', _)) | None => {
                        if let Ok(mut s) = SLOT.lock() {
                            s.close = true;
                        }
                        return;
                    }
                    Some(_) => {}
                }
            }
        });

        // 100ms 定时器驱帧;窗口 orderFront(不抢焦点)。
        send_id_f64_id_sel_id_bool(
            cls(b"NSTimer\0"),
            sel(b"scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:\0"),
            0.1,
            ctrl,
            sel(b"tick:\0"),
            std::ptr::null_mut(),
            true,
        );
        send_void_id(window, sel(b"makeKeyAndOrderFront:\0"), std::ptr::null_mut());
        // headed 是用户「我要看到浏览器」的明确选择:首次启动默认把查看器
        // 带到前台,避免窗口已创建却被 Chrome/终端遮住而误以为没有启动。
        // LAEW_BROWSER_VIEW_ACTIVATE=off 可恢复只 orderFront、不抢焦点。
        let activate = std::env::var("LAEW_BROWSER_VIEW_ACTIVATE")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no"))
            .unwrap_or(true);
        if activate {
            send_void_bool(nsapp, sel(b"activateIgnoringOtherApps:\0"), true);
        }
        send_void(nsapp, sel(b"run\0"));

        if !pool.is_null() {
            send_void(pool, sel(b"drain\0"));
        }
        0
    }
}

/// 消费 stdin 至 EOF(无桌面会话的兜底,防父进程写管道阻塞)。同步阻塞。
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
