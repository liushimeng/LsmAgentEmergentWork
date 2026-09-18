//! MCP_Window_Use 工具(2026-09-18 第 84 轮):桌面软件窗口操控的统一 MCP 风格入口。
//!
//! 由原 WindowUse Agent(第 9 角色)的 7 个独立工具(WindowOpen / WindowList /
//! WindowFind / WindowInspect / WindowAction / WindowOCR / WindowScreenshot)收敛而来:
//! 单工具 + `action` 枚举分发,结构化 JSON 入参/出参,由持有该工具的 Agent
//! (SubAgent-Work)在自身多轮对话循环中调用。
//!
//! - 平台差异封闭在 `agent::window` 驱动层(本工具的"MCP 服务"实现),
//!   本模块只做参数校验、`spawn_blocking` 包裹(UIA COM / AX IPC 可能阻塞)与输出截断;
//! - 控件树输出双重截断(节点数 ≤ 500,序列化 ≤ 32KB),防止 tool_result 打爆上下文;
//! - 错误信息 LLM 可读:权限缺失给开权限步骤,控件未找到给重新检视提示;
//! - **平台门控**:仅 macOS / Windows 注册本工具(见 [`mcp_window_use_available`]),
//!   其余平台不出现在工具列表与系统提示词中。
//!
//! 设计见 `docs/MCP_Window_Use/01-设计与解决方案.md`。
//! 平台技术参考:`docs/MCP_Window_Use/MCP_Window_Use_MacOS_技术文档.md` /
//! `docs/MCP_Window_Use/MCP_Window_Use_Window_技术文档.md`。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::window::{
    current_driver, normalize_unicode_for_match, ControlNode, WindowInfo,
};
use crate::error::{AgentError, Result};

mod inspect;
mod open;
mod query;
mod vision;

#[cfg(test)]
mod tests;

/// 工具名(LLM 可见的唯一窗口操控入口)。
pub const MCP_WINDOW_USE_TOOL_NAME: &str = "MCP_Window_Use";

/// 控件树节点数上限(超出截断)。
const MAX_TREE_NODES: usize = 500;
/// 控件树序列化字节上限(超出截断)。
const MAX_TREE_BYTES: usize = 32 * 1024;
/// 窗口列表条数上限。
const MAX_WINDOWS: usize = 200;

/// 平台门控:仅 macOS / Windows 定义并注册 MCP_Window_Use 工具。
///
/// 运行期 `cfg!` 判定(非条件编译):驱动层在所有平台均可编译(Linux 走
/// fallback 后端),但工具只在桌面双平台出现在注册表与系统提示词中。
pub fn mcp_window_use_available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

// ===================== 共享辅助(自 tools/window 迁入) =====================

pub(crate) fn tool_err(tool: &str, reason: impl Into<String>) -> AgentError {
    AgentError::ToolExecution {
        tool: tool.into(),
        reason: reason.into(),
    }
}

pub(crate) fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(crate) fn require_str<'a>(args: &'a Value, key: &str, tool: &str) -> Result<&'a str> {
    get_str(args, key).ok_or_else(|| tool_err(tool, format!("缺少 string 类型参数 {key}")))
}

/// 扩展常见桌面应用的中英文窗口 / 进程名别名。
///
/// - macOS 微信的 `kCGWindowOwnerName` 可能是「微信」,而模型常按英文查询「WeChat」;
/// - **Windows 微信 4.x 的进程名是 `Weixin.exe`**,3.x 才是 `WeChat.exe` —— 缺这个
///   别名会导致「窗口在眼前也找不到」。
pub fn expand_window_query(query: &str) -> Vec<String> {
    let mut out = vec![query.to_string()];
    let lower = query.to_lowercase();
    if lower.contains("wechat") || lower.contains("weixin") || query.contains("微信") {
        for alias in ["WeChat", "微信", "Weixin"] {
            if !out.iter().any(|s| s.eq_ignore_ascii_case(alias)) {
                out.push(alias.to_string());
            }
        }
    }
    out
}

pub(super) fn window_matches_any(w: &WindowInfo, queries: &[String]) -> bool {
    // 双侧 Unicode 上标归一化后再小写子串匹配
    queries.iter().any(|q| {
        let q = normalize_unicode_for_match(q).to_lowercase();
        normalize_unicode_for_match(&w.title)
            .to_lowercase()
            .contains(&q)
            || normalize_unicode_for_match(&w.process_name)
                .to_lowercase()
                .contains(&q)
    })
}

