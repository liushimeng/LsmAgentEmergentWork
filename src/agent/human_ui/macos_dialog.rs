//! macOS 弹窗后端:Rust FFI 直调 AppKit / ObjC runtime(第 137 轮原生重构)。
//!
//! 设计见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §6。要点:
//! - 本模块**只在 `laew __hitl-dialog` 子进程内运行**(见 `dialog_main.rs`),事件循环
//!   `NSApp.run()` 跑在子进程主线程,满足 AppKit 的主线程要求;
//! - `objc_msgSend` 按具体参数形状逐个包装(tauri/tao 验证过的做法),x86_64 的
//!   结构体返回走 `objc_msgSend_stret`,arm64 统一走 `objc_msgSend`;
//! - **布局单一真源**:`layout_frames`(排布)与 `content_height`(预算)由同一组
//!   常量/参数推导(第 134 轮事故的根治);最大化/还原只重排,**不回读**窗口 frame
//!   (第 137 轮事故的根治);
//! - 窗口 styleMask 仅 `Titled`(无系统关闭/缩放按钮),resize 唯一路径 = 自家
//!   「⛶ 最大化/还原」按钮 → 确定性重排。

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::dialog_main::{countdown_text, DialogPayload};

// ---------- 基础类型与 objc runtime FFI ----------

type Id = *mut c_void;
type Sel = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
}

// AppKit 常量(数值来自 SDK 头文件,勿改)。
const NS_APPLICATION_ACTIVATION_POLICY_REGULAR: i64 = 0;
const NS_WINDOW_STYLE_MASK_TITLED: u64 = 1;
const NS_BACKING_STORE_BUFFERED: i64 = 2;
const NS_FLOATING_WINDOW_LEVEL: i64 = 3; // kCGFloatingWindowLevel
const NS_BEZEL_BORDER: u64 = 2;
const NS_ROUNDED_BEZEL_STYLE: i64 = 1;
const NS_IMAGE_SCALE_PROPORTIONALLY_DOWN: i64 = 3;
const NS_EVENT_TYPE_APPLICATION_DEFINED: i64 = 15;

/// `objc_msgSend` 按具体签名逐形状包装(见模块文档)。
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
msg!(send_id_cstr, (arg: *const c_char) -> Id);
msg!(send_id_usize, (arg: usize) -> Id);
msg!(send_id_rect, (rect: NSRect) -> Id);
msg!(send_id_rect_u64_i64_bool, (rect: NSRect, mask: u64, backing: i64, defer: bool) -> Id);
msg!(send_id_f64_id_sel_id_bool, (t: f64, tgt: Id, act: Sel, info: Id, repeats: bool) -> Id);
msg!(send_id_i64_point_u64_f64_i64_id_i16_i64_i64, (
    ty: i64, loc: NSPoint, flags: u64, ts: f64, win: i64, ctx: Id, sub: i16, d1: i64, d2: i64
) -> Id);
msg!(send_bool, () -> bool);
msg!(send_bool_id, (arg: Id) -> bool);
msg!(send_i64, () -> i64);
msg!(send_usize, () -> usize);
msg!(send_size, () -> NSSize);
msg!(send_cstr, () -> *const c_char);
msg!(send_void, () -> ());
msg!(send_void_id, (arg: Id) -> ());
msg!(send_void_bool, (arg: bool) -> ());
msg!(send_void_i64, (arg: i64) -> ());
msg!(send_void_rect, (rect: NSRect) -> ());
msg!(send_void_point, (point: NSPoint) -> ());
msg!(send_void_size, (size: NSSize) -> ());
msg!(send_void_id_bool, (ev: Id, at_start: bool) -> ());
msg!(send_void_sel, (arg: Sel) -> ());
msg!(send_id_f64, (size: f64) -> Id);

/// 结构体返回(NSRect,32 字节):x86_64 必须走 `objc_msgSend_stret`(隐藏 sret
/// 指针作首参),arm64 统一 `objc_msgSend`。
unsafe fn send_rect(target: Id, cmd: Sel) -> NSRect {
    #[cfg(target_arch = "x86_64")]
    {
        #[allow(clippy::missing_transmute_annotations)]
        let f: unsafe extern "C" fn(*mut NSRect, Id, Sel) =
            std::mem::transmute(objc_msgSend_stret as *const std::ffi::c_void);
        let mut out = NSRect::default();
        f(&mut out, target, cmd);
        out
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        #[allow(clippy::missing_transmute_annotations)]
        let f: unsafe extern "C" fn(Id, Sel) -> NSRect =
            std::mem::transmute(objc_msgSend as *const std::ffi::c_void);
        f(target, cmd)
    }
}

/// 类查找(名字必须 NUL 结尾的字面量)。
fn cls(name: &'static [u8]) -> Id {
    unsafe { objc_getClass(name.as_ptr().cast()) }
}

/// 选择子注册(名字必须 NUL 结尾的字面量;sel_registerName 自带缓存)。
fn sel(name: &'static [u8]) -> Sel {
    unsafe { sel_registerName(name.as_ptr().cast()) }
}

