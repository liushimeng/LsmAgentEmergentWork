//! macOS 窗口驱动:Accessibility 无障碍 API(AXUIElementRef)。
//!
//! - 窗口枚举:`CGWindowListCopyWindowInfo`(CoreGraphics)取 layer-0 在屏窗口,
//!   窗口 id 采用 `"{pid}:{ax_index}"`(AX 以应用为单位,需经
//!   `AXUIElementCreateApplication(pid)` 取 `kAXWindowsAttribute` 数组下标);
//! - 控件遍历:`AXUIElementCopyAttributeValue`(Children/Role/Title/Value/
//!   Position/Size),深度 + 名称/角色过滤双裁剪;
//! - 操作:`AXUIElementPerformAction(kAXPressAction)` 点击,
//!   `AXUIElementSetAttributeValue(kAXValueAttribute)` 写文本,
//!   `kAXFocusedAttribute` 聚焦;
//! - 权限:`AXIsProcessTrustedWithOptions`,默认不弹系统授权窗
//!   (`LAEW_AX_PROMPT=1` 时才带 `kAXTrustedCheckOptionPrompt=true`),
//!   未授权返回可读引导文案(fail-closed)。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.3 macOS 后端。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::OnceLock;

use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::{CFIndex, CFRelease, CFTypeRef, TCFType};
use core_foundation::boolean::CFBooleanRef;
use core_foundation::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};
use core_foundation::string::{
    kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString, CFStringGetCStringPtr,
    CFStringGetLength, CFStringRef,
};

use super::{
    matches_filter, platform_err, ControlAction, ControlNode, Rect, WindowDriver, WindowInfo,
};
use crate::error::Result;

// ===================== FFI 声明 =====================

type AXUIElementRef = CFTypeRef;
type AXError = i32;
type Boolean = u8;

const K_AX_ERROR_SUCCESS: AXError = 0;
const K_AX_ERROR_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
const K_AX_ERROR_ACTION_UNSUPPORTED: AXError = -25206;
const K_AX_ERROR_API_DISABLED: AXError = -25211;
const K_AX_ERROR_INVALID_UI_ELEMENT: AXError = -25212;
const K_AX_ERROR_CANNOT_COMPLETE: AXError = -25204;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

type AXValueType = u32;
const K_AX_VALUE_CGPOINT_TYPE: AXValueType = 1;
const K_AX_VALUE_CGSIZE_TYPE: AXValueType = 2;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXValueGetValue(
        value: CFTypeRef,
        the_type: AXValueType,
        value_ptr: *mut std::ffi::c_void,
    ) -> Boolean;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    // 2026-09-16 第 66 轮:CGEvent 输入事件注入(滚轮滚动 / 按键 / 光标移动)。
    // 注意:CGEventCreateScrollWheelEvent 是 C 可变参数函数(wheel1..N),
    // 必须按 `...` 声明以满足 Apple aarch64 可变参数 ABI(变参走栈,不走寄存器)。
    fn CGEventCreateScrollWheelEvent(
        source: *const std::ffi::c_void,
        units: u32,
        wheel_count: u32,
        ...
    ) -> CFTypeRef;
    fn CGEventCreateMouseEvent(
        source: *const std::ffi::c_void,
        mouse_type: u32,
        position: CGPoint,
        button: u32,
    ) -> CFTypeRef;
    fn CGEventCreateKeyboardEvent(
        source: *const std::ffi::c_void,
        keycode: u16,
        keydown: bool,
    ) -> CFTypeRef;
    fn CGEventPost(tap: u32, event: CFTypeRef);
    // 2026-09-17 第 70 轮:坐标动作(视觉路线)所需 —— 点击计数(双击)与 Unicode 键入。
    fn CGEventSetIntegerValueField(event: CFTypeRef, field: u32, value: i64);
    fn CGEventKeyboardSetUnicodeString(event: CFTypeRef, length: isize, string: *const u16);
}

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
/// kCGScrollEventUnitLine:滚轮事件按「行」计量。
const K_CG_SCROLL_EVENT_UNIT_LINE: u32 = 0;
/// kCGHIDEventTap:事件注入到 HID 层(等价真实硬件输入,目标为光标下/焦点应用)。
const K_CG_HID_EVENT_TAP: u32 = 0;
/// kCGEventMouseMoved。
const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
// 2026-09-17 第 70 轮:kCGEventLeftMouseDown/Up=1/2,RightMouseDown/Up=3/4(来源 HIToolbox/Events.h)。
const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
/// kCGMouseEventClickState 字段号:同一位置的点击计数(双击第 2 次点击置 2)。
const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
/// 鼠标按键编号(左 0 / 右 1)。
const K_CG_MOUSE_BUTTON_LEFT: u32 = 0;
const K_CG_MOUSE_BUTTON_RIGHT: u32 = 1;

