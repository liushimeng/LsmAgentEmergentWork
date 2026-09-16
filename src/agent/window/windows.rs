//! Windows 窗口驱动:UI Automation 为主,Win32 消息降级。
//!
//! - 窗口枚举:`EnumWindows` + `IsWindowVisible` + 非空标题,id = HWND 十进制字符串;
//! - 控件遍历:UIA(`CoCreateInstance(CUIAutomation)` → `ElementFromHandle` →
//!   RawViewWalker 深度遍历),覆盖 Win32 原生 / WPF / Qt(带 UIA Provider)等;
//!   UIA 初始化失败时降级 `GetWindow(GW_CHILD/HWNDNEXT)` 递归枚举子 HWND;
//! - 操作:UIA `IUIAutomationInvokePattern.Invoke` / `IUIAutomationValuePattern.SetValue`
//!   / `SetFocus`;降级路径 `SendMessageW(BM_CLICK / WM_SETTEXT / WM_GETTEXT)`;
//! - 深度 + 名称/角色过滤双裁剪,命中节点的祖先链保留。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.3 Windows 后端。

use windows::core::BSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::ProcessStatus::GetModuleBaseNameW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationValuePattern, UIA_InvokePatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SendMessageW, SetForegroundWindow, BM_CLICK,
    GW_CHILD, GW_HWNDNEXT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_SETTEXT,
};

use super::{
    matches_filter, platform_err, ControlAction, ControlNode, Rect, WindowDriver, WindowInfo,
};
use crate::error::Result;

pub struct WindowsDriver;

impl Default for WindowsDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsDriver {
    pub fn new() -> Self {
        Self
    }

    fn parse_hwnd(window_id: &str) -> Result<HWND> {
        let raw: isize = window_id.trim().parse().map_err(|_| {
            platform_err(
                "windows",
                format!("window_id 应为 WindowList 返回的 HWND 十进制字符串,实际: {window_id}"),
            )
        })?;
        Ok(HWND(raw as *mut std::ffi::c_void))
    }
}

/// COM 初始化守卫(Drop 时 CoUninitialize)。
struct ComGuard;

