//! AX 字面量 CFString 缓存 + kAX*Attribute / kAX*Action inline getter
//! + AX 属性读取辅助函数 + ax_error_text 错误码翻译。
//!
//! 2026-09-20 第 97 轮:从 macos_legacy.rs 拆分出来的 AX 字面量与辅助函数模块。
//!
//! ## AX 常量策略(2026-09-16 第 55 轮修正)
//!
//! 旧实现用 `dlsym("kAXChildrenAttribute")` 运行时取常量,实测在 macOS 26.5 返回 NULL,
//! 于是 `ax_strings_loaded()` 恒为 false → `require_trusted()` 无条件 fail-closed,
//! MCP_Window_Use(action=inspect) / MCP_Window_Use(action=control) 在 macOS 上**整体不可用**(只有走 CoreGraphics 的
//! MCP_Window_Use(action=list) 幸免);该错误结论还被写进系统提示词与 orchestrator 的 fallback 提示,
//! 反过来把 LLM 主动引离本来可用的工具。
//!
//! 探针实测(C 程序 clang 链接 ApplicationServices;对照 CommandLineTools SDK 头文件
//! `HIServices.framework/Headers/AXAttributeConstants.h`)证明:
//!   1. `kAX*Attribute` / `kAX*Action` 在现代 SDK 里是**编译期字面量**
//!      (`#define kAXChildrenAttribute CFSTR("AXChildren")`),从来不是导出符号 ——
//!      dlsym 返回 NULL 与 macOS 版本无关,任何版本都取不到;
//!   2. AX 运行时按**字符串值**比较属性名,用字面量 CFString 调用与 SDK 常量完全等价:
//!      探针以 `CFSTR("AXWindows")` 调 Finder 返回 -25211(kAXErrorAPIDisabled = 未授权),
//!      而不是 -25205(属性不支持),说明字面量被 API 正常接受;
//!   3. 旧实现即便 dlsym 成功也是错的:`const CFStringRef kAXX` 是**全局变量**,
//!      dlsym 给的是变量地址,必须再解引用一次才是 CFStringRef;直接 cast 会传野指针,
//!      外在表现恰好就是「所有 AX 调用返回 ATTRIBUTE_UNSUPPORTED」——当时的误诊来源。
//!
//! 处理:改为进程内一次性创建的**字面量 CFString 缓存**(常驻不释放,数量固定),
//! 不再依赖 dlopen/dlsym;macOS 13~26 行为一致,Windows UIA 主路径不受影响。
//! 字段仍用 `usize` 存指针数值,保证 `AxStrings: Send + Sync` 才能装进 `OnceLock` 当 static;
//! helper 在访问时 cast 回 `CFStringRef`。
//! 字面量取值来源:SDK `AXAttributeConstants.h` / `AXActionConstants.h` / `AXUIElement.h`。

#![allow(non_snake_case)]

use std::ffi::CStr;
use std::sync::OnceLock;

use core_foundation::base::{CFIndex, CFTypeRef};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};
use core_foundation::string::{
    kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString, CFStringGetCStringPtr,
    CFStringGetLength, CFStringRef,
};

// 从父模块 mod.rs 取用类型 / 常量 / FFI 函数。
use super::{
    AXUIElementCopyAttributeValue, CFRelease, K_AX_ERROR_ACTION_UNSUPPORTED, K_AX_ERROR_API_DISABLED,
    K_AX_ERROR_ATTRIBUTE_UNSUPPORTED, K_AX_ERROR_CANNOT_COMPLETE, K_AX_ERROR_INVALID_UI_ELEMENT,
    K_AX_ERROR_SUCCESS, AXError, AXUIElementRef,
};

pub(super) struct AxStrings {
    pub(super) children: usize,
    pub(super) role: usize,
    pub(super) title: usize,
    pub(super) value: usize,
    pub(super) description: usize,
    pub(super) position: usize,
    pub(super) size: usize,
    pub(super) windows: usize,
    pub(super) focused: usize,
    pub(super) frontmost: usize,
    pub(super) press_action: usize,
    pub(super) raise_action: usize,
    pub(super) pick_action: usize,
    pub(super) confirm_action: usize,
    pub(super) open_action: usize,
    pub(super) scroll_to_visible_action: usize,
    pub(super) manual_accessibility: usize,
    pub(super) enhanced_ui: usize,
}