// ===================== AX 字符串常量(字面量缓存,macOS 全版本通用) =====================
//
// 2026-09-16 第 55 轮修正 —— 推翻此前「macOS 26 移除了 AX C API」的错误结论。
// 旧实现用 `dlsym("kAXChildrenAttribute")` 运行时取常量,实测在 macOS 26.5 返回 NULL,
// 于是 `ax_strings_loaded()` 恒为 false → `require_trusted()` 无条件 fail-closed,
// WindowInspect / WindowAction 在 macOS 上**整体不可用**(只有走 CoreGraphics 的
// WindowList 幸免);该错误结论还被写进系统提示词与 orchestrator 的 fallback 提示,
// 反过来把 LLM 主动引离本来可用的工具。
//
// 探针实测(C 程序 clang 链接 ApplicationServices;对照 CommandLineTools SDK 头文件
// `HIServices.framework/Headers/AXAttributeConstants.h`)证明:
//   1. `kAX*Attribute` / `kAX*Action` 在现代 SDK 里是**编译期字面量**
//      (`#define kAXChildrenAttribute CFSTR("AXChildren")`),从来不是导出符号 ——
//      dlsym 返回 NULL 与 macOS 版本无关,任何版本都取不到;
//   2. AX 运行时按**字符串值**比较属性名,用字面量 CFString 调用与 SDK 常量完全等价:
//      探针以 `CFSTR("AXWindows")` 调 Finder 返回 -25211(kAXErrorAPIDisabled = 未授权),
//      而不是 -25205(属性不支持),说明字面量被 API 正常接受;
//   3. 旧实现即便 dlsym 成功也是错的:`const CFStringRef kAXX` 是**全局变量**,
//      dlsym 给的是变量地址,必须再解引用一次才是 CFStringRef;直接 cast 会传野指针,
//      外在表现恰好就是「所有 AX 调用返回 ATTRIBUTE_UNSUPPORTED」——当时的误诊来源。
//
// 处理:改为进程内一次性创建的**字面量 CFString 缓存**(常驻不释放,数量固定),
// 不再依赖 dlopen/dlsym;macOS 13~26 行为一致,Windows UIA 主路径不受影响。
// 字段仍用 `usize` 存指针数值,保证 `AxStrings: Send + Sync` 才能装进 `OnceLock` 当 static;
// helper 在访问时 cast 回 `CFStringRef`。
// 字面量取值来源:SDK `AXAttributeConstants.h` / `AXActionConstants.h` / `AXUIElement.h`。

struct AxStrings {
    children: usize,
    role: usize,
    title: usize,
    value: usize,
    description: usize,
    position: usize,
    size: usize,
    windows: usize,
    focused: usize,
    press_action: usize,
    scroll_to_visible_action: usize,
    manual_accessibility: usize,
}

static AX_STRINGS: OnceLock<AxStrings> = OnceLock::new();

/// 由字面量创建常驻 CFString(进程级常量,数量固定,故意不 CFRelease)。
unsafe fn cfstr_literal(lit: &str) -> usize {
    cfstring_new(lit) as usize
}

fn ax_strings() -> &'static AxStrings {
    AX_STRINGS.get_or_init(|| unsafe {
        AxStrings {
            children: cfstr_literal("AXChildren"),
            role: cfstr_literal("AXRole"),
            title: cfstr_literal("AXTitle"),
            value: cfstr_literal("AXValue"),
            description: cfstr_literal("AXDescription"),
            position: cfstr_literal("AXPosition"),
            size: cfstr_literal("AXSize"),
            windows: cfstr_literal("AXWindows"),
            focused: cfstr_literal("AXFocused"),
            press_action: cfstr_literal("AXPress"),
            scroll_to_visible_action: cfstr_literal("AXScrollToVisible"),
            manual_accessibility: cfstr_literal("AXManualAccessibility"),
        }
    })
}

/// AX 常量是否就绪。字面量创建不依赖系统符号,正常恒为 true;
/// 保留该判定仅作为 OOM 等极端情况下的防御(此时 fail-closed 比传空指针安全)。
fn ax_strings_loaded() -> bool {
    let ax = ax_strings();
    ax.children != 0
        && ax.role != 0
        && ax.title != 0
        && ax.value != 0
        && ax.position != 0
        && ax.size != 0
        && ax.windows != 0
        && ax.focused != 0
        && ax.press_action != 0
}

// 让外部调用点保持 `kAX<X>Attribute` / `kAXPressAction` 的可读命名,提供等价 inline getter。
// 语义上等价于 SDK 的 `CFSTR("...")` 常量,但实际数据走 `ax_strings()` 缓存。
#[inline]
fn kAXChildrenAttribute() -> CFStringRef {
    ax_strings().children as CFStringRef
}
#[inline]
fn kAXRoleAttribute() -> CFStringRef {
    ax_strings().role as CFStringRef
}
#[inline]
fn kAXTitleAttribute() -> CFStringRef {
    ax_strings().title as CFStringRef
}
#[inline]
fn kAXValueAttribute() -> CFStringRef {
    ax_strings().value as CFStringRef
}
#[inline]
fn kAXDescriptionAttribute() -> CFStringRef {
    ax_strings().description as CFStringRef
}
#[inline]
fn kAXPositionAttribute() -> CFStringRef {
    ax_strings().position as CFStringRef
}
#[inline]
fn kAXSizeAttribute() -> CFStringRef {
    ax_strings().size as CFStringRef
}
#[inline]
fn kAXWindowsAttribute() -> CFStringRef {
    ax_strings().windows as CFStringRef
}
#[inline]
fn kAXFocusedAttribute() -> CFStringRef {
    ax_strings().focused as CFStringRef
}
#[inline]
fn kAXPressAction() -> CFStringRef {
    ax_strings().press_action as CFStringRef
}
#[inline]
fn kAXScrollToVisibleAction() -> CFStringRef {
    ax_strings().scroll_to_visible_action as CFStringRef
}
#[inline]
fn kAXManualAccessibilityAttribute() -> CFStringRef {
    ax_strings().manual_accessibility as CFStringRef
}

