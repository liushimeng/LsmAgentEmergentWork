//! MCP_Window_Use 驱动层 macOS 后端(axuielement 版,第 58 轮 P2-B)。
//!
//! 设计目标:
//! - 替换 `macos_legacy.rs` 中手写的 `core-foundation` FFI(700 行 + 5 个 `extern "C"` 块);
//! - 使用 `axuielement = "0.9"`(pure-Rust 绑定 Accessibility framework,活跃维护,2026-06 更新);
//! - 保持与 legacy 完全相同的 `WindowDriver` trait 行为契约(返回字段 / 错误码 / 权限提示);
//! - 默认启用,旧手写 FFI 保留为 `LAEW_MACOS_DRIVER=legacy` 回退路径。
//!
//! 实现要点:
//! - 窗口枚举仍走 `CGWindowListCopyWindowInfo`(axuielement 不提供等价 API),
//!   直接复用 `macos_legacy::list_windows_cg`,保证 process_name / pid / bounds 字段完全一致;
//! - 控件树遍历与操作改走 `axuielement::AXUIElement` 的 typed accessor(`element_array_attribute` /
//!   `string_attribute` / `rect_attribute` / `set_attribute` / `perform_action`),
//!   不再手写 CFTypeRef cast,代码量减半;
//! - 错误码透传:AXUIElement API 失败时统一映射到 `AgentError::ToolExecution`,
//!   -25211(kAXErrorAPIDisabled)等关键错误码在文案中保留。

use crate::agent::window::macos_legacy::list_windows_cg;
use crate::agent::window::{
    matches_filter, platform_err, ControlAction, ControlNode, Rect, WindowDriver, WindowInfo,
};
use crate::error::Result;

/// axuielement 版 macOS 驱动(2026-09-16 第 58 轮 P2-B 新增)。
///
/// 与 legacy 行为对齐:
/// - list_windows 走 CGWindowListCopyWindowInfo(macos_legacy::list_windows_cg),
///   保证 process_name / pid / bounds 字段与 legacy 完全一致;
/// - inspect / act 改走 axuielement typed API,简化 FFI;
/// - permission_hint:辅助功能未授权 → -25211 kAXErrorAPIDisabled + 开权限步骤引导。
pub struct MacosAxuiDriver;

impl Default for MacosAxuiDriver {
    fn default() -> Self {
        Self
    }
}

impl MacosAxuiDriver {
    pub fn new() -> Self {
        Self
    }
}

