//! macOS AX 控件树遍历、节点构建、浅树判定、前台判定。
//!
//! 2026-09-20 第 97 轮:从 macos_legacy.rs 拆分出来的 inspect / 控件树 模块。
//!
//! 主要函数(原单文件第 925~1280 行,逐行搬移):
//! - `parse_window_id` —— 解析 `"{pid}:{index}"` 格式
//! - `window_element` —— 取应用 AX 窗口数组中第 index 个元素
//! - `element_rect` —— 读取元素几何信息(AXPosition + AXSize)
//! - `element_action_names` —— 读取控件真实支持的 AX action
//! - `merge_ax_actions` —— 把 AX action 名称映射为工具动作名
//! - `actions_for_role` —— 按角色推断支持动作
//! - `build_tree` —— 递归构建控件树(深度 + 过滤双裁剪)
//! - `tree_is_shallow` —— 浅树判定(触发 warmup 重试)
//! - `is_frontmost_pid` —— 应用是否已处于前台(AXFrontmost)
//! - `element_at_path` —— 按路径定位元素

#![allow(non_snake_case)]

use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::{CFIndex, CFRelease, CFRetain, CFTypeRef, TCFType};
use core_foundation::boolean::CFBooleanRef;
use core_foundation::string::CFStringRef;

use super::{
    ax_get, ax_get_string, kAXChildrenAttribute, kAXDescriptionAttribute,
    kAXEnhancedUserInterfaceAttribute, kAXFrontmostAttribute, kAXManualAccessibilityAttribute,
    kAXPositionAttribute, kAXRoleAttribute, kAXSizeAttribute, kAXTitleAttribute, kAXValueAttribute,
    kAXWindowsAttribute, platform_err, AXError, AXUIElementCopyActionNames,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetAttributeValue, AXValueGetValue,
    Boolean, CGPoint, CGSize, ControlNode, Rect, K_AX_ERROR_SUCCESS, K_AX_VALUE_CGPOINT_TYPE,
    K_AX_VALUE_CGSIZE_TYPE,
};
use crate::error::Result;

/// 解析窗口 id `"{pid}:{index}"`。
pub(super) fn parse_window_id(window_id: &str) -> Result<(i32, usize)> {
    let (pid_s, idx_s) = window_id.split_once(':').ok_or_else(|| {
        platform_err(
            "macos",
            format!("window_id 格式应为 \"{{pid}}:{{index}}\"(由 MCP_Window_Use(action=list) 返回),实际: {window_id}"),
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
pub(super) unsafe fn window_element(pid: i32, index: usize) -> Result<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(platform_err(
            "macos",
            format!("无法为 pid={pid} 创建 AX 应用元素"),
        ));
    }
    // Electron / 自绘应用(含部分微信版本)常支持 AXManualAccessibility 开关;
    // Qt / wx / 更多自绘框架认 **AXEnhancedUserInterface**(Appium mac2 同款技巧,
    // 第 81 轮新增):置 true 强制应用构建完整无障碍树。两者均 best-effort,
    // 打开失败不影响后续窗口读取;应用异步建树,浅树重试由 inspect 的 warmup 承担。
    let true_v: CFBooleanRef =
        core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef();
    let _ = AXUIElementSetAttributeValue(app, kAXManualAccessibilityAttribute(), true_v.cast());
    let _ = AXUIElementSetAttributeValue(app, kAXEnhancedUserInterfaceAttribute(), true_v.cast());
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
            CFRetain(e);
        }
        e
    } else {
        std::ptr::null()
    };
    CFRelease(wins);
    if el.is_null() {
        return Err(platform_err(
            "macos",
            format!("pid={pid} 第 {index} 个窗口不存在(窗口可能已关闭),请重新 MCP_Window_Use(action=list)"),
        ));
    }
    Ok(el)
}

