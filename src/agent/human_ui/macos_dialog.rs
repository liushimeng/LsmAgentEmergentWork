//! macOS 弹窗后端:Rust FFI 直调 AppKit / ObjC runtime(第 137 轮原生重构)。
//!
//! 设计要点见 `src/agent/human_ui/mod.rs` 模块文档头。要点:
//! - 本模块**只在 `laew __hitl-dialog` 子进程内运行**(见 `dialog_main.rs`),事件循环
//!   `NSApp.run()` 跑在子进程主线程,满足 AppKit 的主线程要求;
//! - `objc_msgSend` 按具体参数形状逐个包装(tauri/tao 验证过的做法),x86_64 的
//!   结构体返回走 `objc_msgSend_stret`,arm64 统一走 `objc_msgSend`;
//! - **布局单一真源**:`layout_frames`(排布)与 `content_height`(预算)由同一组
//!   常量/参数推导(第 134 轮事故的根治);窗口按内容定尺寸、`place_window` 落位,
//!   **永不回读**窗口 frame(第 137 轮事故的根治);
//! - 窗口 styleMask 仅 `Titled`(无系统关闭/缩放按钮),长说明在 NSScrollView 里
//!   滚动阅读(2026-10-13:「⛶ 最大化」按钮随验证码功能移除 —— 它是验证码图片
//!   放大时代的遗物,通用弹窗不需要)。

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
    // imp 统一按 `*const c_void` 收:同一符号在不同模块声明成不同函数指针
    // 类型会触发 clashing_extern_declarations(浏览器查看器要注册 5 参 IMP)。
    fn class_addMethod(
        cls: Id,
        name: Sel,
        imp: *const c_void,
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
const NS_EVENT_TYPE_APPLICATION_DEFINED: i64 = 15;
/// NSButtonType NSSwitchButton = 3:标准复选框外观,点击自动翻转 state。
const NS_SWITCH_BUTTON: i64 = 3;

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
msg!(send_id_id, (arg: Id) -> Id);
msg!(send_id_id_id_id, (a: Id, b: Id, c: Id) -> Id);
msg!(send_bool_id_id, (a: Id, b: Id) -> bool);
msg!(send_id_cstr, (arg: *const c_char) -> Id);
msg!(send_id_usize, (arg: usize) -> Id);
msg!(send_id_rect, (rect: NSRect) -> Id);
msg!(send_id_rect_u64_i64_bool, (rect: NSRect, mask: u64, backing: i64, defer: bool) -> Id);
msg!(send_id_f64_id_sel_id_bool, (t: f64, tgt: Id, act: Sel, info: Id, repeats: bool) -> Id);
msg!(send_id_i64_point_u64_f64_i64_id_i16_i64_i64, (
    ty: i64, loc: NSPoint, flags: u64, ts: f64, win: i64, ctx: Id, sub: i16, d1: i64, d2: i64
) -> Id);
msg!(send_bool_id, (arg: Id) -> bool);
msg!(send_i64, () -> i64);
msg!(send_usize, () -> usize);
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
msg!(send_void_f64, (arg: f64) -> ());
msg!(send_point, () -> NSPoint);
msg!(send_id_f64_f64_f64_f64, (r: f64, g: f64, b: f64, a: f64) -> Id);

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

// 2026-10-13:底行六键位从左到右 = 取消/继续/⏱+2分钟/解除屏蔽·恢复屏蔽(互切)
// ‖ 📋复制(右组)。「⛶ 最大化」按钮随验证码功能移除(说明区可滚动,放大是
// 验证码图片时代的遗物)。左组 16+84+8+96+8+88+8+96 = 404,右组 92 起始 572,
// 间距 168。
const W: f64 = 680.0;
const PAD: f64 = 16.0;
const TITLE_H: f64 = 22.0;
const LINE_H: f64 = 18.0;
const INPUT_H: f64 = 28.0;
const BTN_H: f64 = 30.0;
const GAP: f64 = 8.0;
const CANCEL_W: f64 = 84.0;
/// 主按钮「继续」(空输入=选项1交棒;有输入=把补充说明交给 Agent;多选时收集勾选)。
const SUBMIT_W: f64 = 96.0;
const EXTEND_W: f64 = 88.0;
/// 2026-10-13:「解除屏蔽/恢复屏蔽」互切按钮(= 侵入式语境的「非侵入式」入口,
/// 2026-10-13(三)起文案保持原文案不变)。
const GUARD_W: f64 = 96.0;
/// 2026-10-13(二):「🎮 侵入式」互切按钮(弹窗触控板 → 宿主输入回放)。
/// (三)88→100:退出态标题「⏹ 退出侵入式」更长,防截断。
const INTRUSIVE_W: f64 = 100.0;
const COPY_W: f64 = 92.0;
const INNER: f64 = 8.0;

/// 复制按钮的两个标题(第 139 轮:一键复制全部信息,免去在倒计时行里拖选)。
const COPY_BTN_TITLE: &str = "📋 复制";
const COPY_BTN_DONE: &str = "✓ 已复制";
/// 「✓ 已复制」保持时长(秒);由既有 1s 倒计时 tick 负责回滚。
const COPY_FEEDBACK_SECS: u64 = 2;

/// 一组布局参数(说明区高)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Geometry {
    msg_h: f64,
}

