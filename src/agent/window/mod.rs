//! 桌面窗口操控抽象层(WindowUse Agent 的平台底座,第 9 角色)。
//!
//! 统一数据模型 + [`WindowDriver`] trait,隔离三个平台后端:
//!
//! - **Windows**(`windows.rs`,`cfg(windows)`):UI Automation 为主
//!   (`CoCreateInstance(CUIAutomation)` + TreeWalker 控件树遍历 +
//!   Invoke/Value Pattern 操作),`EnumWindows` 枚举 HWND 顶层窗口,
//!   UIA 不可用时降级 `SendMessageW`(BM_CLICK / WM_SETTEXT / WM_GETTEXT);
//! - **macOS**(默认 `macos_legacy.rs`;启用 `macos-axui` feature 时走 `macos_axui.rs`,
//!   `LAEW_MACOS_DRIVER=legacy` 强制回退):
//!   Accessibility API(`AXUIElementRef` / `axuielement = "0.9"` typed accessor),
//!   `CGWindowListCopyWindowInfo` 枚举窗口,
//!   `AXIsProcessTrustedWithOptions` 权限检测(默认不弹窗,`LAEW_AX_PROMPT=1` 放开);
//! - **其他平台**(`fallback.rs`):`wmctrl` / `xdotool` 尽力而为列举窗口,
//!   控件级操作返回结构化「平台不支持」错误(fail-closed,供 QC 判 Fail 回流)。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.3。

use serde::Serialize;

use crate::error::{AgentError, Result};

#[cfg(all(target_os = "macos", feature = "macos-axui"))]
mod macos_axui;
#[cfg(all(target_os = "macos", feature = "macos-axui"))]
use macos_axui::MacosAxuiDriver as DefaultMacosDriver;
#[cfg(target_os = "macos")]
mod macos_legacy;
#[cfg(target_os = "macos")]
use macos_legacy::MacOsDriver as LegacyMacosDriver;

// 2026-09-16 第 60 轮:公开 macOS 辅助功能权限相关接口,供 tools/window.rs 调用
#[cfg(target_os = "macos")]
pub use macos_legacy::MacOsDriver;
#[cfg(windows)]
mod windows;

pub mod fallback;

/// 矩形边界(像素,屏幕坐标系)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// 顶层窗口信息。
///
/// `id` 是平台相关的不透明标识字符串(Windows = HWND 十进制;macOS = `"{pid}"`
/// 应用级标识;fallback = wmctrl 十六进制窗口 id),仅供同一次任务内回传给
/// `inspect` / `act` 使用,不做持久化假设。
#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub id: String,
    pub title: String,
    pub process_name: String,
    pub pid: u32,
    pub bounds: Rect,
}

/// 控件树节点。
///
/// `path` 为控件在树中的稳定定位路径(子索引链,如 `/0/2/1`;根为 `/`),
/// `WindowAction` 工具凭它定位目标控件,避免 LLM 传递平台句柄等不透明值。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ControlNode {
    pub path: String,
    /// 控件角色(UIA ControlType / AX Role,统一为可读字符串,如 "Button"/"AXButton")
    pub role: String,
    /// 控件名(UIA Name / AX Title)
    pub name: String,
    /// 当前值(Value Pattern / AXValue),无则空串
    pub value: String,
    pub bounds: Rect,
    /// 该控件支持的动作名列表("click" / "set_text" / "get_text" / "focus" / "invoke")
    pub actions: Vec<String>,
    pub children: Vec<ControlNode>,
}

/// 控件操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    /// 点击(UIA Invoke / AXPress / Win32 BM_CLICK)
    Click,
    /// 聚焦
    Focus,
    /// 写入文本(Value Pattern / AXValue / WM_SETTEXT)
    SetText(String),
    /// 读取文本(无副作用)
    GetText,
    /// 发送按键(尽力而为,平台差异大)
    SendKeys(String),
    /// 调用默认动作(等价 Click,保留给 UIA Invoke 语义明确的场景)
    Invoke,
}

impl ControlAction {
    /// 从工具参数解析动作名(大小写不敏感,连字符/下划线归一)。
    pub fn parse(name: &str, text: Option<String>) -> Result<Self> {
        let norm = name.trim().to_lowercase().replace(['-', '_'], "");
        Ok(match norm.as_str() {
            "click" => Self::Click,
            "focus" => Self::Focus,
            "settext" | "input" | "type" => {
                Self::SetText(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "WindowAction".into(),
                    reason: "action=set_text 缺少 string 类型参数 text".into(),
                })?)
            }
            "gettext" | "read" => Self::GetText,
            "sendkeys" | "keys" => {
                Self::SendKeys(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "WindowAction".into(),
                    reason: "action=send_keys 缺少 string 类型参数 text".into(),
                })?)
            }
            "invoke" | "press" => Self::Invoke,
            other => {
                return Err(AgentError::ToolExecution {
                    tool: "WindowAction".into(),
                    reason: format!(
                        "未知 action: {other};可用: click / focus / set_text / get_text / send_keys / invoke"
                    ),
                })
            }
        })
    }
}

