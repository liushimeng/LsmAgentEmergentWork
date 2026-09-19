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

mod chat;
mod inspect;
mod input_batch;
mod open;
mod query;
mod sequence;
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
- open(query*, app_name?, bundle_id?, wait_seconds?): 启动/激活桌面应用并等待窗口出现,返回 window_id/权限状态/next_action;已运行则恢复前置,不重复启动。**支持 bundle_id 优先 + WeChat↔微信↔Weixin 等中英文别名**——若 `open -a 微信` 失败,工具会自动查已知映射表走 `open -b com.tencent.xinWeChat` 兜底。
- list(filter?): 枚举当前桌面全部可见顶层窗口(id/title/进程名/PID/位置尺寸)。macOS 走 CoreGraphics,不需要任何授权。
- find(query*, match_mode?): 按标题/进程名子串找单一最佳窗口,直接返回 window_id(exact/contains/fuzzy,默认 contains)。
- inspect(window_id*, max_depth?, filter?): 枚举窗口控件树(Windows UIA / macOS AX),每个控件含 path(如 /0/2/1)/role/name/value/bounds/actions。需要 macOS 辅助功能授权。
- control(window_id*, path*, control_action*, text?, x?, y?, x2?, y2?, modifiers?): 对窗口执行操作,双路线——
  控件树路线(原生 UI,**无障碍 API → Windows 消息 → 物理鼠标键盘 兜底的优先级链**,返回文案 route= 标注实际路线):path 取 inspect 返回的控件路径,control_action=click/invoke/focus/set_text/get_text/send_keys/scroll/scroll_to_visible(send_keys 第 87 轮起支持 cmd/ctrl/alt/shift 组合键与字母/数字键,如 cmd+f 微信搜索联系人);
  视觉坐标路线(自绘 UI,如微信 4.x 控件树为空;物理鼠标键盘注入):control_action=click_point/double_click_point/right_click_point/scroll_point/type_text/type_text_submit + **第 90 轮新增 move_point(悬停)/ middle_click_point(中键)/ drag_point(拖拽,x/y 起点 + x2/y2 终点)**;x/y 传屏幕绝对坐标(取 ocr 返回的 screen_cx/screen_cy),path 照传 "/";
  **modifiers**(第 90 轮新增,"ctrl"/"ctrl+shift" 等):修饰键 + 鼠标同时操作 —— ctrl+click_point 多选 / shift+click_point 区选 / ctrl+drag_point 拖拽复制。
