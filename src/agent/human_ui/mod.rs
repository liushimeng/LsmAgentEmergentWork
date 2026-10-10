//! 人工介入(HITL)桌面弹窗 UI 呈现层 —— macOS/Windows 弹窗优先,TUI 兜底。
//!
//! 设计见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md`(2026-10-09 第 137 轮全量重构)。
//!
//! 职责边界:本模块只负责「把 [`HumanAssistDisplay`] 呈现给人 + 把人的应答收敛为
//! [`UiResult`]」,pending 槽位 / oneshot / 超时 / 四态结局全部仍在
//! [`crate::agent::human_assist::HumanAssistHub`] —— 弹窗与 TUI 是两个可互换的前端。
//!
//! 原生实现(第 137 轮):弹窗本体是 **`laew __hitl-dialog <payload.json>` 子进程**
//! (与 `__browser-watchdog` 隐藏子命令同构),macOS 走 AppKit FFI(`macos_dialog.rs`)、
//! Windows 走 Win32(`windows_dialog.rs`),js/ps1 脚本层已删除。子进程隔离的理由:
//! AppKit 要求事件循环在进程主线程(laew 主线程被 tokio/TUI 占用)、FFI 崩溃只死弹窗、
//! `kill_on_drop` 杀进程即关窗(回收语义与脚本时代完全一致)。
//!
//! 三层降级链(fail-open,任何一级失败都不阻断任务主流程):
//! 1. 平台原生弹窗(`laew __hitl-dialog` 子进程,kill_on_drop);
//! 2. TUI 行读(`human_assist::mark_gui_failed` 后由 dispatch 协程接管);
//! 3. 均不可用 → 既有 4001 Unavailable 语义。
//!
//! 环境变量:
//! - `LAEW_HUMAN_UI`:`off/0/false/no/tty` 关闭弹窗(强制 TUI);缺省自动;
//! - `LAEW_HITL_DIALOG_EXE`:覆盖弹窗子进程可执行文件(测试/特殊部署,
//!   对齐 `LAEW_BROWSER_WATCHDOG_EXE`)。
//!
//! 历史口径:第 130~134 轮的脚本动态加载机制(`LAEW_HUMAN_UI_SCRIPT`、
//! `.laew/human_ui/` 两级覆盖)已废止,设置不再生效也不报错(详见 03 文档 §8)。

use std::sync::atomic::{AtomicI8, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::human_assist::{kind_label, HumanAssistDisplay};

/// 隐藏子命令名:原生弹窗进程入口(`laew __hitl-dialog <payload.json>`)。
pub const HITL_DIALOG_COMMAND: &str = "__hitl-dialog";

pub mod dialog_main;

#[cfg(target_os = "macos")]
mod macos_dialog;
#[cfg(target_os = "windows")]
mod windows_dialog;

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
    /// 弹窗进程级失败(调用方降级 TUI)。
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

/// 弹窗呈现是否可用(桌面平台 + 总开关未关)。
///
/// 第 137 轮:不再探测外部解释器(osascript/powershell)—— 弹窗是 `laew` 自身的
/// 原生子进程,可执行文件必然存在;真正「无桌面会话」(SSH 等)场景由子进程启动
/// 失败 → Error → 降级链兜底。
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
    platform_supported()
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
    "弹窗子进程启动失败(可能是无桌面会话,如 SSH);失败会自动降级终端作答"
}

/// 弹窗能力诊断(第 131 轮):随 `request_human` 的 4001 信封回传,便于排障。
pub fn diagnostics() -> Value {
    json!({
        "gui_enabled": enabled(),
        "platform": platform_name(),
        "reason_hint": if enabled() { "弹窗可用" } else { disabled_reason() },
        "env_switch": "LAEW_HUMAN_UI=off 关闭弹窗强制走终端;第 137 轮起弹窗为原生实现,旧脚本覆盖(LAEW_HUMAN_UI_SCRIPT/.laew/human_ui)已废止",
    })
}

// ---------- payload 构建(纯函数,可单测) ----------

/// display → 弹窗 payload(弹窗子进程契约,见 03 文档 §5)。
///
/// 第 132 轮新增 `image_path`(验证码等阻断现场的截图;空串 = 无图,弹窗端
/// 对空串/缺省字段按无图处理)。
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
        "image_path": display.image_path,
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

/// 弹窗子进程 stdout → [`UiResult`](容错:取最后一个含 `status` 的 JSON 行)。
///
/// 第 145 轮:`extend` 是**中间行**(「⏱ +2 分钟」按钮的延长通知,进程不退出、
/// 结果仍在最后),解析时显式跳过,不落入「未知弹窗结果」。
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
            "extend" => continue, // 中间行:继续往前找真正的收口行
            "answer" if !text.is_empty() => UiResult::Answered(text),
            "answer" => UiResult::Error("弹窗返回空应答".into()),
            "cancel" => UiResult::Cancelled,
            "timeout" => UiResult::Timeout,
            "error" => UiResult::Error(if text.is_empty() {
                "弹窗进程报错".into()
            } else {
                text
            }),
            other => UiResult::Error(format!("未知弹窗结果: {other}")),
        };
    }
    let head: String = stdout.trim().chars().take(120).collect();
    UiResult::Error(format!("弹窗输出无法解析: {head}"))
}

