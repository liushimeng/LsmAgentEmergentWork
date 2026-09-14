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

    static kAXChildrenAttribute: CFStringRef;
    static kAXRoleAttribute: CFStringRef;
    static kAXTitleAttribute: CFStringRef;
    static kAXValueAttribute: CFStringRef;
    static kAXPositionAttribute: CFStringRef;
    static kAXSizeAttribute: CFStringRef;
    static kAXWindowsAttribute: CFStringRef;
    static kAXFocusedAttribute: CFStringRef;
    static kAXPressAction: CFStringRef;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;

// ===================== CF 辅助 =====================

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
                let key = kAXTrustedCheckOptionPrompt;
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
        if Self::trusted() {
            Ok(())
        } else {
            Err(platform_err("macos", ax_error_text(K_AX_ERROR_API_DISABLED)))
        }
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
        let wins = ax_get(app, kAXWindowsAttribute);
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
        let pos = ax_get(el, kAXPositionAttribute);
        if !pos.is_null() {
            let mut p = CGPoint::default();
            if AXValueGetValue(pos, K_AX_VALUE_CGPOINT_TYPE, (&mut p as *mut CGPoint).cast()) != 0 {
                rect.x = p.x as i64;
                rect.y = p.y as i64;
            }
            CFRelease(pos);
        }
        let size = ax_get(el, kAXSizeAttribute);
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
        let role = ax_get_string(el, kAXRoleAttribute);
        let name = {
            let t = ax_get_string(el, kAXTitleAttribute);
            if t.is_empty() {
                ax_get_string(el, kAXValueAttribute)
            } else {
                t
            }
        };
        let value = ax_get_string(el, kAXValueAttribute);
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
            let children = ax_get(el, kAXChildrenAttribute);
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
            let children = ax_get(cur, kAXChildrenAttribute);
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

impl WindowDriver for MacOsDriver {
    fn platform_name(&self) -> &'static str {
        "macos"
    }

    fn list_windows(&self, filter: Option<&str>) -> Result<Vec<WindowInfo>> {
        self.require_trusted()?;
        unsafe {
            let list = CGWindowListCopyWindowInfo(K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY, 0);
            if list.is_null() {
                return Err(platform_err("macos", "CGWindowListCopyWindowInfo 返回空(无窗口服务器连接?)"));
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
                    let err = AXUIElementPerformAction(el, kAXPressAction);
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已对 {window_id}{path} 执行点击(AXPress)"))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                ControlAction::Focus => {
                    let true_v: CFBooleanRef =
                        core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
                    let err = AXUIElementSetAttributeValue(el, kAXFocusedAttribute, true_v.cast());
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已聚焦 {window_id}{path}"))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                ControlAction::SetText(text) => {
                    let cf = cfstring_new(text);
                    let err = AXUIElementSetAttributeValue(el, kAXValueAttribute, cf.cast());
                    CFRelease(cf.cast());
                    if err == K_AX_ERROR_SUCCESS {
                        Ok(format!("已向 {window_id}{path} 写入文本({} 字符)", text.chars().count()))
                    } else {
                        Err(platform_err("macos", ax_error_text(err)))
                    }
                }
                ControlAction::GetText => {
                    let v = ax_get_string(el, kAXValueAttribute);
                    let t = ax_get_string(el, kAXTitleAttribute);
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
        if Self::trusted() {
            None
        } else {
            Some(ax_error_text(K_AX_ERROR_API_DISABLED))
        }
    }
}