/// UTF-8 → NSString(自动释放;调用方在 autorelease pool 生命周期内使用)。
unsafe fn ns_str(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_default();
    send_id_cstr(cls(b"NSString\0"), sel(b"stringWithUTF8String:\0"), c.as_ptr())
}

// ---------- 布局单一真源 ----------

const W: f64 = 560.0;
const PAD: f64 = 16.0;
const TITLE_H: f64 = 22.0;
const LINE_H: f64 = 18.0;
const INPUT_H: f64 = 28.0;
const BTN_H: f64 = 30.0;
const GAP: f64 = 8.0;
const CANCEL_W: f64 = 84.0;
const SUBMIT_W: f64 = 84.0;
const ORIG_W: f64 = 110.0;
const ZOOM_W: f64 = 110.0;
const INNER: f64 = 8.0;
const IMG_HINT_H: f64 = 14.0;

/// 一组布局参数(说明区高 / 图区高;img_h = 0 表示无图)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Geometry {
    msg_h: f64,
    img_h: f64,
}

/// 各控件的目标 frame(自顶向下排布;AppKit y 轴自底向上,存的是最终坐标)。
#[derive(Clone, Debug, Default)]
struct Frames {
    title: NSRect,
    msg: NSRect,
    image: Option<NSRect>,
    img_hint: Option<NSRect>,
    t1: NSRect,
    t2: NSRect,
    input: NSRect,
    opts: Vec<NSRect>,
    cancel: NSRect,
    submit: NSRect,
    orig: NSRect,
    zoom: NSRect,
}

/// 排布纯函数:给定内容视图尺寸与几何参数,产出全部控件 frame。
///
/// **与 [`content_height`] 的预算序列逐项对应**(同一 GAP/常量/顺序推导),
/// 这是第 134 轮「按钮掉出内容视图」事故的编译期根治。
fn layout_frames(cw: f64, ch: f64, geo: Geometry, opt_count: usize) -> Frames {
    let text_w = cw - PAD * 2.0;
    let mut y = ch;
    let mut place = |h: f64| -> f64 {
        y -= h;
        let top = y;
        y -= GAP;
        top
    };
    let title_y = place(TITLE_H);
    let msg_y = place(geo.msg_h);
    let (image, img_hint) = if geo.img_h > 0.0 {
        let iy = place(geo.img_h);
        let hy = place(IMG_HINT_H);
        (
            Some(ns_rect(PAD, iy, text_w, geo.img_h)),
            Some(ns_rect(PAD, hy, text_w, IMG_HINT_H)),
        )
    } else {
        (None, None)
    };
    let t1_y = place(LINE_H);
    let t2_y = place(LINE_H);
    let input_y = place(INPUT_H);
    let opts = (0..opt_count).map(|_| ns_rect(PAD, place(BTN_H), text_w, BTN_H)).collect();
    // 底部按钮行:钉在内容视图底边(AppKit y 轴自底向上,底边即 y=PAD;窗口比
    // 内容高时,空隙留在选项区与按钮行之间,不让按钮悬在中部);内容超高(窗口被
    // 夹紧)时退化为顺序位置(y - BTN_H),并夹回 ≥0 保证按钮始终可见可点。
    let row_y = (y - BTN_H).min(PAD).max(0.0);
    Frames {
        title: ns_rect(PAD, title_y, text_w, TITLE_H),
        msg: ns_rect(PAD, msg_y, text_w, geo.msg_h),
        image,
        img_hint,
        t1: ns_rect(PAD, t1_y, text_w, LINE_H),
        t2: ns_rect(PAD, t2_y, text_w, LINE_H),
        input: ns_rect(PAD, input_y, text_w, INPUT_H),
        opts,
        cancel: ns_rect(PAD, row_y, CANCEL_W, BTN_H),
        submit: ns_rect(PAD + CANCEL_W + INNER, row_y, SUBMIT_W, BTN_H),
        orig: ns_rect(cw - PAD - ZOOM_W - INNER - ORIG_W, row_y, ORIG_W, BTN_H),
        zoom: ns_rect(cw - PAD - ZOOM_W, row_y, ZOOM_W, BTN_H),
    }
}

/// 预算纯函数:由内容推出所需内容视图高(与 [`layout_frames`] 的消费序列逐项对应)。
fn content_height(geo: Geometry, opt_count: usize) -> f64 {
    let mut consumed = TITLE_H + GAP + geo.msg_h + GAP;
    if geo.img_h > 0.0 {
        consumed += geo.img_h + GAP + IMG_HINT_H + GAP;
    }
    consumed += (LINE_H + GAP) * 2.0;
    consumed += INPUT_H + GAP;
    consumed += opt_count as f64 * (BTN_H + GAP);
    consumed += BTN_H; // 底部按钮行(place 不再扣 GAP)
    consumed + PAD
}