impl ComGuard {
    fn init() -> Result<Self> {
        // 已初始化(RPC_E_CHANGED_MODE / S_FALSE)也算成功,交由调用方继续。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

// ===================== 顶层窗口枚举 =====================

struct EnumCtx {
    windows: Vec<WindowInfo>,
    filter: Option<String>,
}

unsafe extern "system" fn enum_windows_proc(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows::Win32::Foundation::BOOL {
    let ctx = &mut *(lparam.0 as *mut EnumCtx);
    if IsWindowVisible(hwnd).as_bool() {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; (len as usize) + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let process_name = process_name_of(pid);
            let mut rc = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rc);
            let hit = matches_filter(&title, ctx.filter.as_deref())
                || matches_filter(&process_name, ctx.filter.as_deref());
            if hit {
                ctx.windows.push(WindowInfo {
                    id: (hwnd.0 as isize).to_string(),
                    title,
                    process_name,
                    pid,
                    bounds: Rect {
                        x: rc.left as i64,
                        y: rc.top as i64,
                        width: (rc.right - rc.left) as i64,
                        height: (rc.bottom - rc.top) as i64,
                    },
                });
            }
        }
    }
    true.into()
}

/// 进程映像名(失败返回空串,不阻断枚举)。
fn process_name_of(pid: u32) -> String {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 260];
        let n = GetModuleBaseNameW(handle, None, &mut buf);
        let _ = CloseHandle(handle);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

// ===================== UIA 路径 =====================

/// 把控件类型枚举值转可读角色名。
fn uia_role_name(el: &IUIAutomationElement) -> String {
    unsafe {
        el.CurrentControlType()
            .map(|t| format!("{t:?}"))
            .unwrap_or_else(|_| "Unknown".into())
    }
}

fn uia_actions(el: &IUIAutomationElement) -> Vec<String> {
    let mut v = vec!["focus".to_string(), "get_text".to_string()];
    unsafe {
        if el
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .is_ok()
        {
            v.push("click".into());
            v.push("invoke".into());
        }
        if el
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .is_ok()
        {
            v.push("set_text".into());
        }
    }
    v
}

/// UIA 递归构建控件树。
unsafe fn uia_build_tree(
    uia: &IUIAutomation,
    el: &IUIAutomationElement,
    path: String,
    depth: usize,
    max_depth: usize,
    filter: Option<&str>,
) -> Option<ControlNode> {
    let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
    let role = uia_role_name(el);
    let value = el
        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        .and_then(|p| p.CurrentValue())
        .map(|b| b.to_string())
        .unwrap_or_default();
    let bounds = el
        .CurrentBoundingRectangle()
        .map(|rc| Rect {
            x: rc.left as i64,
            y: rc.top as i64,
            width: (rc.right - rc.left) as i64,
            height: (rc.bottom - rc.top) as i64,
        })
        .unwrap_or_default();

    let mut node = ControlNode {
        path: path.clone(),
        role,
        name,
        value,
        bounds,
        actions: uia_actions(el),
        children: Vec::new(),
    };

    if depth < max_depth {
        if let Ok(walker) = uia.RawViewWalker() {
            if let Ok(mut child) = walker.GetFirstChildElement(el) {
                let mut idx = 0usize;
                loop {
                    if let Some(cn) = uia_build_tree(
                        uia,
                        &child,
                        format!("{}/{}", path.trim_end_matches('/'), idx),
                        depth + 1,
                        max_depth,
                        filter,
                    ) {
                        node.children.push(cn);
                    }
                    match walker.GetNextSiblingElement(&child) {
                        Ok(next) => child = next,
                        Err(_) => break,
                    }
                    idx += 1;
                }
            }
        }
    }

    let self_hit = matches_filter(&node.name, filter) || matches_filter(&node.role, filter);
    if filter.is_none() || self_hit || !node.children.is_empty() {
        Some(node)
    } else {
        None
    }
}

/// UIA 按路径定位元素。
unsafe fn uia_element_at_path(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    path: &str,
) -> Result<IUIAutomationElement> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed.is_empty() {
        return Ok(root.clone());
    }
    let walker = uia
        .RawViewWalker()
        .map_err(|e| platform_err("windows", format!("UIA TreeWalker 获取失败: {e}")))?;
    let mut cur = root.clone();
    for seg in trimmed.trim_start_matches('/').split('/') {
        let idx: usize = seg.parse().map_err(|_| {
            platform_err("windows", format!("控件路径段非法: {seg}(应为子控件下标)"))
        })?;
        let mut target = None;
        if let Ok(mut child) = walker.GetFirstChildElement(&cur) {
            let mut i = 0usize;
            loop {
                if i == idx {
                    target = Some(child);
                    break;
                }
                match walker.GetNextSiblingElement(&child) {
                    Ok(next) => child = next,
                    Err(_) => break,
                }
                i += 1;
            }
        }
        cur = target.ok_or_else(|| {
            platform_err(
                "windows",
                format!("路径 {path} 下标 {idx} 不存在(UI 可能已变化),请重新 WindowInspect"),
            )
        })?;
    }
    Ok(cur)
}

// ===================== Win32 降级路径 =====================

/// 子 HWND 递归枚举(GW_CHILD + GW_HWNDNEXT)。
unsafe fn win32_build_tree(
    hwnd: HWND,
    path: String,
    depth: usize,
    max_depth: usize,
    filter: Option<&str>,
) -> Option<ControlNode> {
    let mut class_buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut class_buf);
    let class = String::from_utf16_lossy(&class_buf[..n as usize]);
    let text_len = SendMessageW(hwnd, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0 as usize;
    let mut tbuf = vec![0u16; text_len + 1];
    let got = SendMessageW(
        hwnd,
        WM_GETTEXT,
        WPARAM(tbuf.len()),
        LPARAM(tbuf.as_mut_ptr() as isize),
    )
    .0 as usize;
    let name = String::from_utf16_lossy(&tbuf[..got.min(text_len)]);
    let mut rc = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rc);

    let role = if class.is_empty() {
        "Window".to_string()
    } else {
        class
    };
    let mut node = ControlNode {
        path: path.clone(),
        role,
        name,
        value: String::new(),
        bounds: Rect {
            x: rc.left as i64,
            y: rc.top as i64,
            width: (rc.right - rc.left) as i64,
            height: (rc.bottom - rc.top) as i64,
        },
        actions: vec![
            "focus".into(),
            "get_text".into(),
            "click".into(),
            "set_text".into(),
        ],
        children: Vec::new(),
    };

    if depth < max_depth {
        let mut idx = 0usize;
        let mut child = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
        while !child.0.is_null() {
            if IsWindowVisible(child).as_bool() {
                if let Some(cn) = win32_build_tree(
                    child,
                    format!("{}/{}", path.trim_end_matches('/'), idx),
                    depth + 1,
                    max_depth,
                    filter,
                ) {
                    node.children.push(cn);
                    idx += 1;
                }
            }
            child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
        }
    }

    let self_hit = matches_filter(&node.name, filter) || matches_filter(&node.role, filter);
    if filter.is_none() || self_hit || !node.children.is_empty() {
        Some(node)
    } else {
        None
    }
}