/// 各控件的目标 frame(自顶向下排布;AppKit y 轴自底向上,存的是最终坐标)。
#[derive(Clone, Debug, Default)]
struct Frames {
    title: NSRect,
    msg: NSRect,
    t1: NSRect,
    t2: NSRect,
    input: NSRect,
    opts: Vec<NSRect>,
    cancel: NSRect,
    submit: NSRect,
    extend: NSRect,
    guard: NSRect,
    intrusive: NSRect,
    copy: NSRect,
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
        t1: ns_rect(PAD, t1_y, text_w, LINE_H),
        t2: ns_rect(PAD, t2_y, text_w, LINE_H),
        input: ns_rect(PAD, input_y, text_w, INPUT_H),
        opts,
        cancel: ns_rect(PAD, row_y, CANCEL_W, BTN_H),
        submit: ns_rect(PAD + CANCEL_W + INNER, row_y, SUBMIT_W, BTN_H),
        extend: ns_rect(PAD + CANCEL_W + INNER + SUBMIT_W + INNER, row_y, EXTEND_W, BTN_H),
        guard: ns_rect(
            PAD + CANCEL_W + INNER + SUBMIT_W + INNER + EXTEND_W + INNER,
            row_y,
            GUARD_W,
            BTN_H,
        ),
        intrusive: ns_rect(
            PAD + CANCEL_W + INNER + SUBMIT_W + INNER + EXTEND_W + INNER + GUARD_W + INNER,
            row_y,
            INTRUSIVE_W,
            BTN_H,
        ),
        copy: ns_rect(cw - PAD - COPY_W, row_y, COPY_W, BTN_H),
    }
}

/// 2026-10-13(二):侵入式触控板矩形(纯函数,由 [`Frames`] 派生)——
/// 覆盖说明区 + 候选选项区(msg 之下到最后一个选项底边),与
/// [`layout_frames`] 同源参数,不回读窗口/控件 frame。
fn pad_frame(f: &Frames, opt_count: usize) -> NSRect {
    let mut bottom = f.msg.origin.y;
    let mut top = f.msg.origin.y + f.msg.size.height;
    if opt_count > 0 {
        let first = f.opts[0];
        let last = f.opts[opt_count - 1];
        top = (first.origin.y + first.size.height).max(top);
        bottom = bottom.min(last.origin.y);
    }
    ns_rect(
        f.msg.origin.x,
        bottom,
        f.msg.size.width,
        (top - bottom).max(f.msg.size.height),
    )
}

/// 预算纯函数:由内容推出所需内容视图高(与 [`layout_frames`] 的消费序列逐项对应)。
fn content_height(geo: Geometry, opt_count: usize) -> f64 {
    let mut consumed = TITLE_H + GAP + geo.msg_h + GAP;
    consumed += (LINE_H + GAP) * 2.0;
    consumed += INPUT_H + GAP;
    consumed += opt_count as f64 * (BTN_H + GAP);
    consumed += BTN_H; // 底部按钮行(place 不再扣 GAP)
    consumed + PAD
}

// ---------- 运行时状态(单弹窗进程,UI 线程独占) ----------

struct Views {
    window: Id,
    title: Id,
    msg_scroll: Id,
    t1: Id,
    t2: Id,
    input: Id,
    opts: Vec<Id>,
    cancel: Id,
    submit: Id,
    extend: Id,
    guard: Id,
    /// 2026-10-13(二):「🎮 侵入式」互切按钮。
    intrusive: Id,
    /// 2026-10-13(二):侵入式触控板(动态子类 LAEWRelayPad,缺省隐藏)。
    pad: Id,
    copy: Id,
}

struct MacState {
    views: Views,
    options: Vec<String>,
    /// 「📋 复制」写入剪贴板的文本(第 139 轮:一键复制全部信息)。
    copy_payload: String,
    /// 「✓ 已复制」反馈的回滚时刻(由 `on_tick` 消费)。
    copied_at: Option<Instant>,
    /// 第 145 轮:请求 id / 提出时刻毫秒 —— 「⏱ +2分钟」延长中间行携带 id,
    /// 时间轴第一行的「超时截止」随延长重写。
    payload_id: u64,
    started_at_ms: u64,
    /// 已点「⏱ +2分钟」次数(按钮标题 ×N 反馈,无回滚需求)。
    extend_count: u32,
    /// 2026-10-13:页面当前是否处于放行态 —— 「解除屏蔽/恢复屏蔽」互切按钮的
    /// 当前标题侧(false=标题「🔓 解除屏蔽」,true=「🔒 恢复屏蔽」)。
    guard_unblocked: bool,
    /// 2026-10-13:选项是否多选(复选框组);收口时收集全部勾选。
    multi_select: bool,
    /// 2026-10-13(二):是否处于「侵入式」触控板模式(弹窗内拖动/按键 → 回放)。
    intrusive: bool,
    /// 2026-10-13(二):触控板矩形(进入侵入式时由 [`pad_frame`] 落位;归一化基准)。
    pad_rect: NSRect,
    /// 2026-10-13(二):布局结果(退出侵入式时全量还原;与 [layout_frames] 同源)。
    frames: Frames,
    /// 2026-10-13(二):布局几何(还原用)。
    geo: Geometry,
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

/// 2026-10-13:多选模式选项复选框的 action —— NSSwitchButton 点击时 AppKit 已自动
/// 翻转 state,这里**只观察不收口**(多选的收口动作是「继续」按钮收集勾选);
/// 单选模式的选项仍是 push button,走 `submit:` 立即应答,不经过本 IMP。
extern "C" fn on_opt_toggle(_self: Id, _cmd: Sel, _sender: Id) {
    // 观察 no-op:保留 target/action 接线是为了点击有标准键盘焦点行为。
}

/// 第 145 轮:「⏱ +2分钟」延长等待 —— 本地 `timeout_ms += EXTEND_STEP_MS`
/// (总等待封顶 MAX_TOTAL_WAIT_MS,倒计时与「超时截止」行立即受益),同时向
/// stdout 打 extend **中间行**(进程不退出;Stdout 行缓冲,println! 即时 flush),
/// 父进程逐行读并接线 `HumanAssistHub::extend_timeout` 推迟 hub 侧截止时刻。
/// 根治实测:人工在浏览器完成整条登录链超过 120s 分档超时 → 弹窗自灭 →
/// QC 单元重试 → 二次弹窗(浪费 36s 且验证码已刷新)。
extern "C" fn on_extend(_self: Id, _cmd: Sel, _sender: Id) {
    let (line, title, extend_btn, t1, timeline) = {
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
            st.views.extend,
            st.views.t1,
            timeline,
        )
    };
    super::dialog_main::emit_line_or_exit(&line);
    unsafe {
        if !extend_btn.is_null() {
            send_void_id(extend_btn, sel(b"setTitle:\0"), ns_str(&title));
        }
        if !t1.is_null() {
            send_void_id(t1, sel(b"setStringValue:\0"), ns_str(&timeline));
        }
    }
}