/// 图区高:按图片纵横比适配文本宽,夹在 [60, cap]。
fn image_height_for(ratio: f64, text_w: f64, cap: f64) -> f64 {
    if ratio <= 0.0 {
        0.0
    } else {
        (text_w * ratio).round().clamp(60.0, cap)
    }
}

// ---------- 运行时状态(单弹窗进程,UI 线程独占) ----------

struct Views {
    window: Id,
    title: Id,
    msg_scroll: Id,
    image: Option<Id>,
    img_hint: Option<Id>,
    t1: Id,
    t2: Id,
    input: Id,
    opts: Vec<Id>,
    cancel: Id,
    submit: Id,
    orig: Id,
    zoom: Id,
}

struct MacState {
    views: Views,
    options: Vec<String>,
    image_open_path: Option<String>,
    /// 正常态布局(还原用)。
    normal_geo: Geometry,
    normal_content: (f64, f64),
    img_ratio: f64,
    zoomed: bool,
    timer: Id,
    started: Instant,
    timeout_ms: u64,
    done: bool,
    result: Option<(String, String)>,
}

static STATE: Mutex<Option<MacState>> = Mutex::new(None);

// 裸指针(ObjC 对象引用)默认 !Send,static Mutex 要求 Sync。本模块**只在
// `__hitl-dialog` 子进程的 UI 主线程**创建/触碰 STATE(IMP 由同一事件循环分发),
// 无跨线程访问,Send 标注与实际使用一致。
unsafe impl Send for MacState {}

fn state() -> std::sync::MutexGuard<'static, Option<MacState>> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------- 控制器类(IMP) ----------

extern "C" fn on_submit(_self: Id, _cmd: Sel, sender: Id) {
    let tag = if sender.is_null() {
        -1
    } else {
        unsafe { send_i64(sender, sel(b"tag\0")) }
    };
    finish("answer", tag);
}

extern "C" fn on_cancel(_self: Id, _cmd: Sel, _sender: Id) {
    finish("cancel", -1);
}

extern "C" fn on_zoom(_self: Id, _cmd: Sel, _sender: Id) {
    unsafe { toggle_zoom() };
}

extern "C" fn on_open_image(_self: Id, _cmd: Sel, _sender: Id) {
    let path = {
        let guard = state();
        guard
            .as_ref()
            .and_then(|st| st.image_open_path.clone())
    };
    let Some(path) = path else { return };
    unsafe {
        let ws = send_id(cls(b"NSWorkspace\0"), sel(b"sharedWorkspace\0"));
        if !ws.is_null() {
            send_bool_id(ws, sel(b"openFile:\0"), ns_str(&path));
        }
    }
}

extern "C" fn on_tick(_self: Id, _cmd: Sel, _sender: Id) {
    // 单次锁定取出所需;**必须先解锁再 finish**(finish 会再次锁定 STATE)。
    let (label, text, timed_out) = {
        let guard = state();
        let Some(st) = guard.as_ref() else { return };
        if st.done {
            return;
        }
        let elapsed = st.started.elapsed().as_millis() as u64;
        let remain = st.timeout_ms.saturating_sub(elapsed);
        (
            st.views.t2,
            countdown_text(elapsed, remain),
            remain == 0,
        )
    };
    if !label.is_null() {
        unsafe {
            send_void_id(label, sel(b"setStringValue:\0"), ns_str(&text));
        }
    }
    if timed_out {
        finish("timeout", -1);
    }
}

extern "C" fn on_focus_once(_self: Id, _cmd: Sel, _sender: Id) {
    // 0.05s 二次断言 first responder(第 132 轮「看着有焦点打不进字」教训)。
    let pair = {
        let guard = state();
        guard.as_ref().map(|st| (st.views.window, st.views.input))
    };
    if let Some((window, input)) = pair {
        unsafe {
            send_bool_id(window, sel(b"makeFirstResponder:\0"), input);
        }
    }
}

extern "C" fn on_window_will_close(_self: Id, _cmd: Sel, _sender: Id) {
    finish("cancel", -1);
}

/// 注册控制器类(进程内一次)。
fn controller_class() -> Id {
    static CONTROLLER: OnceLock<usize> = OnceLock::new();
    let ptr = *CONTROLLER.get_or_init(|| unsafe {
        let superclass = cls(b"NSObject\0");
        if superclass.is_null() {
            return 0;
        }
        let c = objc_allocateClassPair(superclass, b"LaewHitlController\0".as_ptr().cast(), 0);
        if c.is_null() {
            return 0;
        }
        let ok = |m: extern "C" fn(Id, Sel, Id), name: &'static [u8]| {
            class_addMethod(c, sel(name), m, b"v@:@\0".as_ptr().cast()) != 0
        };
        let all = ok(on_submit, b"submit:\0")
            & ok(on_cancel, b"cancel:\0")
            & ok(on_zoom, b"zoom:\0")
            & ok(on_open_image, b"openImage:\0")
            & ok(on_tick, b"tick:\0")
            & ok(on_focus_once, b"focusOnce:\0")
            & ok(on_window_will_close, b"windowWillClose:\0");
        if !all {
            return 0;
        }
        objc_registerClassPair(c);
        c as usize
    });
    ptr as Id
}

