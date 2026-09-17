//! Windows 窗口驱动:UI Automation 为主,SendInput 物理输入 / Win32 消息降级。
//!
//! - 窗口枚举:`EnumWindows` + `IsWindowVisible` + 非空标题,id = HWND 十进制字符串;
//!   进程名走 `QueryFullProcessImageNameW`(LIMITED 权限可用;第 67 轮修复
//!   `GetModuleBaseNameW` 拿不到进程名的实测 bug);
//! - 控件遍历:UIA(`CoCreateInstance(CUIAutomation)` → `ElementFromHandle` →
//!   RawViewWalker 深度遍历),覆盖 Win32 原生 / WPF / Qt(带 UIA Provider)等;
//!   UIA 初始化失败时降级 `GetWindow(GW_CHILD/HWNDNEXT)` 递归枚举子 HWND;
//! - 操作决策链(2026-09-16 第 67 轮重排,自绘 UI 优先真实输入):
//!   - click:UIA Invoke → 元素中心 SendInput 物理点击(前台化)→ BM_CLICK;
//!   - set_text:UIA ValuePattern → focus + SendInput 逐字键入 → WM_SETTEXT;
//!   - send_keys:force_foreground + SendInput(组合键已实装);
//!   - scroll:控件中心 SendInput 滚轮 → WM_MOUSEWHEEL 兜底;
//!   - 坐标动作(ClickPoint/ScrollPoint/TypeText):直接走 SendInput 底座;
//! - OCR:`windows_ocr::ocr_window`(GDI 截图 + Windows.Media.Ocr);
//! - `bring_to_front`:`windows_input::force_foreground`(恢复最小化 + 三保险激活)。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.3 Windows 后端。

use windows::core::BSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationScrollItemPattern, IUIAutomationValuePattern, UIA_InvokePatternId,
    UIA_ScrollItemPatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SendMessageW, SetForegroundWindow, BM_CLICK,
    GW_CHILD, GW_HWNDNEXT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_SETTEXT,
};

use super::windows_input as winput;
use super::{matches_filter, platform_err, ControlAction, ControlNode, OcrBlock, Rect, WindowDriver, WindowInfo};
use crate::error::Result;

pub struct WindowsDriver;

impl Default for WindowsDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsDriver {
    pub fn new() -> Self {
        winput::ensure_dpi_aware();
        Self
    }

    pub(crate) fn parse_hwnd(window_id: &str) -> Result<HWND> {
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
        // SAFETY:进程级 COM 计数初始化,与 Drop 中的 CoUninitialize 配对。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY:与 init 配对。
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
                    cg_window_id: None,
                    hwnd: Some(hwnd.0 as isize),
                    wmctrl_id: None,
                });
            }
        }
    }
    true.into()
}

/// 进程映像名(失败返回空串,不阻断枚举)。
///
/// 2026-09-16 第 67 轮:`QueryFullProcessImageNameW` 替换 `GetModuleBaseNameW` ——
/// 后者需要 PROCESS_VM_READ,LIMITED 权限下实测返回空串(微信 4.x 主进程即如此),
/// 导致 WindowFind 无法按进程名(Weixin)匹配。
fn process_name_of(pid: u32) -> String {
    // SAFETY:标准进程查询;句柄本函数内释放。
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, windows::core::PWSTR(
            buf.as_mut_ptr(),
        ), &mut len);
        let _ = CloseHandle(handle);
        if ok.is_ok() && len > 0 {
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            // 取映像文件名(去目录)
            path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
        } else {
            String::new()
        }
    }
}

// ===================== UIA 路径 =====================

/// 把 UIA ControlType 原始值转可读角色名(2026-09-16 第 67 轮:
/// 原 `format!("{t:?}")` 输出 `UIA_CONTROLTYPE_ID(50032)`,LLM 不可读)。
fn uia_role_name(el: &IUIAutomationElement) -> String {
    // SAFETY:属性读取,失败回退 Unknown。
    let raw = unsafe { el.CurrentControlType() }
        .map(|t| t.0)
        .unwrap_or(0);
    let name = match raw {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        _ => "Unknown",
    };
    name.to_string()
}