/// 2026-10-13:「解除屏蔽/恢复屏蔽」互切 —— 人工在弹窗直接掌控页面管控,
/// 不必再打字请 Agent 解锁(实测「登录界面,不能点击,优化一下」事故的根治)。
/// 点击即翻转本地态 + 向 stdout 打 guard **中间行**(进程不退出;父进程逐行读
/// 并接线 `HumanAssistHub::guard_toggle_from_ui` → 对关联 page_id 应用整页 open /
/// 恢复解锁前快照),按钮标题同步切换。
extern "C" fn on_guard_toggle(_self: Id, _cmd: Sel, _sender: Id) {
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
            st.views.guard,
        )
    };
    super::dialog_main::emit_line_or_exit(&line); // 行缓冲:换行即 flush,父进程逐行读
    unsafe {
        if !btn.is_null() {
            send_void_id(btn, sel(b"setTitle:\0"), ns_str(title));
        }
    }
}

extern "C" fn on_tick(_self: Id, _cmd: Sel, _sender: Id) {
    // 单次锁定取出所需;**必须先解锁再 finish**(finish 会再次锁定 STATE)。
    let (label, text, timed_out, rollback_copy) = {
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
                Some(st.views.copy)
            }
            _ => None,
        };
        (
            st.views.t2,
            countdown_text(elapsed, remain),
            remain == 0,
            rollback,
        )
    };
    if !label.is_null() {
        unsafe {
            send_void_id(label, sel(b"setStringValue:\0"), ns_str(&text));
        }
    }
    if let Some(btn) = rollback_copy {
        unsafe {
            if !btn.is_null() {
                send_void_id(btn, sel(b"setTitle:\0"), ns_str(COPY_BTN_TITLE));
            }
        }
    }
    if timed_out {
        finish("timeout", -1);
    }
}

/// 第 139 轮:「📋 复制」—— 把说明 + 页面 URL + 页面 ID 一次性写入系统剪贴板。
///
/// 为什么不让用户自己在倒计时行里拖选:那一行每秒刷新,拖选过程中就被重绘冲掉;
/// 一键复制是确定性最高的路径,顺带把「页面 URL」「验证码提示」一起带走。
unsafe fn copy_to_pasteboard(text: &str) -> bool {
    let pb = send_id(cls(b"NSPasteboard\0"), sel(b"generalPasteboard\0"));
    if pb.is_null() {
        return false;
    }
    send_void(pb, sel(b"clearContents\0"));
    // NSPasteboardTypeString = "public.utf8-plain-text"(SDK 常量,勿改)。
    send_bool_id_id(
        pb,
        sel(b"setString:forType:\0").cast_mut(),
        ns_str(text),
        ns_str("public.utf8-plain-text"),
    )
}

extern "C" fn on_copy(_self: Id, _cmd: Sel, _sender: Id) {
    let (payload, btn) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        st.copied_at = Some(Instant::now());
        (st.copy_payload.clone(), st.views.copy)
    };
    let ok = unsafe { copy_to_pasteboard(&payload) };
    unsafe {
        if !btn.is_null() {
            let title = if ok { COPY_BTN_DONE } else { "⚠ 复制失败" };
            send_void_id(btn, sel(b"setTitle:\0"), ns_str(title));
        }
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
    // 窗口消失 ≠ 人工明确取消:无头/自动化环境下窗口可能被系统回收,
    // 报「取消」会让编排器把整个任务判死(实测猎聘任务 52s 后被误取消)。
    // 真实取消只由「取消任务」按钮(on_cancel)产生。
    finish("closed", -1);
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
            class_addMethod(
                c,
                sel(name),
                m as *const c_void,
                b"v@:@\0".as_ptr().cast(),
            ) != 0
        };
        let all = ok(on_submit, b"submit:\0")
            & ok(on_cancel, b"cancel:\0")
            & ok(on_opt_toggle, b"optToggle:\0")
            & ok(on_extend, b"extend:\0")
            & ok(on_guard_toggle, b"guardToggle:\0")
            & ok(on_relay_toggle, b"relayToggle:\0")
            & ok(on_copy, b"copyInfo:\0")
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

// ---------- 2026-10-13(二):侵入式触控板(NSView 动态子类 + 事件 IMP) ----------

/// 触控板事件 → relay 中间行(归一化坐标 0..1,页面 y 轴向下;AppKit y 轴向上
/// 故 y 取反)。`state` 非侵入式/收口后一律丢弃(迟到的鼠标事件不作用)。
fn pad_relay(event: Id, kind: &str) {
    let (id, rect) = {
        let guard = state();
        match guard.as_ref() {
            Some(st) if st.intrusive && !st.done => (st.payload_id, st.pad_rect),
            _ => return,
        }
    };
    let p = unsafe { send_point(event, sel(b"locationInWindow\0")) };
    let w = rect.size.width.max(1.0);
    let h = rect.size.height.max(1.0);
    let nx = ((p.x - rect.origin.x) / w).clamp(0.0, 1.0);
    let ny = (1.0 - (p.y - rect.origin.y) / h).clamp(0.0, 1.0);
    super::dialog_main::emit_line_or_exit(&super::dialog_main::relay_line(id, kind, nx, ny, ""));
}

extern "C" fn on_pad_down(_self: Id, _cmd: Sel, event: Id) {
    pad_relay(event, "down");
}
extern "C" fn on_pad_drag(_self: Id, _cmd: Sel, event: Id) {
    pad_relay(event, "move");
}
extern "C" fn on_pad_up(_self: Id, _cmd: Sel, event: Id) {
    pad_relay(event, "up");
}