- ocr(window_id*, region?, lang?): 窗口 OCR 文字识别,返回词块文本 + 窗口相对坐标 + 屏幕绝对坐标(视觉路线入口)。**macOS 26.5 实测需要屏幕录制授权**(CGWindowListCreateImage + Vision 都走 TCC 屏录门控,屏录未授权时返回空);screen_recording=false 时直接改走 chat_send(osascript_fallback)。
- screenshot(window_id?, output_path?, region?): 截图落盘 PNG,返回路径。只做截图不做识别;需要识别文字一律用 ocr。macOS 上需要屏幕录制授权(屏录未授权时 screencapture 也失败)。
- capability_probe(): **第 86 轮新增**——返回当前进程真实能力矩阵 `{accessibility, screen_recording, ocr_screenshot_cgwindow, screencapture_cli, inspect_control, coordinate_input, ax_warmup}` 与 next_action 推荐;LLM 第一步必须先调此 action,再决定走 AX/视觉/osascript_fallback 哪条路线。无参数。
- osascript_run(osascript_script*, timeout_ms?): **第 86 轮新增,第 88 轮重写**——直接调 osascript 执行 AppleScript 片段,无需走 BashTool(绕开白名单)。macOS only。**argv 直传不经 shell**:脚本内双引号/反斜杠/多行 tell 块原样生效,不要再做 shell 转义;返回真实 exit_code + stderr,ok=false 时先按 stderr 修正脚本重试,**禁止改用 BashTool 执行 osascript 绕行**(窗口操控必须全程 MCP_Window_Use)。LLM 需要 System Events 键盘注入等场景使用。
- chat_send(window_id*, text*, click_point?, input_field_path?, submit_key?, verify?, chat_log_path?): **复合 action**——一次调用完成「前置窗口 + 前台守卫 + 点击输入框 + Unicode 键入 + Enter + OCR 验证发送」,自动按 WindowCapability 选路线(控件树 / 视觉坐标 / osascript_fallback / 降级提示)。微信/钉钉/飞书 发送消息首选。**第 88 轮新增前台焦点守卫**:所有路线发送前先把目标窗口前置并轮询确认 frontmost,1.5s 拿不到前台就报 focus_acquire 失败**不盲打**(防止用户切窗后消息打进别的软件);osascript_fallback 路线 keystroke 前自动按窗口 bounds 比例估算点击右下输入框聚焦,Enter 前二次校验前台。**第 86 轮新增 osascript_fallback 路线**(AX 已授权 + 屏录未授权时,走 System Events keystroke,不依赖截图)。**chat_log_path 指定后,每条 send/recv/fail/focus_lost 落盘到该文件**。
- chat_loop(window_id*, messages*, interval_seconds?, max_rounds?, reply_detect?, stop_on_reply?, target_query?, chat_log_path?):
   (+第 91 轮 P0-4:click_point?/input_field_path? 透传 chat_send) **复合 action**——长时多轮会话循环,工具内部循环 chat_send + OCR 检测对方回复,返回结构化 `{rounds, sent, replies, reply_rate, focus_aborted, log}`。LLM 一次调用就能跑 N 轮聊天。**第 88 轮新增:连续 3 轮前台守卫失败自动止损中止**(focus_aborted=true),防止用户离开期间消息误发到其他软件。**chat_log_path 同样支持**。