/// Win32 按路径定位子 HWND。
unsafe fn win32_hwnd_at_path(root: HWND, path: &str) -> Result<HWND> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed.is_empty() {
        return Ok(root);
    }
    let mut cur = root;
    for seg in trimmed.trim_start_matches('/').split('/') {
        let idx: usize = seg.parse().map_err(|_| {
            platform_err("windows", format!("控件路径段非法: {seg}(应为子控件下标)"))
        })?;
        let mut found = HWND::default();
        let mut i = 0usize;
        let mut child = GetWindow(cur, GW_CHILD).unwrap_or_default();
        while !child.0.is_null() {
            if IsWindowVisible(child).as_bool() {
                if i == idx {
                    found = child;
                    break;
                }
                i += 1;
            }
            child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
        }
        if found.0.is_null() {
            return Err(platform_err(
                "windows",
                format!("路径 {path} 下标 {idx} 不存在(UI 可能已变化),请重新 WindowInspect"),
            ));
        }
        cur = found;
    }
    Ok(cur)
}

/// Win32 降级操作。
unsafe fn win32_act(root: HWND, path: &str, action: &ControlAction) -> Result<String> {
    let hwnd = win32_hwnd_at_path(root, path)?;
    match action {
        ControlAction::Click | ControlAction::Invoke => {
            SendMessageW(hwnd, BM_CLICK, WPARAM(0), LPARAM(0));
            Ok(format!("已向 HWND {:?} 发送 BM_CLICK", hwnd.0))
        }
        ControlAction::Focus => {
            let _ = SetForegroundWindow(hwnd);
            Ok(format!("已聚焦 HWND {:?}", hwnd.0))
        }
        ControlAction::SetText(text) => {
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            wide.push(0);
            SendMessageW(hwnd, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize));
            Ok(format!(
                "已向 HWND {:?} 写入文本({} 字符)",
                hwnd.0,
                text.chars().count()
            ))
        }
        ControlAction::GetText => {
            let len = SendMessageW(hwnd, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0 as usize;
            let mut buf = vec![0u16; len + 1];
            let got = SendMessageW(
                hwnd,
                WM_GETTEXT,
                WPARAM(buf.len()),
                LPARAM(buf.as_mut_ptr() as isize),
            )
            .0 as usize;
            Ok(String::from_utf16_lossy(&buf[..got.min(len)]))
        }
        ControlAction::SendKeys(_) => Err(platform_err(
            "windows",
            "Win32 降级路径不支持 send_keys(UIA 可用时请走 UIA,或改用 set_text/click)",
        )),
    }
}

// ===================== WindowDriver 实现 =====================