/// 读取元素的几何信息(AXPosition + AXSize)。
pub(super) unsafe fn element_rect(el: AXUIElementRef) -> Rect {
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

/// 读取控件真实支持的 AX action 名称(2026-09-17 第 80 轮)。
///
/// AX 返回的 CFArray 元素生命周期由数组持有;这里拷贝为 Rust String 后
/// 只释放外层数组,不释放元素。
pub(super) unsafe fn element_action_names(el: AXUIElementRef) -> Vec<String> {
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyActionNames(el, &mut value);
    if err != K_AX_ERROR_SUCCESS || value.is_null() {
        return Vec::new();
    }
    let array = value as CFArrayRef;
    let mut out = Vec::new();
    for i in 0..CFArrayGetCount(array) {
        let action = CFArrayGetValueAtIndex(array, i) as CFStringRef;
        if !action.is_null() {
            let name = super::cfstr(action);
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    CFRelease(value);
    out
}

/// 把真实 AX action 名称映射成 MCP_Window_Use(action=control) 工具动作名,并与 role 推断合并。
pub(super) fn merge_ax_actions(role_actions: Vec<String>, ax_actions: &[String]) -> Vec<String> {
    let mut out = role_actions;
    let has = |v: &Vec<String>, key: &str| v.iter().any(|s| s == key);
    let ax_has = |needle: &str| ax_actions.iter().any(|s| s == needle);
    if !has(&out, "click")
        && (ax_has("AXPress") || ax_has("AXPick") || ax_has("AXConfirm") || ax_has("AXOpen"))
    {
        out.push("click".into());
    }
    if !has(&out, "focus") && ax_has("AXRaise") {
        out.push("focus".into());
    }
    if !has(&out, "scroll_to_visible") && ax_has("AXScrollToVisible") {
        out.push("scroll_to_visible".into());
    }
    out.sort();
    out.dedup();
    out
}

/// 按角色推断支持动作(LLM 检视后据此选择合法操作)。
pub(super) fn actions_for_role(role: &str) -> Vec<String> {
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
pub(super) unsafe fn build_tree(
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
    let bounds = element_rect(el);

    let mut node = ControlNode {
        path: path.clone(),
        role,
        name,
        value,
        bounds,
        actions: Vec::new(),
        help_text: String::new(),
        access_key: String::new(),
        accelerator_key: String::new(),
        is_selected: false,
        children: Vec::new(),
    };
    let role_actions = actions_for_role(&node.role);
    let ax_actions = element_action_names(el);
    node.actions = merge_ax_actions(role_actions, &ax_actions);

    if depth < max_depth {
        let children = ax_get(el, kAXChildrenAttribute());
        if !children.is_null() {
            let count = CFArrayGetCount(children as CFArrayRef);
            for i in 0..count {
                let child = CFArrayGetValueAtIndex(children as CFArrayRef, i) as AXUIElementRef;
                if child.is_null() {
                    continue;
                }
                if let Some(cn) = build_tree(
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
    let self_hit =
        super::matches_filter(&node.name, filter) || super::matches_filter(&node.role, filter);
    if filter.is_none() || self_hit || !node.children.is_empty() {
        Some(node)
    } else {
        None
    }
}

/// 第 81 轮:控件树是否「浅」—— 需要等待异步建树重试的判定:
/// 1. 总节点数 ≤ 4(微信实测空壳 = 根 AXWindow + 3 个红绿灯 AXButton);
/// 2. 或除纯容器(Window/Pane/Group/ScrollArea)外没有任何内容控件。
/// Electron / Qt / 微信等自绘 App 在 AXEnhancedUserInterface 置位后需要
/// 数百毫秒才把完整树搭出来,首读常为浅树。
///
/// 注意:macOS 角色带 `AX` 前缀(AXButton),判定前先剥离再做容器名比对。
pub(super) fn tree_is_shallow(root: &ControlNode) -> bool {
    const CONTAINERS: &[&str] = &[
        "window",
        "pane",
        "group",
        "scrollarea",
        "application",
        "layoutarea",
        "splitgroup",
        "splitter",
        "tabgroup",
        "unknown",
        "",
    ];
    fn is_pure_container(role: &str) -> bool {
        let r = role.trim().to_ascii_lowercase();
        let r = r.strip_prefix("ax").unwrap_or(&r);
        CONTAINERS.contains(&r)
    }
    fn count(node: &ControlNode) -> usize {
        1 + node.children.iter().map(count).sum::<usize>()
    }
    fn has_content(node: &ControlNode) -> bool {
        if !is_pure_container(&node.role) {
            return true;
        }
        node.children.iter().any(has_content)
    }
    count(root) <= 4 || !has_content(root)
}

/// 第 81 轮:应用是否已处于前台(读应用级 AXFrontmost;失败回退 false)。
///
/// kAXFrontmostAttribute 返回 kCFBooleanTrue/False,直接做指针等值比较,
/// 不引入新 FFI;属性读取失败(权限/不支持)一律按 false 处理 ——
/// 后果只是多做一次幂等激活,与旧行为一致(安全降级)。
pub(super) fn is_frontmost_pid(pid: i32) -> bool {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        let v = ax_get(app, kAXFrontmostAttribute());
        CFRelease(app);
        if v.is_null() {
            return false;
        }
        let true_ref = core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef()
            as *const std::ffi::c_void;
        let is_true = v == true_ref;
        CFRelease(v);
        is_true
    }
}

/// 按路径定位元素(返回 retained ref,调用方负责 CFRelease)。
pub(super) unsafe fn element_at_path(root: AXUIElementRef, path: &str) -> Result<AXUIElementRef> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed.is_empty() {
        CFRetain(root);
        return Ok(root);
    }
    let mut cur = root;
    CFRetain(cur);
    for seg in trimmed.trim_start_matches('/').split('/') {
        let idx: usize = seg
            .parse()
            .map_err(|_| platform_err("macos", format!("控件路径段非法: {seg}(应为子控件下标)")))?;
        let children = ax_get(cur, kAXChildrenAttribute());
        CFRelease(cur);
        if children.is_null() {
            return Err(platform_err(
                "macos",
                format!("路径 {path} 在段 {seg} 处中断:父控件无子节点,请重新 MCP_Window_Use(action=inspect) 获取最新路径"),
            ));
        }
        let count = CFArrayGetCount(children as CFArrayRef);
        let next = if (idx as CFIndex) < count {
            let e =
                CFArrayGetValueAtIndex(children as CFArrayRef, idx as CFIndex) as AXUIElementRef;
            if !e.is_null() {
                CFRetain(e);
            }
            e
        } else {
            std::ptr::null()
        };
        CFRelease(children);
        if next.is_null() {
            return Err(platform_err(
                "macos",
                format!("路径 {path} 下标 {idx} 越界(UI 可能已变化),请重新 MCP_Window_Use(action=inspect)"),
            ));
        }
        cur = next;
    }
    Ok(cur)
}

// 抑制 unused import 警告 —— Boolean / K_AX_ERROR_SUCCESS 间接通过 ax_get 等函数使用。
#[allow(dead_code)]
fn _suppress_unused(_b: Boolean, _e: AXError) {}
