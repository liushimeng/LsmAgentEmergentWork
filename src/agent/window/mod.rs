//! 桌面窗口操控抽象层(MCP_Window_Use 工具的平台驱动层 / "MCP 服务"实现)。
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
//! 设计见 `docs/MCP_Window_Use/01-设计与解决方案.md`;
//! 平台技术参考:`docs/MCP_Window_Use/MCP_Window_Use_MacOS_技术文档.md` /
//! `docs/MCP_Window_Use/MCP_Window_Use_Window_技术文档.md`。

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
///
/// 2026-09-17 第 76 轮:为多窗口进程(Electron / 自绘 UI 等同一 PID 下多个窗口的进程)
/// 修复 CGWindowID 错位问题,新增 `cg_window_id` / `hwnd` / `wmctrl_id` 三平台
/// 底层句柄字段,MCP_Window_Use(action=ocr) / MCP_Window_Use(action=screenshot) 优先使用平台原生句柄(避免按 PID 匹配
/// 拿到非目标窗口的 CGWindowID)。LLM 始终只看到 `id` 这一个稳定 token。
#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub id: String,
    pub title: String,
    pub process_name: String,
    pub pid: u32,
    pub bounds: Rect,
    /// macOS CGWindowID(kCGWindowNumber),用于 CGWindowListCreateImage 直接截图。
    /// 多窗口进程下,该字段保证截图/OCR 命中正确的目标窗口(按 PID 匹配会拿到第一个窗口)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cg_window_id: Option<u32>,
    /// Windows HWND(同位 isize),UIA / SendInput 直接使用。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hwnd: Option<isize>,
    /// Linux wmctrl 十六进制窗口 ID,xdotool / wmctrl 命令直接使用。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub wmctrl_id: Option<String>,
}

impl WindowInfo {
    /// 构造不含平台原生句柄的基础 WindowInfo(用于 mock / 测试 / fallback)。
    pub fn basic(id: String, title: String, process_name: String, pid: u32, bounds: Rect) -> Self {
        Self {
            id,
            title,
            process_name,
            pid,
            bounds,
            cg_window_id: None,
            hwnd: None,
            wmctrl_id: None,
        }
    }
}

