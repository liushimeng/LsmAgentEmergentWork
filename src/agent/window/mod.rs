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
// 2026-09-17 第 74 轮:macOS Vision OCR + CGWindow 原生截图
#[cfg(target_os = "macos")]
pub(crate) mod macos_vision_ocr;

// 2026-09-16 第 60 轮:公开 macOS 辅助功能权限相关接口,供 tools/window.rs 调用
#[cfg(target_os = "macos")]
pub use macos_legacy::MacOsDriver;
#[cfg(windows)]
mod windows;
// 2026-09-16 第 67 轮:Windows 物理输入(SendInput)与 OCR(Windows.Media.Ocr)底座
#[cfg(windows)]
pub(crate) mod windows_input;
#[cfg(windows)]
pub(crate) mod windows_ocr;

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
    /// 滚动(2026-09-16 第 66 轮):lines>0 向上滚,lines<0 向下滚,单位为「行」。
    /// macOS 走 CGEvent 滚轮事件(光标先移到控件中心);Windows 走 WM_MOUSEWHEEL;
    /// fallback 走 xdotool click 4/5。
    Scroll {
        /// 滚动行数(正=上,负=下)
        lines: i32,
    },
    /// 把目标控件滚动到可见区域(2026-09-16 第 66 轮):
    /// macOS AXScrollToVisible;Windows 暂等价 Scroll 小步;列表逐条定位场景比盲滚精准。
    ScrollToVisible,
    /// ===== 2026-09-16 第 67 轮:坐标动作(自绘 UI 视觉路线) =====
    /// 坐标信息来自 `WindowOCR` 返回的词块(screen_x/screen_y 取中心)或窗口 bounds 计算。
    /// 在屏幕绝对坐标 (x,y) 执行物理鼠标左键单击(SendInput / CGEvent / xdotool)。
    ClickPoint { x: i64, y: i64 },
    /// 坐标双击(展开列表项 / 打开会话等场景)。
    DoubleClickPoint { x: i64, y: i64 },
    /// 坐标右键(呼出上下文菜单)。
    RightClickPoint { x: i64, y: i64 },
    /// 在 (x,y) 处滚动滚轮(先移光标再滚,自绘 UI 对消息滚动不敏感,物理事件最可靠)。
    ScrollPoint { x: i64, y: i64, lines: i32 },
    /// 向**当前焦点控件**真实键入文本(SendInput Unicode / CGEvent keystroke)。
    /// 配合 `click_point` 先点输入框使用;自绘输入框(微信 4.x 等)唯一可靠的输入路径。
    TypeText(String),
}

impl ControlAction {
    /// 从工具参数解析动作名(大小写不敏感,连字符/下划线归一)。
    pub fn parse(name: &str, text: Option<String>) -> Result<Self> {
        Self::parse_ext(name, text, None, None)
    }

    /// 2026-09-16 第 67 轮:带坐标参数的动作解析(x/y 供 click_point / scroll_point 系使用)。
    pub fn parse_ext(
        name: &str,
        text: Option<String>,
        x: Option<i64>,
        y: Option<i64>,
    ) -> Result<Self> {
        let norm = name.trim().to_lowercase().replace(['-', '_'], "");
        // 坐标类动作统一校验 x/y 必填(坐标动作不接受 path 定位,path 照传 "/")
        let need_point = || -> std::result::Result<(i64, i64), AgentError> {
            match (x, y) {
                (Some(px), Some(py)) => Ok((px, py)),
                _ => Err(AgentError::ToolExecution {
                    tool: "WindowAction".into(),
                    reason: format!(
                        "action={norm} 缺少整数参数 x / y(屏幕绝对坐标,取 WindowOCR 返回的 screen_x/screen_y 中心)"
                    ),
                }),
            }
        };
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
            // 2026-09-16 第 66 轮:scroll 动作,text 形如 "down:3" / "up:5" / "down"(缺省 3 行)。
            "scroll" => Self::Scroll {
                lines: parse_scroll_lines(text.as_deref())?,
            },
            "scrolltovisible" | "scrollintoview" | "reveal" => Self::ScrollToVisible,
            // ===== 第 67 轮:坐标动作 =====
            "clickpoint" | "pointclick" | "clickat" => {
                let (px, py) = need_point()?;
                Self::ClickPoint { x: px, y: py }
            }
            "doubleclickpoint" | "pointdoubleclick" | "doubleclickat" => {
                let (px, py) = need_point()?;
                Self::DoubleClickPoint { x: px, y: py }
            }
            "rightclickpoint" | "pointrightclick" | "rightclickat" => {
                let (px, py) = need_point()?;
                Self::RightClickPoint { x: px, y: py }
            }
            "scrollpoint" | "pointscroll" | "scrollat" => {
                let (px, py) = need_point()?;
                Self::ScrollPoint {
                    x: px,
                    y: py,
                    lines: parse_scroll_lines(text.as_deref())?,
                }
            }
            "typetext" | "inputatfocus" | "typeatfocus" => {
                Self::TypeText(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "WindowAction".into(),
                    reason: "action=type_text 缺少 string 类型参数 text".into(),
                })?)
            }
            other => {
                return Err(AgentError::ToolExecution {
                    tool: "WindowAction".into(),
                    reason: format!(
                        "未知 action: {other};可用: click / focus / set_text / get_text / send_keys / invoke / scroll / scroll_to_visible / click_point / double_click_point / right_click_point / scroll_point / type_text"
                    ),
                })
            }
        })
    }

    /// 是否为屏幕坐标类动作(工具层据此提示 LLM 坐标来源)。
    pub fn is_point_action(&self) -> bool {
        matches!(
            self,
            Self::ClickPoint { .. }
                | Self::DoubleClickPoint { .. }
                | Self::RightClickPoint { .. }
                | Self::ScrollPoint { .. }
        )
    }
}