pub(super) static AX_STRINGS: OnceLock<AxStrings> = OnceLock::new();

/// 由字面量创建常驻 CFString(进程级常量,数量固定,故意不 CFRelease)。
unsafe fn cfstr_literal(lit: &str) -> usize {
    cfstring_new(lit) as usize
}

pub(super) fn ax_strings() -> &'static AxStrings {
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
            frontmost: cfstr_literal("AXFrontmost"),
            press_action: cfstr_literal("AXPress"),
            raise_action: cfstr_literal("AXRaise"),
            pick_action: cfstr_literal("AXPick"),
            confirm_action: cfstr_literal("AXConfirm"),
            open_action: cfstr_literal("AXOpen"),
            scroll_to_visible_action: cfstr_literal("AXScrollToVisible"),
            manual_accessibility: cfstr_literal("AXManualAccessibility"),
            enhanced_ui: cfstr_literal("AXEnhancedUserInterface"),
        }
    })
}

/// AX 常量是否就绪。字面量创建不依赖系统符号,正常恒为 true;
/// 保留该判定仅作为 OOM 等极端情况下的防御(此时 fail-closed 比传空指针安全)。
pub(super) fn ax_strings_loaded() -> bool {
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
pub(super) fn kAXChildrenAttribute() -> CFStringRef {
    ax_strings().children as CFStringRef
}
#[inline]
pub(super) fn kAXRoleAttribute() -> CFStringRef {
    ax_strings().role as CFStringRef
}
#[inline]
pub(super) fn kAXTitleAttribute() -> CFStringRef {
    ax_strings().title as CFStringRef
}
#[inline]
pub(super) fn kAXValueAttribute() -> CFStringRef {
    ax_strings().value as CFStringRef
}
#[inline]
pub(super) fn kAXDescriptionAttribute() -> CFStringRef {
    ax_strings().description as CFStringRef
}
#[inline]
pub(super) fn kAXPositionAttribute() -> CFStringRef {
    ax_strings().position as CFStringRef
}
#[inline]
pub(super) fn kAXSizeAttribute() -> CFStringRef {
    ax_strings().size as CFStringRef
}
#[inline]
pub(super) fn kAXWindowsAttribute() -> CFStringRef {
    ax_strings().windows as CFStringRef
}
#[inline]
pub(super) fn kAXFocusedAttribute() -> CFStringRef {
    ax_strings().focused as CFStringRef
}
#[inline]
pub(super) fn kAXFrontmostAttribute() -> CFStringRef {
    ax_strings().frontmost as CFStringRef
}
#[inline]
pub(super) fn kAXPressAction() -> CFStringRef {
    ax_strings().press_action as CFStringRef
}
#[inline]
pub(super) fn kAXRaiseAction() -> CFStringRef {
    ax_strings().raise_action as CFStringRef
}
#[inline]
pub(super) fn kAXPickAction() -> CFStringRef {
    ax_strings().pick_action as CFStringRef
}
#[inline]
pub(super) fn kAXConfirmAction() -> CFStringRef {
    ax_strings().confirm_action as CFStringRef
}
#[inline]
pub(super) fn kAXOpenAction() -> CFStringRef {
    ax_strings().open_action as CFStringRef
}
#[inline]
pub(super) fn kAXScrollToVisibleAction() -> CFStringRef {
    ax_strings().scroll_to_visible_action as CFStringRef
}
#[inline]
pub(super) fn kAXManualAccessibilityAttribute() -> CFStringRef {
    ax_strings().manual_accessibility as CFStringRef
}
#[inline]
pub(super) fn kAXEnhancedUserInterfaceAttribute() -> CFStringRef {
    ax_strings().enhanced_ui as CFStringRef
}

/// 「辅助功能」未授权时的统一文案(供 require_trusted / permission_hint 共用)。
///
/// 2026-09-16 第 55 轮修正:AX C API 在 macOS 13~26 **全版本可用**(按字面量 CFString
/// 调用,与系统版本无关),唯一前置条件是 TCC 辅助功能授权;「AX 不可用」只在**未授权**
/// 这一种情况下成立(kAXErrorAPIDisabled = -25211)。因此文案以「怎么授权」为主,
/// osascript/cliclick 模板仅作为用户不便授权时的降级路径。
/// 文案会作为 tool_result 回填给 LLM,故控制在 ~500 字符内避免重复调用撑爆上下文。
pub(super) const MACOS_AX_UNAVAILABLE_HINT: &str =
    "辅助功能未授权,MCP_Window_Use(action=inspect)/MCP_Window_Use(action=control) 此时不可用(AX 返回 -25211 \
     kAXErrorAPIDisabled)。AX C API 本身在 macOS 13~26 全版本可用,只差授权:\
     系统设置 → 隐私与安全性 → 辅助功能 → 勾选运行 laew 的宿主终端(Terminal/iTerm/VS Code),\
     然后完全退出并重开该终端(TCC 按进程启动时快照生效);设 LAEW_AX_PROMPT=1 可主动弹授权框。\
     授权前可降级走 Bash + osascript(SubAgent Bash 全量可用):\
     activate 应用 / System Events keystroke 输入 / pbcopy·pbpaste 剪贴板 / \
     cliclick c:x,y 坐标点击 / screencapture -x 截图。\
     MCP_Window_Use(action=list) 走 CoreGraphics 不需授权,任何情况下都能枚举窗口标题·PID·位置。";

/// CFStringRef → String(UTF-8;先走快路径指针,失败回退拷贝缓冲区)。
pub(super) unsafe fn cfstr(s: CFStringRef) -> String {
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
pub(super) unsafe fn cfstring_new(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).unwrap_or_else(|_| std::ffi::CString::new("").unwrap());
    CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8)
}

