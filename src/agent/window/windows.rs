//! Windows 窗口驱动:UI Automation 为主,SendInput 物理输入 / Win32 消息降级。
//!
//! - 窗口枚举:`EnumWindows` + `IsWindowVisible` + 非空标题,id = HWND 十进制字符串;
//!   进程名走 `QueryFullProcessImageNameW`(LIMITED 权限可用;第 67 轮修复
//!   `GetModuleBaseNameW` 拿不到进程名的实测 bug);
//! - 控件遍历:UIA(`CoCreateInstance(CUIAutomation)` → `ElementFromHandle` →
//!   RawViewWalker 深度遍历),覆盖 Win32 原生 / WPF / Qt(带 UIA Provider)等;
//!   UIA 初始化失败时降级 `GetWindow(GW_CHILD/HWNDNEXT)` 递归枚举子 HWND;
//! - 操作决策链(2026-09-19 第 90 轮重构为四层优先级:**无障碍 UIA → Win32 消息 →
//!   SendInput 物理输入兜底**;设计见 `docs/MCP_Window_Use/02-鼠标键盘操控与优先级链方案.md`):
//!   - click:UIA Invoke/Toggle/ExpandCollapse/SelectionItem(T1)→ BM_CLICK(原生 Button 系
//!     HWND,T2)→ 元素中心物理点击(T3);
//!   - set_text:UIA ValuePattern(T1)→ WM_SETTEXT(原生 HWND,T2)→ focus+SendInput 逐字键入(T3);
//!   - get_text:Value/Name(T1)→ WM_GETTEXT(原生 HWND,T2);
//!   - send_keys:PostMessage WM_KEYDOWN+WM_CHAR+WM_KEYUP(仅原生 HWND,T2)→ SendInput(T3);
//!   - scroll:UIA ScrollPattern(T1)→ WM_MOUSEWHEEL(T2)→ 物理滚轮(T3);
//!   - 坐标动作(ClickPoint/ScrollPoint/TypeText/MovePoint/MiddleClick/DragPoint):T3 SendInput
//!     底座(第 90 轮:支持 modifiers 修饰键点击 + 插值拖拽);
//!   - 每步返回文案带 route= 标注(uia / win32_msg / physical),QC/Debug 可对账;
//! - OCR:`windows_ocr::ocr_window`(GDI 截图 + Windows.Media.Ocr);
//! - `bring_to_front`:`windows_input::force_foreground`(恢复最小化 + 三保险激活)。
//!
//! 设计见 `docs/MCP_Window_Use/01-设计与解决方案.md` §2.3 Windows 后端。

use windows::core::BSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
    IUIAutomationInvokePattern, IUIAutomationScrollItemPattern, IUIAutomationScrollPattern,
    IUIAutomationSelectionItemPattern, IUIAutomationSelectionPattern, IUIAutomationTextPattern,
    IUIAutomationTogglePattern, IUIAutomationValuePattern,
    ScrollAmount_NoAmount, ScrollAmount_SmallDecrement, ScrollAmount_SmallIncrement,
    UIA_ExpandCollapsePatternId, UIA_InvokePatternId, UIA_ScrollItemPatternId, UIA_ScrollPatternId,
    UIA_SelectionItemPatternId, UIA_SelectionPatternId, UIA_TextPatternId, UIA_TogglePatternId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, SendMessageW, SetForegroundWindow, BM_CLICK, GW_CHILD, GW_HWNDNEXT, WM_CHAR,
    WM_GETTEXT, WM_GETTEXTLENGTH, WM_KEYDOWN, WM_KEYUP, WM_SETTEXT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, VIRTUAL_KEY, MAPVK_VK_TO_VSC};