impl WindowDriver for WindowsDriver {
    fn platform_name(&self) -> &'static str {
        "windows"
    }

    fn list_windows(&self, filter: Option<&str>) -> Result<Vec<WindowInfo>> {
        let mut ctx = EnumCtx {
            windows: Vec::new(),
            filter: filter.map(|s| s.to_string()),
        };
        unsafe {
            // 返回值(BOOL/Result,随 windows crate 版本而定)不影响结果收集,统一忽略。
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM(&mut ctx as *mut EnumCtx as isize),
            );
        }
        Ok(ctx.windows)
    }

    fn inspect(
        &self,
        window_id: &str,
        max_depth: usize,
        filter: Option<&str>,
    ) -> Result<ControlNode> {
        let hwnd = Self::parse_hwnd(window_id)?;
        let max_depth = max_depth.clamp(1, 12);
        let _com = ComGuard::init()?;
        unsafe {
            match CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_ALL) {
                Ok(uia) => {
                    let root = uia.ElementFromHandle(hwnd).map_err(|e| {
                        platform_err("windows", format!("UIA ElementFromHandle 失败: {e}"))
                    })?;
                    uia_build_tree(&uia, &root, "/".to_string(), 1, max_depth, filter).ok_or_else(
                        || {
                            platform_err(
                                "windows",
                                format!(
                                    "filter 未命中窗口 {window_id} 内任何控件,请放宽过滤条件重试"
                                ),
                            )
                        },
                    )
                }
                Err(_) => {
                    // UIA 不可用 → Win32 子控件降级(仅原生 Win32 控件可见)
                    win32_build_tree(hwnd, "/".to_string(), 1, max_depth, filter).ok_or_else(|| {
                        platform_err(
                            "windows",
                            format!("filter 未命中窗口 {window_id} 内任何控件(Win32 降级路径)"),
                        )
                    })
                }
            }
        }
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        let hwnd = Self::parse_hwnd(window_id)?;
        let _com = ComGuard::init()?;
        unsafe {
            match CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_ALL) {
                Ok(uia) => {
                    let root = uia.ElementFromHandle(hwnd).map_err(|e| {
                        platform_err("windows", format!("UIA ElementFromHandle 失败: {e}"))
                    })?;
                    let el = uia_element_at_path(&uia, &root, path)?;
                    match &action {
                        ControlAction::Click | ControlAction::Invoke => {
                            match el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(
                                UIA_InvokePatternId,
                            ) {
                                Ok(p) => {
                                    p.Invoke().map_err(|e| {
                                        platform_err("windows", format!("Invoke 失败: {e}"))
                                    })?;
                                    Ok(format!("已对 {window_id}{path} 执行 Invoke(点击)"))
                                }
                                Err(_) => {
                                    // 无 Invoke Pattern → 退化为 SetFocus + BM_CLICK
                                    let _ = el.SetFocus();
                                    drop(el);
                                    win32_act(hwnd, path, &ControlAction::Click)
                                }
                            }
                        }
                        ControlAction::Focus => {
                            el.SetFocus().map_err(|e| {
                                platform_err("windows", format!("SetFocus 失败: {e}"))
                            })?;
                            Ok(format!("已聚焦 {window_id}{path}"))
                        }
                        ControlAction::SetText(text) => {
                            match el.GetCurrentPatternAs::<IUIAutomationValuePattern>(
                                UIA_ValuePatternId,
                            ) {
                                Ok(p) => {
                                    p.SetValue(&BSTR::from(text.as_str())).map_err(|e| {
                                        platform_err("windows", format!("SetValue 失败: {e}"))
                                    })?;
                                    Ok(format!(
                                        "已向 {window_id}{path} 写入文本({} 字符)",
                                        text.chars().count()
                                    ))
                                }
                                Err(_) => {
                                    drop(el);
                                    win32_act(hwnd, path, &ControlAction::SetText(text.clone()))
                                }
                            }
                        }
                        ControlAction::GetText => {
                            if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(
                                UIA_ValuePatternId,
                            ) {
                                let v = p.CurrentValue().map(|b| b.to_string()).unwrap_or_default();
                                if !v.is_empty() {
                                    return Ok(v);
                                }
                            }
                            el.CurrentName().map(|b| b.to_string()).map_err(|e| {
                                platform_err("windows", format!("读取控件文本失败: {e}"))
                            })
                        }
                        ControlAction::SendKeys(keys) => {
                            // UIA 无通用按键注入:聚焦后由 SendInput 系完成过于平台耦合,
                            // 明确告知改道(对齐设计文档「尽力而为」语义)。
                            Err(platform_err(
                                "windows",
                                format!("send_keys({keys}) 暂不支持:请先 focus 目标控件后用 set_text/click 组合完成"),
                            ))
                        }
                    }
                }
                Err(_) => win32_act(hwnd, path, &action),
            }
        }
    }

    fn permission_hint(&self) -> Option<String> {
        None // Windows UIA 无需显式授权
    }
}