impl WindowDriver for MacosAxuiDriver {
    fn platform_name(&self) -> &'static str {
        "macos-axui"
    }

    fn list_windows(&self, filter: Option<&str>) -> Result<Vec<WindowInfo>> {
        // 窗口枚举仍走 CoreGraphics(同 legacy):这是 OS 级查询,与 AX 权限无关,
        // axuielement 没有提供等价 API,直接复用 legacy 的 cg helper 是最稳选择。
        list_windows_cg(filter)
    }

    fn inspect(
        &self,
        window_id: &str,
        max_depth: usize,
        filter: Option<&str>,
    ) -> Result<ControlNode> {
        let (pid, idx) = parse_window_id(window_id)?;

        // axuielement typed API:AXUIElement::from_pid(pid) 创建应用级 element,
        // 再用 element_array_attribute("AXWindows") 拿到 Vec<AXUIElement>。
        let app = axuielement::AXUIElement::from_pid(pid).ok_or_else(|| {
            platform_err(
                self.platform_name(),
                format!("AXUIElement::from_pid({pid}) 返回 null"),
            )
        })?;
        enable_manual_accessibility(&app);
        let windows = app
            .element_array_attribute(axuielement::ax_attribute::AX_WINDOWS_ATTRIBUTE)
            .map_err(|e| {
                platform_err(
                    self.platform_name(),
                    format!("AX_WINDOWS_ATTRIBUTE 失败:{e}"),
                )
            })?;
        let target = windows
            .into_iter()
            .nth(idx)
            .ok_or_else(|| platform_err(self.platform_name(), format!("窗口 idx={idx} 不存在")))?;

        // 根节点 = 窗口本身
        let title = read_string_attr(&target, axuielement::ax_attribute::AX_TITLE_ATTRIBUTE)
            .unwrap_or_default();
        let role = read_string_attr(&target, axuielement::ax_attribute::AX_ROLE_ATTRIBUTE)
            .unwrap_or_default();
        let bounds = read_bounds(&target).unwrap_or_default();

        let actions: Vec<String> = target
            .action_names()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        let mut root = ControlNode {
            path: "/".into(),
            role,
            name: title,
            value: String::new(),
            bounds,
            actions,
            children: Vec::new(),
        };

        build_children(&target, "/", 1, max_depth, filter, &mut root)?;
        Ok(root)
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        let (pid, idx) = parse_window_id(window_id)?;
        let app = axuielement::AXUIElement::from_pid(pid).ok_or_else(|| {
            platform_err(
                self.platform_name(),
                format!("AXUIElement::from_pid({pid}) 返回 null"),
            )
        })?;
        enable_manual_accessibility(&app);
        let windows = app
            .element_array_attribute(axuielement::ax_attribute::AX_WINDOWS_ATTRIBUTE)
            .map_err(|e| {
                platform_err(
                    self.platform_name(),
                    format!("AX_WINDOWS_ATTRIBUTE 失败:{e}"),
                )
            })?;
        let target_window = windows
            .into_iter()
            .nth(idx)
            .ok_or_else(|| platform_err(self.platform_name(), format!("窗口 idx={idx} 不存在")))?;

        // 沿 path 段(数字索引)逐层 descend
        let element = descend_by_path(&target_window, path)?;

        match action {
            ControlAction::Click | ControlAction::Invoke => {
                element
                    .perform_action(axuielement::ax_action::AX_PRESS_ACTION)
                    .map_err(|e| platform_err(self.platform_name(), format!("press 失败:{e}")))?;
                Ok(if matches!(action, ControlAction::Click) {
                    "clicked".into()
                } else {
                    "invoked".into()
                })
            }
            ControlAction::Focus => {
                // axuielement 的 set_attribute 接 &AXValue;通过 AXValue::from_bool(true) 包装
                let v = axuielement::AXValue::from_bool(true);
                element
                    .set_attribute(axuielement::ax_attribute::AX_FOCUSED_ATTRIBUTE, &v)
                    .map_err(|e| platform_err(self.platform_name(), format!("focus 失败:{e}")))?;
                Ok("focused".into())
            }
            ControlAction::SetText(text) => {
                let v = axuielement::AXValue::from_string(text.as_str()).map_err(|e| {
                    platform_err(
                        self.platform_name(),
                        format!("AXValue::from_string 失败:{e}"),
                    )
                })?;
                element
                    .set_attribute(axuielement::ax_attribute::AX_VALUE_ATTRIBUTE, &v)
                    .map_err(|e| {
                        platform_err(self.platform_name(), format!("set_text 失败:{e}"))
                    })?;
                Ok(format!("set_text len={}", text.len()))
            }
            ControlAction::GetText => {
                read_string_attr(&element, axuielement::ax_attribute::AX_VALUE_ATTRIBUTE)
                    .ok_or_else(|| platform_err(self.platform_name(), "get_text 返回空"))
            }
            ControlAction::SendKeys(keys) => {
                // axuielement 不直接提供 send_keys typed API;
                // 引导 LLM 改走 set_text + 复制粘贴路径。
                return Err(platform_err(
                    self.platform_name(),
                    format!(
                        "send_keys({keys:?}) 暂未实现(axuielement 无 typed send_keys API);\
                         建议改走 set_text 写入文本,或用 pbcopy/osascript;\
                         或使用默认 legacy 驱动(CGEvent 按键注入,第 66 轮起支持)"
                    ),
                ));
            }
            // 2026-09-16 第 66 轮:axuielement feature 路径暂未实现 CGEvent 注入,
            // 返回结构化引导(默认构建走 legacy 驱动,已支持 scroll/send_keys)。
            ControlAction::Scroll { lines } => {
                return Err(platform_err(
                    self.platform_name(),
                    format!(
                        "scroll(lines={lines}) 在 macos-axui 路径暂未实现;\
                         默认 legacy 驱动已支持,请移除 --features macos-axui 构建;\
                         或降级 Bash 白名单 osascript/cliclick"
                    ),
                ));
            }
            ControlAction::ScrollToVisible => {
                element.perform_action("AXScrollToVisible").map_err(|e| {
                    platform_err(self.platform_name(), format!("scroll_to_visible 失败:{e}"))
                })?;
                Ok("scrolled_to_visible".into())
            }
            // 2026-09-17 第 70 轮:第 67 轮坐标动作(视觉路线)在 macos-axui 路径未实现,
            // 返回结构化引导(默认构建走 legacy 驱动,第 69 轮起已支持 CGEvent 全套坐标动作;
            // 第 90 轮起含 move_point / middle_click_point / drag_point / modifiers 新原语)。
            point @ (ControlAction::ClickPoint { .. }
            | ControlAction::DoubleClickPoint { .. }
            | ControlAction::RightClickPoint { .. }
            | ControlAction::ScrollPoint { .. }
            | ControlAction::MovePoint { .. }
            | ControlAction::MiddleClickPoint { .. }
            | ControlAction::DragPoint { .. }) => {
                return Err(platform_err(
                    self.platform_name(),
                    format!(
                        "{point:?} 在 macos-axui 路径暂未实现;\
                         默认 legacy 驱动已支持(CGEvent 物理输入,第 69 轮),请移除 --features macos-axui 构建;\
                         或降级 Bash 白名单 osascript/cliclick"
                    ),
                ));
            }
            ControlAction::TypeText(_) => {
                return Err(platform_err(
                    self.platform_name(),
                    "type_text 在 macos-axui 路径暂未实现(无 CGEvent Unicode 键入);\
                     默认 legacy 驱动已支持,或改用 set_text / Bash 白名单 osascript keystroke",
                ));
            }
            ControlAction::TypeTextSubmit { .. } => {
                return Err(platform_err(
                    self.platform_name(),
                    "type_text_submit 请使用默认 legacy 驱动(CGEvent 原子键入+Enter);\
                     macos-axui typed 路径暂无物理键盘注入",
                ));
            }
        }
    }

    fn permission_hint(&self) -> Option<String> {
        // axuielement 暴露 is_process_trusted() 走系统级 AX 授权检查;
        // 未授权时直接给开权限步骤引导(同 legacy 文案)。
        if axuielement::process_trust::is_process_trusted() {
            None
        } else {
            Some(
                "[macos-axui] 辅助功能未授权,MCP_Window_Use(action=inspect)/MCP_Window_Use(action=control) 暂不可用。\
                 建议:系统设置 → 隐私与安全性 → 辅助功能 → 勾选宿主终端并重开;\
                 或把含 osascript/screencapture/cliclick 的步骤改 delegate_to=subagent \
                 (SubAgent Bash 全量可用);MCP_Window_Use(action=list) 走 CoreGraphics 始终可用。"
                    .into(),
            )
        }
    }
}