/// 「辅助功能」未授权时的统一文案(供 require_trusted / permission_hint 共用)。
///
/// 2026-09-16 第 55 轮修正:AX C API 在 macOS 13~26 **全版本可用**(按字面量 CFString
/// 调用,与系统版本无关),唯一前置条件是 TCC 辅助功能授权;「AX 不可用」只在**未授权**
/// 这一种情况下成立(kAXErrorAPIDisabled = -25211)。因此文案以「怎么授权」为主,
/// osascript/cliclick 模板仅作为用户不便授权时的降级路径。
/// 文案会作为 tool_result 回填给 LLM,故控制在 ~500 字符内避免重复调用撑爆上下文。
const MACOS_AX_UNAVAILABLE_HINT: &str =
    "辅助功能未授权,WindowInspect/WindowAction 此时不可用(AX 返回 -25211 \
     kAXErrorAPIDisabled)。AX C API 本身在 macOS 13~26 全版本可用,只差授权:\
     系统设置 → 隐私与安全性 → 辅助功能 → 勾选运行 laew 的宿主终端(Terminal/iTerm/VS Code),\
     然后完全退出并重开该终端(TCC 按进程启动时快照生效);设 LAEW_AX_PROMPT=1 可主动弹授权框。\
     授权前可降级走 Bash + osascript(WindowUse 已扩白名单):\
     activate 应用 / System Events keystroke 输入 / pbcopy·pbpaste 剪贴板 / \
     cliclick c:x,y 坐标点击 / screencapture -x 截图。\
     WindowList 走 CoreGraphics 不需授权,任何情况下都能枚举窗口标题·PID·位置。";

/// CFStringRef → String(UTF-8;先走快路径指针,失败回退拷贝缓冲区)。
unsafe fn cfstr(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let fast = CFStringGetCStringPtr(s, kCFStringEncodingUTF8);
    if !fast.is_null() {
        return CStr::from_ptr(fast).to_string_lossy().into_owned();
    }
    let len = CFStringGetLength(s);
    let cap = (len as usize).saturating_mul(4).saturating_add(1);
    let mut buf = vec![0u8; cap];
    let ok = CFStringGetCString(
        s,
        buf.as_mut_ptr().cast(),
        cap as CFIndex,
        kCFStringEncodingUTF8,
    );
    if ok != 0 {
        CStr::from_ptr(buf.as_ptr().cast())
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    }
}

/// 创建 CFString(返回拥有所有权的 ref,调用方负责 CFRelease)。
unsafe fn cfstring_new(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).unwrap_or_else(|_| std::ffi::CString::new("").unwrap());
    CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8)
}

// ===================== CGEvent 输入注入(2026-09-16 第 66 轮) =====================
//
// 背景:微信「遍历通信录列表找联系人 → 打开会话 → Enter 发送」链路需要两个新原语:
// - 滚轮滚动(控件树只能看到当前屏,列表其余条目必须滚动后才进 AX 树);
// - 按键注入(Enter 发送消息 / PageDown 翻页),此前 macOS 后端 SendKeys 直接报错。
// 实现:CGEventPost 到 HID 层,等价真实硬件输入;需要辅助功能授权(act() 入口已 gate)。

/// 命名键 → macOS 虚拟键码(kVK_*,来源 HIToolbox/Events.h)。
fn keycode_for_name(name: &str) -> Option<u16> {
    Some(match name.trim().to_lowercase().as_str() {
        "enter" | "return" | "回车" => 36,
        "tab" => 48,
        "esc" | "escape" => 53,
        "space" | "空格" => 49,
        "delete" | "backspace" | "退格" => 51,
        "forwarddelete" | "del" => 117,
        "up" | "arrowup" => 126,
        "down" | "arrowdown" => 125,
        "left" | "arrowleft" => 123,
        "right" | "arrowright" => 124,
        "pageup" => 116,
        "pagedown" => 121,
        "home" => 115,
        "end" => 119,
        _ => return None,
    })
}

/// 把光标移到屏幕坐标(x, y)(CGEvent mouse-moved,无点击)。
unsafe fn cg_move_cursor(x: f64, y: f64) {
    let ev = CGEventCreateMouseEvent(
        std::ptr::null(),
        K_CG_EVENT_MOUSE_MOVED,
        CGPoint { x, y },
        0,
    );
    if !ev.is_null() {
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
    }
}