/// 解析 scroll 的方向与行数:"down:3" / "up:5" / "down"(缺省 3 行)。
/// 返回 lines:正=向上,负=向下(对齐滚轮物理语义)。
fn parse_scroll_lines(text: Option<&str>) -> Result<i32> {
    let raw = text.unwrap_or("down").trim().to_lowercase();
    let (dir, num_part) = match raw.split_once(':') {
        Some((d, n)) => (d.trim(), n.trim()),
        None => (raw.as_str(), ""),
    };
    let magnitude: i32 = if num_part.is_empty() {
        3
    } else {
        num_part.parse().map_err(|_| AgentError::ToolExecution {
            tool: "WindowAction".into(),
            reason: format!("scroll 行数非法: {num_part}(应为正整数,如 \"down:3\")"),
        })?
    };
    if magnitude <= 0 || magnitude > 100 {
        return Err(AgentError::ToolExecution {
            tool: "WindowAction".into(),
            reason: format!("scroll 行数超出范围(1-100): {magnitude}"),
        });
    }
    match dir {
        "up" | "upward" | "上" => Ok(magnitude),
        "down" | "downward" | "下" => Ok(-magnitude),
        other => Err(AgentError::ToolExecution {
            tool: "WindowAction".into(),
            reason: format!("scroll 方向非法: {other}(应为 up/down)"),
        }),
    }
}

/// OCR 词块(2026-09-16 第 67 轮,视觉路线)。
///
/// 坐标为**窗口相对物理像素**;工具层会依据窗口 bounds 换算出屏幕绝对坐标
/// (`screen_x/screen_y`)供坐标动作直接消费。
#[derive(Debug, Clone, Serialize)]
pub struct OcrBlock {
    pub text: String,
    /// 窗口相对坐标(左上角为原点,物理像素)
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
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

    /// 2026-09-16 第 67 轮:把窗口带到前台(恢复最小化 + 激活)。
    ///
    /// 语义:幂等、失败不 panic;默认 no-op(平台暂未实装时静默,
    /// WindowOpen 在「已在运行」分支调用它,避免重复启动第二实例)。
    fn bring_to_front(&self, _window_id: &str) -> Result<()> {
        Ok(())
    }

    /// 2026-09-16 第 67 轮:对窗口(可选区域)做 OCR,返回词级文本块。
    ///
    /// - `region`:窗口相对矩形(物理像素);`None` = 整个窗口客户区;
    /// - `lang`:BCP-47(如 "zh-Hans-CN");`None` = 用户配置语言;
    /// - 默认实现返回「平台暂不支持」(fail-closed),Windows 实装
    ///   (GDI 截图 + GDI+ PNG + Windows.Media.Ocr,全 OS API 离线)。
    /// - 2026-09-17 第 74 轮:macOS 实装 Vision.framework OCR。
    fn ocr(
        &self,
        _window_id: &str,
        _region: Option<Rect>,
        _lang: Option<&str>,
    ) -> Result<Vec<OcrBlock>> {
        Err(platform_err(
            self.platform_name(),
            "当前平台驱动暂不支持 OCR(Windows 已实装 Windows.Media.Ocr;macOS Vision 待后续轮次)",
        ))
    }