/// 触控板按键 → relay key 行(AppKit keyCode → 语义名;回车/空格可直接提交挑战)。
extern "C" fn on_pad_key(_self: Id, _cmd: Sel, event: Id) {
    let code = unsafe { send_i64(event, sel(b"keyCode\0")) };
    let name = match code {
        36 | 76 => "enter",
        53 => "esc",
        48 => "tab",
        49 => "space",
        51 => "backspace",
        117 => "delete",
        123 => "left",
        124 => "right",
        125 => "down",
        126 => "up",
        _ => return,
    };
    let id = {
        let guard = state();
        match guard.as_ref() {
            Some(st) if st.intrusive && !st.done => st.payload_id,
            _ => return,
        }
    };
    super::dialog_main::emit_line_or_exit(&super::dialog_main::relay_line(id, "key", 0.0, 0.0, name));
}

/// 触控板必须能成为第一响应者才能收到 keyDown:(YES = 1 的 BOOL)。
extern "C" fn pad_accepts_first_responder(_self: Id, _cmd: Sel) -> i8 {
    1
}

/// 动态注册 LAEWRelayPad : NSView(进程内一次)。
fn relay_pad_class() -> Id {
    static PAD_CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *PAD_CLASS.get_or_init(|| unsafe {
        let superclass = cls(b"NSView\0");
        if superclass.is_null() {
            return 0;
        }
        let c = objc_allocateClassPair(superclass, b"LaewRelayPad\0".as_ptr().cast(), 0);
        if c.is_null() {
            return 0;
        }
        let ok = |m: extern "C" fn(Id, Sel, Id), name: &'static [u8]| {
            class_addMethod(
                c,
                sel(name),
                m as *const c_void,
                b"v@:@\0".as_ptr().cast(),
            ) != 0
        };
        let all = ok(on_pad_down, b"mouseDown:\0")
            & ok(on_pad_drag, b"mouseDragged:\0")
            & ok(on_pad_up, b"mouseUp:\0")
            & ok(on_pad_key, b"keyDown:\0")
            & {
                // 返回 BOOL 的 IMP:objc 只传 (self, _cmd),第三个参数位是无关位,
                // 按 class_addMethod 声明的三元签名做同尺寸函数指针转换(ARM64/x86_64
                // 调用约定下参数位多余不影响返回寄存器)。
                #[allow(clippy::missing_transmute_annotations)]
                let fpr: extern "C" fn(Id, Sel, Id) =
                    std::mem::transmute(pad_accepts_first_responder as extern "C" fn(Id, Sel) -> i8);
                class_addMethod(
                    c,
                    sel(b"acceptsFirstResponder\0"),
                    fpr as *const c_void,
                    b"c@:\0".as_ptr().cast(),
                ) != 0
            };
        if !all {
            return 0;
        }
        objc_registerClassPair(c);
        c as usize
    });
    ptr as Id
}

/// 建触控板视图(深色底 + 虚线提示文字;缺省隐藏)。
unsafe fn make_relay_pad() -> Id {
    let c = relay_pad_class();
    if c.is_null() {
        return std::ptr::null_mut();
    }
    let pad = send_id_rect(
        send_id(c, sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, 10.0, 10.0),
    );
    send_void_bool(pad, sel(b"setWantsLayer:\0"), true);
    let color = send_id_f64_f64_f64_f64(
        cls(b"NSColor\0"),
        sel(b"colorWithCalibratedRed:green:blue:alpha:\0"),
        0.09,
        0.13,
        0.21,
        1.0,
    );
    let layer = send_id(pad, sel(b"layer\0"));
    if !layer.is_null() {
        send_void_id(layer, sel(b"setBackgroundColor:\0"), send_id(color, sel(b"CGColor\0")));
        send_void_f64(layer, sel(b"setCornerRadius:\0"), 8.0);
    }
    // 提示文字(可选中复制;居中偏上,不挡操作)。
    let hint = make_label(11.0, false);
    send_void_id(hint, sel(b"setTextColor:\0"), send_id_f64_f64_f64_f64(
        cls(b"NSColor\0"),
        sel(b"colorWithCalibratedRed:green:blue:alpha:\0"),
        0.85,
        0.89,
        0.96,
        1.0,
    ));
    send_void_id(hint, sel(b"setStringValue:\0"), ns_str(super::dialog_main::RELAY_PAD_HINT));
    send_void_rect(hint, sel(b"setFrame:\0"), ns_rect(10.0, 8.0, 600.0, 34.0));
    send_void_id(pad, sel(b"addSubview:\0"), hint);
    send_void_bool(pad, sel(b"setHidden:\0"), true);
    pad
}

/// 2026-10-13(二):「🎮 侵入式」互切 —— 进入时隐藏说明区/选项区、显示触控板并
/// 打 `session_start` 中间行(父进程放行页面 + 解析映射区 + 置前窗口);退出时
/// 还原布局并打 `session_end`。
extern "C" fn on_relay_toggle(_self: Id, _cmd: Sel, _sender: Id) {
    let (line, title, pad, msg_scroll, opts, input, btn, snap, frames, geo, now_on, pad_rect) = {
        let mut guard = state();
        let Some(st) = guard.as_mut() else { return };
        if st.done {
            return;
        }
        st.intrusive = !st.intrusive;
        let pad_rect = pad_frame(&st.frames, st.views.opts.len());
        if st.intrusive {
            st.pad_rect = pad_rect;
        }
        let line = if st.intrusive {
            super::dialog_main::relay_line(st.payload_id, "session_start", 0.0, 0.0, "")
        } else {
            super::dialog_main::relay_line(st.payload_id, "session_end", 0.0, 0.0, "")
        };
        let title = if st.intrusive {
            super::dialog_main::INTRUSIVE_BTN_EXIT_TITLE
        } else {
            super::dialog_main::INTRUSIVE_BTN_TITLE
        };
        (
            line,
            title,
            st.views.pad,
            st.views.msg_scroll,
            st.views.opts.clone(),
            st.views.input,
            st.views.intrusive,
            ViewsSnapshot::from(&st.views),
            st.frames.clone(),
            st.geo,
            st.intrusive,
            pad_rect,
        )
    };
    super::dialog_main::emit_line_or_exit(&line);
    unsafe {
        send_void_bool(msg_scroll, sel(b"setHidden:\0"), now_on);
        for o in &opts {
            send_void_bool(*o, sel(b"setHidden:\0"), now_on);
        }
        send_void_bool(input, sel(b"setHidden:\0"), now_on);
        if !pad.is_null() {
            send_void_bool(pad, sel(b"setHidden:\0"), !now_on);
            if now_on {
                send_void_rect(pad, sel(b"setFrame:\0"), pad_rect);
                // 触控板成为第一响应者,接收键盘(Enter/Space 直接提交滑块验证)。
                let window = send_id(pad, sel(b"window\0"));
                if !window.is_null() {
                    send_void_id(window, sel(b"makeFirstResponder:\0"), pad);
                }
            } else {
                apply_frames(&snap, &frames, geo);
            }
        } else if !now_on {
            apply_frames(&snap, &frames, geo);
        }
        if !btn.is_null() {
            send_void_id(btn, sel(b"setTitle:\0"), ns_str(title));
        }
    }
}