/// 控件树节点。
///
/// `path` 为控件在树中的稳定定位路径(子索引链,如 `/0/2/1`;根为 `/`),
/// `MCP_Window_Use(action=control)` 工具凭它定位目标控件,避免 LLM 传递平台句柄等不透明值。
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
    /// 坐标信息来自 `MCP_Window_Use(action=ocr)` 返回的词块(screen_x/screen_y 取中心)或窗口 bounds 计算。
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
    /// 键入完整文本后立即提交(2026-09-17 第 80 轮)。
    ///
    /// 聊天发送框、搜索框、命令面板等场景中,`type_text` 与 `send_keys(enter)`
    /// 拆成两次工具调用会引入 LLM 返场和焦点迁移窗口;本动作在驱动层原子完成
    /// “可选点击定位 → 键入 → 等待应用消费 → Enter”。`x/y` 为可选屏幕绝对坐标。
    TypeTextSubmit {
        text: String,
        x: Option<i64>,
        y: Option<i64>,
    },
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
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: format!(
                        "action={norm} 缺少整数参数 x / y(屏幕绝对坐标,取 MCP_Window_Use(action=ocr) 返回的 screen_x/screen_y 中心)"
                    ),
                }),
            }
        };
        Ok(match norm.as_str() {
            "click" => Self::Click,
            "focus" => Self::Focus,
            "settext" | "input" | "type" => {
                Self::SetText(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=set_text 缺少 string 类型参数 text".into(),
                })?)
            }
            "gettext" | "read" => Self::GetText,
            "sendkeys" | "keys" => {
                Self::SendKeys(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
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
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=type_text 缺少 string 类型参数 text".into(),
                })?)
            }
            "typetextsubmit" | "sendtext" | "typeandsubmit" => {
                let text = text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=type_text_submit 缺少 string 类型参数 text".into(),
                })?;
                Self::TypeTextSubmit {
                    text,
                    x: x.filter(|v| *v > 0),
                    y: y.filter(|v| *v > 0),
                }
            }
            other => {
                return Err(AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: format!(
                        "未知 action: {other};可用: click / focus / set_text / get_text / send_keys / invoke / scroll / scroll_to_visible / click_point / double_click_point / right_click_point / scroll_point / type_text / type_text_submit"
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
            tool: "MCP_Window_Use(action=control)".into(),
            reason: format!("scroll 行数非法: {num_part}(应为正整数,如 \"down:3\")"),
        })?
    };
    if magnitude <= 0 || magnitude > 100 {
        return Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=control)".into(),
            reason: format!("scroll 行数超出范围(1-100): {magnitude}"),
        });
    }
    match dir {
        "up" | "upward" | "上" => Ok(magnitude),
        "down" | "downward" | "下" => Ok(-magnitude),
        other => Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=control)".into(),
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
    /// MCP_Window_Use(action=open) 在「已在运行」分支调用它,避免重复启动第二实例)。
    fn bring_to_front(&self, _window_id: &str) -> Result<()> {
        Ok(())
    }

    /// 2026-09-17 第 81 轮:窗口所属应用当前是否已处于前台。
    ///
    /// 用途:MCP_Window_Use(action=open) 对「已存在窗口」先查本方法 —— 已前台则**跳过激活**
    /// (跳过 AXRaise / osascript frontmost / 400ms sleep),消除失败回流与
    /// 多单元链路中窗口被反复前置导致的闪烁与焦点断续(会话连续性)。
    ///
    /// 默认 `false`:平台未实现时保持旧行为(总是尝试激活,安全降级 ——
    /// 误判的后果只是多做一次幂等激活,与第 80 轮之前行为一致)。
    fn is_frontmost(&self, _window_id: &str) -> bool {
        false
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

    /// 2026-09-17 第 76 轮 P0-1:带 WindowInfo 的 OCR(优先使用平台原生句柄)。
    ///
    /// 默认实现降级到 `ocr(window_id, region, lang)`,macOS 实装直接读取
    /// `info.cg_window_id` 避免按 PID 匹配错位。`MCP_Window_Use(action=ocr)Tool` 会先调
    /// `list_windows` 拿到完整 WindowInfo 后再调本方法,保持向后兼容。
    fn ocr_with_info(
        &self,
        info: &WindowInfo,
        region: Option<Rect>,
        lang: Option<&str>,
    ) -> Result<Vec<OcrBlock>> {
        self.ocr(&info.id, region, lang)
    }

    /// 2026-09-17 第 74 轮 T2:原生截图(落盘到指定路径)。
    ///
    /// 语义:平台原生截图实现,替代 screencapture 等外部命令。
    /// - macOS:CGWindowListCreateImage(第 86/87 轮实测:**需要屏幕录制授权**,
    ///   未授权时按窗口截取返回 null;此前「无需屏幕录制」注释为误写)
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

    /// 2026-09-17 第 76 轮 P0-1:带 WindowInfo 的截图(优先使用平台原生句柄)。
    ///
    /// 默认实现降级到 `screenshot_to(window_id, region, path)`,macOS 实装
    /// 直接使用 `info.cg_window_id`(避免按 PID 匹配错位)。`MCP_Window_Use(action=screenshot)`
    /// 工具会先调 `list_windows` 拿到完整 WindowInfo 后再调本方法。
    fn screenshot_to_with_info(
        &self,
        info: &WindowInfo,
        region: Option<Rect>,
        output_path: &std::path::Path,
    ) -> Result<Rect> {
        self.screenshot_to(&info.id, region, output_path)
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

// ===================== 目标应用自动启动兜底(2026-09-17 第 82+ 轮 P0-1) =====================
//
// 供 MCP_Window_Use(action=open) 与未来的编排层复用:
// - 先调 `auto_launch_target` 做「目标可见性探测」;
// - 不可见则用平台原生通道启动 + 等待 wait_seconds(默认 10s);
// - 返回 ready window_id 或带原因的错误。
//
// 白名单:仅已知应用(WeChat / Chrome / Safari / Firefox / Edge / Slack / Telegram /
// Discord / VSCode / iTerm2 / Terminal / QQ / Weixin / Notion / DingTalk / Feishu /
// QQMail / Outlook / Postman / Datagrip)允许自动启动,避免误启动任意 app。

/// 2026-09-17 第 82+ 轮:目标应用白名单(自动启动兜底)。
///
/// 设计要点:
/// - 大小写不敏感;含子串匹配(`WeChat` ↔ `Weixin` ↔ `微信` 全部命中);
/// - 覆盖主流桌面应用 + 微信系别名,避免误启动任意 app;
/// - 第三方平台可通过 `LAEW_AUTO_LAUNCH_EXTRA` 追加(逗号分隔),延展性。
pub fn is_target_app_allowed(query: &str) -> bool {
    let normalized = query.to_lowercase();
    let normalized = normalized.trim();
    if normalized.is_empty() || normalized.len() > 64 {
        return false;
    }
    // 黑名单字符(防止 shell 注入)
    if normalized
        .chars()
        .any(|c| matches!(c, '\0' | '\n' | '\r' | ';' | '|' | '&' | '`' | '$' | '>' | '<'))
    {
        return false;
    }
    const ALLOWED: &[&str] = &[
        // 微信系(中英文+别名)
        "wechat", "weixin", "微信",
        // 浏览器
        "chrome", "google chrome", "safari", "firefox", "edge", "chromium", "brave", "arc",
        // IDE / 编辑器
        "vscode", "visual studio code", "code", "sublime", "atom", "xcode", "intellij",
        // 终端
        "iterm", "iterm2", "terminal", "warp", "alacritty",
        // 即时通讯
        "slack", "telegram", "discord", "qq", "tim", "钉钉", "dingtalk", "飞书", "feishu", "lark",
        // 邮件
        "mail", "outlook", "thunderbird", "foxmail",
        // 笔记 / 效率
        "notion", "obsidian", "bear", "typora", "evernote", "onenote",
        // 设计 / 开发
        "figma", "sketch", "postman", "datagrip", "insomnia", "tableplus",
        // 系统
        "finder", "explorer", "preview", "previewer", "activity monitor", "活动监视器",
        // 其他常见
        "spotify", "music", "网易云", "netease", "iina", "vlc", "iina",
    ];
    // 用户扩展白名单
    if let Ok(extra) = std::env::var("LAEW_AUTO_LAUNCH_EXTRA") {
        for token in extra.split(',') {
            let t = token.trim().to_lowercase();
            if !t.is_empty() && normalized.contains(&t) {
                return true;
            }
        }
    }
    ALLOWED.iter().any(|k| normalized.contains(k))
}

/// 自动启动目标应用(2026-09-17 第 82+ 轮 P0-1)。
///
/// 与 `tools/mcp_window_use/open.rs::launch_desktop_app` 同源的平台启动解析链
/// (Windows = ShellExecuteW / 快捷方式 / 安装路径;macOS = `open -a` / `open -b`)。
///
/// 返回 ready window_id;若启动失败或不在白名单,返回带原因的错误。
/// 函数为同步阻塞(spawn_blocking 由调用方负责)。
pub fn auto_launch_target(
    app_query: &str,
    bundle_id: Option<&str>,
    wait_secs: u64,
) -> std::result::Result<WindowInfo, String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    if !is_target_app_allowed(app_query) {
        return Err(format!(
            "目标应用 {:?} 不在自动启动白名单;允许: WeChat/Chrome/Slack/Telegram/QQ/钉钉/飞书/VSCode/iTerm2/Terminal/Notion 等。LLM 可显式传目标,或通过 LAEW_AUTO_LAUNCH_EXTRA=xxx 追加。",
            app_query
        ));
    }

    // 平台启动命令构造
    #[cfg(windows)]
    let launch_cmd: Vec<String> = vec!["cmd".to_string(), "/C".to_string(), "start".to_string(), "".to_string(), app_query.to_string()];
    #[cfg(target_os = "macos")]
    let launch_cmd: Vec<String> = if let Some(b) = bundle_id.filter(|s| !s.is_empty()) {
        vec!["open".to_string(), "-b".to_string(), b.to_string()]
    } else {
        vec!["open".to_string(), "-a".to_string(), app_query.to_string()]
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let launch_cmd: Vec<String> = vec!["gtk-launch".to_string(), app_query.to_string()];

    // 优先尝试 shell 命令
    let shell_attempt = || -> std::result::Result<(), String> {
        if launch_cmd.is_empty() {
            return Err("当前平台不支持自动启动".into());
        }
        let status = Command::new(&launch_cmd[0])
            .args(&launch_cmd[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("执行启动命令失败: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("启动命令退出码 {status}"))
        }
    };

    let shell_ok = shell_attempt();
    if let Err(e) = &shell_ok {
        tracing::warn!(app = %app_query, error = %e, "auto_launch_target shell 启动失败,等待 MCP_Window_Use(action=list) 探测");
    }

    // 等待 + 探测
    let driver = current_driver();
    let aliases: Vec<String> = if app_query.to_lowercase().contains("wechat")
        || app_query.to_lowercase().contains("weixin")
        || app_query.contains("微信")
    {
        vec!["WeChat".into(), "微信".into(), "Weixin".into()]
    } else {
        vec![app_query.to_string()]
    };
    let deadline = Instant::now() + Duration::from_secs(wait_secs.min(30));
    let mut last_err = String::new();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        match driver.list_windows(None) {
            Ok(wins) => {
                for w in &wins {
                    let title_l = w.title.to_lowercase();
                    let proc_l = w.process_name.to_lowercase();
                    if aliases.iter().any(|a| {
                        let a = a.to_lowercase();
                        title_l.contains(&a) || proc_l.contains(&a)
                    }) {
                        return Ok(w.clone());
                    }
                }
            }
            Err(e) => last_err = format!("MCP_Window_Use(action=list) 失败: {e}"),
        }
    }

    Err(format!(
        "自动启动 {app_query:?} 后 {wait_secs}s 内未匹配到窗口({aliases:?});shell={:?}, last_err={last_err}",
        shell_ok.as_ref().err()
    ))
}

// ===================== 平台权限快速检测(2026-09-17 第 77 轮 P0-1/P0-2) =====================
//
// 背景:窗口操控任务此前依赖 `driver_preflight` 在每个工具入口触发授权等待,
// 导致 LLM 在 inspect/ocr/screenshot 全部失败时仍会反复重试
// (14-16 次迭代),浪费 200s+ 与 token。调用方需要「一次扫描报告」能力。
//
// 设计:
// - `PermissionReport` 统一结构:accessibility / screen_recording / can_ocr / can_screenshot
// - `check_platform_permissions()` 单点探测 + 缓存(进程级 OnceLock),避免每次重试
//   都触发系统调用;LLM retry 期间缓存命中 → 零开销
// - `build_permission_failure_message()` 把缺失项翻译成 LLM 可读的引导文案 + Bash
//   降级路径(osascript / cliclick / screencapture)
// - 三平台分支:
//   * macOS:accessibility(AX) + screen_recording(CGWindowListCreateImage)
//   * Windows:无 TCC 等价机制(UIA 受 AppContainer / 进程完整性影响);返回全授权
//   * Linux:依赖 xdotool/wmctrl 可执行文件存在;返回全授权(无统一机制)
//
// 落地位置:
// - MCP_Window_Use 的 open/inspect action 在返回体中携带权限状态与 next_action 引导;
//   `build_permission_failure_message(report)` 供编排层在权限缺失时注入 prompt
// - ExecutionTrace 写 `permission_missing: Vec<String>` 字段,供 QC/Debug 报告观测

/// 平台权限报告(2026-09-17 第 77 轮 P0-1)。
///
/// `granted=true` 表示对应能力可用;`false` 表示缺权限或驱动不支持。
/// `hint` 为 LLM 可读的具体引导文案(每个字段独立,缺失时才填)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PermissionReport {
    /// 平台名(macos / windows / linux)
    pub platform: String,
    /// 辅助功能 / UI Automation 等窗口控件枚举能力
    pub accessibility: bool,
    /// 屏幕录制 / 截图能力(macOS CGWindowListCreateImage 必需)
    pub screen_recording: bool,
    /// OCR 能力(综合权限 + 引擎可用性,如 Vision / Windows.Media.Ocr)
    pub can_ocr: bool,
    /// 截图能力(综合权限 + 引擎)
    pub can_screenshot: bool,
    /// 辅助功能未授权引导文案(为空表示已授权)
    pub accessibility_hint: String,
    /// 屏幕录制未授权引导文案(为空表示已授权)
    pub screen_recording_hint: String,
}

impl PermissionReport {
    /// 关键权限是否全部就绪(accessibility + screen_recording)。
    /// can_ocr / can_screenshot 跟随前两项 + 引擎可用性。
    pub fn has_critical_grants(&self) -> bool {
        self.accessibility && self.screen_recording
    }

    /// 缺失项摘要(供 trace / Debug 报告)。
    pub fn missing_summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.accessibility {
            out.push(format!("accessibility({})", self.platform));
        }
        if !self.screen_recording {
            out.push(format!("screen_recording({})", self.platform));
        }
        out
    }
}

/// 单点探测缓存(进程级 OnceLock),避免 Runner 入口每次重试都触发系统调用。
///
/// 关键设计:窗口期间权限状态可能变化(用户主动授权),故缓存仅 30 秒,
/// 让重试链路在合理时间内拿到最新状态。`LAEW_PERMISSION_CACHE_SECS` 可覆盖。
static PERMISSION_CACHE: std::sync::OnceLock<
    std::sync::Mutex<Option<(std::time::Instant, PermissionReport)>>,
> = std::sync::OnceLock::new();

/// 进程级单点权限探测(2026-09-17 第 77 轮 P0-1)。
///
/// 三平台分支:
/// - macOS:accessibility = `AXIsProcessTrustedWithOptions(NULL)`;
///   screen_recording = TCC 官方 `CGPreflightScreenCaptureAccess()`(第 87 轮起;
///   此前全屏 CGWindowListCreateImage 非空判定在 macOS 26.5 实测假阳性);
/// - Windows:全 true(UIA / SendInput 走标准用户权限);
/// - Linux:全 true(wmctrl/xdotool 是普通进程命令,无统一权限机制)。
/// 2026-09-18 第 85 轮:细粒度能力矩阵(MCP_Window_Use 各路线可用性)。
///
/// 与 `PermissionReport`(粗粒度:accessibility / screen_recording)不同,本结构按
/// 实际工具能力切分,LLM 据此选择路线:
/// - `list_find`:枚举窗口 + 找窗口(CGWindowList 不需授权,始终 true);
/// - `inspect_control`:AX / UIA 控件树路线(需 accessibility);
/// - `ocr_screenshot_cgwindow`:CGWindow + Vision / WMI 截图 OCR(**macOS 26.5 实测
///   需要屏幕录制权限**,Windows 的 GDI 不需要);
/// - `coordinate_input`:物理输入(CGEvent / SendInput,需 accessibility);
/// - `screencapture_cli`:走 `screencapture -x` / `import`(macOS 需要屏幕录制);
/// - `ax_warmup`:AXEnhancedUserInterface + AXManualAccessibility 双开关(自绘 UI 必需)。
///
/// 第 86 轮修正:之前的注释误写为"macOS 26.5 CGWindowListCreateImage 不需要屏幕录制",
/// 经实测(bundle_id 自动启动微信后多次 OCR 仍 CGWindowListCreateImage 返回 null),
/// 实测 macOS 26.5 该 API 需要屏幕录制授权,修正 capability 矩阵与文档保持一致。
#[derive(Debug, Clone, Serialize)]
pub struct WindowCapability {
    pub list_find: bool,
    pub inspect_control: bool,
    pub ocr_screenshot_cgwindow: bool,
    pub coordinate_input: bool,
    pub screencapture_cli: bool,
    pub ax_warmup: bool,
}

impl WindowCapability {
    /// 从权限报告推导能力矩阵(各平台统一的探测逻辑)。
    pub fn from_permissions(report: &PermissionReport) -> Self {
        #[cfg(target_os = "macos")]
        {
            // 第 86 轮修正:CGWindowListCreateImage 实测 macOS 26.5 需要屏幕录制权限,
            // 之前误写为"无需屏幕录制"导致 LLM 反复尝试 OCR/screenshot 全失败。
            // screencapture_cli / ocr_screenshot_cgwindow 都依赖 screen_recording。
            Self {
                list_find: true,                            // CGWindowListCopyWindowInfo 永远可用
                inspect_control: report.accessibility,      // AX 控件树
                ocr_screenshot_cgwindow: report.screen_recording, // CGWindow + Vision 需屏录
                coordinate_input: report.accessibility,      // CGEvent 输入需 AX
                screencapture_cli: report.screen_recording,  // screencapture 命令需屏录
                ax_warmup: report.accessibility,
            }
        }
        #[cfg(windows)]
        {
            Self {
                list_find: true,
                inspect_control: report.accessibility, // UIA 需启用
                ocr_screenshot_cgwindow: true, // GDI 截图不需特殊授权
                coordinate_input: report.accessibility, // SendInput 通常不限
                screencapture_cli: true, // PowerShell 可执行
                ax_warmup: false,
            }
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            Self {
                list_find: true,
                inspect_control: false,
                ocr_screenshot_cgwindow: false,
                coordinate_input: true, // xdotool 通常可执行
                screencapture_cli: true,
                ax_warmup: false,
            }
        }
    }

    /// 候选的"下一步行动"提示,LLM 据此选择路线。
    pub fn next_action_hint(&self) -> &'static str {
        match (
            self.list_find,
            self.inspect_control,
            self.ocr_screenshot_cgwindow,
            self.coordinate_input,
        ) {
            (_, true, true, true) => "全权限:inspect/control/ocr/screenshot/click_point 全部可用,任意路线组合",
            (_, true, false, true) => "AX 已授权 + 屏录未授权:inspect/control 主路线完整;ocr/screenshot 改用 CGWindow(无需屏录);坐标用窗口 bounds 估",
            (_, true, _, false) => "AX 已授权但 CGEvent 注入受阻:走 AX 控件路线(click/set_text/send_keys/scroll)",
            (_, false, true, true) => "AX 未授权 + 屏录 OK:走 ocr + click_point + type_text_submit 视觉路线,或 osascript keystroke",
            (_, false, true, false) => "AX 未授权 + CGEvent 也不可用:仅 ocr + osascript(需要单独授权);或授权 AX 后重试",
            (_, false, false, _) => "深度权限全无:仅 list/find;读/操作需先到 系统设置→隐私与安全性→辅助功能 勾选宿主终端",
        }
    }

    /// 当前 capability 简码,供 debug 日志 / agent_context 摘要用。
    pub fn tag(&self) -> String {
        format!(
            "list={}/insp={}/ocr={}/coord={}/scap={}",
            self.list_find as u8,
            self.inspect_control as u8,
            self.ocr_screenshot_cgwindow as u8,
            self.coordinate_input as u8,
            self.screencapture_cli as u8
        )
    }
}