/// 平台窗口驱动(同步方法;GUI 调用可能阻塞,工具层须 `spawn_blocking` 包裹)。
pub trait WindowDriver: Send + Sync {
    /// 平台名(错误文案 / Debug trace 用)
    fn platform_name(&self) -> &'static str;

    /// 枚举可见顶层窗口;`filter` 为标题/进程名子串过滤(大小写不敏感)。
    fn list_windows(&self, filter: Option<&str>) -> Result<Vec<WindowInfo>>;

    /// 枚举指定窗口的控件树。
    ///
    /// - `max_depth`:遍历深度(1 = 仅根窗口本身,2 = 含直接子控件,依此类推);
    /// - `filter`:名称/角色子串过滤,命中节点的祖先链保留,其余子树剪枝。
    fn inspect(
        &self,
        window_id: &str,
        max_depth: usize,
        filter: Option<&str>,
    ) -> Result<ControlNode>;

    /// 对 `path` 定位的控件执行操作,返回操作结果(get_text 返回读到的文本)。
    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String>;

    /// 缺权限/缺依赖时的可读引导文案;`None` 表示一切就绪。
    fn permission_hint(&self) -> Option<String>;
}

/// 构造当前平台的窗口驱动。
///
/// 2026-09-16 第 61 轮:默认走稳定的 core-foundation FFI(macos_legacy.rs);
/// 完整 Xcode 工具链可用 `--features macos-axui` 切到 axuielement typed API。
pub fn current_driver() -> Box<dyn WindowDriver> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsDriver::new())
    }
    #[cfg(target_os = "macos")]
    {
        let want_legacy = std::env::var("LAEW_MACOS_DRIVER")
            .map(|v| v.eq_ignore_ascii_case("legacy"))
            .unwrap_or(false);
        #[cfg(feature = "macos-axui")]
        {
            if want_legacy {
                Box::new(LegacyMacosDriver::new())
            } else {
                Box::new(DefaultMacosDriver::new())
            }
        }
        #[cfg(not(feature = "macos-axui"))]
        {
            let _ = want_legacy;
            Box::new(LegacyMacosDriver::new())
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Box::new(fallback::FallbackDriver::new())
    }
}

/// 工具层统一错误构造(带平台名前缀,LLM 可读)。
pub(crate) fn platform_err(platform: &str, msg: impl Into<String>) -> AgentError {
    AgentError::ToolExecution {
        tool: "Window*".into(),
        reason: format!("[{platform}] {}", msg.into()),
    }
}

/// 2026-09-16 第 62 轮:Unicode 上标字母归一化,解决「赵玲玲ᴬᴵᴬ」vs
/// 「赵玲玲ᴵᴵᴬ」这种混用 Unicode Modifier Letter 与 Latin Letter
/// 块导致的匹配失败场景。仅归一化常用上标字母子集,不引入 NFKC
/// 完整归一化以免破坏其他匹配场景。
fn normalize_unicode_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let mapped = match c {
            // Modifier Letter 块常用上标字母 → Latin 等价
            '\u{1d2c}' => 'A',
            '\u{1d2e}' => 'B',
            '\u{1d30}' => 'D',
            '\u{1d31}' => 'E',
            '\u{1d33}' => 'G',
            '\u{1d34}' => 'H',
            '\u{1d35}' => 'I',
            '\u{1d36}' => 'J',
            '\u{1d37}' => 'K',
            '\u{1d38}' => 'L',
            '\u{1d39}' => 'M',
            '\u{1d3a}' => 'N',
            '\u{1d3c}' => 'O',
            '\u{1d3e}' => 'P',
            '\u{1d40}' => 'R',
            '\u{1d41}' => 'T',
            '\u{1d42}' => 'U',
            '\u{1d43}' => 'W',
            // 小写上标
            '\u{1d62}' => 'i',
            '\u{1d63}' => 'r',
            '\u{1d64}' => 'u',
            '\u{1d65}' => 'v',
            '\u{1d66}' => 'x',
            '\u{1d67}' => 'y',
            _ => c,
        };
        out.push(mapped);
    }
    out
}

/// 子串过滤(大小写不敏感 + Unicode 上标字母归一化);`filter` 为空/None 时恒真。
pub(crate) fn matches_filter(haystack: &str, filter: Option<&str>) -> bool {
    match filter.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => {
            let h = normalize_unicode_for_match(haystack).to_lowercase();
            let q = normalize_unicode_for_match(f).to_lowercase();
            h.contains(&q)
        }
        None => true,
    }
}