// ---------- 收口 ----------

/// 收口(只在 UI 线程调用):结果映射 → 停定时器 → 停 run loop。
/// - answer + 输入框有字 → answer(文本);无字 + tag>=0 → answer(选项 N);
///   无字 + tag<0(提交按钮/输入框回车)→ answer(选项 1,对齐「空回车=选项1」);
/// - cancel / timeout 直传。
fn finish(kind: &str, tag: i64) {
    {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        st.done = true;
        if !st.timer.is_null() {
            unsafe { send_void(st.timer, sel(b"invalidate\0")) };
        }
        let result = if kind == "timeout" {
            ("timeout".to_string(), String::new())
        } else if kind == "cancel" {
            ("cancel".to_string(), String::new())
        } else {
            let typed = unsafe { read_input(st.views.input) };
            if !typed.is_empty() {
                ("answer".to_string(), typed)
            } else if tag >= 0 && (tag as usize) < st.options.len() {
                (
                    "answer".to_string(),
                    format!("{}. {}", tag + 1, st.options[tag as usize]),
                )
            } else {
                // options 由 payload 投影保证非空
                ("answer".to_string(), format!("1. {}", st.options[0]))
            }
        };
        st.result = Some(result);
    }
    unsafe { stop_run_loop() };
}

unsafe fn read_input(input: Id) -> String {
    if input.is_null() {
        return String::new();
    }
    let s = send_id(input, sel(b"stringValue\0"));
    if s.is_null() {
        return String::new();
    }
    let c = send_cstr(s, sel(b"UTF8String\0"));
    if c.is_null() {
        return String::new();
    }
    CStr::from_ptr(c).to_string_lossy().trim().to_string()
}

/// 停止 `NSApp.run()`:stop 只置标志,再投递一个空事件唤醒事件循环立即返回(标准 trick)。
unsafe fn stop_run_loop() {
    let nsapp = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
    if nsapp.is_null() {
        return;
    }
    send_void_id(nsapp, sel(b"stop:\0"), std::ptr::null_mut());
    let ev = send_id_i64_point_u64_f64_i64_id_i16_i64_i64(
        cls(b"NSEvent\0"),
        sel(b"otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:\0"),
        NS_EVENT_TYPE_APPLICATION_DEFINED,
        NSPoint { x: 0.0, y: 0.0 },
        0,
        0.0,
        0,
        std::ptr::null_mut(),
        0,
        0,
        0,
    );
    if !ev.is_null() {
        send_void_id_bool(nsapp, sel(b"postEvent:atStart:\0"), ev, true);
    }
}

// ---------- 最大化/还原(第 137 轮事故根治:确定性重排,不回读 frame) ----------

unsafe fn toggle_zoom() {
    // 目标几何:确定性推导(最大化 = 主屏可视区 - 40px,布局参数按屏高重推),
    // 排版高度 = 我们传入的值,**永不回读**窗口 frame。
    let (w, h, geo, opt_count) = {
        let guard = state();
        let Some(st) = guard.as_ref() else { return };
        if st.zoomed {
            (
                st.normal_content.0,
                st.normal_content.1,
                st.normal_geo,
                st.options.len(),
            )
        } else {
            let vf = pick_screen_visible();
            let zw = (vf.size.width - 40.0).max(W);
            let zh = (vf.size.height - 40.0).max(st.normal_content.1);
            let msg_h = (zh * 0.22).round().clamp(90.0, 240.0);
            let cap = (zh * 0.55).round().clamp(60.0, 900.0);
            let img_h = image_height_for(st.img_ratio, zw - PAD * 2.0, cap);
            let g = Geometry { msg_h, img_h };
            (zw, zh.max(content_height(g, st.options.len())), g, st.options.len())
        }
    };
    let (views, window, input) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        st.zoomed = !st.zoomed;
        let title = if st.zoomed { "⛶ 还原" } else { "⛶ 最大化" };
        send_void_id(st.views.zoom, sel(b"setTitle:\0"), ns_str(title));
        (
            ViewsSnapshot::from(&st.views),
            st.views.window,
            st.views.input,
        )
    };
    let (fw, fh) = place_window(window, w, h);
    apply_frames(&views, &layout_frames(fw, fh, geo, opt_count), geo);
    send_bool_id(window, sel(b"makeFirstResponder:\0"), input);
}

/// IMP 与主流程之间传递的控件快照(裸指针拷贝;子进程 UI 线程内恒有效)。
struct ViewsSnapshot {
    title: Id,
    msg_scroll: Id,
    image: Option<Id>,
    img_hint: Option<Id>,
    t1: Id,
    t2: Id,
    input: Id,
    opts: Vec<Id>,
    cancel: Id,
    submit: Id,
    orig: Id,
    zoom: Id,
}