/// 探测当前进程的实际能力矩阵(走 cache + PermissionReport 派生)。
pub fn probe_capability() -> WindowCapability {
    WindowCapability::from_permissions(&check_platform_permissions())
}

pub fn check_platform_permissions() -> PermissionReport {
    if let Some(mutex) = PERMISSION_CACHE.get() {
        if let Ok(mut cached) = mutex.lock() {
            let ttl_secs = std::env::var("LAEW_PERMISSION_CACHE_SECS")
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(30);
            if let Some((when, report)) = cached.as_ref() {
                if when.elapsed().as_secs() < ttl_secs {
                    return report.clone();
                }
            }
            let fresh = probe_platform_permissions();
            *cached = Some((std::time::Instant::now(), fresh.clone()));
            return fresh;
        }
    }
    // 第一次初始化
    let probe = probe_platform_permissions();
    let _ = PERMISSION_CACHE.set(std::sync::Mutex::new(Some((
        std::time::Instant::now(),
        probe.clone(),
    ))));
    probe
}

/// 强制刷新缓存(连续窗口操控任务之间需要拿到最新状态时调用)。
pub fn invalidate_permission_cache() {
    if let Some(mutex) = PERMISSION_CACHE.get() {
        if let Ok(mut cached) = mutex.lock() {
            *cached = None;
        }
    }
}

