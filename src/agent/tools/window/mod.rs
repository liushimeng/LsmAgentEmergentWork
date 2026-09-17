//! 窗口操控工具(WindowUse Agent 专用):WindowOpen / WindowList / WindowFind /
//! WindowInspect / WindowAction(视觉工具 WindowOCR / WindowScreenshot 见 window_vision.rs)。
//!
//! - 平台差异封闭在 `agent::window` 驱动层,本模块只做参数校验、
//!   `spawn_blocking` 包裹(UIA COM / AX IPC 可能阻塞)与输出截断;
//! - 控件树输出双重截断(节点数 ≤ 500,序列化 ≤ 32KB),防止 tool_result
//!   打爆上下文(对齐 overflow.rs 排水策略);
//! - 错误信息 LLM 可读:权限缺失给开权限步骤,控件未找到给重新检视提示;
//! - **WindowFind**(2026-09-16 第 56 轮):按「标题/进程名子串」+ 模糊匹配打分排序,
//!   返回最佳匹配窗口的 id(LLM 拿到 WindowList 后常不知道取哪个,这一步省一
//!   次「读 JSON 全列表再选」);
//! - **WindowScreenshot**(2026-09-16 第 56 轮):跨平台截图,落盘 PNG 后返回路径,
//!   为后续 OCR / 视觉验证铺路。macOS 走 screencapture(白名单内),Windows 走
//!   PowerShell + System.Drawing,Linux 走 import/scrot(尽力而为)。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.4。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::window::{
    current_driver, normalize_unicode_for_match, ControlAction, ControlNode, WindowInfo,
};
use crate::error::{AgentError, Result};

/// 控件树节点数上限(超出截断)。
const MAX_TREE_NODES: usize = 500;
/// 控件树序列化字节上限(超出截断)。
const MAX_TREE_BYTES: usize = 32 * 1024;
/// 窗口列表条数上限。
const MAX_WINDOWS: usize = 200;

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
/// - **Windows 微信 4.x 的进程名是 `Weixin.exe`**(2026-09-16 第 67 轮实测),
///   3.x 才是 `WeChat.exe` —— 缺这个别名会导致「窗口在眼前也找不到」。
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

fn window_matches_any(w: &WindowInfo, queries: &[String]) -> bool {
    // 2026-09-16 第 66 轮 P1-5:双侧 Unicode 上标归一化后再小写子串匹配
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

fn find_window_by_aliases(
    windows: &[WindowInfo],
    aliases: &[String],
) -> Option<(String, WindowInfo)> {
    aliases.iter().find_map(|q| {
        pick_top_hit(windows, q, MatchMode::Contains).map(|h| (q.clone(), h.info.clone()))
    })
}

/// 统计控件树节点数。
fn count_nodes(node: &ControlNode) -> usize {
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
fn tree_to_json(mut root: ControlNode) -> String {
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

/// 驱动不可用/缺权限时的前置检查。
///
/// 2026-09-16 第 60 轮:macOS 上未授权时自动触发系统授权弹窗。
/// 首次调用 WindowInspect/WindowAction 时,如果辅助功能未授权,
/// 主动调用 `AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})`
/// 触发系统弹窗,引导用户授权。
async fn driver_preflight(tool: &str) -> Result<()> {
    let driver = current_driver();
    if let Some(hint) = driver.permission_hint() {
        // macOS 未授权:尝试自动触发授权弹窗(第 60 轮新增)
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

/// 2026-09-16 第 60 轮:尝试请求 macOS 辅助功能授权。
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
         当前任务已保留 Session ID 与窗口状态;WindowOpen/WindowList 无需该权限,\
         但控件检视、控件输入与 System Events 键盘注入必须授权。"
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

#[cfg(not(target_os = "macos"))]
fn driver_preflight_non_macos(tool: &str) -> Result<()> {
    let driver = current_driver();
    if let Some(hint) = driver.permission_hint() {
        return Err(tool_err(tool, hint));
    }
    Ok(())
}

pub(crate) async fn run_blocking<F>(tool: &str, f: F) -> Result<String>
where
    F: FnOnce() -> Result<String> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| tool_err(tool, format!("窗口驱动任务 join 失败: {e}")))?
}


// ===========================================================================
// 模块拆分(2026-09-17,方案 tmpPlan/2026-09-17_01-单文件1800行超标拆分重构方案.md):
// 本文件原 1542 行且近 5 轮高速增长(1043→1542),按工具族拆为:
//   matching.rs  WindowList / WindowFind + 模糊匹配打分
//   open.rs      WindowOpen + 应用启动扫描
//   inspect.rs   WindowInspect / WindowAction
//   tests.rs     单元测试
// 本文件保留共享辅助(树预算剪枝/查询扩展/别名匹配/driver preflight/AX 权限)。
// 工具结构体经下方 `pub use` 再导出,`crate::agent::tools::window::XxxTool` 路径不变。
// ===========================================================================
mod inspect;
mod matching;
mod open;

pub use inspect::{WindowActionTool, WindowInspectTool};
pub use matching::{WindowFindTool, WindowListTool};
use matching::{damerau_levenshtein, pick_top_hit, MatchMode};
pub use open::WindowOpenTool;

#[cfg(test)]
mod tests;
