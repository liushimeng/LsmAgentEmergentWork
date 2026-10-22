//! macOS 有头帧查看器:AppKit FFI(NSWindow + 工具栏 + NSImageView + NSTimer 轮询)。
//!
//! 只在 `laew __browser-view` 子进程内运行(与 `__hitl-dialog` 同一模式),
//! 事件循环跑在子进程主线程。布局单一真源:内容区高度 = argv 传入的视口高度
//! + 工具栏高度,**永不回读**窗口 frame(human_ui 第 137 轮铁律同源)。
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
/// 用户是否正在地址栏里编辑(编辑中不覆盖用户输入)。
static EDITING: AtomicBool = AtomicBool::new(false);

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

/// 地址栏结束编辑。
extern "C" fn on_text_end(_self: Id, _cmd: Sel, _note: Id) {
    EDITING.store(false, Ordering::Relaxed);
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

        // 内容:NSImageView 铺满内容区下半部 + 等比缩放。
        let content = send_id(window, sel(b"contentView\0"));
        let image_view = send_id_rect(
            send_id(cls(b"NSImageView\0"), sel(b"alloc\0")),
            sel(b"initWithFrame:\0"),
            ns_rect(0.0, 0.0, w, h),
        );
        send_void_i64(image_view, sel(b"setImageScaling:\0"), NS_IMAGE_SCALE_PROPORTIONALLY);
        send_void_id(content, sel(b"addSubview:\0"), image_view);

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
        // 默认不抢焦点(不 activate),窗口已 orderFront 可见。
        // LAEW_BROWSER_VIEW_ACTIVATE=1:抢到前台(窗口被别的应用盖住时手动召回)。
        if std::env::var("LAEW_BROWSER_VIEW_ACTIVATE")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
        {
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