/// 在光标当前位置注入滚轮事件:lines>0 向上,lines<0 向下(单位:行)。
/// 分片投递(每片 ≤3 行 + 30ms 间隔),避免一次性大 delta 被应用按「甩尾」处理跳屏。
unsafe fn cg_scroll_lines(lines: i32) {
    let mut remaining = lines;
    while remaining != 0 {
        let step = remaining.clamp(-3, 3);
        let ev = CGEventCreateScrollWheelEvent(
            std::ptr::null(),
            K_CG_SCROLL_EVENT_UNIT_LINE,
            1,
            step,
        );
        if ev.is_null() {
            break;
        }
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
        remaining -= step;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

/// 注入一次按键(keydown + keyup,间隔 20ms)。
unsafe fn cg_send_key(keycode: u16) {
    for keydown in [true, false] {
        let ev = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, keydown);
        if ev.is_null() {
            return;
        }
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
        if keydown {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// 在屏幕坐标 (x,y) 注入物理鼠标点击(2026-09-17 第 70 轮,坐标动作视觉路线)。
/// 先移光标到位再按压;双击时第二次点击 clickState=2(应用按该字段判定双击语义)。
unsafe fn cg_click_at(x: f64, y: f64, right: bool, double: bool) {
    let (down, up, button) = if right {
        (
            K_CG_EVENT_RIGHT_MOUSE_DOWN,
            K_CG_EVENT_RIGHT_MOUSE_UP,
            K_CG_MOUSE_BUTTON_RIGHT,
        )
    } else {
        (
            K_CG_EVENT_LEFT_MOUSE_DOWN,
            K_CG_EVENT_LEFT_MOUSE_UP,
            K_CG_MOUSE_BUTTON_LEFT,
        )
    };
    cg_move_cursor(x, y);
    std::thread::sleep(std::time::Duration::from_millis(60));
    let clicks = if double { 2 } else { 1 };
    for seq in 1..=clicks {
        for &mouse_type in &[down, up] {
            let ev = CGEventCreateMouseEvent(
                std::ptr::null(),
                mouse_type,
                CGPoint { x, y },
                button,
            );
            if ev.is_null() {
                return;
            }
            if double {
                CGEventSetIntegerValueField(ev, K_CG_MOUSE_EVENT_CLICK_STATE, seq as i64);
            }
            CGEventPost(K_CG_HID_EVENT_TAP, ev);
            CFRelease(ev);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if double {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
}

/// 向当前焦点控件真实键入文本(2026-09-17 第 70 轮)。
/// CGEventKeyboardSetUnicodeString 按 UTF-16 直达,无键盘布局/IME 依赖;
/// 单个事件携带 ≤20 个 UTF-16 单元,分批注入(SendInput Unicode 的 macOS 对偶)。
unsafe fn cg_type_text(text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(20) {
        for &keydown in &[true, false] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), 0, keydown);
            if ev.is_null() {
                return;
            }
            CGEventKeyboardSetUnicodeString(ev, chunk.len() as isize, chunk.as_ptr());
            CGEventPost(K_CG_HID_EVENT_TAP, ev);
            CFRelease(ev);
            if keydown {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// CFNumberRef → i64。
unsafe fn cfnum_i64(n: CFTypeRef) -> i64 {
    let mut out: i64 = 0;
    if !n.is_null() {
        CFNumberGetValue(
            n as CFNumberRef,
            kCFNumberSInt64Type,
            (&mut out as *mut i64).cast(),
        );
    }
    out
}

/// CFDictionary 取键(键为 CFStr 字面量创建)。
unsafe fn dict_get(dict: CFDictionaryRef, key: &str) -> CFTypeRef {
    if dict.is_null() {
        return std::ptr::null();
    }
    let k = cfstring_new(key);
    let v = CFDictionaryGetValue(dict, k.cast());
    CFRelease(k.cast());
    v
}

/// AX 属性读取(返回拥有所有权的 CFTypeRef;失败为 null)。
unsafe fn ax_get(el: AXUIElementRef, attr: CFStringRef) -> CFTypeRef {
    let mut out: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(el, attr, &mut out);
    if err != K_AX_ERROR_SUCCESS {
        return std::ptr::null();
    }
    out
}

/// AX 字符串属性。
unsafe fn ax_get_string(el: AXUIElementRef, attr: CFStringRef) -> String {
    let v = ax_get(el, attr);
    if v.is_null() {
        return String::new();
    }
    let s = cfstr(v as CFStringRef);
    CFRelease(v);
    s
}

/// AX 错误码 → 可读文案。
fn ax_error_text(code: AXError) -> String {
    match code {
        K_AX_ERROR_API_DISABLED => {
            "无障碍权限未授予(kAXErrorAPIDisabled)。请到 系统设置 → 隐私与安全性 → 辅助功能 \
             中勾选本终端应用(laew 所在终端,如 Terminal/iTerm),然后重试"
                .to_string()
        }
        K_AX_ERROR_INVALID_UI_ELEMENT => {
            "控件已失效(窗口可能已关闭或 UI 已刷新),请重新 WindowInspect".into()
        }
        K_AX_ERROR_ATTRIBUTE_UNSUPPORTED => "该控件不支持此属性(应用未实现对应无障碍属性)".into(),
        K_AX_ERROR_ACTION_UNSUPPORTED => "该控件不支持此动作(应用未实现对应无障碍动作)".into(),
        K_AX_ERROR_CANNOT_COMPLETE => "AX 调用无法完成(目标应用无响应或 IPC 失败)".into(),
        other => format!("AX 错误码 {other}"),
    }
}

// ===================== 驱动实现 =====================

pub struct MacOsDriver;

impl Default for MacOsDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsDriver {
    pub fn new() -> Self {
        Self
    }

    /// 是否已获无障碍授权(不弹窗)。
    fn trusted_quiet() -> bool {
        unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) != 0 }
    }

    /// 是否已获无障碍授权。`LAEW_AX_PROMPT=1` 时允许触发系统授权弹窗。
    fn trusted() -> bool {
        let prompt = std::env::var("LAEW_AX_PROMPT")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        unsafe {
            if prompt {
                Self::request_permission_internal()
            } else {
                Self::trusted_quiet()
            }
        }
    }

    /// 2026-09-16 第 60 轮:主动触发系统授权引导,返回用户是否已授权。
    ///
    /// 2026-09-17 第 70 轮改版:不再调用带 `{kAXTrustedCheckOptionPrompt: true}` 的
    /// `AXIsProcessTrustedWithOptions` —— macOS 26.5 未授权进程走该路径会在
    /// HIServices 内部 SIGSEGV 连带崩掉主进程(C 探针实测,详见函数体注释)。
    /// 现行为:先静默探测;未授权则进程外 `open` 系统设置的辅助功能面板引导用户。
    ///
    /// 返回值:
    /// - `true`:已授权
    /// - `false`:未授权(已拉起系统设置面板,等待用户操作;轮询方静默感知授权变化)
    unsafe fn request_permission_internal() -> bool {
        // 2026-09-17 第 70 轮:先静默探测(NULL options,macOS 26.5 实测安全);
        // 已授权直接返回,不进入弹窗路径。
        if Self::trusted_quiet() {
            return true;
        }
        // macOS 26.5(25F71)实测(C 探针 /tmp/ax_probe.c 复现):未授权进程调用带
        // `{kAXTrustedCheckOptionPrompt: true}` 字典的 AXIsProcessTrustedWithOptions
        // 会在 HIServices 内部 SIGSEGV(CFGetTypeID 野指针),**整个进程连带崩溃** ——
        // 主进程绝不能冒这个险。改为进程外打开系统设置的辅助功能面板:
        // `open` 命令无崩溃风险,用户操作路径等价(勾选终端 → 授权生效),
        // 授权状态变化由 is_ax_trusted_with_retry 的静默轮询感知。
        // 非 GUI 会话(SSH/CI)下 open 可能失败,忽略错误静默降级。
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
        false
    }

    /// 公开接口:主动请求辅助功能授权(触发系统弹窗)。
    ///
    /// 2026-09-16 第 60 轮:供 `driver_preflight` 在检测到未授权时调用,
    /// 让 LLM 在首次使用 WindowInspect/WindowAction 时自动触发授权流程。
    pub fn request_permission() -> bool {
        unsafe { Self::request_permission_internal() }
    }

    /// 公开接口:静默检查辅助功能授权状态(不弹窗)。
    ///
    /// 2026-09-16 第 60 轮:供 `try_request_ax_permission` 在弹窗前检查,
    /// 避免重复弹窗干扰用户。
    pub fn is_trusted() -> bool {
        Self::trusted_quiet()
    }

    /// 2026-09-16 第 62 轮:持续等待 macOS 辅助功能授权,带进度回调。
    ///
    /// 行为:
    /// - `prompt=true` 时每 10s 主动触发系统授权弹窗(避免用户错过);
    /// - 每 2s 静默轮询授权状态(不弹窗);
    /// - 默认上限 120s(`LAEW_AX_WAIT_SECS` 环境变量可调,0 = 立即返回不等待);
    /// - 授权成功后立即返回 `(true, waited_secs)`;
    /// - 超时返回 `(false, waited_secs)`;
    /// - `on_progress(secs, granted)` 回调用于上报到 TUI(可空)。
    ///
    /// 设计要点:
    /// - 阻塞在 `inspect/act` 调用上时,TUI 阶段打印协程持续刷 spinner;
    /// - 系统授权弹窗由 TCC 异步派发,本函数轮询时立即看到授权状态变化;
    /// - 等待期间用户需在系统设置手动勾选终端 / 输入密码,函数自然阻塞。
    pub fn is_ax_trusted_with_retry<F>(
        prompt: bool,
        max_wait_secs: u64,
        mut on_progress: F,
    ) -> (bool, u64)
    where
        F: FnMut(u64, bool),
    {
        let wait_secs = std::env::var("LAEW_AX_WAIT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(max_wait_secs);
        if wait_secs == 0 {
            let granted = Self::trusted_quiet();
            on_progress(0, granted);
            return (granted, 0);
        }
        // 首次检查
        if Self::trusted_quiet() {
            on_progress(0, true);
            return (true, 0);
        }
        // 立即触发一次系统授权弹窗,缩短用户响应路径
        if prompt {
            unsafe {
                let _ = Self::request_permission_internal();
            }
        }
        let start = std::time::Instant::now();
        loop {
            let elapsed = start.elapsed().as_secs();
            if elapsed >= wait_secs {
                on_progress(elapsed, false);
                return (false, elapsed);
            }
            // 2026-09-17 第 70 轮:移除旧的「每 10s 重触发弹窗」——弹窗路径已改为
            // 进程外 open 系统设置面板(见 request_permission_internal),周期性重触发
            // 会反复拉起系统设置干扰用户;入口处的首次触发已足够。
            // 每 2 秒静默轮询 + 进度回报
            let poll_deadline = start + std::time::Duration::from_secs(elapsed + 2);
            let now = std::time::Instant::now();
            if poll_deadline > now {
                std::thread::sleep(poll_deadline - now);
            }
            if Self::trusted_quiet() {
                let waited = start.elapsed().as_secs();
                on_progress(waited, true);
                return (true, waited);
            }
            on_progress(start.elapsed().as_secs(), false);
        }
    }

    fn require_trusted(&self) -> Result<()> {
        // AX 常量走字面量缓存,与系统版本无关;仅在极端情况(内存不足导致 CFString
        // 创建失败)下 fail-closed,避免把空指针传进 AX 调用。
        if !ax_strings_loaded() {
            return Err(platform_err(
                "macos",
                "AX 属性名常量初始化失败(CFString 创建返回空,通常为内存不足),请重试",
            ));
        }
        if Self::trusted_quiet() {
            Ok(())
        } else {
            Err(platform_err("macos", MACOS_AX_UNAVAILABLE_HINT))
        }
    }

    /// AX 常量是否就绪(不检查权限)。
    ///
    /// 字面量缓存与 macOS 版本无关,正常恒为 true;`list_windows` 用它决定
    /// 是否需要降级到 CoreGraphics 枚举(实际上 macOS 后端窗口枚举恒走 CoreGraphics,
    /// 因为它不需要授权、且能拿到窗口标题/位置)。
    fn ax_available(&self) -> bool {
        ax_strings_loaded()
    }

    /// 解析窗口 id `"{pid}:{index}"`。
    fn parse_window_id(window_id: &str) -> Result<(i32, usize)> {
        let (pid_s, idx_s) = window_id.split_once(':').ok_or_else(|| {
            platform_err(
                "macos",
                format!("window_id 格式应为 \"{{pid}}:{{index}}\"(由 WindowList 返回),实际: {window_id}"),
            )
        })?;
        let pid: i32 = pid_s
            .parse()
            .map_err(|_| platform_err("macos", format!("window_id 中 pid 非法: {pid_s}")))?;
        let idx: usize = idx_s
            .parse()
            .map_err(|_| platform_err("macos", format!("window_id 中 index 非法: {idx_s}")))?;
        Ok((pid, idx))
    }

    /// 取应用的 AX 窗口数组中第 index 个窗口元素(调用方负责 CFRelease)。
    unsafe fn window_element(pid: i32, index: usize) -> Result<AXUIElementRef> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(platform_err(
                "macos",
                format!("无法为 pid={pid} 创建 AX 应用元素"),
            ));
        }
        // Electron / 自绘应用(含部分微信版本)常支持 AXManualAccessibility 开关。
        // 打开失败不影响后续窗口读取,因此忽略错误。
        let true_v: CFBooleanRef =
            core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
        let _ = AXUIElementSetAttributeValue(app, kAXManualAccessibilityAttribute(), true_v.cast());
        let wins = ax_get(app, kAXWindowsAttribute());
        CFRelease(app);
        if wins.is_null() {
            return Err(platform_err(
                "macos",
                format!("pid={pid} 的应用未暴露 AX 窗口列表(应用可能已退出或未实现无障碍)"),
            ));
        }
        let count = CFArrayGetCount(wins as CFArrayRef);
        let el = if (index as CFIndex) < count {
            let e = CFArrayGetValueAtIndex(wins as CFArrayRef, index as CFIndex) as AXUIElementRef;
            if !e.is_null() {
                core_foundation::base::CFRetain(e);
            }
            e
        } else {
            std::ptr::null()
        };
        CFRelease(wins);
        if el.is_null() {
            return Err(platform_err(
                "macos",
                format!("pid={pid} 第 {index} 个窗口不存在(窗口可能已关闭),请重新 WindowList"),
            ));
        }
        Ok(el)
    }

    /// 读取元素的几何信息(AXPosition + AXSize)。
    unsafe fn element_rect(el: AXUIElementRef) -> Rect {
        let mut rect = Rect::default();
        let pos = ax_get(el, kAXPositionAttribute());
        if !pos.is_null() {
            let mut p = CGPoint::default();
            if AXValueGetValue(
                pos,
                K_AX_VALUE_CGPOINT_TYPE,
                (&mut p as *mut CGPoint).cast(),
            ) != 0
            {
                rect.x = p.x as i64;
                rect.y = p.y as i64;
            }
            CFRelease(pos);
        }
        let size = ax_get(el, kAXSizeAttribute());
        if !size.is_null() {
            let mut s = CGSize::default();
            if AXValueGetValue(size, K_AX_VALUE_CGSIZE_TYPE, (&mut s as *mut CGSize).cast()) != 0 {
                rect.width = s.width as i64;
                rect.height = s.height as i64;
            }
            CFRelease(size);
        }
        rect
    }

    /// 按角色推断支持动作(LLM 检视后据此选择合法操作)。
    fn actions_for_role(role: &str) -> Vec<String> {
        let r = role.to_lowercase();
        let mut v = vec!["focus".to_string(), "get_text".to_string()];
        if r.contains("button")
            || r.contains("menuitem")
            || r.contains("checkbox")
            || r.contains("radio")
            || r.contains("link")
            || r.contains("tab")
        {
            v.push("click".into());
            v.push("invoke".into());
        }
        if r.contains("textfield")
            || r.contains("textarea")
            || r.contains("combobox")
            || r.contains("searchfield")
            || r.contains("securetextfield")
        {
            v.push("set_text".into());
        }
        // 2026-09-16 第 66 轮:滚动容器/列表类角色补 scroll / scroll_to_visible 提示
        if r.contains("scrollarea")
            || r.contains("scroll")
            || r.contains("table")
            || r.contains("outline")
            || r.contains("list")
            || r.contains("row")
            || r.contains("browser")
        {
            v.push("scroll".into());
            v.push("scroll_to_visible".into());
        }
        // 任何可聚焦控件都可能接受按键(Enter 发送 / 方向键导航)
        v.push("send_keys".into());
        v
    }

    /// 递归构建控件树(深度 + 过滤双裁剪;命中节点的祖先链保留)。
    unsafe fn build_tree(
        el: AXUIElementRef,
        path: String,
        depth: usize,
        max_depth: usize,
        filter: Option<&str>,
    ) -> Option<ControlNode> {
        let role = ax_get_string(el, kAXRoleAttribute());
        // 名称优先级:AXTitle > AXValue > AXDescription(微信/QQ 等应用按钮常用 AXDescription 承载可
        // 读文案,AXTitle 反为空;补上这一级可显著提升控件可辨识度,便于 LLM 在控件树中定位目标)。
        let description = ax_get_string(el, kAXDescriptionAttribute());
        let name = {
            let t = ax_get_string(el, kAXTitleAttribute());
            if t.is_empty() {
                let v = ax_get_string(el, kAXValueAttribute());
                if v.is_empty() {
                    description.clone()
                } else {
                    v
                }
            } else {
                t
            }
        };
        let value = ax_get_string(el, kAXValueAttribute());
        let bounds = Self::element_rect(el);

        let mut node = ControlNode {
            path: path.clone(),
            role,
            name,
            value,
            bounds,
            actions: Vec::new(),
            children: Vec::new(),
        };
        node.actions = Self::actions_for_role(&node.role);

        if depth < max_depth {
            let children = ax_get(el, kAXChildrenAttribute());
            if !children.is_null() {
                let count = CFArrayGetCount(children as CFArrayRef);
                for i in 0..count {
                    let child = CFArrayGetValueAtIndex(children as CFArrayRef, i) as AXUIElementRef;
                    if child.is_null() {
                        continue;
                    }
                    if let Some(cn) = Self::build_tree(
                        child,
                        format!("{}/{}", path.trim_end_matches('/'), i),
                        depth + 1,
                        max_depth,
                        filter,
                    ) {
                        node.children.push(cn);
                    }
                }
                CFRelease(children);
            }
        }

        // 过滤:自身命中 或 任一子孙命中(祖先链保留) 或 无过滤条件
        let self_hit = matches_filter(&node.name, filter) || matches_filter(&node.role, filter);
        if filter.is_none() || self_hit || !node.children.is_empty() {
            Some(node)
        } else {
            None
        }
    }

    /// 按路径定位元素(返回 retained ref,调用方负责 CFRelease)。
    unsafe fn element_at_path(root: AXUIElementRef, path: &str) -> Result<AXUIElementRef> {
        let trimmed = path.trim();
        if trimmed == "/" || trimmed.is_empty() {
            core_foundation::base::CFRetain(root);
            return Ok(root);
        }
        let mut cur = root;
        core_foundation::base::CFRetain(cur);
        for seg in trimmed.trim_start_matches('/').split('/') {
            let idx: usize = seg.parse().map_err(|_| {
                platform_err("macos", format!("控件路径段非法: {seg}(应为子控件下标)"))
            })?;
            let children = ax_get(cur, kAXChildrenAttribute());
            CFRelease(cur);
            if children.is_null() {
                return Err(platform_err(
                    "macos",
                    format!("路径 {path} 在段 {seg} 处中断:父控件无子节点,请重新 WindowInspect 获取最新路径"),
                ));
            }
            let count = CFArrayGetCount(children as CFArrayRef);
            let next = if (idx as CFIndex) < count {
                let e = CFArrayGetValueAtIndex(children as CFArrayRef, idx as CFIndex)
                    as AXUIElementRef;
                if !e.is_null() {
                    core_foundation::base::CFRetain(e);
                }
                e
            } else {
                std::ptr::null()
            };
            CFRelease(children);
            if next.is_null() {
                return Err(platform_err(
                    "macos",
                    format!("路径 {path} 下标 {idx} 越界(UI 可能已变化),请重新 WindowInspect"),
                ));
            }
            cur = next;
        }
        Ok(cur)
    }
}

/// 2026-09-16 第 58 轮 P2:抽离 list_windows CG 实现为 pub fn,
/// 供 macos_axui.rs(axuielement 版)复用,以保证 list_windows 在两版下输出字段一致。
pub fn list_windows_cg(filter: Option<&str>) -> Result<Vec<WindowInfo>> {
    unsafe {
        let list = CGWindowListCopyWindowInfo(K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY, 0);
        if list.is_null() {
            return Err(platform_err(
                "macos",
                "CGWindowListCopyWindowInfo 返回空(无窗口服务器连接?)",
            ));
        }
        let count = CFArrayGetCount(list);
        // 每个 pid 已见到的窗口计数 → ax_index
        let mut seen: HashMap<i64, usize> = HashMap::new();
        let mut out = Vec::new();
        for i in 0..count {
            let dict = CFArrayGetValueAtIndex(list, i) as CFDictionaryRef;
            if dict.is_null() {
                continue;
            }
            let layer = cfnum_i64(dict_get(dict, "kCGWindowLayer"));
            if layer != 0 {
                continue; // 只要常规应用窗口
            }
            let pid = cfnum_i64(dict_get(dict, "kCGWindowOwnerPID"));
            let owner = cfstr(dict_get(dict, "kCGWindowOwnerName") as CFStringRef);
            let title = cfstr(dict_get(dict, "kCGWindowName") as CFStringRef);
            if title.is_empty() && owner.is_empty() {
                continue;
            }
            if !matches_filter(&title, filter) && !matches_filter(&owner, filter) {
                continue;
            }
            let mut bounds = Rect::default();
            let bdict = dict_get(dict, "kCGWindowBounds") as CFDictionaryRef;
            if !bdict.is_null() {
                bounds = Rect {
                    x: cfnum_i64(dict_get(bdict, "X")),
                    y: cfnum_i64(dict_get(bdict, "Y")),
                    width: cfnum_i64(dict_get(bdict, "Width")),
                    height: cfnum_i64(dict_get(bdict, "Height")),
                };
            }
            let idx = seen.entry(pid).or_insert(0);
            let id = format!("{pid}:{idx}");
            *idx += 1;
            out.push(WindowInfo {
                id,
                title,
                process_name: owner,
                pid: pid.max(0) as u32,
                bounds,
            });
        }
        CFRelease(list.cast());
        Ok(out)
    }
}

impl WindowDriver for MacOsDriver {
    fn platform_name(&self) -> &'static str {
        "macos"
    }

    fn list_windows(&self, filter: Option<&str>) -> Result<Vec<WindowInfo>> {
        list_windows_cg(filter)
    }

    fn inspect(
        &self,
        window_id: &str,
        max_depth: usize,
        filter: Option<&str>,
    ) -> Result<ControlNode> {
        // 2026-09-16 第 62 轮:未授权时持续等待用户完成系统设置 / 密码输入;
        // 默认上限 120s(LAEW_AX_WAIT_SECS 环境变量可调,0 = 立即失败)。
        let prompt = std::env::var("LAEW_AX_PROMPT")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let (granted, waited) = Self::is_ax_trusted_with_retry(prompt, 120, |elapsed, granted| {
            tracing::debug!(
                elapsed_secs = elapsed,
                granted = granted,
                "WindowInspect 等待 macOS 辅助功能授权"
            );
        });
        if !granted {
            return Err(platform_err(
                "macos",
                format!(
                    "辅助功能未授权;已等待 {waited}s。请到「系统设置→隐私与安全性→辅助功能」勾选当前终端后重试"
                ),
            ));
        }
        let (pid, idx) = Self::parse_window_id(window_id)?;
        let max_depth = max_depth.clamp(1, 12);
        unsafe {
            let win = Self::window_element(pid, idx)?;
            let tree = Self::build_tree(win, "/".to_string(), 1, max_depth, filter);
            CFRelease(win);
            tree.ok_or_else(|| {
                platform_err(
                    "macos",
                    format!(
                        "filter 未命中窗口 {window_id} 内任何控件(已遍历到 max_depth={max_depth})。\
                         【同义词建议】中文 UI 名称常见笔误:通讯录 ↔ 通信录 ↔ 联系人 ↔ Contacts;\
                         消息 ↔ 发送 ↔ Send;输入框 ↔ 搜索 ↔ Search;按钮 ↔ Button;关闭 ↔ X ↔ close。\
                         建议:1) 改用上表同义词重试;2) filter 留空 + max_depth=4-5 看完整树;\
                         3) 用 WindowScreenshot + 视觉识别(若应用无障碍支持极差)"
                    ),
                )
            })
        }
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        // 2026-09-16 第 62 轮:同 inspect,未授权时持续等待。
        let prompt = std::env::var("LAEW_AX_PROMPT")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let (granted, waited) = Self::is_ax_trusted_with_retry(prompt, 120, |elapsed, granted| {
            tracing::debug!(
                elapsed_secs = elapsed,
                granted = granted,
                "WindowAction 等待 macOS 辅助功能授权"
            );
        });
        if !granted {
            return Err(platform_err(
                "macos",
                format!(
                    "辅助功能未授权;已等待 {waited}s。请到「系统设置→隐私与安全性→辅助功能」勾选当前终端后重试"
                ),
            ));
        }
        let (pid, idx) = Self::parse_window_id(window_id)?;
        unsafe {
            let win = Self::window_element(pid, idx)?;
            let el = Self::element_at_path(win, path)?;
            CFRelease(win);
            let result = match &action {
                ControlAction::Click | ControlAction::Invoke => {
                    let err = AXUIElementPerformAction(el, kAXPressAction());
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已对 {window_id}{path} 执行点击(AXPress)"))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                ControlAction::Focus => {
                    let true_v: CFBooleanRef =
                        core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
                    let err = AXUIElementSetAttributeValue(el, kAXFocusedAttribute(), true_v.cast());
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已聚焦 {window_id}{path}"))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                ControlAction::SetText(text) => {
                    let cf = cfstring_new(text);
                    let err = AXUIElementSetAttributeValue(el, kAXValueAttribute(), cf.cast());
                    CFRelease(cf.cast());
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已向 {window_id}{path} 写入文本({} 字符)", text.chars().count()))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                ControlAction::GetText => {
                    let v = ax_get_string(el, kAXValueAttribute());
                    let t = ax_get_string(el, kAXTitleAttribute());
                    Ok(if v.is_empty() { t } else { v })
                }
                // 2026-09-16 第 66 轮:CGEvent 按键注入(此前直接报「暂不支持」,
                // 微信 Enter 发送 / PageDown 翻页链路无原语)。
                ControlAction::SendKeys(keys) => {
                    let key = keys.trim();
                    match keycode_for_name(key) {
                        Some(kc) => {
                            cg_send_key(kc);
                            Ok(format!("已向 {window_id}{path} 注入按键 {key}(keycode={kc})"))
                        }
                        None => Err(platform_err(
                            "macos",
                            format!(
                                "未知按键: {key};支持 enter/tab/esc/space/delete/up/down/left/right/pageup/pagedown/home/end;\
                                 输入文本请用 set_text,或 Bash 白名单 osascript keystroke"
                            ),
                        )),
                    }
                }
                // 2026-09-16 第 66 轮:滚轮滚动。CGEvent 滚轮事件投递到光标下的窗口,
                // 因此先把光标移到目标控件中心再滚动。
                ControlAction::Scroll { lines } => {
                    let rect = Self::element_rect(el);
                    let cx = rect.x as f64 + rect.width as f64 / 2.0;
                    let cy = rect.y as f64 + rect.height as f64 / 2.0;
                    cg_move_cursor(cx, cy);
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    cg_scroll_lines(*lines);
                    Ok(format!(
                        "已在 {window_id}{path} 中心({cx:.0},{cy:.0})滚动 {} 行({})",
                        lines.abs(),
                        if *lines > 0 { "向上" } else { "向下" }
                    ))
                }
                ControlAction::ScrollToVisible => {
                    let err = AXUIElementPerformAction(el, kAXScrollToVisibleAction());
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已把 {window_id}{path} 滚动到可见区域(AXScrollToVisible)"))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                // ===== 2026-09-17 第 70 轮:补齐第 67 轮坐标动作(视觉路线)的 macOS 路径 =====
                // 此前仅 windows.rs(SendInput)实现了 5 个坐标变体,本 match 漏覆盖导致
                // 编译失败(E0004)→ rebuild 脚本中止 → 产物不更新。
                // 坐标动作用屏幕绝对坐标,与 path 控件无关(path 恒传 "/",el 为窗口根)。
                ControlAction::ClickPoint { x, y } => {
                    cg_click_at(*x as f64, *y as f64, false, false);
                    Ok(format!("已在屏幕坐标 ({x},{y}) 执行物理左键单击(CGEvent)"))
                }
                ControlAction::DoubleClickPoint { x, y } => {
                    cg_click_at(*x as f64, *y as f64, false, true);
                    Ok(format!("已在屏幕坐标 ({x},{y}) 执行物理双击(CGEvent,clickState=2)"))
                }
                ControlAction::RightClickPoint { x, y } => {
                    cg_click_at(*x as f64, *y as f64, true, false);
                    Ok(format!("已在屏幕坐标 ({x},{y}) 执行物理右键单击(CGEvent)"))
                }
                ControlAction::ScrollPoint { x, y, lines } => {
                    cg_move_cursor(*x as f64, *y as f64);
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    cg_scroll_lines(*lines);
                    Ok(format!(
                        "已在 ({x},{y}) 滚动 {} 行({})",
                        lines.abs(),
                        if *lines > 0 { "向上" } else { "向下" }
                    ))
                }
                ControlAction::TypeText(text) => {
                    cg_type_text(text);
                    Ok(format!(
                        "已向当前焦点控件真实键入 {} 字符(CGEvent Unicode)",
                        text.chars().count()
                    ))
                }
            };
            CFRelease(el);
            result
        }
    }

    fn permission_hint(&self) -> Option<String> {
        if !ax_strings_loaded() {
            // 字面量创建失败(OOM 等极端情况):按"常量不可用"兜底,语义同未授权
            return Some(MACOS_AX_UNAVAILABLE_HINT.to_string());
        }
        if Self::trusted() {
            None
        } else {
            // 未授权时给完整引导(含授权步骤 + osascript 降级模板),作为 tool_result 回填给 LLM
            Some(MACOS_AX_UNAVAILABLE_HINT.to_string())
        }
    }
}