impl ViewsSnapshot {
    fn from(v: &Views) -> Self {
        Self {
            title: v.title,
            msg_scroll: v.msg_scroll,
            image: v.image,
            img_hint: v.img_hint,
            t1: v.t1,
            t2: v.t2,
            input: v.input,
            opts: v.opts.clone(),
            cancel: v.cancel,
            submit: v.submit,
            orig: v.orig,
            zoom: v.zoom,
        }
    }

}

/// 把布局结果落到控件(ImageView 只在尺寸变化时 setFrame 亦可,统一全量设置)。
unsafe fn apply_frames(v: &ViewsSnapshot, f: &Frames, geo: Geometry) {
    let set = |view: Id, rect: NSRect| {
        if !view.is_null() {
            send_void_rect(view, sel(b"setFrame:\0"), rect);
        }
    };
    set(v.title, f.title);
    set(v.msg_scroll, f.msg);
    if geo.img_h > 0.0 {
        if let (Some(view), Some(rect)) = (v.image, f.image) {
            set(view, rect);
        }
        if let (Some(view), Some(rect)) = (v.img_hint, f.img_hint) {
            set(view, rect);
        }
    }
    set(v.t1, f.t1);
    set(v.t2, f.t2);
    set(v.input, f.input);
    for (view, rect) in v.opts.iter().zip(f.opts.iter()) {
        set(*view, *rect);
    }
    set(v.cancel, f.cancel);
    set(v.submit, f.submit);
    set(v.orig, f.orig);
    set(v.zoom, f.zoom);
}

// ---------- 选屏与定位(第 134 轮结论延续) ----------

/// 取 `visibleFrame` 面积最大的屏幕(多屏下 `[NSWindow center]` 可能把窗口丢到
/// 副屏,弃用;主屏天然胜出)。无屏(无桌面会话)返回零矩形。
unsafe fn pick_screen_visible() -> NSRect {
    let screens = send_id(cls(b"NSScreen\0"), sel(b"screens\0"));
    if screens.is_null() {
        return NSRect::default();
    }
    let count = send_usize(screens, sel(b"count\0"));
    let mut best = NSRect::default();
    let mut best_area = -1.0f64;
    for i in 0..count {
        let screen = send_id_usize(screens, sel(b"objectAtIndex:\0"), i);
        if screen.is_null() {
            continue;
        }
        let vf = send_rect(screen, sel(b"visibleFrame\0"));
        let area = vf.size.width * vf.size.height;
        if area > best_area {
            best_area = area;
            best = vf;
        }
    }
    if best_area < 0.0 {
        let main = send_id(cls(b"NSScreen\0"), sel(b"mainScreen\0"));
        if !main.is_null() {
            best = send_rect(main, sel(b"visibleFrame\0"));
        }
    }
    best
}

/// 按内容尺寸落到主屏可见区居中(超高/超宽夹紧);返回**实际设置**的内容尺寸
/// —— 排版一律使用返回值,永不回读窗口 frame。
unsafe fn place_window(window: Id, w: f64, h: f64) -> (f64, f64) {
    let vf = pick_screen_visible();
    let fw = w.min((vf.size.width - 40.0).max(320.0));
    let fh = h.min((vf.size.height - 40.0).max(240.0));
    let x = vf.origin.x + (vf.size.width - fw) / 2.0;
    let y = vf.origin.y + (vf.size.height - fh) / 2.0;
    send_void_size(window, sel(b"setContentSize:\0"), NSSize { width: fw, height: fh });
    send_void_point(window, sel(b"setFrameOrigin:\0"), NSPoint { x, y });
    (fw, fh)
}

// ---------- 呈现主流程 ----------

/// 弹出原生弹窗并阻塞至收口;返回 (status, text) 契约 JSON 字段。
pub(super) fn run_dialog(p: &DialogPayload) -> (String, String) {
    unsafe { run_appkit(p) }
}