// ---------- 收口 ----------

/// 收口(只在 UI 线程调用):结果映射 → 停定时器 → 停 run loop。
/// - answer + 输入框有字 → answer(文本);
/// - 无字 + 多选 → 收集全部勾选(`join_multi_answer`);空勾选回退选项 1(交棒);
/// - 无字 + tag>=0 → answer(选项 N,单选按钮点击);
/// - 无字 + tag<0(提交按钮/输入框回车)→ answer(选项 1,对齐「空回车=选项1」);
/// - cancel / timeout 直传。
fn finish(kind: &str, tag: i64) {
    let mut was_intrusive = false;
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
        } else if kind == "closed" {
            ("closed".to_string(), String::new())
        } else {
            let typed = unsafe { read_input(st.views.input) };
            if !typed.is_empty() {
                ("answer".to_string(), typed)
            } else if st.multi_select {
                let picked = collect_checked(&st.views.opts);
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
                // options 由 payload 投影保证非空
                ("answer".to_string(), format!("1. {}", st.options[0]))
            }
        };
        st.result = Some(result);
        // 2026-10-13(二):侵入式会话随收口一并结束(父进程据此恢复页面管控;
        // 收口路径本身也会恢复,幂等双保险)。
        was_intrusive = st.intrusive;
    }
    if was_intrusive {
        let id = {
            let guard = state();
            guard.as_ref().map(|st| st.payload_id).unwrap_or(0)
        };
        if id > 0 {
            super::dialog_main::emit_line_or_exit(&super::dialog_main::relay_line(
                id, "session_end", 0.0, 0.0, "",
            ));
        }
    }
    unsafe { stop_run_loop() };
}

