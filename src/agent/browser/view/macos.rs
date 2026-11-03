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
// NSTrackingArea initWithRect:options:owner:userInfo:
msg!(send_id_rect_u64_id_id, (rect: NSRect, opts: u64, owner: Id, info: Id) -> Id);
// NSMenu popUpMenuPositioningItem:atLocation:inView:(右键原生菜单,阶段二)。
msg!(send_i8_id_point_id, (item: Id, loc: NSPoint, view: Id) -> i8);

fn cls(name: &'static [u8]) -> Id {
    unsafe { objc_getClass(name.as_ptr().cast()) }
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
    close: bool,
    seq: u64,
    shown_seq: u64,
}

static SLOT: once_cell::sync::Lazy<Mutex<FrameSlot>> =
    once_cell::sync::Lazy::new(|| Mutex::new(FrameSlot {
        image: None,
        title: None,
        urlbar: None,
        close: false,
        seq: 0,
        shown_seq: 0,
    }));

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
/// AppKit 原点在左下:画面区在 `[0, view_h]`,工具栏在 `[view_h, view_h+TOOLBAR_H]`。
unsafe fn reflow_controls(cw: f64, ch: f64) {
    let view_h = (ch - TOOLBAR_H).max(1.0);
    let iv = IMAGE_VIEW.lock().unwrap().0;
    if !iv.is_null() {
        send_void_rect(iv, sel(b"setFrame:\0"), ns_rect(0.0, 0.0, cw, view_h));
    }
    let btn_w = if cw >= 640.0 { 46.0 } else { 30.0 };
    let btn_y = view_h + (TOOLBAR_H - BTN_H) / 2.0;
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
}

/// 定时器一拍:同步窗口内容区尺寸——变了就重排控件并上报宿主。
///
/// 宿主收到后按新视口重排重渲染,新帧按 1:1 铺满内容区(不变形)。
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
    // 上报宿主:内容区宽 = 视口宽;内容区高扣掉工具栏 = 视口高(四舍五入到整像素)。
    let vw = cw.round().clamp(1.0, 7680.0) as u32;
    let vh = (ch - TOOLBAR_H).round().clamp(1.0, 4320.0) as u32;
    // 记录本查看器认知里的视口尺寸(键鼠坐标换算用;帧按此等比居中)。
    *VIEWPORT.lock().unwrap() = (vw as f64, vh as f64);
    send_cmd(&super::resize::resize_cmd_json(vw, vh));
}