    /// 2026-09-17 第 74 轮 T2:原生截图(落盘到指定路径)。
    ///
    /// 语义:平台原生截图实现,替代 screencapture 等外部命令。
    /// - macOS:CGWindowListCreateImage(只需辅助功能权限,无需屏幕录制权限)
    /// - Windows:GDI 截图(已有 windows_ocr::capture_window_png_to)
    /// - 其他平台:返回 Err,工具层降级到外部命令
    ///
    /// 返回实际截取的屏幕矩形(用于工具层记录)。
    fn screenshot_to(
        &self,
        _window_id: &str,
        _region: Option<Rect>,
        _output_path: &std::path::Path,
    ) -> Result<Rect> {
        Err(platform_err(
            self.platform_name(),
            "当前平台驱动暂未实装原生截图,工具层将降级到外部命令(screencapture/import)",
        ))
    }
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

/// 2026-09-16 第 67 轮:window_id → HWND(供 tools/window_vision.rs 的
/// Windows 截图路径直接拿句柄;非 Windows 平台不存在本函数)。
#[cfg(windows)]
pub fn windows_driver_hwnd(window_id: &str) -> Result<::windows::Win32::Foundation::HWND> {
    windows::WindowsDriver::parse_hwnd(window_id)
}

/// 2026-09-16 第 67 轮:按窗口(可选区域)截图并**保存到指定路径**
/// (WindowScreenshot 的 Windows 纯 Rust 路径;返回实际截取的屏幕矩形)。
#[cfg(windows)]
pub fn windows_ocr_capture_to(
    hwnd: ::windows::Win32::Foundation::HWND,
    region: Option<Rect>,
    out_path: &std::path::Path,
) -> Result<Rect> {
    windows_ocr::capture_window_png_to(hwnd, region, out_path)
}

/// 2026-09-16 第 62 轮:Unicode 上标字母归一化,解决「赵玲玲ᴬᴵᴬ」vs
/// 「赵玲玲ᴵᴵᴬ」这种混用 Unicode Modifier Letter 与 Latin Letter
/// 块导致的匹配失败场景。仅归一化常用上标字母子集,不引入 NFKC
/// 完整归一化以免破坏其他匹配场景。
/// 第 66 轮:提升为 pub(crate),供 tools/window.rs 的 WindowFind 打分复用。
pub(crate) fn normalize_unicode_for_match(s: &str) -> String {
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
    fn control_action_parse_scroll() {
        // 2026-09-16 第 66 轮:scroll 动作解析(方向 + 行数,正=上,负=下)
        assert_eq!(
            ControlAction::parse("scroll", Some("down:3".into())).unwrap(),
            ControlAction::Scroll { lines: -3 }
        );
        assert_eq!(
            ControlAction::parse("scroll", Some("up:5".into())).unwrap(),
            ControlAction::Scroll { lines: 5 }
        );
        // 缺省 3 行向下
        assert_eq!(
            ControlAction::parse("scroll", None).unwrap(),
            ControlAction::Scroll { lines: -3 }
        );
        assert_eq!(
            ControlAction::parse("scroll", Some("down".into())).unwrap(),
            ControlAction::Scroll { lines: -3 }
        );
        // 中文方向别名
        assert_eq!(
            ControlAction::parse("scroll", Some("上:2".into())).unwrap(),
            ControlAction::Scroll { lines: 2 }
        );
        // scroll_to_visible 别名
        assert_eq!(
            ControlAction::parse("scroll_to_visible", None).unwrap(),
            ControlAction::ScrollToVisible
        );
        assert_eq!(
            ControlAction::parse("scroll-into-view", None).unwrap(),
            ControlAction::ScrollToVisible
        );
        // 非法:方向未知 / 行数非数 / 超范围
        assert!(ControlAction::parse("scroll", Some("left:3".into())).is_err());
        assert!(ControlAction::parse("scroll", Some("down:abc".into())).is_err());
        assert!(ControlAction::parse("scroll", Some("down:0".into())).is_err());
        assert!(ControlAction::parse("scroll", Some("down:101".into())).is_err());
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