/// 按子索引路径(如 `/0/2/1`)在控件树中定位节点;路径非法/越界返回 None。
pub fn find_by_path<'a>(root: &'a ControlNode, path: &str) -> Option<&'a ControlNode> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed.is_empty() {
        return Some(root);
    }
    let mut node = root;
    for seg in trimmed.trim_start_matches('/').split('/') {
        let idx: usize = seg.parse().ok()?;
        node = node.children.get(idx)?;
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> ControlNode {
        ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "根窗口".into(),
            children: vec![
                ControlNode {
                    path: "/0".into(),
                    role: "Button".into(),
                    name: "确定".into(),
                    ..Default::default()
                },
                ControlNode {
                    path: "/1".into(),
                    role: "Pane".into(),
                    name: "面板".into(),
                    children: vec![ControlNode {
                        path: "/1/0".into(),
                        role: "Edit".into(),
                        name: "用户名".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn find_by_path_locates_nodes() {
        let tree = sample_tree();
        assert_eq!(find_by_path(&tree, "/").unwrap().name, "根窗口");
        assert_eq!(find_by_path(&tree, "").unwrap().name, "根窗口");
        assert_eq!(find_by_path(&tree, "/0").unwrap().name, "确定");
        assert_eq!(find_by_path(&tree, "/1/0").unwrap().name, "用户名");
        assert!(find_by_path(&tree, "/9").is_none());
        assert!(find_by_path(&tree, "/1/5").is_none());
        assert!(find_by_path(&tree, "/abc").is_none());
        assert!(find_by_path(&tree, "/1/0/0").is_none());
    }

    #[test]
    fn control_action_parse_aliases() {
        assert_eq!(
            ControlAction::parse("click", None).unwrap(),
            ControlAction::Click
        );
        assert_eq!(
            ControlAction::parse("CLICK", None).unwrap(),
            ControlAction::Click
        );
        assert_eq!(
            ControlAction::parse("focus", None).unwrap(),
            ControlAction::Focus
        );
        assert_eq!(
            ControlAction::parse("set_text", Some("hi".into())).unwrap(),
            ControlAction::SetText("hi".into())
        );
        assert_eq!(
            ControlAction::parse("input", Some("x".into())).unwrap(),
            ControlAction::SetText("x".into())
        );
        assert_eq!(
            ControlAction::parse("get-text", None).unwrap(),
            ControlAction::GetText
        );
        assert_eq!(
            ControlAction::parse("send_keys", Some("Enter".into())).unwrap(),
            ControlAction::SendKeys("Enter".into())
        );
        assert_eq!(
            ControlAction::parse("invoke", None).unwrap(),
            ControlAction::Invoke
        );
        assert!(ControlAction::parse("set_text", None).is_err());
        assert!(ControlAction::parse("send_keys", None).is_err());
        assert!(ControlAction::parse("explode", None).is_err());
    }

    #[test]
    fn matches_filter_case_insensitive() {
        assert!(matches_filter("记事本 - Notepad", Some("NOTE")));
        assert!(matches_filter("anything", None));
        assert!(matches_filter("anything", Some("  ")));
        assert!(!matches_filter("abc", Some("xyz")));
    }

    #[test]
    fn matches_filter_normalizes_modifier_letters() {
        // 2026-09-16 第 62 轮:Unicode Modifier Letter 归一化。
        // 关键场景:haystack=赵玲玲ᴬᴵᴵᵢ → normalize → 赵玲玲AIIi
        // filter=aii → 子串匹配 AIIi。
        assert!(matches_filter(
            "赵玲玲\u{1d2c}\u{1d35}\u{1d35}\u{1d62}",
            Some("aii")
        ));
        // 反向:filter=ᴬᴵᴵᵢ → normalize → AIIi → lowercase → aiii,
        // haystack 含 aii 子串。
        assert!(matches_filter(
            "张三aIIi",
            Some("\u{1d2c}\u{1d35}\u{1d35}\u{1d62}")
        ));
        // 大写 filter 同样命中(整体 lowercase 比较)
        assert!(matches_filter("AII", Some("aii")));
        // 不归一化的字符保持原样
        assert!(matches_filter("赵玲玲", Some("赵玲玲")));
        assert!(!matches_filter("赵玲玲", Some("赵丽丽")));
        // 极端 case:filter 跟 haystack 完全归一化后相等
        assert!(matches_filter(
            "\u{1d2c}\u{1d35}\u{1d35}",
            Some("\u{1d2c}\u{1d35}\u{1d35}")
        ));
    }

    #[test]
    fn current_driver_constructs_on_any_platform() {
        let d = current_driver();
        assert!(!d.platform_name().is_empty());
    }
}