/// 定时器回调:取新帧 → NSImage initWithData → setImage;标题/地址栏/关闭同理。
extern "C" fn on_tick(_self: Id, _cmd: Sel, _timer: Id) {
    unsafe {
        let (img, title, urlbar, close, seq) = {
            let mut s = SLOT.lock().unwrap();
            let img = s.image.take();
            let title = s.title.take();
            let urlbar = s.urlbar.take();
            (img, title, urlbar, s.close, s.seq)
        };
        if close {
            let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
            send_void_id(app, sel(b"terminate:\0"), std::ptr::null_mut());
            return;
        }
        // 人工拖动窗口 → 内容区尺寸变化 → 重排控件 + 上报宿主(视口自适应)。
        sync_content_size();
        if let Some(t) = title {
            let w = WINDOW.lock().unwrap().0;
            if !w.is_null() {
                send_void_id(w, sel(b"setTitle:\0"), ns_str(&t));
            }
        }
        if let Some((url, can_back, can_forward)) = urlbar {
            apply_urlbar(&url, can_back, can_forward);
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
    }
}

/// 地址栏同步:用户不在编辑态才覆盖文本;按钮按可用性置灰。
unsafe fn apply_urlbar(url: &str, can_back: bool, can_forward: bool) {
    let field = ADDRESS_FIELD.lock().unwrap().0;
    if std::env::var("LAEW_BROWSER_VIEW_DEBUG").is_ok() {
        eprintln!("[bview] apply_urlbar url={url:?} field={:?} editing={}", field, EDITING.load(Ordering::Relaxed));
    }
    if let Ok(mut last) = LAST_URL.lock() {
        *last = url.to_string();
    }
    if !field.is_null() && !EDITING.load(Ordering::Relaxed) {
        send_void_id(field, sel(b"setStringValue:\0"), ns_str(url));
    }
    let back = BACK_BTN.lock().unwrap().0;
    if !back.is_null() {
        send_void_bool(back, sel(b"setEnabled:\0"), can_back);
    }
    let fwd = FWD_BTN.lock().unwrap().0;
    if !fwd.is_null() {
        send_void_bool(fwd, sel(b"setEnabled:\0"), can_forward);
    }
}

/// 窗口关闭 → 退出查看器(读线程随进程一起结束)。
extern "C" fn on_window_close(_self: Id, _cmd: Sel, _note: Id) {
    unsafe {
        let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
        send_void_id(app, sel(b"terminate:\0"), std::ptr::null_mut());
    }
}

/// 工具栏按钮动作。
macro_rules! button_action {
    ($name:ident, $cmd:expr) => {
        extern "C" fn $name(_self: Id, _cmd: Sel, _sender: Id) {
            send_cmd($cmd);
        }
    };
}

button_action!(on_back, "{\"cmd\":\"back\"}");
button_action!(on_forward, "{\"cmd\":\"forward\"}");
button_action!(on_reload, "{\"cmd\":\"reload\"}");
button_action!(on_home, "{\"cmd\":\"home\"}");
// 右键菜单的剪贴板项(39 号设计文档阶段二;宿主侧按聚焦控件整值近似)。
button_action!(on_clip_copy, "{\"cmd\":\"clip\",\"act\":\"copy\"}");
button_action!(on_clip_paste, "{\"cmd\":\"clip\",\"act\":\"paste\"}");

/// 地址栏回车:`control:textView:doCommandBySelector:` 收到 `insertNewline:`。
extern "C" fn on_text_command(
    _self: Id,
    _cmd: Sel,
    _control: Id,
    _text_view: Id,
    command: Sel,
) -> i64 {
    if command == sel(b"insertNewline:\0") {
        let text = unsafe { address_text() };
        let text = text.trim();
        if !text.is_empty() {
            send_cmd(&cmd_json("navigate", Some(text)));
        }
        return 1;
    }
    0
}

/// 地址栏开始编辑(此后宿主不同步覆盖文本)。
extern "C" fn on_text_begin(_self: Id, _cmd: Sel, _note: Id) {
    EDITING.store(true, Ordering::Relaxed);
}

/// 地址栏结束编辑:回填宿主下发的 URL(清掉误入的外来输入)。
extern "C" fn on_text_end(_self: Id, _cmd: Sel, _note: Id) {
    EDITING.store(false, Ordering::Relaxed);
    unsafe {
        let field = ADDRESS_FIELD.lock().unwrap().0;
        let url = LAST_URL.lock().map(|s| s.clone()).unwrap_or_default();
        if !field.is_null() {
            send_void_id(field, sel(b"setStringValue:\0"), ns_str(&url));
        }
    }
}

// ---------- 人类键鼠输入(39 号设计文档) ----------
//
// 页面视图(NSImageView 子类 `LsmBrowserPageView`)捕获原始鼠标/键盘事件,
// 坐标换算成视口坐标后经 stdout 命令通道上报宿主;宿主 `human_input` 管线
// 做控制权闸门 + 命中 + 复用 Agent 侧交互语义。`LAEW_HUMAN_INPUT=off` 时
// 查看器停止上报(宿主侧同时拒绝,双保险)。

/// 宿主侧当前视口尺寸(= 最近一次上报的 resize;初始来自 argv)。
/// 帧画面按此尺寸与内容区做等比居中换算(稳态 1:1)。
static VIEWPORT: once_cell::sync::Lazy<Mutex<(f64, f64)>> =
    once_cell::sync::Lazy::new(|| Mutex::new((0.0, 0.0)));
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

/// 修饰键位(NSEvent modifierFlags)。
const NSEVENT_SHIFT: u64 = 0x0002_0000;
const NSEVENT_CONTROL: u64 = 0x0004_0000;
const NSEVENT_OPTION: u64 = 0x0008_0000;
const NSEVENT_COMMAND: u64 = 0x0010_0000;
/// NSTrackingArea 选项:进入/离开 + 移动 + 始终活跃 + 自动跟随 visibleRect。
const TRACKING_OPTS: u64 = 0x01 | 0x02 | 0x80 | 0x100;

fn input_disabled() -> bool {
    static OFF: once_cell::sync::Lazy<bool> = once_cell::sync::Lazy::new(|| {
        matches!(
            std::env::var("LAEW_HUMAN_INPUT")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    });
    *OFF
}

/// macOS 虚拟键码 → 浏览器键名(特殊键);可打印字符走
/// `charactersIgnoringModifiers`(0x20..=0x7E,IME 组合输入属阶段二)。
fn mac_special_key(code: u64) -> Option<&'static str> {
    Some(match code {
        36 | 76 => "Enter",
        48 => "Tab",
        53 => "Escape",
        51 => "Backspace",
        117 => "Delete",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        115 => "Home",
        119 => "End",
        116 => "PageUp",
        121 => "PageDown",
        _ => return None,
    })
}

/// NSEvent → JSON 字符串的 key 字段转义(键名/单字符,无控制字符)。
fn key_json_str(k: &str) -> String {
    let mut out = String::new();
    for c in k.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// 视图本地坐标(原点左下)→ 视口坐标(原点左上;帧等比居中换算)。
/// 越出画面(拖到工具栏/边框)返回 None。
unsafe fn page_point(view: Id, event: Id) -> Option<(f64, f64)> {
    let loc = send_point(event, sel(b"locationInWindow\0"));
    let local = send_point_point_id(view, sel(b"convertPoint:fromView:\0"), loc, std::ptr::null_mut());
    let (cw, ch) = *CONTENT_SIZE.lock().unwrap();
    let (vw, vh) = *VIEWPORT.lock().unwrap();
    if vw <= 0.0 || vh <= 0.0 || cw <= 0.0 || ch <= 0.0 {
        return None;
    }
    let view_h = (ch - TOOLBAR_H).max(1.0);
    let s = (cw / vw).min(view_h / vh);
    if s <= 0.0 {
        return None;
    }
    let dx = (cw - vw * s) / 2.0;
    let dy_bottom = (view_h - vh * s) / 2.0;
    let top_edge = dy_bottom + vh * s;
    let px = (local.x - dx) / s;
    let py = (top_edge - local.y) / s;
    if px < 0.0 || py < 0.0 || px >= vw || py >= vh {
        return None;
    }
    Some((px, py))
}

/// 上报一条输入事件(命令通道)。
fn send_input(line: &str) {
    if input_disabled() {
        return;
    }
    send_cmd(line);
}

fn move_allowed(x: f64, y: f64) -> bool {
    let mut last = LAST_MOVE.lock().unwrap();
    match *last {
        Some((at, lx, ly)) => {
            let due = at.elapsed().as_millis() >= 30
                && ((lx - x).abs() >= 1.0 || (ly - y).abs() >= 1.0);
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

/// 指针事件公共出口:坐标换算 + 组 JSON。
unsafe fn forward_pointer(view: Id, event: Id, act: &str, btn: Option<&str>, is_move: bool) {
    let Some((x, y)) = page_point(view, event) else {
        return;
    };
    if is_move && !move_allowed(x, y) {
        return;
    }
    let line = match btn {
        Some(b) => format!("{{\"cmd\":\"input\",\"act\":\"{act}\",\"x\":{x:.0},\"y\":{y:.0},\"btn\":\"{b}\"}}"),
        None => format!("{{\"cmd\":\"input\",\"act\":\"{act}\",\"x\":{x:.0},\"y\":{y:.0}}}"),
    };
    send_input(&line);
}

macro_rules! pointer_action {
    ($name:ident, $act:expr, $btn:expr, $is_move:expr) => {
        extern "C" fn $name(_self: Id, _cmd: Sel, event: Id) {
            if input_disabled() {
                return;
            }
            unsafe { forward_pointer(_self, event, $act, $btn, $is_move) };
        }
    };
}

pointer_action!(on_pv_mouse_down, "down", Some("left"), false);
pointer_action!(on_pv_mouse_up, "up", Some("left"), false);
pointer_action!(on_pv_dragged, "move", None, true);
pointer_action!(on_pv_moved, "move", None, true);
pointer_action!(on_pv_other_up, "up", Some("middle"), false);

/// 右键菜单是否接管(接管期间 down/up 都不再走 contextmenu 事件路径)。
static RMENU_ACTIVE: AtomicBool = AtomicBool::new(false);
/// 控制器实例(右键菜单的 target;`run()` 创建后登记)。
static CTRL: once_cell::sync::Lazy<Mutex<IdCell>> =
    once_cell::sync::Lazy::new(|| Mutex::new(IdCell(std::ptr::null_mut())));

/// 右键按下:默认弹**原生菜单**(后退/前进/刷新/首页/复制/粘贴);
/// `LAEW_HUMAN_RMENU=off` 回退阶段一的 contextmenu 事件路径。
extern "C" fn on_pv_right_down(_self: Id, _cmd: Sel, event: Id) {
    if input_disabled() || *RMENU_OFF {
        RMENU_ACTIVE.store(false, Ordering::Relaxed);
        unsafe { forward_pointer(_self, event, "down", Some("right"), false) };
        return;
    }
    unsafe {
        // 弹菜单(阻塞至人工选择/点空白关闭);选择经命令通道驱动引擎。
        show_context_menu(_self, event);
        RMENU_ACTIVE.store(true, Ordering::Relaxed);
    }
}

/// 右键抬起:菜单已接管时不重复上报(contextmenu 由菜单项表达意图)。
extern "C" fn on_pv_right_up(_self: Id, _cmd: Sel, event: Id) {
    if RMENU_ACTIVE.swap(false, Ordering::Relaxed) {
        return;
    }
    if input_disabled() {
        return;
    }
    unsafe { forward_pointer(_self, event, "up", Some("right"), false) };
}

/// 构建并弹出右键菜单(NSMenu + NSMenuItem,action 挂到控制器)。
unsafe fn show_context_menu(view: Id, event: Id) {
    let menu = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str("LAEW"),
    );
    if menu.is_null() {
        return;
    }
    // (标题, 动作选择器, 命令) —— 动作直接走 send_cmd,与工具栏按钮同构。
    let items: [(&str, &'static [u8], &str); 6] = [
        ("后退", b"on_back:\0", "{\"cmd\":\"back\"}"),
        ("前进", b"on_forward:\0", "{\"cmd\":\"forward\"}"),
        ("刷新", b"on_reload:\0", "{\"cmd\":\"reload\"}"),
        ("首页", b"on_home:\0", "{\"cmd\":\"home\"}"),
        ("复制", b"on_clip_copy:\0", "{\"cmd\":\"clip\",\"act\":\"copy\"}"),
        ("粘贴", b"on_clip_paste:\0", "{\"cmd\":\"clip\",\"act\":\"paste\"}"),
    ];
    let ctrl = CTRL.lock().unwrap().0;
    for (title, action, _) in items {
        let item = send_id_id_id_id(
            send_id(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
            sel(b"initWithTitle:action:keyEquivalent:\0"),
            ns_str(title),
            sel(action).cast_mut(),
            ns_str(""),
        );
        if item.is_null() {
            continue;
        }
        if !ctrl.is_null() {
            send_void_id(item, sel(b"setTarget:\0"), ctrl);
        }
        send_void_id(menu, sel(b"addItem:\0"), item);
    }
    let loc = send_point(event, sel(b"locationInWindow\0"));
    let local = send_point_point_id(view, sel(b"convertPoint:fromView:\0"), loc, std::ptr::null_mut());
    send_i8_id_point_id(menu, sel(b"popUpMenuPositioningItem:atLocation:inView:\0"), std::ptr::null_mut(), local, view);
}

/// 中键按下:仅 buttonNumber==2 视为中键(侧键忽略,阶段二)。
extern "C" fn on_pv_other_down(_self: Id, _cmd: Sel, event: Id) {
    if input_disabled() {
        return;
    }
    unsafe {
        let btn = send_u64(event, sel(b"buttonNumber\0"));
        if btn == 2 {
            forward_pointer(_self, event, "down", Some("middle"), false);
        }
    }
}

/// 滚轮:精确增量(触控板,像素)归一到 48px/档;离散滚轮每档 ±1。
extern "C" fn on_pv_scroll(_self: Id, _cmd: Sel, event: Id) {
    if input_disabled() {
        return;
    }
    unsafe {
        let Some((x, y)) = page_point(_self, event) else {
            return;
        };
        let precise = send_i64(event, sel(b"hasPreciseDeltas\0")) != 0;
        let raw_dx = send_f64(event, sel(b"deltaX\0"));
        let raw_dy = send_f64(event, sel(b"deltaY\0"));
        let norm = |d: f64| -> f64 {
            let v = if precise { d / 48.0 } else { d };
            v.clamp(-20.0, 20.0)
        };
        let (dx, dy) = (norm(raw_dx), norm(raw_dy));
        if dx.abs() < f64::EPSILON && dy.abs() < f64::EPSILON {
            return;
        }
        let line = format!(
            "{{\"cmd\":\"input\",\"act\":\"wheel\",\"x\":{x:.0},\"y\":{y:.0},\"dx\":{dx:.3},\"dy\":{dy:.3}}}"
        );
        send_input(&line);
    }
}

/// 键按下:浏览器级快捷键(⌘R/⌘[/⌘]/Ctrl+R/F5)本地消化,其余转发宿主。
extern "C" fn on_pv_key_down(_self: Id, _cmd: Sel, event: Id) {
    if input_disabled() {
        return;
    }
    unsafe {
        let flags = send_u64(event, sel(b"modifierFlags\0"));
        let code = send_u64(event, sel(b"keyCode\0"));
        let meta = flags & NSEVENT_COMMAND != 0;
        let ctrl = flags & NSEVENT_CONTROL != 0;
        // 浏览器级快捷键:刷新/后退/前进(转既有命令,不进页面)。
        if (meta || ctrl) && code == 15 {
            send_cmd("{\"cmd\":\"reload\"}");
            return;
        }
        if meta && code == 33 {
            send_cmd("{\"cmd\":\"back\"}");
            return;
        }
        if meta && code == 30 {
            send_cmd("{\"cmd\":\"forward\"}");
            return;
        }
        if code == 96 {
            send_cmd("{\"cmd\":\"reload\"}");
            return;
        }
        // 标签页快捷键(39 号设计文档阶段二):⌘/Ctrl+T 新标签、⌘/Ctrl+W 关标签
        //、Ctrl+Tab 切换(⌘+Tab 是系统级切换器,不占用)。
        if (meta || ctrl) && code == 17 {
            send_cmd("{\"cmd\":\"new_tab\"}");
            return;
        }
        if (meta || ctrl) && code == 13 {
            send_cmd("{\"cmd\":\"close_tab\"}");
            return;
        }
        if ctrl && code == 48 {
            send_cmd("{\"cmd\":\"next_tab\"}");
            return;
        }
        let shift = flags & NSEVENT_SHIFT != 0;
        let alt = flags & NSEVENT_OPTION != 0;
        let key = match mac_special_key(code) {
            Some(k) => k.to_string(),
            None => {
                let chars = send_id(event, sel(b"charactersIgnoringModifiers\0"));
                if chars.is_null() {
                    return;
                }
                let ptr = send_id_ptr(chars, sel(b"UTF8String\0"));
                if ptr.is_null() {
                    return;
                }
                let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string();
                // 仅 ASCII 可打印(P0 边界:IME 组合输入未接)。
                let Some(c) = s.chars().next() else { return };
                if !((0x20..=0x7e).contains(&(c as u32))) {
                    return;
                }
                if meta || ctrl {
                    // 组合键只转发剪贴板三键(宿主侧按聚焦控件整值处理),
                    // 其余组合键不产生页面文本。
                    if !matches!(c.to_ascii_lowercase(), 'c' | 'v' | 'x') {
                        return;
                    }
                }
                c.to_string()
            }
        };
        let line = format!(
            "{{\"cmd\":\"input\",\"act\":\"key\",\"key\":\"{}\",\"mods\":{{\"shift\":{shift},\"ctrl\":{ctrl},\"alt\":{alt},\"meta\":{meta}}}}}",
            key_json_str(&key)
        );
        send_input(&line);
    }
}

/// 页面视图可成为第一响应者(接收键盘事件;地址栏编辑时不抢)。
extern "C" fn on_pv_accepts_first_responder(_self: Id, _cmd: Sel) -> i8 {
    1
}

/// 注册页面视图类(NSImageView 子类:保留 setImage 绘制,叠加事件捕获)。
unsafe fn ensure_page_view_class() -> Id {
    {
        let cached = PAGE_VIEW_CLASS.lock().unwrap().0;
        if !cached.is_null() {
            return cached;
        }
    }
    let page_cls = objc_allocateClassPair(
        cls(b"NSImageView\0"),
        b"LsmBrowserPageView\0".as_ptr().cast(),
        0,
    );
    if page_cls.is_null() {
        // 兜底:子类化失败退回原生 NSImageView(无键鼠转发,窗口仍可用)。
        return cls(b"NSImageView\0");
    }
    let add = |name: &'static [u8], imp: *const c_void, enc: &'static [u8]| {
        class_addMethod(page_cls, sel(name), imp, enc.as_ptr().cast())
    };
    let mut ok = 1;
    ok &= add(
        b"acceptsFirstResponder\0",
        on_pv_accepts_first_responder as extern "C" fn(Id, Sel) -> i8 as *const c_void,
        b"B@:\0",
    );
    for (name, imp) in [
        (b"mouseDown:\0".as_slice(), on_pv_mouse_down as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"mouseUp:\0".as_slice(), on_pv_mouse_up as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"mouseDragged:\0".as_slice(), on_pv_dragged as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"mouseMoved:\0".as_slice(), on_pv_moved as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"rightMouseDown:\0".as_slice(), on_pv_right_down as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"rightMouseUp:\0".as_slice(), on_pv_right_up as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"otherMouseDown:\0".as_slice(), on_pv_other_down as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"otherMouseUp:\0".as_slice(), on_pv_other_up as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"scrollWheel:\0".as_slice(), on_pv_scroll as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"keyDown:\0".as_slice(), on_pv_key_down as extern "C" fn(Id, Sel, Id) as *const c_void),
    ] {
        ok &= add(name, imp, b"v@:@\0");
    }
    objc_registerClassPair(page_cls);
    let final_cls = if ok == 0 { cls(b"NSImageView\0") } else { page_cls };
    *PAGE_VIEW_CLASS.lock().unwrap() = IdCell(final_cls);
    final_cls
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

/// 建一个工具栏按钮。
unsafe fn make_button(
    ctrl: Id,
    frame: NSRect,
    title: &str,
    action: Sel,
    tooltip: &str,
) -> Id {
    let btn = send_id_rect(
        send_id(cls(b"NSButton\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        frame,
    );
    if btn.is_null() {
        return btn;
    }
    send_void_id(btn, sel(b"setTitle:\0"), ns_str(title));
    send_void_id(btn, sel(b"setTarget:\0"), ctrl);
    send_void_sel(btn, sel(b"setAction:\0"), action);
    send_void_i64(btn, sel(b"setBezelStyle:\0"), NS_BEZEL_ROUNDED);
    send_void_id(btn, sel(b"setToolTip:\0"), ns_str(tooltip));
    let font = send_id_f64(cls(b"NSFont\0"), sel(b"systemFontOfSize:\0"), 12.0);
    if !font.is_null() {
        send_void_id(btn, sel(b"setFont:\0"), font);
    }
    btn
}

/// 菜单项(标题/动作/快捷键;无修饰键时 AppKit 默认按 ⌘ 解析)。
struct MenuItem {
    title: &'static str,
    action: &'static [u8],
    key: &'static str,
}

/// 构建一个 NSMenu:返回挂好 submenu 的顶层菜单项。
unsafe fn build_menu(title: &str, items: &[MenuItem]) -> Id {
    let sub = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str(title),
    );
    for it in items {
        let item = send_id_id_id_id(
            send_id(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
            sel(b"initWithTitle:action:keyEquivalent:\0"),
            ns_str(it.title),
            sel(it.action).cast_mut(),
            ns_str(it.key),
        );
        send_void_id(sub, sel(b"addItem:\0"), item);
    }
    let parent = send_id_id_id_id(
        send_id(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:action:keyEquivalent:\0"),
        ns_str(title),
        std::ptr::null_mut(),
        ns_str(""),
    );
    send_void_id(parent, sel(b"setSubmenu:\0"), sub);
    parent
}

/// 装主菜单:地址栏复制/粘贴/编辑的**系统路径**(与 `human_ui` 弹窗同款)。
///
/// 查看器此前完全没有 NSMenu:⌘C/⌘V/⌘X/⌘A 的 key equivalent 依赖
/// 「菜单项 → responder chain → first responder(地址栏)」这条路径,
/// 无菜单条时这些快捷键行为不可预期,人工没法往地址栏粘贴网址。
unsafe fn install_main_menu() {
    let menu = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str("MainMenu"),
    );
    let app_item = build_menu(
        "LAEW 浏览器",
        &[MenuItem { title: "关于浏览器", action: b"orderFrontStandardAboutPanel:\0", key: "" }],
    );
    send_void_id(menu, sel(b"addItem:\0"), app_item);
    let edit_item = build_menu(
        "编辑",
        &[
            MenuItem { title: "剪切", action: b"cut:\0", key: "x" },
            MenuItem { title: "复制", action: b"copy:\0", key: "c" },
            MenuItem { title: "粘贴", action: b"paste:\0", key: "v" },
            MenuItem { title: "全选", action: b"selectAll:\0", key: "a" },
        ],
    );
    send_void_id(menu, sel(b"addItem:\0"), edit_item);
    let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
    if !app.is_null() {
        send_void_id(app, sel(b"setMainMenu:\0"), menu);
    }
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
        {
            if !pool.is_null() {
                send_void(pool, sel(b"drain\0"));
            }
            drain_stdin();
            return 1;
        }
        let ctrl = send_id(send_id(ctrl_cls, sel(b"alloc\0")), sel(b"init\0"));
        *CTRL.lock().unwrap() = IdCell(ctrl);

        // 窗口:内容区 = 视口 + 工具栏(单一真源,不回读)。
        let w = width.clamp(320, 3840) as f64;
        let h = height.clamp(240, 2160) as f64;
        let content_h = h + TOOLBAR_H;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac特殊键映射_核心键位() {
        assert_eq!(mac_special_key(36), Some("Enter"));
        assert_eq!(mac_special_key(76), Some("Enter"), "小键盘回车同语义");
        assert_eq!(mac_special_key(48), Some("Tab"));
        assert_eq!(mac_special_key(53), Some("Escape"));
        assert_eq!(mac_special_key(51), Some("Backspace"));
        assert_eq!(mac_special_key(117), Some("Delete"));
        assert_eq!(mac_special_key(123), Some("ArrowLeft"));
        assert_eq!(mac_special_key(126), Some("ArrowUp"));
        assert_eq!(mac_special_key(115), Some("Home"));
        assert_eq!(mac_special_key(121), Some("PageDown"));
        // 可打印字符无特殊映射(走 charactersIgnoringModifiers)。
        assert_eq!(mac_special_key(0), None);
    }

    #[test]
    fn key字段转义_引号反斜杠与控制字符() {
        assert_eq!(key_json_str("a"), "a");
        assert_eq!(key_json_str("\""), "\\\"");
        assert_eq!(key_json_str("\\"), "\\\\");
        assert_eq!(key_json_str("A\r"), "A", "控制字符被剔除");
    }
}