pub(super) fn find_window_by_aliases(
    windows: &[WindowInfo],
    aliases: &[String],
) -> Option<(String, WindowInfo)> {
    aliases.iter().find_map(|q| {
        query::pick_top_hit(windows, q, query::MatchMode::Contains)
            .map(|h| (q.clone(), h.info.clone()))
    })
}

/// 统计控件树节点数。
pub(super) fn count_nodes(node: &ControlNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

/// 深度优先裁剪子树,使总节点数 ≤ budget。
fn prune_nodes(node: &mut ControlNode, budget: &mut usize) {
    let mut kept = Vec::with_capacity(node.children.len());
    for mut child in std::mem::take(&mut node.children) {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        prune_nodes(&mut child, budget);
        kept.push(child);
    }
    node.children = kept;
}

/// 把控件树序列化为带截断标注的 JSON 字符串。
pub(super) fn tree_to_json(mut root: ControlNode) -> String {
    let total = count_nodes(&root);
    let mut truncated = false;
    if total > MAX_TREE_NODES {
        let mut budget = MAX_TREE_NODES;
        prune_nodes(&mut root, &mut budget);
        truncated = true;
    }
    let mut s = serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".into());
    if s.len() > MAX_TREE_BYTES {
        s.truncate(MAX_TREE_BYTES);
        s.push_str("\n... [truncated: 输出超过 32KB]");
        truncated = true;
    }
    if truncated {
        s.push_str(&format!(
            "\n[truncated] 控件树共 {total} 个节点,仅展示前 {} 个;可用 max_depth 调小或 filter 过滤后重试",
            MAX_TREE_NODES.min(total)
        ));
    }
    s
}

/// 驱动不可用/缺权限时的前置检查(inspect / control 需要,list / find / open 不需要)。
///
/// macOS 上未授权时自动触发系统授权弹窗:首次调用 inspect/control 时,如果辅助功能
/// 未授权,主动调用 `AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})`
/// 触发系统弹窗,引导用户授权。
pub(super) async fn driver_preflight(tool: &str) -> Result<()> {
    let driver = current_driver();
    if let Some(hint) = driver.permission_hint() {
        // macOS 未授权:尝试自动触发授权弹窗
        #[cfg(target_os = "macos")]
        {
            if hint.contains("辅助功能未授权") {
                return match try_request_ax_permission(tool).await {
                    AxPermissionResult::Granted => Ok(()), // 授权成功,继续执行
                    AxPermissionResult::Denied { message } => {
                        // 授权失败/用户拒绝,返回带引导的提示
                        Err(tool_err(tool, message))
                    }
                };
            }
        }
        // 其他平台或 AX 常量初始化失败:返回原提示
        return Err(tool_err(tool, hint));
    }
    Ok(())
}

/// macOS 辅助功能授权请求结果。
#[cfg(target_os = "macos")]
enum AxPermissionResult {
    /// 授权成功(用户已授权或弹窗后授权)。
    Granted,
    /// 授权失败/用户拒绝。
    Denied { message: String },
}

/// 尝试请求 macOS 辅助功能授权。
///
/// 触发系统授权弹窗,等待用户响应后返回结果。
/// - 用户已授权(弹窗前) → Granted
/// - 弹窗后用户授权 → Granted
/// - 用户拒绝/忽略/超时 → Denied(带引导文案)
#[cfg(target_os = "macos")]
async fn try_request_ax_permission(tool: &str) -> AxPermissionResult {
    use crate::agent::window::MacOsDriver;
    use std::time::{Duration, Instant};

    // 先检查是否已授权(避免重复弹窗)
    if MacOsDriver::is_trusted() {
        return AxPermissionResult::Granted;
    }

    let wait_secs = ax_permission_wait_secs();
    let started = Instant::now();
    let mut next_prompt = Instant::now();
    let mut prompt_count = 0usize;
    loop {
        if MacOsDriver::request_permission() {
            eprintln!("  [{tool}权限] ✓ macOS 辅助功能已授权,继续执行");
            return AxPermissionResult::Granted;
        }
        if prompt_count == 0 {
            eprintln!(
                "  [{tool}权限] macOS 辅助功能未授权;已弹出系统授权引导,最多等待 {wait_secs}s"
            );
        }
        prompt_count += 1;
        if started.elapsed().as_secs() >= wait_secs {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
        if Instant::now() >= next_prompt {
            next_prompt = Instant::now() + Duration::from_secs(10);
            if MacOsDriver::request_permission() {
                eprintln!("  [{tool}权限] ✓ macOS 辅助功能已授权,继续执行");
                return AxPermissionResult::Granted;
            }
            eprintln!(
                "  [{tool}权限] 等待授权中 {}/{}s;请在系统设置 → 隐私与安全性 → 辅助功能添加/勾选宿主终端",
                started.elapsed().as_secs(),
                wait_secs
            );
        }
    }

    let message = format!(
        "辅助功能授权等待 {wait_secs}s 后仍未成功(已请求 {prompt_count} 次)。\n\
         请完成:系统设置 → 隐私与安全性 → 辅助功能 → 添加并勾选运行 laew 的宿主终端\
         (Terminal/iTerm2/VS Code);如系统提示,输入管理员密码。\n\
         TCC 对进程启动时状态有快照语义:授权后通常需要完全退出并重开宿主终端。\
         当前任务已保留 Session ID;MCP_Window_Use 的 open/list/find 无需该权限,\
         但 inspect/control 与 System Events 键盘注入必须授权。"
    );
    AxPermissionResult::Denied { message }
}

/// `LAEW_AX_WAIT_SECS` 控制授权等待;0 关闭,默认 120 秒,非法值回退默认。
#[cfg(target_os = "macos")]
fn ax_permission_wait_secs() -> u64 {
    std::env::var("LAEW_AX_WAIT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(120)
}

pub(crate) async fn run_blocking<F>(tool: &str, f: F) -> Result<String>
where
    F: FnOnce() -> Result<String> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| tool_err(tool, format!("窗口驱动任务 join 失败: {e}")))?
}

// ===================== MCP_Window_Use 工具门面 =====================

/// 桌面软件窗口操控统一入口(MCP 风格单工具 + action 分发)。
pub struct McpWindowUseTool;

/// 工具描述:同时承担「使用说明」职责(原 WindowUse Agent 系统提示词的工具部分精炼)。
const MCP_WINDOW_USE_DESCRIPTION: &str = r#"通过软件窗口读取与操作桌面软件(macOS / Windows 桌面 GUI 自动化统一入口,MCP 风格单工具多 action)。
用 action 参数选择操作:
- open(query*, app_name?, bundle_id?, wait_seconds?): 启动/激活桌面应用并等待窗口出现,返回 window_id/权限状态/next_action;已运行则恢复前置,不重复启动。支持 WeChat↔微信↔Weixin 等中英文别名。
- list(filter?): 枚举当前桌面全部可见顶层窗口(id/title/进程名/PID/位置尺寸)。macOS 走 CoreGraphics,不需要辅助功能授权。
- find(query*, match_mode?): 按标题/进程名子串找单一最佳窗口,直接返回 window_id(exact/contains/fuzzy,默认 contains)。
- inspect(window_id*, max_depth?, filter?): 枚举窗口控件树(Windows UIA / macOS AX),每个控件含 path(如 /0/2/1)/role/name/value/bounds/actions。需要 macOS 辅助功能授权。
- control(window_id*, path*, control_action*, text?, x?, y?): 对窗口执行操作,双路线——
  控件树路线(原生 UI):path 取 inspect 返回的控件路径,control_action=click/invoke/focus/set_text/get_text/send_keys/scroll/scroll_to_visible;
  视觉坐标路线(自绘 UI,如微信 4.x 控件树为空):control_action=click_point/double_click_point/right_click_point/scroll_point/type_text/type_text_submit,x/y 传屏幕绝对坐标(取 ocr 返回的 screen_cx/screen_cy),path 照传 "/"。
- ocr(window_id*, region?, lang?): 窗口 OCR 文字识别,返回词块文本 + 窗口相对坐标 + 屏幕绝对坐标(视觉路线入口)。需要 macOS 屏幕录制授权。
- screenshot(window_id?, output_path?, region?): 截图落盘 PNG,返回路径。只做截图不做识别;需要识别文字一律用 ocr。

【标准作业顺序】open(应用未启动)→ find/list(定位 window_id)→ inspect 或 ocr(理解界面)→ control(操作)→ inspect/ocr 复查。open 已返回 window_id 时直接复用,不要重复启动。
【双路线决策】inspect 控件树为空/只有少量 Pane(自绘 UI / Electron canvas)时立即切换视觉路线 ocr + click_point/type_text_submit,不要反复重试 inspect。
【权限矩阵(macOS)】辅助功能未授权 → 仅 open/list/find 可用,inspect/control/ocr/screenshot 一次都不要试,直接告知用户授权步骤;辅助功能✅+屏幕录制❌ → inspect/control 主路线完整可用,仅 ocr/screenshot 不可用,坐标用窗口 bounds 比例估算。
【filter 失败同义词表】通讯录/通信录/联系人/Contacts、按钮/Button、输入框/搜索/Search/TextField/Edit、关闭/X/退出、设置/Settings/Preferences;目标名含 Unicode 上标(如 ᴬᴵᴬ)时用 ASCII 归一形(AIA)。
【发送消息范式】首选 control(control_action=type_text_submit, text=完整内容) 一调用完成「点击输入框+键入+Enter 提交」;仅当应用把 Enter 定义为换行时才拆成 type_text + click「发送」。
【安全红线】禁止对支付/删除/发送/确认类按钮做无把握点击,必须点击时在最终回答说明点了什么、为什么;只读优先:能 list/inspect/get_text 回答的不操作;禁止用 Read 读取 screenshot 产出的 PNG;3 轮无进展立即止损,不要重复相同失败操作。"#;

#[async_trait]
impl Tool for McpWindowUseTool {
    fn name(&self) -> &str {
        MCP_WINDOW_USE_TOOL_NAME
    }

    fn description(&self) -> &str {
        MCP_WINDOW_USE_DESCRIPTION
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "list", "find", "inspect", "control", "ocr", "screenshot"],
                    "description": "要执行的窗口操作:open(启动/激活应用) / list(枚举窗口) / find(查窗口) / inspect(控件树) / control(执行操作) / ocr(文字识别) / screenshot(截图)"
                },
                "query": { "type": "string", "description": "open/find 必填:应用名或窗口标题/进程名查询词(大小写不敏感,支持中英文别名)" },
                "app_name": { "type": "string", "description": "open 可选:启动用应用名或完整路径,缺省=query" },
                "bundle_id": { "type": "string", "description": "open 可选:macOS Bundle ID(open -b 启动)" },
                "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 30, "description": "open 可选:启动后等待窗口出现秒数,默认 10" },
                "filter": { "type": "string", "description": "list/inspect 可选:窗口标题/进程名或控件名/角色子串过滤" },
                "match_mode": { "type": "string", "enum": ["exact", "contains", "fuzzy"], "description": "find 可选:匹配模式,默认 contains" },
                "window_id": { "type": "string", "description": "inspect/control/ocr/screenshot 必填:open/list/find 返回的窗口 id" },
                "path": { "type": "string", "description": "control 必填:控件路径(如 /0/2/1;\"/\" 表示窗口本身,坐标动作照传)" },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 12, "description": "inspect 可选:控件树遍历深度,默认 3" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "invoke", "focus", "set_text", "get_text", "send_keys", "scroll", "scroll_to_visible",
                        "click_point", "double_click_point", "right_click_point", "scroll_point", "type_text", "type_text_submit"
                    ],
                    "description": "control 必填:控件树路线(click/set_text/get_text/send_keys/scroll 等)或视觉坐标路线(click_point/type_text_submit 等)"
                },
                "text": { "type": "string", "description": "control 可选:set_text/send_keys/type_text/type_text_submit 的文本;scroll/scroll_point 传方向行数(如 \"down:3\")" },
                "x": { "type": "integer", "description": "control 坐标动作必填:屏幕绝对 X(取 ocr 返回的 screen_cx 中心)" },
                "y": { "type": "integer", "description": "control 坐标动作必填:屏幕绝对 Y(取 ocr 返回的 screen_cy 中心)" },
                "region": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "width": { "type": "integer" },
                        "height": { "type": "integer" }
                    },
                    "required": ["x", "y", "width", "height"],
                    "description": "ocr/screenshot 可选:窗口相对区域(物理像素)"
                },
                "lang": { "type": "string", "description": "ocr 可选:BCP-47 语言标签,默认系统语言" },
                "output_path": { "type": "string", "description": "screenshot 可选:输出 PNG 路径,默认临时目录" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let action = require_str(&args, "action", self.name())?;
        match action {
            "open" => open::run(args).await,
            "list" => query::run_list(args).await,
            "find" => query::run_find(args).await,
            "inspect" => inspect::run_inspect(args).await,
            "control" => inspect::run_control(args).await,
            "ocr" => vision::run_ocr(args).await,
            "screenshot" => vision::run_screenshot(args).await,
            other => Err(tool_err(
                self.name(),
                format!(
                    "未知 action={other:?};合法值:open / list / find / inspect / control / ocr / screenshot"
                ),
            )),
        }
    }
}