/// 多选模式:读取全部选项复选框的勾选态(NSState 0=off/1=on),返回勾选下标(0 基)。
fn collect_checked(opts: &[Id]) -> Vec<usize> {
    let mut picked = Vec::with_capacity(opts.len());
    for (i, b) in opts.iter().enumerate() {
        if !b.is_null() && unsafe { send_i64(*b, sel(b"state\0")) } == 1 {
            picked.push(i);
        }
    }
    picked
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

// ---------- 排版落位 ----------

/// IMP 与主流程之间传递的控件快照(裸指针拷贝;子进程 UI 线程内恒有效)。
struct ViewsSnapshot {
    title: Id,
    msg_scroll: Id,
    t1: Id,
    t2: Id,
    input: Id,
    opts: Vec<Id>,
    cancel: Id,
    submit: Id,
    extend: Id,
    guard: Id,
    intrusive: Id,
    pad: Id,
    copy: Id,
}

impl ViewsSnapshot {
    fn from(v: &Views) -> Self {
        Self {
            title: v.title,
            msg_scroll: v.msg_scroll,
            t1: v.t1,
            t2: v.t2,
            input: v.input,
            opts: v.opts.clone(),
            cancel: v.cancel,
            submit: v.submit,
            extend: v.extend,
            guard: v.guard,
            intrusive: v.intrusive,
            pad: v.pad,
            copy: v.copy,
        }
    }
}

/// 把布局结果落到控件(统一全量设置)。
unsafe fn apply_frames(v: &ViewsSnapshot, f: &Frames, _geo: Geometry) {
    let set = |view: Id, rect: NSRect| {
        if !view.is_null() {
            send_void_rect(view, sel(b"setFrame:\0"), rect);
        }
    };
    set(v.title, f.title);
    set(v.msg_scroll, f.msg);
    set(v.t1, f.t1);
    set(v.t2, f.t2);
    set(v.input, f.input);
    for (view, rect) in v.opts.iter().zip(f.opts.iter()) {
        set(*view, *rect);
    }
    set(v.cancel, f.cancel);
    set(v.submit, f.submit);
    set(v.extend, f.extend);
    set(v.guard, f.guard);
    set(v.intrusive, f.intrusive);
    set(v.copy, f.copy);
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
    // ---- 主菜单(第 139 轮):必须在建任何控件之前装,保证 ⌘C/⌘V/⌘A 随时可用 ----
    install_main_menu("人工介入");

    // ---- 控制器 ----
    let ctrl_cls = controller_class();
    if ctrl_cls.is_null() {
        return ("error".into(), "控制器类注册失败(libobjc)".into());
    }
    let ctrl = send_id(send_id(ctrl_cls, sel(b"alloc\0")), sel(b"init\0"));

    // ---- 说明与几何 ----
    let geo = Geometry { msg_h: 90.0 };
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
    // 第 139 轮:显式声明可选中,不再依赖 editable 隐式带出的 field editor 行为。
    send_void_bool(input, sel(b"setSelectable:\0"), true);
    send_void_id(input, sel(b"setFont:\0"), font(13.0, false));
    // 2026-10-13:通用补充说明语义(验证码专属功能已移除);留空点「继续」= 交棒。
    send_void_id(input, sel(b"setPlaceholderString:\0"), ns_str("可选:给 Agent 的补充说明;留空点『继续』交还控制"));
    send_void_id(input, sel(b"setTarget:\0"), ctrl);
    send_void_sel(input, sel(b"setAction:\0"), sel(b"submit:\0"));
    send_void_i64(input, sel(b"setTag:\0"), -1); // 输入框回车:有字=文本,无字=选项 1

    let mut opt_buttons = Vec::new();
    for (i, opt) in p.options.iter().enumerate() {
        // 2026-10-13:多选 = NSSwitchButton 复选框(optToggle: 只观察不收口,
        // 收口动作是「继续」收集勾选);单选 = push button 点击即应答(现状)。
        let b = if p.multi_select {
            make_check_option(opt, ctrl, i as i64)
        } else {
            make_button(opt, None, ctrl, b"submit:\0", i as i64)
        };
        opt_buttons.push(b);
    }
    let cancel = make_button("取消", Some("\x1b"), ctrl, b"cancel:\0", -1);
    // 2026-10-13:主按钮「继续」 —— 人工操作完成后点击,把流程与控制权交还 Agent
    // (有输入=把补充说明交给 Agent;多选=收集勾选;空输入=选项 1「我已完成人工操作,继续」)。
    let submit = make_button("继续", Some("\r"), ctrl, b"submit:\0", -1);
    // 第 145 轮:「⏱ +2分钟」延长等待(人工在浏览器完成整条登录链超 120s 分档
    // 超时的根治;stdout extend 中间行 → 父进程 hub 延长)。
    let extend = make_button(super::dialog_main::EXTEND_BTN_TITLE, None, ctrl, b"extend:\0", -1);
    // 2026-10-13:「解除屏蔽/恢复屏蔽」互切(初始标题按页面实际放行态)。
    let guard = make_button(
        if p.guard_unlocked {
            super::dialog_main::GUARD_BTN_LOCK_TITLE
        } else {
            // (三)文案保持原文案:「🔓 解除屏蔽」即侵入式语境的「非侵入式」入口
            // (整页开放,人工直接操作;实测改名会造成「文案跑到按钮位置上」的错位观感)。
            super::dialog_main::GUARD_BTN_UNLOCK_TITLE
        },
        None,
        ctrl,
        b"guardToggle:\0",
        -1,
    );
    // 2026-10-13(二):「🎮 侵入式」互切按钮 + 触控板(弹窗内拖动/按键 → 宿主可信
    // 物理输入回放;根治实测「登录后滑块弹窗人工无论如何都操作不了」)。
    let intrusive_btn = make_button(
        super::dialog_main::INTRUSIVE_BTN_TITLE,
        None,
        ctrl,
        b"relayToggle:\0",
        -1,
    );
    let pad = make_relay_pad();
    // 第 139 轮:一键复制全部信息(说明 + 页面 URL + 页面 ID + 候选选项)。
    let copy_btn = make_button(COPY_BTN_TITLE, None, ctrl, b"copyInfo:\0", -1);

    // ---- 装配 ----
    let add = |parent: Id, child: Id| {
        if !child.is_null() {
            send_void_id(parent, sel(b"addSubview:\0"), child);
        }
    };
    add(content, title);
    add(content, msg_scroll);
    add(content, t1);
    add(content, t2);
    add(content, input);
    for b in &opt_buttons {
        add(content, *b);
    }
    add(content, cancel);
    add(content, submit);
    add(content, extend);
    add(content, guard);
    add(content, intrusive_btn);
    add(content, pad);
    add(content, copy_btn);

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
        t1,
        t2,
        input,
        opts: opt_buttons,
        cancel,
        submit,
        extend,
        guard,
        intrusive: intrusive_btn,
        pad,
        copy: copy_btn,
    };
    let snapshot = ViewsSnapshot::from(&views);

    // ---- 定位 + 排版(用 place_window 的返回值,不回读)----
    let (fw, fh) = place_window(window, W, normal_h);
    let frames = layout_frames(fw, fh, geo, p.options.len());
    apply_frames(&snapshot, &frames, geo);

    // ---- 置前显示(2026-10-16:不再抢键盘焦点) ----
    // 实测事故(Windows 侧同病,BOSS 直聘任务):弹窗弹出瞬间抢占系统键盘焦点,
    // 用户在其它窗口打字,下一次 Enter 落进弹窗触发提交 →「假人工已响应」,
    // 复验不过→再弹窗→再误触空转。macOS 同构根治:
    // - orderFrontRegardless:置前可见但**不夺 key window**;
    // - 不 activateIgnoringOtherApps(不强抢 app 激活);
    // - 不预设 makeFirstResponder(人工点击弹窗后窗口自然 becomeKey,点输入框
    //   才聚焦;输入框 Enter → submit: 仍是真交互提交路径,见 1118 行 setAction)。
    send_void_id(window, sel(b"orderFrontRegardless\0"), std::ptr::null_mut());

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
    let _ = tick; // 事件循环跑完由 NSWindow 释放;state 置空后 tick 空转无害
    // 2026-10-16:删除 focusOnce(0.05s 二次断言 first responder)——弹窗不再
    // 预设键盘焦点后,该断言只会把焦点抢回输入框,重新制造误触面。

    // ---- 状态入位 + 跑事件循环 ----
    *state() = Some(MacState {
        views,
        options: p.options.clone(),
        copy_payload: p.copy_text(),
        copied_at: None,
        payload_id: p.id,
        started_at_ms: p.started_at_ms,
        extend_count: 0,
        guard_unblocked: p.guard_unlocked,
        multi_select: p.multi_select,
        intrusive: false,
        pad_rect: ns_rect(0.0, 0.0, 1.0, 1.0),
        frames,
        geo,
        timer: tick,
        started: Instant::now(),
        timeout_ms: p.timeout_ms,
        done: false,
        result: None,
    });

    send_void(send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0")), sel(b"run\0"));

    // run 返回:取结果。**意外返回不得报「人工取消」** —— AppKit 的 run 循环
    // 在无头/自动化环境可能提前返回,报 cancel 会让编排器把整个任务判死
    // (实测猎聘任务 request_human 52s 后返 4002 人工取消,实际无人操作)。
    // 兜底为 closed(未收到应答),由上层按「通道不可用」诚实收口。
    let result = {
        let mut guard = state();
        guard
            .as_mut()
            .and_then(|st| st.result.take())
            .unwrap_or_else(|| ("closed".to_string(), String::new()))
    };
    *state() = None;
    result
}

unsafe fn nsapp_ptr() -> Id {
    send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"))
}

// ---------- 主菜单(第 139 轮:文本选择与复制) ----------

/// 菜单项定义(纯数据,可单测)。`action` 为标准 responder action —— 系统把它沿
/// responder chain 派发给 first responder,因此输入框(NSTextField)与说明区
/// (NSTextView)同时生效,无需各自接线。
const EDIT_MENU_ITEMS: &[MenuItem] = &[
    MenuItem { title: "剪切", action: b"cut:\0", key: "x" },
    MenuItem { title: "复制", action: b"copy:\0", key: "c" },
    MenuItem { title: "粘贴", action: b"paste:\0", key: "v" },
    MenuItem { title: "全选", action: b"selectAll:\0", key: "a" },
];

#[derive(Clone, Copy)]
struct MenuItem {
    title: &'static str,
    action: &'static [u8],
    /// 无修饰键时 AppKit 默认按 ⌘ 解析,故单写字母即可。
    key: &'static str,
}

/// 构建一个 NSMenu:返回 (顶层菜单项, 子菜单)。
///
/// 顶层菜单项本身是空 NSMenuItem,真正的菜单项挂在它的 submenu 上 —— 这是
/// NSMenuItem 作为子菜单容器时的标准用法。
unsafe fn build_menu(title: &str, items: &[MenuItem]) -> (Id, Id) {
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
    (parent, sub)
}

/// 装主菜单(第 139 轮)。
///
/// 此前本弹窗**完全没有 NSMenu**:输入框的 ⌘C/⌘V 全靠 NSTextField 自己在
/// responder chain 上接住 `copy:`/`paste:`,无菜单条、无右键菜单,行为不可预期;
/// 标签控件更是 `selectable=NO`,一个字都复制不出来。这里补一条标准 Edit 菜单,
/// 让「菜单项 → responder chain → first responder」这条系统路径显式可用。
unsafe fn install_main_menu(app_name: &str) {
    let menu = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str("MainMenu"),
    );
    // 应用菜单(窗口只带 Titled,没有系统按钮,应用菜单给出 ⌘Q 之外的退出路径,
    // 也让菜单栏首项符合 macOS 惯例)。
    let (app_item, _app_sub) = build_menu(
        app_name,
        &[MenuItem { title: "关于本弹窗", action: b"orderFrontStandardAboutPanel:\0", key: "" }],
    );
    send_void_id(menu, sel(b"addItem:\0"), app_item);
    let (edit_item, _edit_sub) = build_menu("编辑", EDIT_MENU_ITEMS);
    send_void_id(menu, sel(b"addItem:\0"), edit_item);
    let app = nsapp_ptr();
    if !app.is_null() {
        send_void_id(app, sel(b"setMainMenu:\0"), menu);
    }
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
    // 第 139 轮:文档视图必须可垂直伸缩,长说明才能在 NSScrollView 里真滚起来 ——
    // 滚不起来就等于「看不到尾部」,自然也选不中尾部文本。
    send_void_bool(tv, sel(b"setVerticallyResizable:\0"), true);
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

/// 单行只读标签(第 139 轮:`selectable=YES` + `editable=NO` = 标准只读可选文本,
/// 鼠标可拖选、⌘C 可复制 —— 标题/时间轴/倒计时/图片提示全靠这一行变成可复制)。
unsafe fn make_label(size: f64, bold: bool) -> Id {
    let l = send_id_rect(
        send_id(cls(b"NSTextField\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, 10.0, 10.0),
    );
    send_void_bool(l, sel(b"setEditable:\0"), false);
    send_void_bool(l, sel(b"setSelectable:\0"), true);
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

/// 2026-10-13:多选模式的选项复选框(NSSwitchButton)—— 标题即选项文本,点击由
/// AppKit 自动翻转勾选态(action `optToggle:` 只观察不收口);与 `make_button`
/// 的差异:无 bezel(标准复选框外观)、无快捷键。
unsafe fn make_check_option(text: &str, target: Id, tag: i64) -> Id {
    let b = send_id(send_id(cls(b"NSButton\0"), sel(b"alloc\0")), sel(b"init\0"));
    send_void_id(b, sel(b"setTitle:\0"), ns_str(text));
    send_void_i64(b, sel(b"setButtonType:\0"), NS_SWITCH_BUTTON);
    send_void_i64(b, sel(b"setTag:\0"), tag);
    send_void_id(b, sel(b"setTarget:\0"), target);
    send_void_sel(b, sel(b"setAction:\0"), sel(b"optToggle:\0"));
    b
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
            ("extend", f.extend),
            ("guard", f.guard),
            ("intrusive", f.intrusive),
            ("copy", f.copy),
        ];
        for (i, r) in f.opts.iter().enumerate() {
            rects.push((Box::leak(format!("opt{i}").into_boxed_str()) as &str, *r));
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
        assert_eq!(f.extend.origin.y, row);
        assert_eq!(f.guard.origin.y, row);
        assert_eq!(f.copy.origin.y, row);
        // 底部按钮行不重叠(2026-10-13:六键位 = 取消/继续/+2分钟/管控切换/复制)
        let mut row_rects = [
            ("submit", f.submit),
            ("extend", f.extend),
            ("guard", f.guard),
            ("intrusive", f.intrusive),
            ("copy", f.copy),
        ];
        row_rects.sort_by(|a, b| a.1.origin.x.total_cmp(&b.1.origin.x));
        for w in row_rects.windows(2) {
            let (an, a) = w[0];
            let (bn, b) = w[1];
            let a_right = a.origin.x + a.size.width;
            assert!(
                a_right <= b.origin.x + 0.01,
                "底部按钮重叠:{an} 右缘 {a_right} > {bn} 左缘 {}",
                b.origin.x
            );
        }
    }

    #[test]
    fn layout_budget_matches_placement() {
        for opt_count in [1usize, 2, 6] {
            for geo in [
                Geometry { msg_h: 90.0 },
                Geometry { msg_h: 240.0 },
                Geometry { msg_h: 133.0 },
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

    /// 2026-10-13:「解除屏蔽/恢复屏蔽」切换按钮的两个标题必须互异且与
    /// dialog_main 常量同源(少一个都会导致按钮停在错误标题上)。
    #[test]
    fn guard_button_titles_are_paired() {
        use super::super::dialog_main::{GUARD_BTN_LOCK_TITLE, GUARD_BTN_UNLOCK_TITLE};
        assert_eq!(GUARD_BTN_UNLOCK_TITLE, "🔓 解除屏蔽");
        assert_eq!(GUARD_BTN_LOCK_TITLE, "🔒 恢复屏蔽");
        assert_ne!(GUARD_BTN_UNLOCK_TITLE, GUARD_BTN_LOCK_TITLE);
    }

    /// 第 139 轮:「📋 复制」按钮的文案常量必须成对(标题 + 反馈 + 回滚目标),
    /// 少一个都会导致按钮停在错误标题上。
    #[test]
    fn copy_button_titles_are_paired() {
        assert_eq!(COPY_BTN_TITLE, "📋 复制");
        assert_eq!(COPY_BTN_DONE, "✓ 已复制");
        assert!(COPY_BTN_TITLE != COPY_BTN_DONE);
        assert!(COPY_FEEDBACK_SECS >= 1, "反馈至少显示 1s,否则肉眼看不到");
    }

    /// 第 139 轮:Edit 菜单必须覆盖剪切/复制/粘贴/全选,且每项都有单字母快捷键
    /// (AppKit 默认按 ⌘ 解析 keyEquivalent),否则 ⌘C/⌘V 依然走不通。
    #[test]
    fn edit_menu_covers_clipboard_shortcuts() {
        let actions: Vec<&[u8]> = EDIT_MENU_ITEMS.iter().map(|i| i.action).collect();
        for want in [b"cut:\0".as_slice(), b"copy:\0", b"paste:\0", b"selectAll:\0"] {
            assert!(actions.contains(&want), "Edit 菜单缺少 action {:?}", want);
        }
        let keys: Vec<&str> = EDIT_MENU_ITEMS.iter().map(|i| i.key).collect();
        for want in ["x", "c", "v", "a"] {
            assert!(keys.contains(&want), "Edit 菜单缺少快捷键 ⌘{want}");
        }
    }

    /// 最窄内容宽下底部按钮行仍不重叠(`layout_budget_matches_placement`
    /// 里的 1400 是大窗,这里专门盯默认窄窗;2026-10-13 六键位(移除「⛶ 最大化」)。
    #[test]
    fn bottom_row_fits_narrow_window() {
        let geo = Geometry { msg_h: 90.0 };
        let h = content_height(geo, 3);
        let f = layout_frames(W, h, geo, 3);
        assert_frames_in_bounds(W, h, geo, 3);
        assert!(f.submit.origin.x + f.submit.size.width <= f.extend.origin.x + 0.01);
        assert!(f.extend.origin.x + f.extend.size.width <= f.guard.origin.x + 0.01);
        assert!(f.guard.origin.x + f.guard.size.width <= f.intrusive.origin.x + 0.01);
        assert!(f.intrusive.origin.x + f.intrusive.size.width <= f.copy.origin.x + 0.01);
    }

    /// 2026-10-13(二):侵入式触控板矩形 = 说明区 + 候选选项区(与布局同源派生,
    /// 不回读窗口/控件 frame)。
    #[test]
    fn pad_frame_covers_msg_and_opts() {
        let geo = Geometry { msg_h: 90.0 };
        let h = content_height(geo, 3);
        let f = layout_frames(W, h, geo, 3);
        let pf = pad_frame(&f, 3);
        assert_eq!(pf.origin.x, f.msg.origin.x);
        assert_eq!(pf.size.width, f.msg.size.width);
        // AppKit y 轴自底向上:触控板顶 = msg 顶,底 = 最后一个选项底
        assert!(
            (pf.origin.y + pf.size.height - (f.msg.origin.y + f.msg.size.height)).abs() < 0.01,
            "触控板顶缘应等于 msg 顶缘:{pf:?} vs {:?}",
            f.msg
        );
        let last = f.opts[2];
        assert!(
            (pf.origin.y - last.origin.y).abs() < 0.01,
            "触控板底缘应等于最后一个选项底缘:{pf:?} vs {last:?}"
        );
        // 无选项时 = 说明区
        let f0 = layout_frames(W, h, geo, 0);
        let pf0 = pad_frame(&f0, 0);
        assert_eq!(pf0.origin.y, f0.msg.origin.y);
        assert_eq!(pf0.size.height, f0.msg.size.height);
    }

    /// 2026-10-13(二):侵入式按钮标题对(进入/退出)与 dialog_main 常量同源。
    #[test]
    fn intrusive_button_titles_are_paired() {
        use super::super::dialog_main::{
            GUARD_BTN_UNLOCK_TITLE, INTRUSIVE_BTN_EXIT_TITLE, INTRUSIVE_BTN_TITLE,
        };
        assert_eq!(INTRUSIVE_BTN_TITLE, "🎮 侵入式");
        assert_eq!(INTRUSIVE_BTN_EXIT_TITLE, "⏹ 退出侵入式");
        assert_ne!(INTRUSIVE_BTN_TITLE, INTRUSIVE_BTN_EXIT_TITLE);
        // (三)「非侵入式」入口 = 既有解除屏蔽按钮,文案保持原文案不变
        assert_eq!(GUARD_BTN_UNLOCK_TITLE, "🔓 解除屏蔽");
    }
}