- input_batch(window_id*, steps*, continue_on_error?): **第 90 轮新增复合 action**——一次调用编排「鼠标 + 键盘 + 控件树」任意顺序多步操作(同时操作鼠标键盘的统一入口),一次前台守卫 + 批量执行,步骤间零返场零焦点竞态。steps 数组每步 {\"op\":...}:
  鼠标:mouse_move{x,y} / mouse_click{x,y,button?=left|right|middle,clicks?=1|2,modifiers?} / mouse_drag{x,y,x2,y2,modifiers?} / mouse_scroll{x,y,direction?=down,lines?=3};
  键盘:key_press{keys}(组合键如 ctrl+a) / type_text{text};
  控件树(自动走无障碍→消息→物理优先级链):click{path} / set_text{path,text} / get_text{path,key?}(读取值进 results 回传);
  wait{ms}(≤5000)。每步可选 delay_ms(≤2000);steps ≤ 40;整批 ≤ 60s;默认失败即停,continue_on_error=true 继续。典型:点输入框→ctrl+a→type_text→enter 一次完成;滑块/文件用 mouse_drag;多选用 mouse_click+modifiers=ctrl。
- run_sequence(window_id*, steps*, on_error?, retry_times?, retry_delay_ms?, focus_guard?, focus_wait_ms?, max_total_ms?, log_path?): **第 91 轮新增「连续工作模式」复合 action**——先摸索排查窗口结构(capability_probe→open/list/find→inspect/ocr),统一 plan 之后,把一整套动作(含验证/等待)一次调用连续执行完毕,逐步落盘执行记录,异常按策略处理。
  在 input_batch 10 个 op 之上新增 3 个验证/等待 op:assert_text{path,contains}(关键节点断言)/ wait_for_text{path,contains,timeout_ms≤30000,poll_ms}(轮询等异步 UI 就绪,替代盲 wait)/ wait_front{timeout_ms≤30000}(显式恢复前台);wait 上限放宽到 30000。
  **逐步焦点守护(focus_guard 默认 true)**:每个物理输入步骤(mouse_click/mouse_drag/mouse_scroll/key_press/type_text/click)执行前确认窗口仍在前台——用户随时会用键盘鼠标切窗;焦点丢失自动 bring_to_front + 轮询 ≤focus_wait_ms(默认 5000)重夺,连续 3 次重夺失败止损中止(focus_aborted=true,防止误输入其他软件,与 chat_loop 止损同源)。
  **错误策略 on_error**:abort(默认,失败即停,剩余标记 skipped)/ continue(记 failed_steps 继续)/ retry(逐步自动重试,retry_times≤3 默认 1、retry_delay_ms≤5000 默认 500,步骤级同名键覆盖);步骤级 optional=true 容忍非关键步骤失败。
  **执行记录**:每步落盘 [STEP]/[RETRY]/[FOCUS_LOST]/[FOCUS_REGAINED]/[FAIL] + 末尾 [SUMMARY] 到 log_path(默认 <工作目录>/laew_sequence_<unix_ts>.log);返回 steps 逐步记录 + results(get_text 读取值)+ failed_steps + focus_lost_count。
  护栏:steps ≤ 100;整批默认 ≤180s(max_total_ms 可调,≤600s)。典型:点会话→wait_for_text 等列表刷新→click 控件→type_text→key_press enter→assert_text 验证已发送,一次调用完成。

【第 86 轮 · capability_probe_first 原则】**第一步必须是 `MCP_Window_Use(action=capability_probe)` 拿到真实能力矩阵**,再决定下一步。capability.ocr_screenshot_cgwindow=false 表示截图/OCR 完全不可用,此时**禁止**重试 screenshot/ocr,直接走 `chat_send(osascript_fallback)` 或 `chat_loop`。
【标准作业顺序】open(应用未启动)→ find/list(定位 window_id)→ capability_probe(拿真实能力)→ inspect 或 ocr(理解界面,前提 cap=true)→ control/chat_send(单步/发送)→ **run_sequence(第 91 轮:探索后统一 plan 的多步连续执行,含 assert_text/wait_for_text 验证等待、retry 重试、逐步焦点守护、逐步落盘记录;人机共用机器、用户会切窗的场景一律用它,不要拆成多次 control)** → inspect/ocr 复查 / chat_loop(批量会话)。
【双路线决策】inspect 控件树为空/只有少量 Pane(自绘 UI / Electron canvas)时立即切换视觉路线 ocr + click_point/type_text_submit(前提 cap.ocr_screenshot_cgwindow=true),或切到 osascript_fallback(AX 已授权但屏录未授权时)。
【权限矩阵(macOS 第 86 轮实测)】辅助功能未授权 → 仅 open/list/find/capability_probe/osascript_run 可用;辅助功能✅+屏幕录制❌ → inspect/control 主路线完整可用,ocr/screenshot 全部走 CGWindow 也需屏录(实测失败),**chat_send 自动走 osascript_fallback 路线**(System Events keystroke 只需 AX);全✅→所有路线全开。
【filter 失败同义词表】通讯录/通信录/联系人/Contacts、按钮/Button、输入框/搜索/Search/TextField/Edit、关闭/X/退出、设置/Settings/Preferences;目标名含 Unicode 上标(如 ᴬᴵᴬ)时用 ASCII 归一形(AIA)。
【发送消息范式】首选 chat_send(window_id, text, click_point) 一调用完成「点击输入框+键入+Enter+OCR 验证」;其次 control(control_action=type_text_submit, text=完整内容) 一调用完成「点击输入框+键入+Enter 提交」(无 OCR 验证);仅当应用把 Enter 定义为换行时才拆成 type_text + click「发送」。
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
                    "enum": ["open", "list", "find", "inspect", "control", "ocr", "screenshot", "capability_probe", "osascript_run", "chat_send", "chat_loop", "input_batch", "run_sequence"],
                    "description": "要执行的窗口操作:open(启动/激活应用) / list(枚举窗口) / find(查窗口) / inspect(控件树) / control(执行操作) / ocr(文字识别) / screenshot(截图) / capability_probe(第 86 轮新增:探测真实能力矩阵) / osascript_run(第 86 轮新增:执行 AppleScript 片段) / chat_send(单条消息原子发送) / chat_loop(长时多轮会话循环) / input_batch(第 90 轮新增:鼠标+键盘+控件树复合步骤批处理) / run_sequence(第 91 轮新增:连续工作模式——探索后统一 plan 的一整套动作连续执行,含验证/等待/重试/逐步焦点守护/落盘记录)"
                },
                "query": { "type": "string", "description": "open/find 必填:应用名或窗口标题/进程名查询词(大小写不敏感,支持中英文别名)" },
                "app_name": { "type": "string", "description": "open 可选:启动用应用名或完整路径,缺省=query" },
                "bundle_id": { "type": "string", "description": "open 可选:macOS Bundle ID(open -b 启动;第 85 轮起 query='微信' 会自动用 KNOWN_BUNDLE_IDS 映射)" },
                "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 30, "description": "open 可选:启动后等待窗口出现秒数,默认 10" },
                "filter": { "type": "string", "description": "list/inspect 可选:窗口标题/进程名或控件名/角色子串过滤" },
                "match_mode": { "type": "string", "enum": ["exact", "contains", "fuzzy"], "description": "find 可选:匹配模式,默认 contains" },
                "window_id": { "type": "string", "description": "inspect/control/ocr/screenshot/chat_send/chat_loop 必填:open/list/find 返回的窗口 id" },
                "path": { "type": "string", "description": "control 必填:控件路径(如 /0/2/1;\"/\" 表示窗口本身,坐标动作照传)" },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 12, "description": "inspect 可选:控件树遍历深度,默认 3" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "invoke", "focus", "set_text", "get_text", "send_keys", "scroll", "scroll_to_visible",
                        "click_point", "double_click_point", "right_click_point", "scroll_point", "type_text", "type_text_submit",
                        "move_point", "middle_click_point", "drag_point"
                    ],
                    "description": "control 必填:控件树路线(click/set_text/get_text/send_keys/scroll 等,无障碍→消息→物理优先级链)或视觉坐标路线(click_point/type_text_submit/move_point 悬停/middle_click_point 中键/drag_point 拖拽等)"
                },
                "text": { "type": "string", "description": "control/chat_send 必填:set_text/send_keys/type_text/type_text_submit/chat_send 的文本;scroll/scroll_point 传方向行数(如 \"down:3\")" },
                "x": { "type": "integer", "description": "control 坐标动作必填:屏幕绝对 X(取 ocr 返回的 screen_cx 中心;drag_point 为起点 X)" },
                "y": { "type": "integer", "description": "control 坐标动作必填:屏幕绝对 Y(取 ocr 返回的 screen_cy 中心;drag_point 为起点 Y)" },
                "x2": { "type": "integer", "description": "第 90 轮新增:drag_point 必填,拖拽终点屏幕绝对 X" },
                "y2": { "type": "integer", "description": "第 90 轮新增:drag_point 必填,拖拽终点屏幕绝对 Y" },
                "modifiers": { "type": "string", "description": "第 90 轮新增:修饰键规格(\"ctrl\" / \"ctrl+shift\" / \"alt\" / \"win\"),作用于 click_point/double_click_point/right_click_point/drag_point —— 按住修饰键执行鼠标操作(ctrl+点击多选 / shift+点击区选 / ctrl+拖拽复制),键盘与鼠标同时操作" },
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
                "output_path": { "type": "string", "description": "screenshot 可选:输出 PNG 路径,默认临时目录" },
                "click_point": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" }
                    },
                    "required": ["x", "y"],
                    "description": "chat_send 可选:视觉路线输入框中心坐标(取 action=ocr 返回的 screen_cx/screen_cy)"
                },
                "input_field_path": { "type": "string", "description": "chat_send 可选:控件树路线输入框路径(取 action=inspect)" },
                "submit_key": { "type": "string", "description": "chat_send 可选:提交键名,默认 enter(微信/钉钉);QQ/部分 App 用 cmd+enter" },
                "verify": { "type": "boolean", "description": "chat_send 可选:发送后是否 OCR 验证上屏,默认 true" },
                "verify_timeout_ms": { "type": "integer", "minimum": 50, "maximum": 5000, "description": "chat_send 可选:发送后等多久开始 OCR 验证,默认 250ms" },
                "messages": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "chat_loop 必填:按顺序发送的消息字符串数组"
                },
                "interval_seconds": { "type": "integer", "minimum": 0, "maximum": 300, "description": "chat_loop 可选:每两条消息间隔秒数,默认 30" },
                "max_rounds": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "chat_loop 可选:最大发送条数,默认 30(防阻塞)" },
                "reply_detect": { "type": "boolean", "description": "chat_loop 可选:每条发完后 OCR 检测右侧是否出现对方消息,默认 true" },
                "stop_on_reply": { "type": "boolean", "description": "chat_loop 可选:对方回复后立即停下,默认 false" },
                "target_query": { "type": "string", "description": "chat_loop 可选:对话对象名字,用于 OCR 检测对方回复" },
                "chat_log_path": { "type": "string", "description": "chat_send/chat_loop 可选:每次 send/recv/fail 落盘的工作日志文件绝对路径;默认 <工作目录>/llaew_chat_<unix_ts>.log。QC 可 grep `[SEND]`/`[RECV]`/`[FAIL]` 行验证(第 86 轮新增)" },
                "chat_loop_click_point": { "type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}}, "required": ["x", "y"], "description": "chat_loop 可选(第 91 轮 P0-4):向内部 chat_send 透传视觉路径的输入框中心坐标;Windows 必须依赖 OCR 或 bounds 比例估算的点击点以保证键入不会打到别处" },
                "chat_loop_input_field_path": { "type": "string", "description": "chat_loop 可选(第 91 轮 P0-4):内部 chat_send 透传的 input_field_path(控件树路径,取 action=inspect)" },
                "osascript_script": { "type": "string", "description": "osascript_run 必填:要执行的 AppleScript 片段(第 88 轮起 argv 直传 osascript -e,不经 shell;双引号/多行 tell 块原样书写,不要做 shell 转义;macOS only)" },
                "osascript_timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 60000, "description": "osascript_run 可选:超时毫秒,默认 5000" },
                "steps": {
                    "type": "array",
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": { "type": "string", "enum": ["mouse_move", "mouse_click", "mouse_drag", "mouse_scroll", "key_press", "type_text", "click", "set_text", "get_text", "wait", "assert_text", "wait_for_text", "wait_front"], "description": "步骤类型(第 90 轮 10 个动作 op + 第 91 轮 run_sequence 3 个验证/等待 op)" },
                            "x": { "type": "integer" }, "y": { "type": "integer" },
                            "x2": { "type": "integer" }, "y2": { "type": "integer" },
                            "button": { "type": "string", "enum": ["left", "right", "middle"] },
                            "clicks": { "type": "integer", "minimum": 1, "maximum": 2 },
                            "modifiers": { "type": "string" },
                            "direction": { "type": "string", "enum": ["up", "down"] },
                            "lines": { "type": "integer", "minimum": 1, "maximum": 100 },
                            "keys": { "type": "string", "description": "key_press:单键或组合键(如 ctrl+a / enter / cmd+f)" },
                            "text": { "type": "string", "description": "type_text / set_text 文本" },
                            "path": { "type": "string", "description": "click / set_text / get_text / assert_text / wait_for_text 控件路径(取 action=inspect)" },
                            "key": { "type": "string", "description": "get_text 读取值在 results 中的键名(缺省用 path)" },
                            "ms": { "type": "integer", "minimum": 0, "maximum": 30000, "description": "wait 毫秒数(input_batch ≤5000;run_sequence ≤30000)" },
                            "delay_ms": { "type": "integer", "minimum": 0, "maximum": 2000, "description": "本步执行前等待毫秒" },
                            "contains": { "type": "string", "description": "第 91 轮 run_sequence:assert_text / wait_for_text 期望包含的文本" },
                            "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 30000, "description": "第 91 轮 run_sequence:wait_for_text / wait_front 超时毫秒(默认 10000 / focus_wait_ms)" },
                            "poll_ms": { "type": "integer", "minimum": 100, "maximum": 5000, "description": "第 91 轮 run_sequence:wait_for_text 轮询间隔毫秒,默认 500" },
                            "optional": { "type": "boolean", "description": "第 91 轮 run_sequence:本步失败容忍(不触发 on_error 策略、不进 failed_steps),默认 false" },
                            "retry_times": { "type": "integer", "minimum": 0, "maximum": 3, "description": "第 91 轮 run_sequence:本步重试次数(覆盖顶层,仅 on_error=retry 生效)" },
                            "retry_delay_ms": { "type": "integer", "minimum": 0, "maximum": 5000, "description": "第 91 轮 run_sequence:本步重试间隔毫秒(覆盖顶层)" }
                        },
                        "required": ["op"],
                        "additionalProperties": false
                    },
                    "description": "input_batch / run_sequence 必填:操作步骤数组(第 90 轮鼠标+键盘+控件树混合编排;第 91 轮 run_sequence 扩展 assert_text/wait_for_text/wait_front 验证与等待 op,≤100 步)"
                },
                "continue_on_error": { "type": "boolean", "description": "input_batch 可选:步骤失败后继续执行后续步骤(默认 false 即 fail-fast 停止)" },
                "on_error": { "type": "string", "enum": ["abort", "continue", "retry"], "description": "第 91 轮 run_sequence 可选:错误策略——abort(默认,失败即停)/ continue(记 failed_steps 继续)/ retry(逐步自动重试,仍败则停)" },
                "retry_times": { "type": "integer", "minimum": 0, "maximum": 3, "description": "第 91 轮 run_sequence 可选:每步重试次数,默认 1(仅 on_error=retry 生效;步骤级同名键覆盖)" },
                "retry_delay_ms": { "type": "integer", "minimum": 0, "maximum": 5000, "description": "第 91 轮 run_sequence 可选:重试前等待毫秒,默认 500(步骤级同名键覆盖)" },
                "focus_guard": { "type": "boolean", "description": "第 91 轮 run_sequence 可选:逐步焦点守护(默认 true)——物理输入步骤执行前确认窗口前台,丢失自动重夺;连续 3 次重夺失败止损中止" },
                "focus_wait_ms": { "type": "integer", "minimum": 0, "maximum": 30000, "description": "第 91 轮 run_sequence 可选:焦点丢失后重夺预算毫秒,默认 5000" },
                "max_total_ms": { "type": "integer", "minimum": 1000, "maximum": 600000, "description": "第 91 轮 run_sequence 可选:整批时长硬顶毫秒,默认 180000" },
                "log_path": { "type": "string", "description": "第 91 轮 run_sequence 可选:执行记录落盘路径([STEP]/[RETRY]/[FOCUS_LOST]/[FAIL]/[SUMMARY]);默认 <工作目录>/laew_sequence_<unix_ts>.log" }
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
            // 2026-09-18 第 86 轮:能力矩阵探测(LLM 第一步必调)+ AppleScript 直接执行
            "capability_probe" => chat::run_capability_probe(args).await,
            "osascript_run" => chat::run_osascript_run(args).await,
            // 2026-09-18 第 85 轮:复合 action(把点击+键入+提交+验证封装成 1 次调用,
            // 把长时多轮会话循环封装成 1 次工具调用)。
            "chat_send" => chat::run_chat_send(args).await,
            "chat_loop" => chat::run_chat_loop(args).await,
            // 2026-09-19 第 90 轮:鼠标 + 键盘 + 控件树复合步骤动作(同时操作鼠标键盘)。
            "input_batch" => input_batch::run_input_batch(args).await,
            // 2026-09-19 第 91 轮:连续工作模式(探索→统一 plan→连续执行+验证/重试/焦点守护/落盘记录)。
            "run_sequence" => sequence::run_input_sequence(args).await,
            other => Err(tool_err(
                self.name(),
                format!(
                    "未知 action={other:?};合法值:open / list / find / inspect / control / ocr / screenshot / capability_probe / osascript_run / chat_send / chat_loop / input_batch / run_sequence"
                ),
            )),
        }
    }
}