fn uia_actions(el: &IUIAutomationElement) -> Vec<String> {
    let mut v = vec!["focus".to_string(), "get_text".to_string()];
    // SAFETY:模式查询,无副作用。
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
///
/// # Safety
/// `el` 必须是有效的 UIA 元素;walker 遍历失败即剪枝。
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
///
/// # Safety
/// `root` 必须有效;路径段非法 / 越界返回 Err。
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
///
/// # Safety
/// `hwnd` 必须有效。
unsafe fn win32_build_tree(
    hwnd: HWND,
    path: String,
    depth: usize,
    max_depth: usize,
    filter: Option<&str>,
) -> Option<ControlNode> {
    let mut class_buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut class_buf);
    let class = String::from_utf16_lossy(&class_buf[..(n as usize)]);
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
///
/// # Safety
/// `root` 必须有效。
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
///
/// # Safety
/// `root` 必须有效。
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
        ControlAction::SendKeys(spec) => {
            // 第 67 轮:Win32 降级路径也可用 SendInput(系统级注入)
            winput::force_foreground(hwnd)?;
            winput::send_keys_spec(spec)?;
            Ok(format!("已向 HWND {:?} 发送按键 {spec:?}", hwnd.0))
        }
        ControlAction::TypeText(text) => {
            winput::force_foreground(hwnd)?;
            winput::type_text(text)?;
            Ok(format!("已向 HWND {:?} 真实键入 {} 字符", hwnd.0, text.chars().count()))
        }
        // 2026-09-16 第 66 轮:WM_MOUSEWHEEL 滚动(目标控件 HWND 优先,未命中发窗口根)。
        ControlAction::Scroll { lines } => {
            const WM_MOUSEWHEEL: u32 = 0x020A;
            let delta = lines.saturating_mul(120);
            // wParam 高 16 位 = 带符号 delta(正=向上/远离用户)
            let wparam = WPARAM(((delta as u16) as usize) << 16);
            SendMessageW(hwnd, WM_MOUSEWHEEL, wparam, LPARAM(0));
            Ok(format!(
                "已向 HWND {:?} 发送 WM_MOUSEWHEEL({} 行,{})",
                hwnd.0,
                lines.abs(),
                if *lines > 0 { "向上" } else { "向下" }
            ))
        }
        ControlAction::ScrollToVisible => Err(platform_err(
            "windows",
            "Win32 降级路径不支持 scroll_to_visible(UIA 可用时走 ScrollItemPattern;或改用 scroll)",
        )),
        // 坐标动作与 UIA 无关,直接走 SendInput
        ControlAction::ClickPoint { x, y } => {
            winput::click_point(*x, *y, false, false)?;
            Ok(format!("已在 ({x},{y}) 执行物理左键单击"))
        }
        ControlAction::DoubleClickPoint { x, y } => {
            winput::click_point(*x, *y, true, false)?;
            Ok(format!("已在 ({x},{y}) 执行物理双击"))
        }
        ControlAction::RightClickPoint { x, y } => {
            winput::click_point(*x, *y, false, true)?;
            Ok(format!("已在 ({x},{y}) 执行物理右键单击"))
        }
        ControlAction::ScrollPoint { x, y, lines } => {
            winput::wheel_at(*x, *y, *lines)?;
            Ok(format!("已在 ({x},{y}) 滚动 {} 行({})", lines.abs(), if *lines > 0 { "向上" } else { "向下" }))
        }
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
        // SAFETY:EnumWindows 回调参数指向本栈变量,枚举同步完成。
        unsafe {
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
        // SAFETY:UIA COM 调用;错误转结构化 platform_err。
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
                                    "filter 未命中窗口 {window_id} 内任何控件(已遍历到 max_depth={max_depth})。\
                                     【同义词建议】中文 UI 名称常见笔误:通讯录 ↔ 通信录 ↔ 联系人 ↔ Contacts;\
                                     消息 ↔ 发送 ↔ Send;输入框 ↔ 搜索 ↔ Search;按钮 ↔ Button;关闭 ↔ X ↔ close。\
                                     建议:1) 改用上表同义词重试;2) filter 留空 + max_depth=4-5 看完整树;\
                                     3) 若树里只有少量 Pane(如微信 4.x 的 MMUIRenderSubWindow,自绘 UI 无控件),\
                                     改走视觉路线:WindowOCR(window_id) 拿文本坐标 → WindowAction click_point/type_text"
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
                            format!(
                                "filter 未命中窗口 {window_id} 内任何控件(Win32 降级路径,仅原生控件可见)。\
                                 【同义词建议】中文 UI 名称常见笔误:通讯录 ↔ 通信录 ↔ 联系人 ↔ Contacts;\
                                 消息 ↔ 发送 ↔ Send;输入框 ↔ 搜索 ↔ Search;按钮 ↔ Button;关闭 ↔ X ↔ close。\
                                 建议改用上表同义词,或改走视觉路线 WindowOCR + click_point"
                            ),
                        )
                    })
                }
            }
        }
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        let hwnd = Self::parse_hwnd(window_id)?;
        let _com = ComGuard::init()?;
        // SAFETY:UIA COM 调用;错误转结构化 platform_err。
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
                                    // 第 67 轮:无 Invoke Pattern → 元素中心 SendInput 物理点击
                                    // (自绘 UI 的可靠路径)→ BM_CLICK 最终兜底
                                    let bounds = el.CurrentBoundingRectangle().unwrap_or_default();
                                    let cx = (bounds.left + bounds.right) / 2;
                                    let cy = (bounds.top + bounds.bottom) / 2;
                                    if cx > 0 && cy > 0 && bounds.right > bounds.left {
                                        let _ = el.SetFocus();
                                        winput::force_foreground(hwnd)?;
                                        winput::click_point(cx as i64, cy as i64, false, false)?;
                                        Ok(format!(
                                            "已对 {window_id}{path} 中心 ({cx},{cy}) 执行物理点击(SendInput)"
                                        ))
                                    } else {
                                        let _ = el.SetFocus();
                                        drop(el);
                                        win32_act(hwnd, path, &ControlAction::Click)
                                    }
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
                                    // 第 67 轮:focus + SendInput 逐字键入(自绘输入框唯一可靠路径)
                                    let _ = el.SetFocus();
                                    winput::force_foreground(hwnd)?;
                                    winput::type_text(text)?;
                                    Ok(format!(
                                        "已向 {window_id}{path} 真实键入文本({} 字符,SendInput)",
                                        text.chars().count()
                                    ))
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
                        ControlAction::SendKeys(spec) => {
                            // 第 67 轮实装:聚焦 + 前台化 + SendInput(支持 enter / ctrl+a 等组合键)
                            let _ = el.SetFocus();
                            drop(el);
                            winput::force_foreground(hwnd)?;
                            winput::send_keys_spec(spec)?;
                            Ok(format!("已向 {window_id} 发送按键 {spec:?}(SendInput)"))
                        }
                        // 2026-09-16 第 66 轮:scroll_to_visible 优先 UIA ScrollItemPattern,
                        // 不支持时降级控件中心物理滚轮。
                        ControlAction::ScrollToVisible => {
                            match el.GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(
                                UIA_ScrollItemPatternId,
                            ) {
                                Ok(p) => {
                                    p.ScrollIntoView().map_err(|e| {
                                        platform_err("windows", format!("ScrollIntoView 失败: {e}"))
                                    })?;
                                    Ok(format!("已把 {window_id}{path} 滚动到可见区域(ScrollItemPattern)"))
                                }
                                Err(_) => {
                                    let bounds = el.CurrentBoundingRectangle().unwrap_or_default();
                                    let cx = (bounds.left + bounds.right) / 2;
                                    let cy = (bounds.top + bounds.bottom) / 2;
                                    drop(el);
                                    if cx > 0 && cy > 0 {
                                        winput::wheel_at(cx as i64, cy as i64, -3)?;
                                        Ok(format!(
                                            "已对 {window_id}{path} 中心 ({cx},{cy}) 滚动 3 行(物理滚轮降级)"
                                        ))
                                    } else {
                                        win32_act(hwnd, path, &ControlAction::Scroll { lines: -3 })
                                    }
                                }
                            }
                        }
                        // 第 67 轮:滚动优先「控件中心物理滚轮」(自绘 UI 忽略消息滚动),
                        // 元素无有效 bounds 时降级 WM_MOUSEWHEEL。
                        ControlAction::Scroll { lines } => {
                            let lines = *lines;
                            let bounds = el.CurrentBoundingRectangle().unwrap_or_default();
                            let cx = (bounds.left + bounds.right) / 2;
                            let cy = (bounds.top + bounds.bottom) / 2;
                            drop(el);
                            if cx > 0 && cy > 0 && bounds.right > bounds.left {
                                winput::wheel_at(cx as i64, cy as i64, lines)?;
                                Ok(format!(
                                    "已对 {window_id}{path} 中心 ({cx},{cy}) 滚动 {} 行(物理滚轮,{})",
                                    lines.abs(),
                                    if lines > 0 { "向上" } else { "向下" }
                                ))
                            } else {
                                win32_act(hwnd, path, &ControlAction::Scroll { lines })
                            }
                        }
                        // 坐标动作(视觉路线):前台化 + SendInput
                        point @ (ControlAction::ClickPoint { .. }
                        | ControlAction::DoubleClickPoint { .. }
                        | ControlAction::RightClickPoint { .. }
                        | ControlAction::ScrollPoint { .. }) => {
                            drop(el);
                            winput::force_foreground(hwnd)?;
                            win32_act(hwnd, "/", point)
                        }
                        ControlAction::TypeText(text) => {
                            let _ = el.SetFocus();
                            drop(el);
                            winput::force_foreground(hwnd)?;
                            winput::type_text(text)?;
                            Ok(format!(
                                "已向 {window_id} 当前焦点真实键入 {} 字符(SendInput)",
                                text.chars().count()
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

    fn bring_to_front(&self, window_id: &str) -> Result<()> {
        let hwnd = Self::parse_hwnd(window_id)?;
        winput::force_foreground(hwnd)
    }

    fn ocr(
        &self,
        window_id: &str,
        region: Option<Rect>,
        lang: Option<&str>,
    ) -> Result<Vec<OcrBlock>> {
        let hwnd = Self::parse_hwnd(window_id)?;
        // OCR 前把窗口恢复并前台化:最小化窗口截屏无效
        let _ = winput::force_foreground(hwnd);
        super::windows_ocr::ocr_window(hwnd, region, lang)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第 67 轮回归:进程名读取在真实进程(自身进程)上非空。
    /// (此前 GetModuleBaseNameW + LIMITED 权限返回空串。)
    #[test]
    fn process_name_of_current_process_nonempty() {
        let pid = std::process::id();
        let name = process_name_of(pid);
        assert!(!name.is_empty(), "当前进程映像名不应为空");
        assert!(name.to_lowercase().ends_with(".exe"), "应为 .exe: {name}");
    }

    /// 只读冒烟:枚举窗口应有结果(交互桌面);无桌面环境跳过。
    #[test]
    fn list_windows_smoke() {
        let wins = WindowsDriver::new().list_windows(None).unwrap();
        if std::env::var("CI").is_ok() {
            return;
        }
        assert!(!wins.is_empty(), "交互桌面应至少有一个可见窗口");
        for w in &wins {
            assert!(!w.id.is_empty());
        }
    }
}
