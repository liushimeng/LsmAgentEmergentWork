#![allow(non_snake_case)]

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
//! 设计见 `docs/MCP_Window_Use/01-设计与解决方案.md` §2.3 macOS 后端。
//!
//! 2026-09-20 第 97 轮:macos_legacy 单文件 2032 行超线,目录化为 6 文件
//! (mod.rs / ax_attrs.rs / cg_event.rs / inspect.rs / act.rs / tests.rs),
//! 拆分前后公开 API 路径零变化,代码逐行机械搬移零改写。

use std::collections::HashMap;

use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::{CFRelease, CFTypeRef};
use core_foundation::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation::string::CFStringRef;

use super::{
    matches_filter, platform_err, ControlAction, ControlNode, Rect, WindowDriver, WindowInfo,
};
use crate::error::Result;

// ===================== FFI 声明(2026-09-20 第 97 轮:所有 extern 块集中在 mod.rs,各子模块 super::*) =====================

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
    // 2026-09-17 第 80 轮:读取控件真实支持的 AX action。
    // 微信 4.x/Electron/Qt 控件不一定支持 AXPress,常用 AXPick/AXConfirm/AXOpen。
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyActionNames(element: AXUIElementRef, value: *mut CFTypeRef) -> AXError;
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
    // 2026-09-18 第 87 轮:修饰键组合(cmd+f 等)所需 —— 设置事件修饰键位。
    fn CGEventSetFlags(event: CFTypeRef, flags: u64);
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
// 2026-09-19 第 90 轮:中键(kCGEventOtherMouseDown/Up = 25/26,button 2)。
const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const K_CG_MOUSE_BUTTON_CENTER: u32 = 2;

// ===================== 子模块(2026-09-20 第 97 轮拆分) =====================

mod act;
mod ax_attrs;
mod cg_event;
mod inspect;

#[cfg(test)]
mod tests;

// 把 ax_attrs / cg_event / inspect 子模块的常用导出在本模块内再导出(供 act 子模块 super::* 取用),
// 子模块间的 cross-use 通过 super::xxx 自然可见。
use ax_attrs::{
    ax_error_text, ax_get, ax_get_string, ax_strings_loaded, cfnum_i64, cfstr, cfstring_new,
    dict_get, kAXChildrenAttribute, kAXConfirmAction, kAXDescriptionAttribute,
    kAXEnhancedUserInterfaceAttribute, kAXFocusedAttribute, kAXFrontmostAttribute,
    kAXManualAccessibilityAttribute, kAXOpenAction, kAXPickAction, kAXPositionAttribute,
    kAXPressAction, kAXRaiseAction, kAXRoleAttribute, kAXScrollToVisibleAction, kAXSizeAttribute,
    kAXTitleAttribute, kAXValueAttribute, kAXWindowsAttribute, MACOS_AX_UNAVAILABLE_HINT,
};
use cg_event::{
    cg_click_at, cg_click_at_ex, cg_drag, cg_mod_flags, cg_mod_note, cg_move_cursor,
    cg_scroll_lines, cg_send_key, cg_send_key_with_flags, cg_type_text, keycode_for_name,
    parse_key_combo, CgMouseButton,
};
use inspect::{
    build_tree, element_action_names, element_at_path, is_frontmost_pid, parse_window_id,
    tree_is_shallow, window_element,
};

