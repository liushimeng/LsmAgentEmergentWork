//! 人工介入(HITL)桌面弹窗 UI 呈现层 —— macOS/Windows 弹窗优先,TUI 兜底。
//!
//! 设计见 `docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md`(2026-10-08 第 130 轮)。
//!
//! 职责边界:本模块只负责「把 [`HumanAssistDisplay`] 呈现给人 + 把人的应答收敛为
//! [`UiResult`]」,pending 槽位 / oneshot / 超时 / 四态结局全部仍在
//! [`crate::agent::human_assist::HumanAssistHub`] —— 弹窗与 TUI 是两个可互换的前端。
//!
//! 三层降级链(fail-open,任何一级失败都不阻断任务主流程):
//! 1. 平台弹窗(macOS JXA / Windows PowerShell WinForms,脚本运行时动态加载);
//! 2. TUI 行读(`human_assist::mark_gui_failed` 后由 dispatch 协程接管);
//! 3. 均不可用 → 既有 4001 Unavailable 语义。
//!
//! 环境变量:
//! - `LAEW_HUMAN_UI`:`off/0/false/no/tty` 关闭弹窗(强制 TUI);缺省自动;
//! - `LAEW_HUMAN_UI_SCRIPT`:显式指定弹窗脚本路径(企业定制/调试)。

use std::sync::atomic::{AtomicI8, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::human_assist::{kind_label, HumanAssistDisplay};

mod script;
#[cfg(test)]
pub(crate) use script::{resolve_script, ResolvedScript, ScriptOrigin};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(test)]
mod tests;

/// 弹窗呈现结局(内部四态;经 Hub 映射回 `HumanAssistOutcome`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiResult {
    /// 人工应答(选项文本形如 `1. xxx`,或自由文本如短信验证码数字)。
    Answered(String),
    /// 人工取消(取消按钮 / Esc / 关窗)。
    Cancelled,
    /// 弹窗倒计时归零自灭。
    Timeout,
    /// 脚本/进程级失败(调用方降级 TUI)。
    Error(String),
}

/// 平台展示名(TUI 通知行/日志用)。
pub fn platform_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS 弹窗"
    }
    #[cfg(target_os = "windows")]
    {
        "Windows 弹窗"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "桌面弹窗"
    }
}

// ---------- 开关与能力探测 ----------

/// 测试钩子覆盖(0=自动,1=强制开,2=强制关)。生产代码不触达。
static TEST_OVERRIDE: AtomicI8 = AtomicI8::new(0);

/// 测试钩子:同步 stub 后端(命中时 `present` 直接返回 stub 结果,不起弹窗进程)。
static TEST_BACKEND: Mutex<Option<fn(&HumanAssistDisplay) -> UiResult>> = Mutex::new(None);

/// 测试钩子:覆盖 enabled 判定(单测互斥全局态用;`None` 恢复自动)。
pub fn set_test_override(v: Option<bool>) {
    let code = match v {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    TEST_OVERRIDE.store(code, Ordering::SeqCst);
}

/// 测试钩子:设置同步 stub 弹窗后端(`None` 恢复真实平台后端)。
pub fn set_test_backend(f: Option<fn(&HumanAssistDisplay) -> UiResult>) {
    *lock_backend() = f;
}

fn lock_backend() -> std::sync::MutexGuard<'static, Option<fn(&HumanAssistDisplay) -> UiResult>> {
    TEST_BACKEND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 弹窗呈现是否可用(桌面平台 + 解释器存在 + 总开关未关)。
///
/// 单测构建(`cfg(test)`)下默认关闭,避免单测误弹真窗;需要弹窗路径的测试
/// 用 [`set_test_override`] / [`set_test_backend`] 显式打开。
pub fn enabled() -> bool {
    match TEST_OVERRIDE.load(Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    if cfg!(test) {
        return false;
    }
    if matches!(
        std::env::var("LAEW_HUMAN_UI").unwrap_or_default().trim().to_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "tty"
    ) {
        return false;
    }
    platform_supported() && interpreter_available()
}

fn platform_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// 弹窗不可用时的原因(纯函数,可单测)。
///
/// [`enabled`] 是布尔的 fail-open 判定,出问题时对用户是黑盒:信封只说「弹窗与 TUI
/// 均不可用」,用户既不知道是没弹、还是弹了没反应、还是压根没请求过。本函数把判定链
/// 逐条摊开,随 4001 信封回传给 Agent / 日志,供排障。
fn disabled_reason() -> &'static str {
    if cfg!(test) {
        return "测试构建(cfg(test))下默认关闭弹窗";
    }
    if matches!(
        std::env::var("LAEW_HUMAN_UI").unwrap_or_default().trim().to_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "tty"
    ) {
        return "LAEW_HUMAN_UI 开关已关闭(off/0/false/no/tty)";
    }
    if !platform_supported() {
        return "当前平台不支持桌面弹窗(仅 macOS / Windows)";
    }
    "弹窗解释器缺失(macOS: /usr/bin/osascript;Windows: powershell.exe)"
}

/// 弹窗能力诊断(第 131 轮):随 `request_human` 的 4001 信封回传,便于排障。
pub fn diagnostics() -> Value {
    json!({
        "gui_enabled": enabled(),
        "platform": platform_name(),
        "reason_hint": if enabled() { "弹窗可用" } else { disabled_reason() },
        "env_switch": "LAEW_HUMAN_UI=off 关闭弹窗强制走终端;LAEW_HUMAN_UI_SCRIPT=<path> 自定义脚本",
    })
}

/// 平台脚本解释器是否就位(macOS osascript / Windows powershell)。
/// 探测失败按「就位」保守处理 —— 真启动失败会走降级链,不会比禁用更糟。
fn interpreter_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            std::path::Path::new("/usr/bin/osascript").exists()
        }
        #[cfg(target_os = "windows")]
        {
            let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            let ps = std::path::PathBuf::from(sys_root)
                .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
            ps.exists()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            false
        }
    })
}