// ============== 辅助函数 ==============

/// 解析 "pid:idx" 格式的 window_id → (pid, idx)。
fn parse_window_id(window_id: &str) -> Result<(i32, usize)> {
    let mut parts = window_id.split(':');
    let pid: i32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| platform_err("macos-axui", format!("window_id 格式非法:{window_id}")))?;
    let idx: usize = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Ok((pid, idx))
}

/// 读 string attribute,转 String;失败返回 None。
fn read_string_attr(element: &axuielement::AXUIElement, attr: &str) -> Option<String> {
    element.string_attribute(attr).ok().flatten()
}

/// 尽力开启 Electron / 自绘应用的 AXManualAccessibility。
///
/// 部分微信 / Electron 版本默认不导出控件树;该属性打开失败时不阻断读取。
fn enable_manual_accessibility(app: &axuielement::AXUIElement) {
    let value = axuielement::AXValue::from_bool(true);
    let _ = app.set_attribute("AXManualAccessibility", &value);
}

/// 读 bounds:pos + size 合并成 Rect。
fn read_bounds(element: &axuielement::AXUIElement) -> Option<Rect> {
    let pos = element
        .rect_attribute(axuielement::ax_attribute::AX_POSITION_ATTRIBUTE)
        .ok()
        .flatten()?;
    let size = element
        .rect_attribute(axuielement::ax_attribute::AX_SIZE_ATTRIBUTE)
        .ok()
        .flatten()?;
    Some(Rect {
        x: pos.origin.x as i64,
        y: pos.origin.y as i64,
        width: size.size.width as i64,
        height: size.size.height as i64,
    })
}

/// 递归构造 ControlNode 子节点;深度达 max_depth 停止。
///
/// axuielement 的 typed children() 返回 `Vec<AXUIElement>`,需要逐项包装成
/// `ControlNode` 并附带 path / role / name / value / bounds / actions。
fn build_children(
    element: &axuielement::AXUIElement,
    parent_path: &str,
    depth: usize,
    max_depth: usize,
    filter: Option<&str>,
    parent_node: &mut ControlNode,
) -> Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    let children = element
        .children()
        .map_err(|e| platform_err("macos-axui", format!("children 失败:{e}")))?;
    for (idx, child) in children.into_iter().enumerate() {
        let path = format!("{parent_path}/{idx}");
        let role = read_string_attr(&child, axuielement::ax_attribute::AX_ROLE_ATTRIBUTE)
            .unwrap_or_default();
        let name = read_string_attr(&child, axuielement::ax_attribute::AX_TITLE_ATTRIBUTE)
            .unwrap_or_default();
        let value = read_string_attr(&child, axuielement::ax_attribute::AX_VALUE_ATTRIBUTE)
            .unwrap_or_default();
        // filter:命中 name/role 任一即保留,否则剪枝
        if !matches_filter(&name, filter) && !matches_filter(&role, filter) {
            continue;
        }
        let actions: Vec<String> = child
            .action_names()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let mut node = ControlNode {
            path,
            role,
            name,
            value,
            bounds: read_bounds(&child).unwrap_or_default(),
            actions,
            children: Vec::new(),
        };
        build_children(
            &child,
            &node.path.clone(),
            depth + 1,
            max_depth,
            filter,
            &mut node,
        )?;
        parent_node.children.push(node);
    }
    Ok(())
}

/// 沿 path 段(/N/M/L)在 axuielement 树上 descend 到目标元素。
fn descend_by_path(
    window: &axuielement::AXUIElement,
    path: &str,
) -> Result<axuielement::AXUIElement> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed.is_empty() {
        return Ok(window.clone());
    }
    let mut current = window.clone();
    for seg in trimmed.trim_start_matches('/').split('/') {
        let idx: usize = seg
            .parse()
            .map_err(|_| platform_err("macos-axui", format!("路径段非法:{seg}")))?;
        let children = current
            .children()
            .map_err(|e| platform_err("macos-axui", format!("children 失败:{e}")))?;
        current = children
            .into_iter()
            .nth(idx)
            .ok_or_else(|| platform_err("macos-axui", format!("子节点 idx={idx} 不存在")))?;
    }
    Ok(current)
}