unsafe fn run_appkit(p: &DialogPayload) -> (String, String) {
    // Rust 二进制默认不链接 AppKit/Foundation(无静态符号引用),ObjC 类不会注册进
    // runtime,`objc_getClass("NSApplication")` 会拿到 null —— 显式 dlopen 装载。
    if !ensure_frameworks() {
        return ("error".into(), "AppKit/Foundation 框架装载失败(可能无桌面会话)".into());
    }
    let nsapp = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
    if nsapp.is_null() {
        return ("error".into(), "NSApplication 初始化失败(可能无桌面会话)".into());
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

    let result = build_and_run(p);

    if !pool.is_null() {
        send_void(pool, sel(b"drain\0"));
    }
    result
}

unsafe fn build_and_run(p: &DialogPayload) -> (String, String) {
    // ---- 控制器 ----
    let ctrl_cls = controller_class();
    if ctrl_cls.is_null() {
        return ("error".into(), "控制器类注册失败(libobjc)".into());
    }
    let ctrl = send_id(send_id(ctrl_cls, sel(b"alloc\0")), sel(b"init\0"));

    // ---- 说明与几何 ----
    let text_w = W - PAD * 2.0;
    let (image_view, img_hint_label, img_ratio, image_open_path) = load_image(p);
    let geo = Geometry {
        msg_h: 90.0,
        img_h: image_height_for(img_ratio, text_w, 300.0),
    };
    let normal_h = content_height(geo, p.options.len());

    // ---- 窗口:Titled(无关闭/缩放按钮 → 杜绝误关;resize 只走自家 ⛶ 按钮)----
    let window = send_id_rect_u64_i64_bool(
        send_id(cls(b"NSWindow\0"), sel(b"alloc\0")),
        sel(b"initWithContentRect:styleMask:backing:defer:\0"),
        ns_rect(0.0, 0.0, W, normal_h),
        NS_WINDOW_STYLE_MASK_TITLED,
        NS_BACKING_STORE_BUFFERED,
        false,
    );
    if window.is_null() {
        return ("error".into(), "NSWindow 创建失败(可能无桌面会话)".into());
    }
    send_void_id(window, sel(b"setTitle:\0"), ns_str(&format!("🔐 人工介入 · {}", p.kind_label)));
    send_void_i64(window, sel(b"setLevel:\0"), NS_FLOATING_WINDOW_LEVEL);
    send_void_id(window, sel(b"setDelegate:\0"), ctrl);
    let content = send_id(window, sel(b"contentView\0"));

    // ---- 控件 ----
    let title = make_label(13.0, true);
    let (msg_scroll, _msg_view) = make_message_view(&p.info_text());
    let t1 = make_label(11.0, false);
    let t2 = make_label(11.0, false);

    let input = send_id(send_id(cls(b"NSTextField\0"), sel(b"alloc\0")), sel(b"init\0"));
    send_void_bool(input, sel(b"setBezeled:\0"), true);
    send_void_bool(input, sel(b"setEditable:\0"), true);
    send_void_id(input, sel(b"setFont:\0"), font(13.0, false));
    send_void_id(input, sel(b"setPlaceholderString:\0"), ns_str("可输入验证码/动态码/说明;留空点选项或提交"));
    send_void_id(input, sel(b"setTarget:\0"), ctrl);
    send_void_sel(input, sel(b"setAction:\0"), sel(b"submit:\0"));
    send_void_i64(input, sel(b"setTag:\0"), -1); // 输入框回车:有字=文本,无字=选项 1

    let mut opt_buttons = Vec::new();
    for (i, opt) in p.options.iter().enumerate() {
        let b = make_button(opt, None, ctrl, b"submit:\0", i as i64);
        opt_buttons.push(b);
    }
    let cancel = make_button("取消", Some("\x1b"), ctrl, b"cancel:\0", -1);
    let submit = make_button("提交", Some("\r"), ctrl, b"submit:\0", -1);
    let zoom = make_button("⛶ 最大化", None, ctrl, b"zoom:\0", -1);
    let orig = make_button("查看原图", None, ctrl, b"openImage:\0", -1);

    // 图片尺寸标注(有图才有)
    let img_hint = img_hint_label;

    // ---- 装配 ----
    let add = |parent: Id, child: Id| {
        if !child.is_null() {
            send_void_id(parent, sel(b"addSubview:\0"), child);
        }
    };
    add(content, title);
    add(content, msg_scroll);
    if let Some(iv) = image_view {
        add(content, iv);
        if let Some(hint) = img_hint {
            add(content, hint);
        }
    }
    add(content, t1);
    add(content, t2);
    add(content, input);
    for b in &opt_buttons {
        add(content, *b);
    }
    add(content, cancel);
    add(content, submit);
    add(content, zoom);
    add(content, orig);

    send_void_id(
        t1,
        sel(b"setStringValue:\0"),
        ns_str(&format!("提出时间: {}   超时截止: {}", p.started_clock(), p.deadline_clock())),
    );
    send_void_id(
        t2,
        sel(b"setStringValue:\0"),
        ns_str(&countdown_text(0, p.timeout_ms)),
    );
    send_void_id(title, sel(b"setStringValue:\0"), ns_str(&format!("人工介入 · {}", p.kind_label)));

    let views = Views {
        window,
        title,
        msg_scroll,
        image: image_view,
        img_hint,
        t1,
        t2,
        input,
        opts: opt_buttons,
        cancel,
        submit,
        orig,
        zoom,
    };
    let snapshot = ViewsSnapshot::from(&views);

    // ---- 定位 + 排版(用 place_window 的返回值,不回读)----
    let (fw, fh) = place_window(window, W, normal_h);
    apply_frames(&snapshot, &layout_frames(fw, fh, geo, p.options.len()), geo);

    // ---- 焦点与激活 ----
    send_void_id(window, sel(b"makeKeyAndOrderFront:\0"), std::ptr::null_mut());
    send_void_bool(nsapp_ptr(), sel(b"activateIgnoringOtherApps:\0"), true);
    send_bool_id(window, sel(b"makeFirstResponder:\0"), input);

    // ---- 定时器 ----
    let tick = send_id_f64_id_sel_id_bool(
        cls(b"NSTimer\0"),
        sel(b"scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:\0"),
        1.0,
        ctrl,
        sel(b"tick:\0"),
        std::ptr::null_mut(),
        true,
    );
    let focus_once = send_id_f64_id_sel_id_bool(
        cls(b"NSTimer\0"),
        sel(b"scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:\0"),
        0.05,
        ctrl,
        sel(b"focusOnce:\0"),
        std::ptr::null_mut(),
        false,
    );
    let _ = focus_once; // 一次性 timer fire 后自动失效,无需手工清理

    // ---- 状态入位 + 跑事件循环 ----
    *state() = Some(MacState {
        views,
        options: p.options.clone(),
        image_open_path,
        normal_geo: geo,
        normal_content: (fw, fh),
        img_ratio,
        zoomed: false,
        timer: tick,
        started: Instant::now(),
        timeout_ms: p.timeout_ms,
        done: false,
        result: None,
    });

    send_void(send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0")), sel(b"run\0"));

    // run 返回:取结果(意外返回兜底 cancel,对齐旧脚本)
    let result = {
        let mut guard = state();
        guard
            .as_mut()
            .and_then(|st| st.result.take())
            .unwrap_or_else(|| ("cancel".to_string(), String::new()))
    };
    *state() = None;
    result
}

unsafe fn nsapp_ptr() -> Id {
    send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"))
}

/// 装载 Foundation + AppKit(失败 = 无桌面会话或系统异常)。
/// RTLD_GLOBAL 让框架镜像注册的 ObjC 类对 objc_getClass 可见。
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

/// 说明区:NSScrollView + NSTextView(selectable = 可选中复制,第 132 轮关键)。
unsafe fn make_message_view(text: &str) -> (Id, Id) {
    let scroll = send_id_rect(
        send_id(cls(b"NSScrollView\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, 10.0, 10.0),
    );
    send_void_bool(scroll, sel(b"setHasVerticalScroller:\0"), true);
    send_void_i64(scroll, sel(b"setBorderType:\0"), NS_BEZEL_BORDER as i64);
    send_void_bool(scroll, sel(b"setDrawsBackground:\0"), false);
    let tv = send_id_rect(
        send_id(cls(b"NSTextView\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, 10.0, 10.0),
    );
    send_void_bool(tv, sel(b"setEditable:\0"), false);
    send_void_bool(tv, sel(b"setSelectable:\0"), true);
    send_void_bool(tv, sel(b"setRichText:\0"), false);
    send_void_bool(tv, sel(b"setDrawsBackground:\0"), false);
    send_void_id(tv, sel(b"setFont:\0"), font(12.0, false));
    send_void_id(tv, sel(b"setString:\0"), ns_str(text));
    send_void_id(scroll, sel(b"setDocumentView:\0"), tv);
    (scroll, tv)
}

unsafe fn font(size: f64, bold: bool) -> Id {
    let name = if bold {
        sel(b"boldSystemFontOfSize:\0")
    } else {
        sel(b"systemFontOfSize:\0")
    };
    send_id_f64(cls(b"NSFont\0"), name, size)
}

/// 单行不可编辑标签。
unsafe fn make_label(size: f64, bold: bool) -> Id {
    let l = send_id_rect(
        send_id(cls(b"NSTextField\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, 10.0, 10.0),
    );
    send_void_bool(l, sel(b"setEditable:\0"), false);
    send_void_bool(l, sel(b"setBezeled:\0"), false);
    send_void_bool(l, sel(b"setDrawsBackground:\0"), false);
    send_void_id(l, sel(b"setFont:\0"), font(size, bold));
    l
}

unsafe fn make_button(
    text: &str,
    key_eq: Option<&str>,
    target: Id,
    action: &'static [u8],
    tag: i64,
) -> Id {
    let b = send_id(send_id(cls(b"NSButton\0"), sel(b"alloc\0")), sel(b"init\0"));
    send_void_id(b, sel(b"setTitle:\0"), ns_str(text));
    send_void_i64(b, sel(b"setBezelStyle:\0"), NS_ROUNDED_BEZEL_STYLE);
    if let Some(k) = key_eq {
        send_void_id(b, sel(b"setKeyEquivalent:\0"), ns_str(k));
    }
    send_void_i64(b, sel(b"setTag:\0"), tag);
    send_void_id(b, sel(b"setTarget:\0"), target);
    send_void_sel(b, sel(b"setAction:\0"), sel(action));
    b
}

/// 载入验证码图片(NSImage;失败静默降级为无图,fail-open)。
unsafe fn load_image(p: &DialogPayload) -> (Option<Id>, Option<Id>, f64, Option<String>) {
    if p.image_path.is_empty() {
        return (None, None, 0.0, None);
    }
    let img = send_id_cstr(
        send_id(cls(b"NSImage\0"), sel(b"alloc\0")),
        sel(b"initWithContentsOfFile:\0"),
        CString::new(p.image_path.as_str()).unwrap_or_default().as_ptr(),
    );
    if img.is_null() || !send_bool(img, sel(b"isValid\0")) {
        return (None, None, 0.0, None);
    }
    let size = send_size(img, sel(b"size\0"));
    let (w, h) = (size.width, size.height);
    if w <= 0.0 || h <= 0.0 {
        return (None, None, 0.0, None);
    }
    let iv = send_id_rect(
        send_id(cls(b"NSImageView\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, 10.0, 10.0),
    );
    send_void_id(iv, sel(b"setImage:\0"), img);
    send_void_i64(iv, sel(b"setImageScaling:\0"), NS_IMAGE_SCALE_PROPORTIONALLY_DOWN);
    let hint = make_label(10.0, false);
    send_void_bool(hint, sel(b"setSelectable:\0"), false);
    let color = send_id(cls(b"NSColor\0"), sel(b"secondaryLabelColor\0"));
    if !color.is_null() {
        send_void_id(hint, sel(b"setTextColor:\0"), color);
    }
    send_void_id(
        hint,
        sel(b"setStringValue:\0"),
        ns_str(&format!(
            "原图 {}×{} px · 「查看原图」可用系统查看器放大细读",
            w as i64, h as i64
        )),
    );
    (Some(iv), Some(hint), h / w, Some(p.image_path.clone()))
}

// ---------- 布局单测(纯函数,不触达 AppKit) ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_frames_in_bounds(cw: f64, ch: f64, geo: Geometry, opt_count: usize) {
        let f = layout_frames(cw, ch, geo, opt_count);
        let mut rects = vec![
            ("title", f.title),
            ("msg", f.msg),
            ("t1", f.t1),
            ("t2", f.t2),
            ("input", f.input),
            ("cancel", f.cancel),
            ("submit", f.submit),
            ("orig", f.orig),
            ("zoom", f.zoom),
        ];
        for (i, r) in f.opts.iter().enumerate() {
            rects.push((Box::leak(format!("opt{i}").into_boxed_str()) as &str, *r));
        }
        if geo.img_h > 0.0 {
            rects.push(("image", f.image.unwrap()));
            rects.push(("img_hint", f.img_hint.unwrap()));
        }
        for (name, r) in rects {
            assert!(r.origin.x >= 0.0, "{name} x={}", r.origin.x);
            assert!(r.origin.y >= 0.0, "{name} y={} (ch={ch})", r.origin.y);
            assert!(
                r.origin.y + r.size.height <= ch + 0.5,
                "{name} bottom={} 超出内容高 {ch}",
                r.origin.y + r.size.height
            );
            assert!(
                r.origin.x + r.size.width <= cw + 0.5,
                "{name} right={} 超出内容宽 {cw}",
                r.origin.x + r.size.width
            );
        }
        // 底部按钮行同行对齐
        let row = f.cancel.origin.y;
        assert_eq!(f.submit.origin.y, row);
        assert_eq!(f.orig.origin.y, row);
        assert_eq!(f.zoom.origin.y, row);
    }

    #[test]
    fn layout_budget_matches_placement() {
        for opt_count in [1usize, 2, 6] {
            for geo in [
                Geometry { msg_h: 90.0, img_h: 0.0 },
                Geometry { msg_h: 90.0, img_h: 60.0 },
                Geometry { msg_h: 240.0, img_h: 300.0 },
                Geometry { msg_h: 133.0, img_h: 217.5 },
            ] {
                // 精确预算:底行钉在底边距 PAD
                let h = content_height(geo, opt_count);
                let f = layout_frames(W, h, geo, opt_count);
                assert_frames_in_bounds(W, h, geo, opt_count);
                assert!(
                    (f.cancel.origin.y - PAD).abs() < 0.51,
                    "精确预算下底部行 y={} 应 ≈{PAD}",
                    f.cancel.origin.y
                );
                // 同一几何在「更大的窗口」里:仍在界内,且底行钉在窗口底边(y=PAD)
                let ch = 1200.0f64.max(h);
                let big = layout_frames(1400.0, ch, geo, opt_count);
                assert_frames_in_bounds(1400.0, ch, geo, opt_count);
                assert!(
                    (big.cancel.origin.y - PAD).abs() < 0.51,
                    "大窗口下底部行 y={} 应钉在 {}",
                    big.cancel.origin.y,
                    PAD
                );
            }
        }
    }

    #[test]
    fn image_height_clamps() {
        assert_eq!(image_height_for(0.0, 528.0, 300.0), 0.0);
        assert_eq!(image_height_for(0.01, 528.0, 300.0), 60.0); // 下限
        assert_eq!(image_height_for(1.0, 528.0, 300.0), 300.0); // 上限
        assert!((image_height_for(0.5, 528.0, 300.0) - 264.0).abs() <= 0.5);
    }
}