// ---------- payload 构建(纯函数,可单测) ----------

/// display → 弹窗 payload(两平台脚本共用同一契约,见设计文档 §4.3)。
pub fn build_payload(display: &HumanAssistDisplay) -> Value {
    let now_ms = display.created_at_ms;
    json!({
        "id": display.id,
        "kind": display.kind,
        "kind_label": kind_label(&display.kind),
        "message": display.message,
        "options": display.options,
        "url": display.url,
        "page_id": display.page_id,
        "started_at": fmt_local_ms(now_ms),
        "started_at_ms": now_ms,
        "timeout_ms": display.timeout_ms,
        "deadline_at": fmt_local_ms(now_ms.saturating_add(display.timeout_ms)),
    })
}

/// 毫秒时间戳 → 本地时区 `YYYY-MM-DD HH:MM:SS`(`time` crate,无宏依赖)。
pub fn fmt_local_ms(ms: u64) -> String {
    use time::OffsetDateTime;
    let ts = (ms / 1000) as i64;
    let base = OffsetDateTime::from_unix_timestamp(ts)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let dt = match OffsetDateTime::now_local() {
        Ok(now) => base.to_offset(now.offset()),
        Err(_) => base,
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

// ---------- 弹窗结果解析(纯函数,可单测) ----------

/// 弹窗脚本 stdout → [`UiResult`](容错:取最后一个含 `status` 的 JSON 行)。
pub fn parse_result(stdout: &str) -> UiResult {
    for line in stdout.lines().rev() {
        let t = line.trim();
        if t.is_empty() || !t.starts_with('{') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else {
            continue;
        };
        let Some(status) = v.get("status").and_then(|s| s.as_str()) else {
            continue;
        };
        let text = v
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        return match status {
            "answer" if !text.is_empty() => UiResult::Answered(text),
            "answer" => UiResult::Error("弹窗返回空应答".into()),
            "cancel" => UiResult::Cancelled,
            "timeout" => UiResult::Timeout,
            "error" => UiResult::Error(if text.is_empty() {
                "弹窗脚本报错".into()
            } else {
                text
            }),
            other => UiResult::Error(format!("未知弹窗结果: {other}")),
        };
    }
    let head: String = stdout.trim().chars().take(120).collect();
    UiResult::Error(format!("弹窗输出无法解析: {head}"))
}

// ---------- 呈现入口 ----------

/// 弹出人工介入弹窗并等待应答(进程化;未来可被 drop → kill_on_drop 回收)。
///
/// 测试构建下若设置了 [`set_test_backend`] 则走 stub;否则走平台后端。
/// 任何失败以 [`UiResult::Error`] 返回,调用方降级 TUI,不 panic。
pub async fn present(display: &HumanAssistDisplay) -> UiResult {
    if let Some(backend) = *lock_backend() {
        return backend(display);
    }
    let payload = build_payload(display);
    let resolved = match script::resolve_script() {
        Ok(r) => r,
        Err(e) => return UiResult::Error(e),
    };
    let payload_path = match script::write_payload(&payload, display.id) {
        Ok(p) => p,
        Err(e) => return UiResult::Error(e),
    };
    let materialized = match resolved.materialize() {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_file(&payload_path);
            return UiResult::Error(e);
        }
    };
    let res = run_platform(&materialized.path, &payload_path).await;
    let _ = std::fs::remove_file(&payload_path);
    drop(materialized); // 内置脚本的临时文件在此清理
    res
}

/// 平台分发:macOS osascript(JXA)/ Windows powershell(WinForms)。
#[allow(unused_variables)]
async fn run_platform(script_path: &std::path::Path, payload_path: &std::path::Path) -> UiResult {
    #[cfg(target_os = "macos")]
    {
        macos::run(script_path, payload_path).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::run(script_path, payload_path).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (script_path, payload_path);
        UiResult::Error("当前平台不支持桌面弹窗".into())
    }
}

/// 通用子进程等待:进程化执行脚本并收集 stdout(kill_on_drop 保证被 drop 即回收)。
pub(super) async fn run_script_process(
    program: &str,
    args: &[std::ffi::OsString],
) -> UiResult {
    match tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(out) if out.status.success() => parse_result(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => {
            let stderr: String = String::from_utf8_lossy(&out.stderr).chars().take(200).collect();
            UiResult::Error(format!(
                "弹窗脚本退出码 {}: {stderr}",
                out.status.code().unwrap_or(-1)
            ))
        }
        Err(e) => UiResult::Error(format!("弹窗进程启动失败: {e}")),
    }
}