/// 第 145 轮:解析弹窗子进程的 `extend` **中间行**
/// `{"status":"extend","id":<请求id>,"ms":<延长毫秒>}`(「⏱ +2 分钟」按钮)。
/// 非延长行 / 字段缺失 / ms=0 一律返回 None(容错,绝不因瑕疵 panic)。
pub fn parse_extend_line(line: &str) -> Option<(u64, u64)> {
    let t = line.trim();
    if t.is_empty() || !t.starts_with('{') {
        return None;
    }
    let v = serde_json::from_str::<Value>(t).ok()?;
    if v.get("status").and_then(|s| s.as_str())? != "extend" {
        return None;
    }
    let id = v.get("id").and_then(Value::as_u64)?;
    let ms = v.get("ms").and_then(Value::as_u64).filter(|ms| *ms > 0)?;
    Some((id, ms))
}

// ---------- 呈现入口 ----------

/// 弹出人工介入弹窗并等待应答(原生子进程;drop 即 kill,窗口随之回收)。
///
/// 测试构建下若设置了 [`set_test_backend`] 则走 stub;否则 spawn
/// `laew __hitl-dialog <payload.json>`(第 137 轮:macOS AppKit FFI / Windows Win32)。
/// 任何失败以 [`UiResult::Error`] 返回,调用方降级 TUI,不 panic。
pub async fn present(display: &HumanAssistDisplay) -> UiResult {
    if let Some(backend) = *lock_backend() {
        return backend(display);
    }
    let payload = build_payload(display);
    let payload_path = match write_payload(&payload, display.id) {
        Ok(p) => p,
        Err(e) => return UiResult::Error(e),
    };
    // payload 临时文件 RAII 守卫(第 136 轮):`present_via_gui` 在超时/取消路径上
    // 会直接 drop 整个 future,`await` 之后的清理代码不执行 —— Drop 守卫在任何
    // 退出路径(正常 / 提前 drop / panic unwind)都会清理。
    let payload_guard = PayloadFileGuard(Some(payload_path.clone()));
    let res = run_dialog_process(&payload_path).await;
    drop(payload_guard);
    res
}

/// `/tmp` payload 临时文件的 RAII 清理守卫(第 136 轮)。
struct PayloadFileGuard(Option<std::path::PathBuf>);

impl Drop for PayloadFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 弹窗子进程可执行文件:测试钩子 `LAEW_HITL_DIALOG_EXE` 优先,缺省 `current_exe()`。
fn dialog_exe() -> Result<std::path::PathBuf, String> {
    if let Some(explicit) = std::env::var_os("LAEW_HITL_DIALOG_EXE") {
        let p = std::path::PathBuf::from(explicit);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("LAEW_HITL_DIALOG_EXE 指向的文件不存在: {}", p.display()));
    }
    std::env::current_exe().map_err(|e| format!("定位 laew 可执行文件失败: {e}"))
}

/// 启动原生弹窗子进程并等待应答(kill_on_drop 保证被 drop 即回收,窗口消失)。
///
/// 第 145 轮:stdout 从「整体 `.output()`」改为 **piped 逐行流式读** —— 弹窗内
/// 「⏱ +2 分钟」按钮会随时输出 `{"status":"extend",...}` 中间行(进程不退出),
/// 父进程必须即时把它接线到 `HumanAssistHub::extend_timeout` 才能真正推迟 hub 侧
/// 截止时刻;其余行留存到进程退出后交 [`parse_result`] 收口解析。stderr 并联排空
/// (防子进程写满 pipe 缓冲阻塞),失败路径语义与旧 `.output()` 版本一致。
async fn run_dialog_process(payload_path: &std::path::Path) -> UiResult {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let exe = match dialog_exe() {
        Ok(p) => p,
        Err(e) => return UiResult::Error(e),
    };
    let args: Vec<std::ffi::OsString> = vec![
        HITL_DIALOG_COMMAND.into(),
        payload_path.as_os_str().to_owned(),
    ];
    let mut child = match tokio::process::Command::new(exe)
        .args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return UiResult::Error(format!("弹窗子进程启动失败: {e}")),
    };
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => return UiResult::Error("弹窗子进程 stdout 未接管".into()),
    };
    let mut stderr_pipe = match child.stderr.take() {
        Some(s) => s,
        None => return UiResult::Error("弹窗子进程 stderr 未接管".into()),
    };
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf).await;
        buf
    });
    // 逐行读:extend 中间行即时接线 hub,其余行留存供收口解析。
    let mut kept: Vec<String> = Vec::new();
    let mut lines = BufReader::new(&mut stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let t = line.trim().to_string();
                if t.is_empty() {
                    continue;
                }
                if let Some((id, ms)) = parse_extend_line(&t) {
                    crate::agent::human_assist::HumanAssistHub::global().extend_timeout(id, ms);
                } else {
                    kept.push(t);
                }
            }
            _ => break,
        }
    }
    let status = child.wait().await;
    let stderr = stderr_task.await.unwrap_or_default();
    match status {
        Ok(st) if st.success() => parse_result(&kept.join("\n")),
        Ok(st) => {
            let s: String = stderr.chars().take(200).collect();
            UiResult::Error(format!(
                "弹窗子进程退出码 {}: {s}",
                st.code().unwrap_or(-1)
            ))
        }
        Err(e) => UiResult::Error(format!("弹窗子进程等待失败: {e}")),
    }
}

/// payload JSON 落盘到临时文件(弹窗子进程入参;调用方负责删除)。
pub fn write_payload(payload: &Value, id: u64) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "laew_human_ui_payload_{}_{}.json",
        std::process::id(),
        id
    ));
    let body = serde_json::to_string(payload).map_err(|e| format!("payload 序列化失败: {e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("payload 落盘失败 {}: {e}", path.display()))?;
    Ok(path)
}