/// CFNumberRef → i64。
pub(super) unsafe fn cfnum_i64(n: CFTypeRef) -> i64 {
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
pub(super) unsafe fn dict_get(dict: CFDictionaryRef, key: &str) -> CFTypeRef {
    if dict.is_null() {
        return std::ptr::null();
    }
    let k = cfstring_new(key);
    let v = super::CFDictionaryGetValue(dict, k.cast());
    super::CFRelease(k.cast());
    v
}

/// AX 属性读取(返回拥有所有权的 CFTypeRef;失败为 null)。
pub(super) unsafe fn ax_get(el: AXUIElementRef, attr: CFStringRef) -> CFTypeRef {
    let mut out: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(el, attr, &mut out);
    if err != K_AX_ERROR_SUCCESS {
        return std::ptr::null();
    }
    out
}

/// AX 字符串属性。
pub(super) unsafe fn ax_get_string(el: AXUIElementRef, attr: CFStringRef) -> String {
    let v = ax_get(el, attr);
    if v.is_null() {
        return String::new();
    }
    let s = cfstr(v as CFStringRef);
    CFRelease(v);
    s
}

/// AX 错误码 → 可读文案。
pub(super) fn ax_error_text(code: AXError) -> String {
    match code {
        K_AX_ERROR_API_DISABLED => {
            "无障碍权限未授予(kAXErrorAPIDisabled)。请到 系统设置 → 隐私与安全性 → 辅助功能 \
             中勾选本终端应用(laew 所在终端,如 Terminal/iTerm),然后重试"
                .to_string()
        }
        K_AX_ERROR_INVALID_UI_ELEMENT => {
            "控件已失效(窗口可能已关闭或 UI 已刷新),请重新 MCP_Window_Use(action=inspect)".into()
        }
        K_AX_ERROR_ATTRIBUTE_UNSUPPORTED => "该控件不支持此属性(应用未实现对应无障碍属性)".into(),
        K_AX_ERROR_ACTION_UNSUPPORTED => "该控件不支持此动作(应用未实现对应无障碍动作)".into(),
        K_AX_ERROR_CANNOT_COMPLETE => "AX 调用无法完成(目标应用无响应或 IPC 失败)".into(),
        other => format!("AX 错误码 {other}"),
    }
}
