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
use core_foundation::number::{CFNumberGetValue, CFNumberRef, kCFNumberSInt64Type};
use core_foundation::string::{
    CFStringCreateWithCString, CFStringGetCString, CFStringGetCStringPtr, CFStringGetLength,
    CFStringRef, kCFStringEncodingUTF8,
};

use super::{matches_filter, platform_err, ControlAction, ControlNode, Rect, WindowDriver, WindowInfo};
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
    fn AXValueGetValue(value: CFTypeRef, the_type: AXValueType, value_ptr: *mut std::ffi::c_void) -> Boolean;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;

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
    trusted_check_prompt: usize,
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
            trusted_check_prompt: cfstr_literal("AXTrustedCheckOptionPrompt"),
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
#[inline] fn kAXChildrenAttribute()      -> CFStringRef { ax_strings().children as CFStringRef }
#[inline] fn kAXRoleAttribute()           -> CFStringRef { ax_strings().role as CFStringRef }
#[inline] fn kAXTitleAttribute()          -> CFStringRef { ax_strings().title as CFStringRef }
#[inline] fn kAXValueAttribute()          -> CFStringRef { ax_strings().value as CFStringRef }
#[inline] fn kAXDescriptionAttribute()    -> CFStringRef { ax_strings().description as CFStringRef }
#[inline] fn kAXPositionAttribute()       -> CFStringRef { ax_strings().position as CFStringRef }
#[inline] fn kAXSizeAttribute()           -> CFStringRef { ax_strings().size as CFStringRef }
#[inline] fn kAXWindowsAttribute()        -> CFStringRef { ax_strings().windows as CFStringRef }
#[inline] fn kAXFocusedAttribute()        -> CFStringRef { ax_strings().focused as CFStringRef }
#[inline] fn kAXPressAction()             -> CFStringRef { ax_strings().press_action as CFStringRef }
#[inline] fn kAXTrustedCheckOptionPrompt() -> CFStringRef { ax_strings().trusted_check_prompt as CFStringRef }

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
    let ok = CFStringGetCString(s, buf.as_mut_ptr().cast(), cap as CFIndex, kCFStringEncodingUTF8);
    if ok != 0 {
        CStr::from_ptr(buf.as_ptr().cast()).to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

/// 创建 CFString(返回拥有所有权的 ref,调用方负责 CFRelease)。
unsafe fn cfstring_new(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).unwrap_or_else(|_| std::ffi::CString::new("").unwrap());
    CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8)
}

/// CFNumberRef → i64。
unsafe fn cfnum_i64(n: CFTypeRef) -> i64 {
    let mut out: i64 = 0;
    if !n.is_null() {
        CFNumberGetValue(n as CFNumberRef, kCFNumberSInt64Type, (&mut out as *mut i64).cast());
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
        K_AX_ERROR_INVALID_UI_ELEMENT => "控件已失效(窗口可能已关闭或 UI 已刷新),请重新 WindowInspect".into(),
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

    /// 是否已获无障碍授权。`LAEW_AX_PROMPT=1` 时允许触发系统授权弹窗。
    fn trusted() -> bool {
        let prompt = std::env::var("LAEW_AX_PROMPT")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        unsafe {
            if prompt {
                // {kAXTrustedCheckOptionPrompt: true}
                let key = kAXTrustedCheckOptionPrompt();
                let val: CFBooleanRef =
                    core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
                let keys = [key as CFTypeRef];
                let vals = [val as CFTypeRef];
                let dict = core_foundation::dictionary::CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr() as *const *const std::ffi::c_void,
                    vals.as_ptr() as *const *const std::ffi::c_void,
                    1,
                    std::ptr::null(),
                    std::ptr::null(),
                );
                let ok = AXIsProcessTrustedWithOptions(dict);
                if !dict.is_null() {
                    CFRelease(dict.cast());
                }
                ok != 0
            } else {
                AXIsProcessTrustedWithOptions(std::ptr::null()) != 0
            }
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
        if Self::trusted() {
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
        let pid: i32 = pid_s.parse().map_err(|_| {
            platform_err("macos", format!("window_id 中 pid 非法: {pid_s}"))
        })?;
        let idx: usize = idx_s.parse().map_err(|_| {
            platform_err("macos", format!("window_id 中 index 非法: {idx_s}"))
        })?;
        Ok((pid, idx))
    }

    /// 取应用的 AX 窗口数组中第 index 个窗口元素(调用方负责 CFRelease)。
    unsafe fn window_element(pid: i32, index: usize) -> Result<AXUIElementRef> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(platform_err("macos", format!("无法为 pid={pid} 创建 AX 应用元素")));
        }
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
            if AXValueGetValue(pos, K_AX_VALUE_CGPOINT_TYPE, (&mut p as *mut CGPoint).cast()) != 0 {
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
        if r.contains("button") || r.contains("menuitem") || r.contains("checkbox")
            || r.contains("radio") || r.contains("link") || r.contains("tab")
        {
            v.push("click".into());
            v.push("invoke".into());
        }
        if r.contains("textfield") || r.contains("textarea") || r.contains("combobox")
            || r.contains("searchfield") || r.contains("securetextfield")
        {
            v.push("set_text".into());
        }
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
                let e = CFArrayGetValueAtIndex(children as CFArrayRef, idx as CFIndex) as AXUIElementRef;
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

    fn inspect(&self, window_id: &str, max_depth: usize, filter: Option<&str>) -> Result<ControlNode> {
        self.require_trusted()?;
        let (pid, idx) = Self::parse_window_id(window_id)?;
        let max_depth = max_depth.clamp(1, 12);
        unsafe {
            let win = Self::window_element(pid, idx)?;
            let tree = Self::build_tree(win, "/".to_string(), 1, max_depth, filter);
            CFRelease(win);
            tree.ok_or_else(|| {
                platform_err(
                    "macos",
                    format!("filter 未命中窗口 {window_id} 内任何控件,请放宽过滤条件重试"),
                )
            })
        }
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        self.require_trusted()?;
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
                ControlAction::SendKeys(_) => Err(platform_err(
                    "macos",
                    "macOS 后端暂不支持 send_keys(AX 无通用按键注入;请改用 set_text 写入,或 click 目标按钮)",
                )),
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