use super::windows_input as winput;
use super::{
    matches_filter, platform_err, ControlAction, ControlNode, OcrBlock, Rect, WindowDriver,
    WindowInfo,
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
        winput::ensure_dpi_aware();
        Self
    }

    pub(crate) fn parse_hwnd(window_id: &str) -> Result<HWND> {
        let raw: isize = window_id.trim().parse().map_err(|_| {
            platform_err(
                "windows",
                format!("window_id 应为 MCP_Window_Use(action=list) 返回的 HWND 十进制字符串,实际: {window_id}"),
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
/// 导致 MCP_Window_Use(action=find) 无法按进程名(Weixin)匹配。
fn process_name_of(pid: u32) -> String {
    // SAFETY:标准进程查询;句柄本函数内释放。
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
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
    let raw = unsafe { el.CurrentControlType() }.map(|t| t.0).unwrap_or(0);
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
        // 第 90 轮:Toggle(复选框/开关)/ ExpandCollapse(下拉/树节点)/
        // SelectionItem(列表项)都具备语义化「点击」能力,标注进 actions 供 LLM 决策。
        if el
            .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            .is_ok()
        {
            v.push("click".into());
        }
        if el
            .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId)
            .is_ok()
        {
            v.push("click".into());
        }
        if el
            .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
            .is_ok()
        {
            v.push("click".into());
        }
    }
    v
}

// ===================== 第 90 轮:T2 消息层辅助(优先级链中间层) =====================

/// UIA 元素的原生 HWND(非空 = 标准 Win32 控件,WM_* 消息路线可用;
/// 空/读取失败 = 自绘/虚拟元素,只能走 T1 Pattern 或 T3 物理输入)。
fn uia_native_hwnd(el: &IUIAutomationElement) -> Option<HWND> {
    // SAFETY:属性读取,失败(自绘 UI)返回 None。
    let h = unsafe { el.CurrentNativeWindowHandle() }.ok()?;
    if h.0.is_null() {
        None
    } else {
        Some(h)
    }
}

/// HWND 的窗口类名(空串 = 获取失败)。
fn class_name_of(hwnd: HWND) -> String {
    // SAFETY:标准类名查询,无副作用。
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 是否为标准 Button 系类(标准按钮/复选框/单选按钮同为 "Button" 类,BM_CLICK 语义成立)。
fn is_button_class(class: &str) -> bool {
    class.eq_ignore_ascii_case("Button")
}

/// 消息级单键投递(WM_KEYDOWN / WM_CHAR / WM_KEYUP)。
///
/// lParam 布局:0-15 重复数 1;16-23 扫描码(MapVirtualKeyW);30 上次状态;
/// 31 转换状态(keyup 置 30|31)。异步 PostMessage,不要求目标响应、不阻塞。
///
/// # Safety
/// `hwnd` 必须有效;`vk` 必须是合法虚拟键码。
unsafe fn post_key_msg(hwnd: HWND, vk: VIRTUAL_KEY, down: bool) {
    let scan = MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) as isize;
    let mut lparam = 1 | (scan << 16);
    if !down {
        lparam |= 1 << 30 | 1 << 31; // 上次按下 + 状态转换
    }
    let msg = if down { WM_KEYDOWN } else { WM_KEYUP };
    let _ = PostMessageW(hwnd, msg, WPARAM(vk.0 as usize), LPARAM(lparam));
}

/// 消息级按键规格投递(2026-09-19 第 90 轮,T2 层):
/// 修饰键逐个 WM_KEYDOWN → 主键 WM_KEYDOWN(+可打印字符补 WM_CHAR)→ WM_KEYUP →
/// 修饰键逆序 WM_KEYUP。全部 PostMessage 异步投递,不抢前台焦点。
///
/// 仅对**原生 HWND 控件**启用(标准 Win32 应用);Electron/自绘根窗口(无子 HWND)
/// 保持物理 SendInput 路线,零回归。消息注入对部分应用可能无效且无法感知失败,
/// 返回文案带 route=win32_msg 标注,QC/Debug 可对账。
fn post_message_keys(hwnd: HWND, spec: &str) -> Result<()> {
    let (modifiers, main) = winput::parse_key_spec(spec)?;
    // SAFETY:hwnd 由 uia_native_hwnd 产出(元素原生 HWND);VK 均为合法常量。
    unsafe {
        for m in &modifiers {
            post_key_msg(hwnd, *m, true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        post_key_msg(hwnd, main, true);
        // 可打印 ASCII 主键补 WM_CHAR(字母/数字;ctrl 组合键/命名键不补 ——
        // Ctrl+X 语义由目标控件按 WM_KEYDOWN 自行解释)
        if modifiers.is_empty() {
            let c = main.0 as u8 as char;
            if main.0 >= 0x30 && main.0 <= 0x5A && c.is_ascii_alphanumeric() {
                let ch = c.to_ascii_lowercase();
                let _ = PostMessageW(hwnd, WM_CHAR, WPARAM(ch as usize), LPARAM(1));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        post_key_msg(hwnd, main, false);
        for m in modifiers.iter().rev() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            post_key_msg(hwnd, *m, false);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    Ok(())
}

/// 修饰键规格 → SendInput VK 序列(空规格 → 空序列)。
fn modifier_vks(spec: Option<&str>) -> Result<Vec<VIRTUAL_KEY>> {
    match spec.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => winput::parse_modifiers(s),
        None => Ok(Vec::new()),
    }
}

/// UIA 递归构建控件树。
///
/// # Safety
/// `el` 必须是有效的 UIA 元素;walker 遍历失败即剪枝。
///
/// 2026-09-20 第 98 轮 G9:用 `ControlViewWalker` + `ControlViewCondition` 过滤装饰层
/// (Group/Pane/TitleBar 等非交互元素),LLM 看到的树直接命中可交互控件;条件获取
/// 失败时兜底 `RawViewWalker` 保证零回归。
/// 2026-09-20 第 98 轮 G7:填充 `help_text` / `access_key` / `accelerator_key` /
/// `is_selected` 四个 UIA 辅助功能属性。
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
    // 2026-09-20 第 98 轮 G7:UIA 辅助功能属性填充(空值 skip_serializing_if 不出现)。
    let help_text = el.CurrentHelpText().map(|b| b.to_string()).unwrap_or_default();
    let access_key = el.CurrentAccessKey().map(|b| b.to_string()).unwrap_or_default();
    let accelerator_key = el
        .CurrentAcceleratorKey()
        .map(|b| b.to_string())
        .unwrap_or_default();
    let is_selected = el
        .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
        .and_then(|p| p.CurrentIsSelected())
        .map(|b| b.as_bool())
        .unwrap_or(false);

    let mut node = ControlNode {
        path: path.clone(),
        role,
        name,
        value,
        bounds,
        actions: uia_actions(el),
        help_text,
        access_key,
        accelerator_key,
        is_selected,
        children: Vec::new(),
    };

    if depth < max_depth {
        // 第 98 轮 G9:ControlViewWalker 过滤装饰层;失败兜底 RawViewWalker。
        let walker_result = uia
            .ControlViewCondition()
            .ok()
            .and_then(|cond| uia.CreateTreeWalker(&cond).ok())
            .map_or_else(|| uia.RawViewWalker(), Ok);
        if let Ok(walker) = walker_result {
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
///
/// 2026-09-20 第 98 轮 G9:`uia_element_at_path` 必须与 `uia_build_tree` 使用同一
/// Walker(ControlViewWalker),否则 inspect 返回的 path 在 act 时定位会落空;
/// 失败兜底 RawViewWalker。
unsafe fn uia_element_at_path(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    path: &str,
) -> Result<IUIAutomationElement> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed.is_empty() {
        return Ok(root.clone());
    }
    let cond = uia
        .ControlViewCondition()
        .map_err(|e| platform_err("windows", format!("UIA ControlViewCondition 获取失败: {e}")))?;
    let walker = uia
        .CreateTreeWalker(&cond)
        .or_else(|_| uia.RawViewWalker())
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
                format!("路径 {path} 下标 {idx} 不存在(UI 可能已变化),请重新 MCP_Window_Use(action=inspect)"),
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
        // 第 98 轮 G7:Win32 路径无 UIA 辅助功能属性,取空/false(skip_serializing_if 不出现)。
        help_text: String::new(),
        access_key: String::new(),
        accelerator_key: String::new(),
        is_selected: false,
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
                format!("路径 {path} 下标 {idx} 不存在(UI 可能已变化),请重新 MCP_Window_Use(action=inspect)"),
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
            Ok(format!("已向 HWND {:?} 发送 BM_CLICK(route=win32_msg)", hwnd.0))
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
                "已向 HWND {:?} 写入文本({} 字符,route=win32_msg)",
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
            Ok(format!(
                "已向 HWND {:?} 真实键入 {} 字符",
                hwnd.0,
                text.chars().count()
            ))
        }
        ControlAction::TypeTextSubmit { text, x, y } => {
            if let (Some(x), Some(y)) = (x, y) {
                winput::click_point(*x, *y, false, false)?;
            } else {
                winput::force_foreground(hwnd)?;
            }
            winput::type_text(text)?;
            // 给 Win32 消息队列/Electron 输入模型一个固定吸收窗口,再提交。
            std::thread::sleep(std::time::Duration::from_millis(120));
            winput::send_keys_spec("enter")?;
            Ok(format!(
                "已键入 {} 字符并提交(type_text_submit)",
                text.chars().count()
            ))
        }
        // 2026-09-16 第 66 轮:WM_MOUSEWHEEL 滚动(目标控件 HWND 优先,未命中发窗口根)。
        ControlAction::Scroll { lines } => {
            const WM_MOUSEWHEEL: u32 = 0x020A;
            let delta = lines.saturating_mul(120);
            // wParam 高 16 位 = 带符号 delta(正=向上/远离用户)
            let wparam = WPARAM(((delta as u16) as usize) << 16);
            SendMessageW(hwnd, WM_MOUSEWHEEL, wparam, LPARAM(0));
            Ok(format!(
                "已向 HWND {:?} 发送 WM_MOUSEWHEEL({} 行,{},route=win32_msg)",
                hwnd.0,
                lines.abs(),
                if *lines > 0 { "向上" } else { "向下" }
            ))
        }
        ControlAction::ScrollToVisible => Err(platform_err(
            "windows",
            "Win32 降级路径不支持 scroll_to_visible(UIA 可用时走 ScrollItemPattern;或改用 scroll)",
        )),
        // 坐标动作与 UIA 无关,直接走 SendInput(第 90 轮:含 modifiers / 新原语)
        ControlAction::ClickPoint { x, y, modifiers } => {
            let mods = modifier_vks(modifiers.as_deref())?;
            winput::click_point_ex(*x, *y, winput::MouseButton::Left, 1, &mods)?;
            Ok(format!(
                "已在 ({x},{y}) 执行物理左键单击{}(route=physical)",
                modifiers
                    .as_deref()
                    .map(|m| format!(" + 按住 {m}"))
                    .unwrap_or_default()
            ))
        }
        ControlAction::DoubleClickPoint { x, y, modifiers } => {
            let mods = modifier_vks(modifiers.as_deref())?;
            winput::click_point_ex(*x, *y, winput::MouseButton::Left, 2, &mods)?;
            Ok(format!(
                "已在 ({x},{y}) 执行物理双击{}(route=physical)",
                modifiers
                    .as_deref()
                    .map(|m| format!(" + 按住 {m}"))
                    .unwrap_or_default()
            ))
        }
        ControlAction::RightClickPoint { x, y, modifiers } => {
            let mods = modifier_vks(modifiers.as_deref())?;
            winput::click_point_ex(*x, *y, winput::MouseButton::Right, 1, &mods)?;
            Ok(format!(
                "已在 ({x},{y}) 执行物理右键单击{}(route=physical)",
                modifiers
                    .as_deref()
                    .map(|m| format!(" + 按住 {m}"))
                    .unwrap_or_default()
            ))
        }
        ControlAction::ScrollPoint { x, y, lines } => {
            winput::wheel_at(*x, *y, *lines)?;
            Ok(format!(
                "已在 ({x},{y}) 滚动 {} 行({})(route=physical)",
                lines.abs(),
                if *lines > 0 { "向上" } else { "向下" }
            ))
        }
        // ===== 第 90 轮:鼠标原子能力(悬停 / 中键 / 拖拽) =====
        ControlAction::MovePoint { x, y } => {
            winput::move_cursor(*x, *y)?;
            Ok(format!("已把光标移动到 ({x},{y})(悬停,route=physical)"))
        }
        ControlAction::MiddleClickPoint { x, y, modifiers } => {
            let mods = modifier_vks(modifiers.as_deref())?;
            winput::click_point_ex(*x, *y, winput::MouseButton::Middle, 1, &mods)?;
            Ok(format!(
                "已在 ({x},{y}) 执行物理中键单击{}(route=physical)",
                modifiers
                    .as_deref()
                    .map(|m| format!(" + 按住 {m}"))
                    .unwrap_or_default()
            ))
        }
        ControlAction::DragPoint {
            x,
            y,
            x2,
            y2,
            modifiers,
        } => {
            let mods = modifier_vks(modifiers.as_deref())?;
            winput::drag_point(*x, *y, *x2, *y2, &mods)?;
            Ok(format!(
                "已从 ({x},{y}) 拖拽到 ({x2},{y2}){}(route=physical)",
                modifiers
                    .as_deref()
                    .map(|m| format!(" + 按住 {m}"))
                    .unwrap_or_default()
            ))
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
                                     改走视觉路线:MCP_Window_Use(action=ocr)(window_id) 拿文本坐标 → MCP_Window_Use(action=control) click_point/type_text"
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
                                 建议改用上表同义词,或改走视觉路线 MCP_Window_Use(action=ocr) + click_point"
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
                            // 第 90 轮四层优先级链:T1 UIA Pattern(Invoke/Toggle/
                            // ExpandCollapse/SelectionItem,语义化点击)→ T2 BM_CLICK
                            // (原生 Button 系 HWND)→ T3 元素中心物理点击(自绘 UI 兜底)。
                            // SAFETY:模式查询与调用均为 UIA COM;失败逐层降级。
                            {
                                // T1a Invoke(按钮/链接)
                                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(
                                    UIA_InvokePatternId,
                                ) {
                                    if p.Invoke().is_ok() {
                                        return Ok(format!(
                                            "已对 {window_id}{path} 执行 Invoke(点击,route=uia)"
                                        ));
                                    }
                                }
                                // T1b Toggle(复选框/开关:点击语义 = 切换状态)
                                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(
                                    UIA_TogglePatternId,
                                ) {
                                    if p.Toggle().is_ok() {
                                        return Ok(format!(
                                            "已对 {window_id}{path} 执行 Toggle(点击,route=uia)"
                                        ));
                                    }
                                }
                                // T1c ExpandCollapse(下拉框/树节点:点击语义 = 展开)
                                if let Ok(p) = el
                                    .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                                    UIA_ExpandCollapsePatternId,
                                ) {
                                    if p.Expand().is_ok() {
                                        return Ok(format!(
                                            "已对 {window_id}{path} 执行 Expand(点击,route=uia)"
                                        ));
                                    }
                                }
                                // T1d SelectionItem(列表项:点击语义 = 选中)
                                if let Ok(p) = el
                                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                                    UIA_SelectionItemPatternId,
                                ) {
                                    if p.Select().is_ok() {
                                        return Ok(format!(
                                            "已对 {window_id}{path} 执行 Select(点击,route=uia)"
                                        ));
                                    }
                                }
                                // T2 BM_CLICK(标准 Button 系 HWND;同步消息,仅对语义成立的类)
                                if let Some(nh) = uia_native_hwnd(&el) {
                                    if is_button_class(&class_name_of(nh)) {
                                        SendMessageW(nh, BM_CLICK, WPARAM(0), LPARAM(0));
                                        return Ok(format!(
                                            "已向 {window_id}{path} 原生 HWND 发送 BM_CLICK(route=win32_msg)"
                                        ));
                                    }
                                }
                                // T3 物理点击(自绘 UI 唯一可靠路径,第 67 轮行为保留)
                                let bounds = el.CurrentBoundingRectangle().unwrap_or_default();
                                let cx = (bounds.left + bounds.right) / 2;
                                let cy = (bounds.top + bounds.bottom) / 2;
                                if cx > 0 && cy > 0 && bounds.right > bounds.left {
                                    let _ = el.SetFocus();
                                    winput::force_foreground(hwnd)?;
                                    winput::click_point(cx as i64, cy as i64, false, false)?;
                                    Ok(format!(
                                        "已对 {window_id}{path} 中心 ({cx},{cy}) 执行物理点击(route=physical)"
                                    ))
                                } else {
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
                            // 第 90 轮三层链:T1 ValuePattern → T2 WM_SETTEXT(原生 HWND)
                            // → T3 focus+SendInput 逐字键入(自绘输入框唯一可靠路径)。
                            // SAFETY:UIA/Win32 消息调用;失败逐层降级。
                            {
                                // T1 UIA ValuePattern(语义级,不抢焦点)
                                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(
                                    UIA_ValuePatternId,
                                ) {
                                    if p.SetValue(&BSTR::from(text.as_str())).is_ok() {
                                        return Ok(format!(
                                            "已向 {window_id}{path} 写入文本({} 字符,route=uia)",
                                            text.chars().count()
                                        ));
                                    }
                                }
                                // T2 WM_SETTEXT(标准 Win32 控件;整体替换语义)
                                if let Some(nh) = uia_native_hwnd(&el) {
                                    let mut wide: Vec<u16> = text.encode_utf16().collect();
                                    wide.push(0);
                                    SendMessageW(
                                        nh,
                                        WM_SETTEXT,
                                        WPARAM(0),
                                        LPARAM(wide.as_ptr() as isize),
                                    );
                                    return Ok(format!(
                                        "已向 {window_id}{path} 原生 HWND 写入文本({} 字符,route=win32_msg)",
                                        text.chars().count()
                                    ));
                                }
                                // T3 物理:焦点 + SendInput 逐字键入(第 67 轮路径保留)
                                let _ = el.SetFocus();
                                winput::force_foreground(hwnd)?;
                                winput::type_text(text)?;
                                Ok(format!(
                                    "已向 {window_id}{path} 真实键入文本({} 字符,route=physical)",
                                    text.chars().count()
                                ))
                            }
                        }
                        ControlAction::GetText => {
                            // 第 98 轮 G5 + G6:四层优先级链
                            //   T1a ValuePattern(表单控件)→ T1b Name 属性 →
                            //   T1c TextPattern(只读文档区:Word/Excel/VS Code/记事本等)→
                            //   T1d SelectionPattern(ComboBox/List 当前选中项)→
                            //   T2 WM_GETTEXT(原生 HWND)→ 失败时结构化错误引导(G12)。
                            // T1a ValuePattern
                            if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(
                                UIA_ValuePatternId,
                            ) {
                                let v = p.CurrentValue().map(|b| b.to_string()).unwrap_or_default();
                                if !v.is_empty() {
                                    return Ok(v);
                                }
                            }
                            // T1b Name 属性
                            if let Ok(name) = el.CurrentName() {
                                let n = name.to_string();
                                if !n.is_empty() {
                                    return Ok(n);
                                }
                            }
                            // T1c TextPattern(第 98 轮 G5:只读文档区 route=text_pattern)
                            // SAFETY:UIA COM 指针;DocumentRange / GetText 返回 BSTR,空指针由 windows crate 包装成 Err。
                            {
                                if let Ok(tp) = el
                                    .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                                {
                                    if let Ok(range) = tp.DocumentRange() {
                                        if let Ok(text_bstr) = range.GetText(-1) {
                                            let t = text_bstr.to_string();
                                            if !t.is_empty() {
                                                // 长文档截断:>8000 字符截到 8000 + 提示(防撑爆上下文)
                                                let char_count = t.chars().count();
                                                let display = if char_count > 8000 {
                                                    let truncated: String =
                                                        t.chars().take(8000).collect();
                                                    format!(
                                                        "{truncated}\n[截断:长文档已截断到 8000 字符,完整内容可用 MCP_Window_Use(action=ocr) 读全文]"
                                                    )
                                                } else {
                                                    t
                                                };
                                                return Ok(format!(
                                                    "{display}\n(route=text_pattern)"
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            // T1d SelectionPattern(第 98 轮 G6:ComboBox/List 当前选中项 route=selection_pattern)
                            // SAFETY:UIA COM 指针;失败时自动跳过,继续走 T2。
                            {
                                if let Ok(sp) = el.GetCurrentPatternAs::<IUIAutomationSelectionPattern>(
                                    UIA_SelectionPatternId,
                                ) {
                                    if let Ok(sel) = sp.GetCurrentSelection() {
                                        if let Ok(first) = sel.GetElement(0) {
                                            if let Ok(name) = first.CurrentName() {
                                                let n = name.to_string();
                                                if !n.is_empty() {
                                                    return Ok(format!(
                                                        "{n}\n(route=selection_pattern)"
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // T2 WM_GETTEXT(原生 HWND 同步消息;第 90 轮补)
                            // SAFETY:标准 WM_GETTEXT 同步消息;缓冲区指针本栈有效。
                            {
                                if let Some(nh) = uia_native_hwnd(&el) {
                                    let len = SendMessageW(
                                        nh,
                                        WM_GETTEXTLENGTH,
                                        WPARAM(0),
                                        LPARAM(0),
                                    )
                                    .0 as usize;
                                    if len > 0 && len < 1_000_000 {
                                        let mut buf = vec![0u16; len + 1];
                                        let got = SendMessageW(
                                            nh,
                                            WM_GETTEXT,
                                            WPARAM(buf.len()),
                                            LPARAM(buf.as_mut_ptr() as isize),
                                        )
                                        .0 as usize;
                                        let t = String::from_utf16_lossy(&buf[..got.min(len)]);
                                        if !t.is_empty() {
                                            return Ok(t);
                                        }
                                    }
                                }
                            }
                            // 第 98 轮 G12:失败时结构化错误引导(LLM 可读 + next_action 提示)
                            Err(platform_err(
                                "windows",
                                format!(
                                    "读取控件文本失败:控件 {window_id}{path} 无 ValuePattern/Name/TextPattern/SelectionPattern 且原生 HWND 无 WM_GETTEXT。\
                                     【下一步】(1) 若为只读文档区(Word/Excel/VS Code/记事本),可重新 inspect 加大 max_depth(6-8)让 value 自动捕获;\
                                     (2) 若为自绘 UI(控件树仅有少量 Pane),改走 MCP_Window_Use(action=ocr)(window_id) + control_action=click_point;\
                                     (3) 若需读 ListView 选中行,先 control_action=click 触发选中再重试 get_text。\
                                     route=exhausted;control_action=get_text"
                                ),
                            ))
                        }
                        ControlAction::SendKeys(spec) => {
                            // 第 90 轮两层链:T2 PostMessage 消息级按键(仅原生 HWND 控件,
                            // 不抢焦点)→ T3 物理 SendInput(Electron/自绘 UI 路径,第 67 轮保留)。
                            // SAFETY:消息投递目标为元素原生 HWND。
                            {
                                if let Some(nh) = uia_native_hwnd(&el) {
                                    drop(el);
                                    post_message_keys(nh, spec)?;
                                    return Ok(format!(
                                        "已向 {window_id}{path} 投递按键 {spec:?}(route=win32_msg)"
                                    ));
                                }
                                let _ = el.SetFocus();
                                drop(el);
                                winput::force_foreground(hwnd)?;
                                winput::send_keys_spec(spec)?;
                                Ok(format!(
                                    "已向 {window_id} 发送按键 {spec:?}(route=physical)"
                                ))
                            }
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
                                    Ok(format!(
                                        "已把 {window_id}{path} 滚动到可见区域(ScrollItemPattern)"
                                    ))
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
                        // 第 90 轮滚动三层链:T1 UIA ScrollPattern(语义级)→
                        // T2 WM_MOUSEWHEEL(原生 HWND)→ T3 物理滚轮(自绘 UI 兜底,
                        // 第 67 轮「自绘 UI 忽略消息滚动」的行为经前两层自然落穿保留)。
                        ControlAction::Scroll { lines } => {
                            let lines = *lines;
                            // SAFETY:UIA Pattern 调用;失败逐层降级。
                            {
                                // T1 ScrollPattern:Small 步进 × min(|lines|,3) 次(有界 IPC)
                                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationScrollPattern>(
                                    UIA_ScrollPatternId,
                                ) {
                                    let amount = if lines > 0 {
                                        ScrollAmount_SmallDecrement
                                    } else {
                                        ScrollAmount_SmallIncrement
                                    };
                                    let mut ok_calls = 0u32;
                                    for _ in 0..lines.unsigned_abs().min(3) {
                                        if p.Scroll(ScrollAmount_NoAmount, amount).is_ok() {
                                            ok_calls += 1;
                                        } else {
                                            break;
                                        }
                                    }
                                    if ok_calls > 0 {
                                        return Ok(format!(
                                            "已对 {window_id}{path} 滚动(UIA ScrollPattern Small×{ok_calls},请求 {} 行,{},route=uia)",
                                            lines.abs(),
                                            if lines > 0 { "向上" } else { "向下" }
                                        ));
                                    }
                                }
                                // T2 WM_MOUSEWHEEL(原生 HWND)
                                if let Some(nh) = uia_native_hwnd(&el) {
                                    const WM_MOUSEWHEEL: u32 = 0x020A;
                                    let delta = lines.saturating_mul(120);
                                    let wparam = WPARAM(((delta as u16) as usize) << 16);
                                    SendMessageW(nh, WM_MOUSEWHEEL, wparam, LPARAM(0));
                                    return Ok(format!(
                                        "已向 {window_id}{path} 原生 HWND 发送 WM_MOUSEWHEEL({} 行,{},route=win32_msg)",
                                        lines.abs(),
                                        if lines > 0 { "向上" } else { "向下" }
                                    ));
                                }
                                // T3 物理滚轮
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
                        }
                        // 坐标动作(视觉路线):前台化 + SendInput
                        // 第 90 轮:纳入 move_point(悬停,免前台化)/
                        // middle_click_point / drag_point 新原语,全部 route=physical。
                        point @ (ControlAction::ClickPoint { .. }
                        | ControlAction::DoubleClickPoint { .. }
                        | ControlAction::RightClickPoint { .. }
                        | ControlAction::ScrollPoint { .. }
                        | ControlAction::MiddleClickPoint { .. }
                        | ControlAction::DragPoint { .. }) => {
                            drop(el);
                            winput::force_foreground(hwnd)?;
                            win32_act(hwnd, "/", point)
                        }
                        // move_point 仅移动光标,不需要前台化(悬停语义)
                        ControlAction::MovePoint { .. } => {
                            drop(el);
                            win32_act(hwnd, "/", &action)
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
                        ControlAction::TypeTextSubmit { text, x, y } => {
                            let clicked = if let (Some(x), Some(y)) = (x, y) {
                                winput::click_point(*x, *y, false, false)?;
                                true
                            } else {
                                let _ = el.SetFocus();
                                false
                            };
                            drop(el);
                            winput::force_foreground(hwnd)?;
                            winput::type_text(text)?;
                            std::thread::sleep(std::time::Duration::from_millis(120));
                            winput::send_keys_spec("enter")?;
                            Ok(format!(
                                "已向 {window_id} 键入 {} 字符并提交(SendInput{})",
                                text.chars().count(),
                                if clicked { ", 坐标定位" } else { "" }
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

    // 第 81 轮:已前台 → 跳过激活(MCP_Window_Use(action=open) 幂等前置,消除窗口反复闪烁)。
    fn is_frontmost(&self, window_id: &str) -> bool {
        match Self::parse_hwnd(window_id) {
            Ok(hwnd) => {
                // SAFETY:标准 GetForegroundWindow 查询,无副作用。
                unsafe { GetForegroundWindow() == hwnd }
            }
            Err(_) => false,
        }
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
