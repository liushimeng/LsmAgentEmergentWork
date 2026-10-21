//! macOS 有头帧查看器:AppKit FFI(NSWindow + NSImageView + NSTimer 轮询)。
//!
//! 只在 `laew __browser-view` 子进程内运行(与 `__hitl-dialog` 同一模式),
//! 事件循环跑在子进程主线程。布局单一真源:窗口内容尺寸 = argv 传入的视口尺寸,
//! **永不回读**窗口 frame(human_ui 第 137 轮铁律同源)。

use std::ffi::{c_char, c_void, CString};
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
    fn class_addMethod(
        cls: Id,
        name: Sel,
        imp: extern "C" fn(Id, Sel, Id),
        types: *const c_char,
    ) -> i8;
    fn objc_registerClassPair(cls: Id);
    fn objc_msgSend();
}

const NS_APPLICATION_ACTIVATION_POLICY_REGULAR: i64 = 0;
/// Titled|Closable|Miniaturizable|Resizable。
const NS_WINDOW_STYLE_ALL: u64 = 1 | 2 | 4 | 8;
const NS_BACKING_STORE_BUFFERED: i64 = 2;
/// NSImageScaleProportionallyUpOrDown。
const NS_IMAGE_SCALE_PROPORTIONALLY: i64 = 3;

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
msg!(send_id_rect, (rect: NSRect) -> Id);
msg!(send_id_rect_u64_i64_bool, (rect: NSRect, mask: u64, backing: i64, defer: bool) -> Id);
msg!(send_id_f64_id_sel_id_bool, (t: f64, tgt: Id, act: Sel, info: Id, repeats: bool) -> Id);
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
    close: bool,
    seq: u64,
    shown_seq: u64,
}

static SLOT: once_cell::sync::Lazy<Mutex<FrameSlot>> =
    once_cell::sync::Lazy::new(|| Mutex::new(FrameSlot {
        image: None,
        title: None,
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

/// 定时器回调:取新帧 → NSImage initWithData → setImage;标题/关闭同理。
extern "C" fn on_tick(_self: Id, _cmd: Sel, _timer: Id) {
    unsafe {
        let (img, title, close, seq) = {
            let mut s = SLOT.lock().unwrap();
            let img = s.image.take();
            let title = s.title.take();
            (img, title, s.close, s.seq)
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

/// 窗口关闭 → 退出查看器(读线程随进程一起结束)。
extern "C" fn on_window_close(_self: Id, _cmd: Sel, _note: Id) {
    unsafe {
        let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
        send_void_id(app, sel(b"terminate:\0"), std::ptr::null_mut());
    }
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
        let pool = {
            let a = send_id(cls(b"NSAutoreleasePool\0"), sel(b"alloc\0"));
            send_id(a, sel(b"init\0"))
        };

        // 控制器类:tick: 驱帧 + windowWillClose: 退出。
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
            on_tick,
            b"v@:@\0".as_ptr().cast(),
        );
        let ok_close = class_addMethod(
            ctrl_cls,
            sel(b"windowWillClose:\0"),
            on_window_close,
            b"v@:@\0".as_ptr().cast(),
        );
        objc_registerClassPair(ctrl_cls);
        if ok_tick == 0 || ok_close == 0 {
            if !pool.is_null() {
                send_void(pool, sel(b"drain\0"));
            }
            drain_stdin();
            return 1;
        }
        let ctrl = send_id(send_id(ctrl_cls, sel(b"alloc\0")), sel(b"init\0"));

        // 窗口:内容尺寸 = 视口(单一真源,不回读)。
        let w = width.clamp(320, 3840) as f64;
        let h = height.clamp(240, 2160) as f64;
        let window = send_id_rect_u64_i64_bool(
            send_id(cls(b"NSWindow\0"), sel(b"alloc\0")),
            sel(b"initWithContentRect:styleMask:backing:defer:\0"),
            ns_rect(60.0, 60.0, w, h),
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

        // 内容:NSImageView 铺满 + 等比缩放。
        let content = send_id(window, sel(b"contentView\0"));
        let image_view = send_id_rect(
            send_id(cls(b"NSImageView\0"), sel(b"alloc\0")),
            sel(b"initWithFrame:\0"),
            ns_rect(0.0, 0.0, w, h),
        );
        send_void_i64(image_view, sel(b"setImageScaling:\0"), NS_IMAGE_SCALE_PROPORTIONALLY);
        send_void_id(content, sel(b"addSubview:\0"), image_view);

        *IMAGE_VIEW.lock().unwrap() = IdCell(image_view);
        *WINDOW.lock().unwrap() = IdCell(window);

        // 读线程:stdin → 帧槽。
        std::thread::spawn(|| {
            use std::io::Read as _;
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
        // 不 activate(不抢 TUI 焦点),窗口已 orderFront 可见。
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