/// 真实探测(各平台实现)。
fn probe_platform_permissions() -> PermissionReport {
    #[cfg(target_os = "macos")]
    {
        probe_macos_permissions()
    }
    #[cfg(windows)]
    {
        PermissionReport {
            platform: "windows".into(),
            accessibility: true,
            screen_recording: true,
            can_ocr: true,
            can_screenshot: true,
            accessibility_hint: String::new(),
            screen_recording_hint: String::new(),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // Linux:wmctrl / xdotool 是普通进程命令,无统一权限机制。
        // 但 xdotool / wmctrl 不在 → 工具层会报"命令未找到",这里不影响 grant 标记。
        PermissionReport {
            platform: "linux".into(),
            accessibility: true,
            screen_recording: true,
            can_ocr: false,
            can_screenshot: false,
            accessibility_hint: String::new(),
            screen_recording_hint: String::new(),
        }
    }
}

/// macOS 权限探测:accessibility = `AXIsProcessTrustedWithOptions(NULL)`,
/// screen_recording = 探测 CGWindowListCreateImage 是否返回非空(10×10 像素测试)。
///
/// 实现细节:
/// - accessibility 探测直接调 macos_legacy::trusted_quiet,避免重复实现;
/// - screen_recording 探测是「写一个 10×10 透明 PNG 到磁盘 → 检查文件大小 > 0」,
///   不需要启动截图会话;CGWindowListCreateImage 屏幕录制未授权时返回 NULL,
///   而窗口级(`kCGWindowListOptionIncludingWindow`)即使未授权也可能返回有效图像,
///   所以这里用全屏 `kCGWindowListOptionAll` + 单像素测试:
///   - 已授权 → 创建 image 成功 → 释放 → 返回 true
///   - 未授权 → 创建 image 返回 NULL → 返回 false
/// - 该探测是 best-effort:第一次探测失败时缓存 false,后续不再尝试(避免阻塞);
///   用户授权后需手动 `LAEW_PERMISSION_CACHE_SECS=0` 重启或等待 TTL 过期
///
/// 2026-09-18 第 87 轮 P0 修正:上述「全屏截图未授权返回 NULL」的假设在 macOS 26.5
/// 实测**不成立** —— 未授权屏录时全屏 CGWindowListCreateImage 仍返回非空(壁纸
/// 图像),导致 capability_probe 误报 screen_recording=true,LLM 走视觉路线反复
/// OCR 撞墙(按窗口截 `kCGWindowListOptionIncludingWindow` 才返回 null)。
/// 故 screen_recording 主判定改用 TCC 官方 API `CGPreflightScreenCaptureAccess()`
/// (core-graphics 0.23 `ScreenCaptureAccess::preflight()`),全屏 CGWindow 测试
/// 降级为辅助参考(见 [`probe_macos_screen_recording`])。
#[cfg(target_os = "macos")]
fn probe_macos_permissions() -> PermissionReport {
    use crate::agent::window::macos_legacy::MacOsDriver;

    let accessibility = MacOsDriver::is_trusted();
    let screen_recording = probe_macos_screen_recording();

    PermissionReport {
        platform: "macos".into(),
        accessibility,
        screen_recording,
        can_ocr: accessibility && screen_recording,
        can_screenshot: accessibility && screen_recording,
        accessibility_hint: if accessibility {
            String::new()
        } else {
            // 与 macos_legacy::MACOS_AX_UNAVAILABLE_HINT 保持一致文案
            "辅助功能未授权(AX -25211 kAXErrorAPIDisabled)。授权:系统设置 → 隐私与安全性 → 辅助功能 → 勾选宿主终端(Terminal/iTerm2/VS Code);TCC 按进程启动时快照,授权后需完全退出并重开终端。授权前可降级走 Bash + osascript(已扩白名单)或 cliclick / screencapture -x $TMPDIR/x.png。".into()
        },
        screen_recording_hint: if screen_recording {
            String::new()
        } else {
            "屏幕录制未授权(CGPreflightScreenCaptureAccess=false)。授权:系统设置 → 隐私与安全性 → 屏幕录制 → 勾选宿主终端;授权后必须重启终端生效。授权前可改用 MCP_Window_Use(action=inspect) 控件树路线(纯辅助功能),或降级到 screencapture / osascript System Events 路径。".into()
        },
    }
}

/// macOS 屏幕录制权限探测(2026-09-18 第 87 轮重写)。
///
/// 主判定:`CGPreflightScreenCaptureAccess()` —— TCC 官方 preflight API,
/// 语义与实际 OCR/截图路径(按窗口 `kCGWindowListOptionIncludingWindow`)一致。
///
/// 历史教训(第 86 轮前):「全屏 CGWindowListCreateImage 非空即已授权」的假设在
/// macOS 26.5 实测不成立 —— 未授权时全屏截图返回非空壁纸图像,造成假阳性;
/// 只有按窗口截取才在未授权时返回 null。全屏测试保留为辅助参考,仅当与
/// preflight 结论不一致时打 debug 日志(便于现场排查),不影响判定。
#[cfg(target_os = "macos")]
fn probe_macos_screen_recording() -> bool {
    let preflight = core_graphics::access::ScreenCaptureAccess::default().preflight();
    // 辅助参考:全屏 1 像素测试(第 87 轮起仅作日志对照,不参与判定)。
    let cgwindow_ok = probe_macos_screen_recording_cgwindow();
    if preflight != cgwindow_ok {
        tracing::debug!(
            preflight,
            cgwindow_ok,
            "屏录探测:preflight 与全屏 CGWindow 测试不一致,以 preflight 为准\
             (macOS 未授权屏录时全屏截图返回壁纸图像,属预期假阳性)"
        );
    }
    preflight
}

/// 全屏 CGWindowListCreateImage 1 像素测试(辅助参考,第 87 轮起不作主判定)。
#[cfg(target_os = "macos")]
fn probe_macos_screen_recording_cgwindow() -> bool {
    use core_graphics::display::CGRectNull;
    use core_graphics::image::CGImage;
    use core_graphics::window::{kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionAll};
    use foreign_types::ForeignType;

    unsafe {
        let cg_image = core_graphics::window::CGWindowListCreateImage(
            CGRectNull,
            kCGWindowListOptionAll,
            0,
            kCGWindowImageBoundsIgnoreFraming,
        );
        if cg_image.is_null() {
            return false;
        }
        let img = CGImage::from_ptr(cg_image);
        let ok = img.width() > 0;
        drop(img);
        ok
    }
}

/// 把缺失项翻译成 LLM 可读的引导文案(第 77 轮 P0-1 引入,第 81 轮矩阵化,
/// 第 84 轮改写为 MCP_Window_Use action 命名)。
///
/// 用途:编排层 / 工具调用方在权限缺失时把这段文案追加到 prompt,
/// LLM 第一轮响应即可拿到与权限事实一致的可用/禁用清单,不必通过
/// 14+ 次失败自己摸索。
///
/// 按 macOS TCC 真实矩阵分两个分支输出:
///
/// - **辅助功能缺失**:仅 list/find/open + Apple Events 激活可用;
///   System Events / cliclick / CGEvent 同受一道门禁,一并列入禁用;
/// - **辅助功能 ✅ + 屏幕录制 ❌**:inspect / control(AX 主路线)
///   完整可用;仅禁 ocr / screenshot / screencapture 截图识别路线,
///   并给出「AX 深挖 / bounds 比例估坐标 / 键盘路线」三条替代识别方案。
pub fn build_permission_failure_message(report: &PermissionReport) -> String {
    let mut lines = Vec::new();
    lines.push("\n\n【平台权限快速检测(第 81 轮矩阵化)】".to_string());
    let mut missing = Vec::new();
    if !report.accessibility {
        missing.push("辅助功能(accessibility)");
    }
    if !report.screen_recording {
        missing.push("屏幕录制(screen_recording)");
    }
    if missing.is_empty() {
        lines.push("✅ 平台权限齐全,MCP_Window_Use 全部 action 正常可用。".to_string());
        return lines.join("\n");
    }

    lines.push(format!(
        "⚠️ 检测到权限缺失({} 项):{}",
        missing.len(),
        missing.join("、")
    ));

    // ---------- 分支 1:辅助功能缺失(最受限,一切 UI 读取/操控不可用) ----------
    // TCC 事实:osascript System Events / cliclick / CGEvent 注入与 MCP_Window_Use(action=inspect)
    // 同受「辅助功能」一道门禁,未授权时全部失败 —— 降级清单里绝不能出现它们。
    if !report.accessibility {
        if !report.accessibility_hint.is_empty() {
            lines.push(format!("【辅助功能授权步骤】\n  {}", report.accessibility_hint));
        }
        lines.push(
            "【可用操作(仅此 4 个)】MCP_Window_Use(action=list) / MCP_Window_Use(action=find) / MCP_Window_Use(action=open) / Bash(open -a、\
             osascript tell app to activate —— Apple Events 不受辅助功能门禁)。"
                .to_string(),
        );
        lines.push(
            "【禁止尝试(已知必败,连 1 次都不要试)】MCP_Window_Use(action=inspect) / MCP_Window_Use(action=control) / \
             MCP_Window_Use(action=ocr) / MCP_Window_Use(action=screenshot) / Bash screencapture / cliclick / \
             osascript System Events —— 它们与 inspect 同受一道辅助功能门禁,\
             未授权时全部失败。"
                .to_string(),
        );
        lines.push(
            "【策略】只能完成「启动/激活应用 + 枚举窗口」级别的工作;读取/操作 UI \
             必须等用户授权。立即在最终回答里告知用户授权步骤并结束,不要空转迭代。"
                .to_string(),
        );
        return lines.join("\n");
    }

    // ---------- 分支 2:辅助功能 ✅、屏幕录制 ❌(AX 主路线完整可用) ----------
    if !report.screen_recording {
        if !report.screen_recording_hint.is_empty() {
            lines.push(format!(
                "【屏幕录制授权步骤】\n  {}",
                report.screen_recording_hint
            ));
        }
        lines.push(
            "✅ 辅助功能已授权:**MCP_Window_Use(action=inspect) / MCP_Window_Use(action=control)(AX 控件树 + 坐标动作)完整可用,\
             这是主路线**。osascript System Events / pbcopy / pbpaste 同样可用。"
                .to_string(),
        );
        lines.push(
            "【禁止尝试(屏幕录制缺失,已知必败)】MCP_Window_Use(action=ocr) / MCP_Window_Use(action=screenshot) / \
             Bash screencapture —— 全部截图识别路线一次都不要试。"
                .to_string(),
        );
        lines.push(
            "【无 OCR 的界面识别替代路线】\n  \
             1. action=inspect 加大 max_depth(6-8)且不带 filter 深挖\
                (驱动已自动 AXEnhancedUserInterface 建树等待);\n  \
             2. 控件树仍为空(自绘 UI):按窗口 bounds **比例估算坐标**直接用坐标动作\
                (click_point 不依赖 OCR;如输入框≈窗口底部 85% 高度、搜索框≈顶部 5%),\
                操作后用 action=inspect / get_text 验证;\n  \
             3. 键盘路线:type_text_submit 直接向焦点控件键入(无需知道控件路径);\n  \
             4. 文本读取优先 AX:get_text / AXValue,其次 pbpaste\
                (先 System Events keystroke \"c\" cmd+c)。"
                .to_string(),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn permission_report_default_is_empty() {
        let r = PermissionReport::default();
        assert!(!r.has_critical_grants());
        assert!(r.missing_summary().is_empty() == false || r.missing_summary().len() == 2);
    }

    #[test]
    fn permission_report_missing_summary_works() {
        let mut r = PermissionReport::default();
        r.platform = "macos".into();
        assert_eq!(
            r.missing_summary(),
            vec!["accessibility(macos)", "screen_recording(macos)"]
        );

        r.accessibility = true;
        assert_eq!(r.missing_summary(), vec!["screen_recording(macos)"]);

        r.screen_recording = true;
        assert!(r.missing_summary().is_empty());
        assert!(r.has_critical_grants());
    }

    #[test]
    fn build_permission_message_with_all_granted() {
        let r = PermissionReport {
            platform: "macos".into(),
            accessibility: true,
            screen_recording: true,
            can_ocr: true,
            can_screenshot: true,
            accessibility_hint: String::new(),
            screen_recording_hint: String::new(),
        };
        let msg = build_permission_failure_message(&r);
        assert!(msg.contains("✅"));
        assert!(!msg.contains("⚠️"));
        assert!(!msg.contains("禁止尝试"));
    }

    #[test]
    fn build_permission_message_ax_only_screencapture_missing() {
        // 第 81 轮核心回归:辅助功能 ✅ + 屏幕录制 ❌(14:38 场次的真实状态)。
        // 必须主推 MCP_Window_Use(action=inspect)/MCP_Window_Use(action=control)(可用),只禁截图识别路线。
        let r = PermissionReport {
            platform: "macos".into(),
            accessibility: true,
            screen_recording: false,
            can_ocr: false,
            can_screenshot: false,
            accessibility_hint: String::new(),
            screen_recording_hint: "授权步骤...".into(),
        };
        let msg = build_permission_failure_message(&r);
        assert!(msg.contains("⚠️"));
        assert!(msg.contains("屏幕录制"));
        // AX 主路线必须被肯定(不再一刀切禁令 MCP_Window_Use(action=inspect))
        assert!(
            msg.contains("完整可用"),
            "辅助功能已授权时必须明确 MCP_Window_Use(action=inspect)/MCP_Window_Use(action=control) 可用: {msg}"
        );
        assert!(msg.contains("MCP_Window_Use(action=inspect) / MCP_Window_Use(action=control)"));
        // 截图识别路线必须整体禁止
        assert!(msg.contains("禁止尝试"));
        assert!(msg.contains("MCP_Window_Use(action=ocr) / MCP_Window_Use(action=screenshot)"));
        // 不得再建议 screencapture 作为可用降级路径(第 77 轮 bug)
        assert!(
            !msg.contains("screencapture -x"),
            "不得把 screencapture 列为可用降级路径: {msg}"
        );
        // 必须给出无 OCR 的替代识别路线
        assert!(msg.contains("比例估算坐标"));
        assert!(msg.contains("type_text_submit"));
    }

    #[test]
    fn build_permission_message_accessibility_missing_forbids_all_ui_tools() {
        // 辅助功能 ❌:System Events / cliclick 与 MCP_Window_Use(action=inspect) 同受一道门禁,
        // 全部列入禁用;只保留 MCP_Window_Use(action=list)/Find/Open + Apple Events 激活。
        let r = PermissionReport {
            platform: "macos".into(),
            accessibility: false,
            screen_recording: false,
            can_ocr: false,
            can_screenshot: false,
            accessibility_hint: "授权步骤...".into(),
            screen_recording_hint: "授权步骤...".into(),
        };
        let msg = build_permission_failure_message(&r);
        assert!(msg.contains("⚠️"));
        assert!(msg.contains("辅助功能"));
        assert!(msg.contains("仅此 4 个"));
        assert!(msg.contains("MCP_Window_Use(action=list) / MCP_Window_Use(action=find) / MCP_Window_Use(action=open)"));
        assert!(msg.contains("禁止尝试"));
        assert!(msg.contains("MCP_Window_Use(action=inspect)"));
        assert!(msg.contains("System Events"));
        assert!(msg.contains("不要空转迭代"));
    }
}

/// 2026-09-16 第 67 轮:window_id → HWND(供 tools/mcp_window_use/vision.rs 的
/// Windows 截图路径直接拿句柄;非 Windows 平台不存在本函数)。
#[cfg(windows)]
pub fn windows_driver_hwnd(window_id: &str) -> Result<::windows::Win32::Foundation::HWND> {
    windows::WindowsDriver::parse_hwnd(window_id)
}

/// 2026-09-16 第 67 轮:按窗口(可选区域)截图并**保存到指定路径**
/// (MCP_Window_Use(action=screenshot) 的 Windows 纯 Rust 路径;返回实际截取的屏幕矩形)。
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
/// 第 66 轮:提升为 pub(crate),供 tools/window.rs 的 MCP_Window_Use(action=find) 打分复用。
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
    fn control_action_parse_type_text_submit() {
        // 2026-09-17 第 80 轮:原子发送动作是跨平台工具契约。
        assert_eq!(
            ControlAction::parse_ext("type_text_submit", Some("hello".into()), None, None).unwrap(),
            ControlAction::TypeTextSubmit {
                text: "hello".into(),
                x: None,
                y: None
            }
        );
        assert_eq!(
            ControlAction::parse_ext("sendtext", Some("发送".into()), Some(120), Some(240))
                .unwrap(),
            ControlAction::TypeTextSubmit {
                text: "发送".into(),
                x: Some(120),
                y: Some(240)
            }
        );
        assert!(
            ControlAction::parse_ext("type_text_submit", None, None, None).is_err(),
            "缺少 text 时必须结构化报错"
        );
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

    // ===== 2026-09-17 第 82+ 轮 P0-1 测试:目标应用白名单 =====
    #[test]
    fn is_target_app_allowed_accepts_common() {
        // 常见桌面应用 / 微信系全部放行
        assert!(is_target_app_allowed("WeChat"));
        assert!(is_target_app_allowed("wechat"));
        assert!(is_target_app_allowed("Weixin"));
        assert!(is_target_app_allowed("微信"));
        assert!(is_target_app_allowed("Chrome"));
        assert!(is_target_app_allowed("google chrome"));
        assert!(is_target_app_allowed("Slack"));
        assert!(is_target_app_allowed("Telegram"));
        assert!(is_target_app_allowed("QQ"));
        assert!(is_target_app_allowed("钉钉"));
        assert!(is_target_app_allowed("VSCode"));
        assert!(is_target_app_allowed("iTerm2"));
        assert!(is_target_app_allowed("Notion"));
        assert!(is_target_app_allowed("Figma"));
    }

    #[test]
    fn is_target_app_allowed_rejects_unknown_or_dangerous() {
        // 未知应用 / shell 注入字符 → 拒绝
        assert!(!is_target_app_allowed(""));
        assert!(!is_target_app_allowed("rm -rf /"));
        assert!(!is_target_app_allowed("random_unknown_app_xyz"));
        // 长度超限
        assert!(!is_target_app_allowed(&"a".repeat(100)));
        // shell 注入字符
        assert!(!is_target_app_allowed("Chrome; rm -rf /"));
        assert!(!is_target_app_allowed("WeChat && echo evil"));
        assert!(!is_target_app_allowed("`whoami`"));
    }
}