// ===================== 驱动实现(2026-09-20 第 97 轮拆分,核心逻辑仍在 mod.rs) =====================

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
    /// 让 LLM 在首次使用 MCP_Window_Use(action=inspect)/MCP_Window_Use(action=control) 时自动触发授权流程。
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
    /// - `prompt=true` 时主动触发系统授权弹窗(避免用户错过);
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
                on_progress(elapsed, Self::trusted_quiet());
                return (false, elapsed);
            }
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

    /// 第 81 轮:窗口所属应用是否已处于前台(AXFrontmost)。
    /// MCP_Window_Use(action=open) 对已存在窗口先查本方法,已前台则跳过激活(窗口不再反复闪烁)。
    fn is_frontmost(&self, window_id: &str) -> bool {
        match parse_window_id(window_id) {
            Ok((pid, _)) => is_frontmost_pid(pid),
            Err(_) => false,
        }
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
            // 2026-09-17 第 76 轮 P0-1:记录 CGWindowID(kCGWindowNumber),
            // 用于 MCP_Window_Use(action=ocr) / MCP_Window_Use(action=screenshot) 直接命中目标窗口(避免多窗口
            // 进程按 PID 匹配拿错窗口)。对 LLM 暴露的稳定 token 仍是 {pid}:{idx}。
            let cg_window_id = cfnum_i64(dict_get(dict, "kCGWindowNumber")).max(0) as u32;
            let idx = seen.entry(pid).or_insert(0);
            let id = format!("{pid}:{idx}");
            *idx += 1;
            out.push(WindowInfo {
                id,
                title,
                process_name: owner,
                pid: pid.max(0) as u32,
                bounds,
                cg_window_id: Some(cg_window_id),
                hwnd: None,
                wmctrl_id: None,
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
                "MCP_Window_Use(action=inspect) 等待 macOS 辅助功能授权"
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
        let (pid, idx) = parse_window_id(window_id)?;
        let max_depth = max_depth.clamp(1, 12);
        unsafe {
            // 第 81 轮:AX 异步建树等待(warmup)。AXEnhancedUserInterface 置位后
            // Electron/Qt/自绘 App 需要数百毫秒搭树,首读常为「浅树」;检测到浅树时
            // 等 300ms 重建,最多 3 次。带 filter 的调用不参与(命中即返回,避免把
            // 合法的「仅容器命中」当浅树);LAEW_DISABLE_AX_WARMUP=1 一键关闭。
            let warmup_enabled = !std::env::var("LAEW_DISABLE_AX_WARMUP")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false);
            let attempts = if warmup_enabled && filter.is_none() {
                3
            } else {
                1
            };
            for attempt in 0..attempts {
                let win = window_element(pid, idx)?;
                let tree = build_tree(win, "/".to_string(), 1, max_depth, filter);
                CFRelease(win);
                let tree = tree.ok_or_else(|| {
                    platform_err(
                        "macos",
                        format!(
                            "filter 未命中窗口 {window_id} 内任何控件(已遍历到 max_depth={max_depth})。\
                             【同义词建议】中文 UI 名称常见笔误:通讯录 ↔ 通信录 ↔ 联系人 ↔ Contacts;\
                             消息 ↔ 发送 ↔ Send;输入框 ↔ 搜索 ↔ Search;按钮 ↔ Button;关闭 ↔ X ↔ close。\
                             建议:1) 改用上表同义词重试;2) filter 留空 + max_depth=4-5 看完整树;\
                             3) AX 树为空时改走 MCP_Window_Use(action=ocr) 视觉路线"
                        ),
                    )
                })?;
                // 第 81 轮:浅树重试(warmup)。
                if warmup_enabled && filter.is_none() && tree_is_shallow(&tree) {
                    if attempt + 1 < attempts {
                        tracing::debug!(
                            attempt = attempt + 1,
                            window_id = %window_id,
                            "AX 浅树(异步建树未完成),等待 300ms 后重建"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        continue;
                    }
                }
                return Ok(tree);
            }
            // 循环内必然 return;此处仅为类型闭合
            unreachable!("inspect warmup loop must return")
        }
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        // 第 97 轮拆分:act match 分发独立到 act.rs 子模块
        act::dispatch_act(self, window_id, path, action)
    }

    fn bring_to_front(&self, window_id: &str) -> Result<()> {
        // 第 81 轮:已前台 → 直接返回(跳过 AXRaise + osascript frontmost),
        // 消除 MCP_Window_Use(action=open) 在多单元/重试链路中把窗口反复前置导致的闪烁与焦点断续。
        if self.is_frontmost(window_id) {
            tracing::debug!(window_id = %window_id, "窗口已在前台,跳过激活(幂等前置)");
            return Ok(());
        }
        let (pid, idx) = parse_window_id(window_id)?;
        // 先 AXRaise 对应 NSWindow,再通过 System Events 让 App frontmost。
        // AXRaise 只调整窗口层级;部分 App(尤其微信/Electron)仍需 frontmost
        // 才会接受键盘事件,因此两步都不能省。
        unsafe {
            let win = window_element(pid, idx)?;
            let raise_err = AXUIElementPerformAction(win, kAXRaiseAction());
            CFRelease(win);
            if raise_err != K_AX_ERROR_SUCCESS
                && raise_err != K_AX_ERROR_ACTION_UNSUPPORTED
                && raise_err != K_AX_ERROR_ATTRIBUTE_UNSUPPORTED
            {
                return Err(platform_err("macos", ax_error_text(raise_err)));
            }
        }
        let script = format!(
            "tell application \"System Events\" to set frontmost of (first application process whose unix id is {pid}) to true"
        );
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| platform_err("macos", format!("执行 osascript 激活窗口失败: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(platform_err(
                "macos",
                format!("osascript 激活 pid={pid} 失败: {}", stderr.trim()),
            ));
        }
        Ok(())
    }

    // 2026-09-17 第 74 轮 T1:macOS Vision OCR 实装
    // 此前默认实现返回「平台暂不支持」,导致微信 4.x 等自绘 UI 完全无法识别。
    // 现在走 CGWindowListCreateImage 截图 + Vision.framework OCR,返回词级坐标。
    fn ocr(
        &self,
        window_id: &str,
        region: Option<Rect>,
        lang: Option<&str>,
    ) -> Result<Vec<super::OcrBlock>> {
        use crate::agent::window::macos_vision_ocr::{self, VisionOcrConfig};

        // 1. 解析 window_id → CGWindowID
        let (pid, _) = parse_window_id(window_id)?;
        let cg_window_id = macos_vision_ocr::resolve_cgwindow_id(pid, None).map_err(|e| {
            platform_err(
                self.platform_name(),
                format!("解析 CGWindowID 失败(pid={pid}): {e}"),
            )
        })?;

        // 2. 构建 OCR 配置
        let mut cfg = VisionOcrConfig::default();
        if let Some(lang_str) = lang {
            // 解析 BCP-47 语言标签
            cfg.languages = lang_str.split(',').map(|s| s.trim().to_string()).collect();
        }

        // 3. 转换 region 格式
        let region_tuple = region.map(|r| (r.x, r.y, r.width, r.height));

        // 4. 执行 OCR
        let blocks = macos_vision_ocr::ocr_window(cg_window_id, region_tuple, Some(cfg))
            .map_err(|e| platform_err(self.platform_name(), format!("Vision OCR 失败: {e}")))?;

        // 5. 转换为统一的 OcrBlock 格式
        Ok(blocks
            .into_iter()
            .map(|b| super::OcrBlock {
                text: b.text,
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
            })
            .collect())
    }

    // 2026-09-17 第 76 轮 P0-1:带 WindowInfo 的 OCR(优先使用 cached cg_window_id,
    // 避免多窗口进程按 PID 匹配拿到错窗口的 CGWindowID 导致截图/OCR 失败)。
    fn ocr_with_info(
        &self,
        info: &super::WindowInfo,
        region: Option<Rect>,
        lang: Option<&str>,
    ) -> Result<Vec<super::OcrBlock>> {
        use crate::agent::window::macos_vision_ocr::{self, VisionOcrConfig};

        // 优先使用 WindowInfo 中缓存的 cg_window_id;
        // 缺失(legacy / fallback 路径)时降级到按 PID 解析
        let cg_window_id = match info.cg_window_id {
            Some(id) if id > 0 => id,
            _ => {
                let (pid, _) = parse_window_id(&info.id)?;
                macos_vision_ocr::resolve_cgwindow_id(pid, None).map_err(|e| {
                    platform_err(
                        self.platform_name(),
                        format!("解析 CGWindowID 失败(pid={pid}): {e}"),
                    )
                })?
            }
        };

        let mut cfg = VisionOcrConfig::default();
        if let Some(lang_str) = lang {
            cfg.languages = lang_str.split(',').map(|s| s.trim().to_string()).collect();
        }
        let region_tuple = region.map(|r| (r.x, r.y, r.width, r.height));
        let blocks = macos_vision_ocr::ocr_window(cg_window_id, region_tuple, Some(cfg))
            .map_err(|e| platform_err(self.platform_name(), format!("Vision OCR 失败: {e}")))?;
        Ok(blocks
            .into_iter()
            .map(|b| super::OcrBlock {
                text: b.text,
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
            })
            .collect())
    }

    // 2026-09-17 第 74 轮 T2:macOS 原生截图(CGWindowListCreateImage)
    // 替代 screencapture 命令行,无需屏幕录制权限,只需辅助功能权限。
    fn screenshot_to(
        &self,
        window_id: &str,
        region: Option<Rect>,
        output_path: &std::path::Path,
    ) -> Result<Rect> {
        use crate::agent::window::macos_vision_ocr;

        // 1. 解析 window_id → CGWindowID
        let (pid, _) = parse_window_id(window_id)?;
        let cg_window_id = macos_vision_ocr::resolve_cgwindow_id(pid, None).map_err(|e| {
            platform_err(
                self.platform_name(),
                format!("解析 CGWindowID 失败(pid={pid}): {e}"),
            )
        })?;

        // 2. 转换 region 格式
        let region_tuple = region.map(|r| (r.x, r.y, r.width, r.height));

        // 3. 执行截图
        macos_vision_ocr::screenshot_window(cg_window_id, region_tuple, output_path)
            .map_err(|e| platform_err(self.platform_name(), format!("CGWindow 截图失败: {e}")))?;

        // 4. 返回实际截取的区域
        let _meta = std::fs::metadata(output_path).map_err(|e| {
            platform_err(self.platform_name(), format!("读取截图文件元数据失败: {e}"))
        })?;

        // 返回窗口 bounds(截图前已获取)
        let win = self.list_windows(None).unwrap_or_default();
        let window_info = win.iter().find(|w| w.id == window_id);
        Ok(window_info.map(|w| w.bounds).unwrap_or_default())
    }

    // 2026-09-17 第 76 轮 P0-1:带 WindowInfo 的截图(优先使用 cached cg_window_id,
    // 避免多窗口进程按 PID 匹配拿到错窗口的 CGWindowID 导致截图失败)。
    fn screenshot_to_with_info(
        &self,
        info: &super::WindowInfo,
        region: Option<Rect>,
        output_path: &std::path::Path,
    ) -> Result<Rect> {
        use crate::agent::window::macos_vision_ocr;

        let cg_window_id = match info.cg_window_id {
            Some(id) if id > 0 => id,
            _ => {
                let (pid, _) = parse_window_id(&info.id)?;
                macos_vision_ocr::resolve_cgwindow_id(pid, None).map_err(|e| {
                    platform_err(
                        self.platform_name(),
                        format!("解析 CGWindowID 失败(pid={pid}): {e}"),
                    )
                })?
            }
        };

        let region_tuple = region.map(|r| (r.x, r.y, r.width, r.height));
        macos_vision_ocr::screenshot_window(cg_window_id, region_tuple, output_path)
            .map_err(|e| platform_err(self.platform_name(), format!("CGWindow 截图失败: {e}")))?;
        Ok(info.bounds)
    }
}
