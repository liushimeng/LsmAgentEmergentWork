//! 复用已登录浏览器(connect 模式增强,第 142 轮)。
//!
//! 背景:用户在本机 Chrome 已登录某网站(邮箱/后台/AI 网站),Agent 需要
//! 复用该登录态操作页面。chromiumoxide 的 connect 模式(接管已开浏览器)
//! 是唯一能复用登录态的 CDP 路径——launch 模式总是一次性临时 profile,
//! 无登录态。
//!
//! 三档能力(设计见 `docs/MCP_Web_Use/06-复用已登录浏览器会话.md`):
//! 1. **自动探测**:`open(reuse_existing=true)` 探测本机调试端口
//!    (默认 9222,`LAEW_CHROME_DEBUG_PORT` 可覆盖),命中即接管;
//! 2. **结构化引导**:探测失败返回 3002 信封 + 平台相关重启命令;
//! 3. **自动重启接管**:`auto_relaunch=true` 时复制默认 profile 登录态关键文件到
//!    独立调试 profile → 以调试参数**另启独立 Chrome 实例**(与用户正在运行的
//!    浏览器并存)→ 等端口就绪。**第 144 轮起绝不退出用户 Chrome**(原实现
//!    `pkill`/`taskkill` 按进程名误杀用户浏览器,见
//!    `docs/MCP_Web_Use/08-浏览器进程保护与凭证区域部分放行.md`)。
//!
//! 关键约束(Chrome 136+):调试端口在默认 profile 上会被忽略,必须用
//! 独立 `--user-data-dir`;独立 profile 无登录态,故 auto_relaunch 先复制
//! 默认 profile 的 Cookies/Local Storage/Session Storage 等关键文件
//! (用户 Chrome 运行中直接复制,best-effort,单条失败跳过并回报计数)。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 缺省 CDP 调试端口(Chrome DevTools 协议标准端口)。
pub const DEFAULT_DEBUG_PORT: u16 = 9222;

/// 调试端口环境变量(覆盖缺省 9222)。
pub const DEBUG_PORT_ENV: &str = "LAEW_CHROME_DEBUG_PORT";

/// 探测超时:端口无监听时快速失败,不拖垮 open 语义。
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 等端口就绪:Chrome 冷启动 2-5s,15s 上限足够。
const RELAUNCH_WAIT_TIMEOUT: Duration = Duration::from_secs(15);

/// 等端口就绪轮询间隔。
const RELAUNCH_POLL_INTERVAL: Duration = Duration::from_millis(300);

/// 探测成功的调试端点信息。
#[derive(Debug, Clone)]
pub struct DebugEndpointInfo {
    /// 实际探测成功的端口。
    pub port: u16,
    /// 浏览器版本串(如 `Chrome/131.0.6778.87`),取自 `/json/version` 的 `Browser` 字段。
    pub browser_version: String,
    /// WebSocket 调试地址(透出供排查;实际连接由 chromiumoxide 内部完成)。
    pub web_socket_debugger_url: String,
    /// 传给 `Browser::connect` 的 http 地址。
    pub connect_url: String,
}

/// 读调试端口:`LAEW_CHROME_DEBUG_PORT` 覆盖,非法值回退默认 9222。
pub fn debug_port() -> u16 {
    std::env::var(DEBUG_PORT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p > 0)
        .unwrap_or(DEFAULT_DEBUG_PORT)
}

/// 探测地址候选(按优先级)。
///
/// Chrome 154+ 的 DevTools HTTP 端点只服务 **IPv6 loopback(`::1`)连接**,
/// IPv4(`127.0.0.1`)连接返回 404(实测 2026-10,Chrome 154.0.8037.98);
/// 旧版本两者皆可。`localhost` 在多数系统优先解析到 `::1`,放首位;
/// `[::1]` 兜底 localhost 解析到 IPv4 的环境;`127.0.0.1` 兜底旧版本。
fn probe_candidates(port: u16) -> Vec<String> {
    vec![
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
        format!("http://127.0.0.1:{port}"),
    ]
}

/// 探测本机 CDP 调试端点:依次尝试候选地址的 `GET /json/version`。
///
/// 命中(200 + 非空 `webSocketDebuggerUrl`)返回端点信息(`connect_url`
/// 为实际命中的地址,供 `Browser::connect` 复用);端口无监听 /
/// 非 CDP 服务 / 超时一律返回 `None`(fail-open,不 panic)。
pub async fn probe_debug_endpoint(port: u16) -> Option<DebugEndpointInfo> {
    for base in probe_candidates(port) {
        if let Some(info) = probe_url(port, &base).await {
            return Some(info);
        }
    }
    None
}

/// 单地址探测:`GET {base}/json/version` 并解析端点信息。
async fn probe_url(port: u16, base: &str) -> Option<DebugEndpointInfo> {
    let url = format!("{base}/json/version");
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    let v: serde_json::Value = resp.json().await.ok()?;
    let ws = v.get("webSocketDebuggerUrl")?.as_str()?;
    if ws.is_empty() {
        return None;
    }
    Some(DebugEndpointInfo {
        port,
        browser_version: v
            .get("Browser")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        web_socket_debugger_url: ws.to_string(),
        connect_url: base.to_string(),
    })
}

/// 轮询等待调试端口就绪(auto_relaunch 第 4 步,15s 上限)。
pub async fn wait_for_debug_port(port: u16) -> Option<DebugEndpointInfo> {
    wait_for_debug_port_with_timeout(port, RELAUNCH_WAIT_TIMEOUT).await
}

/// 轮询等待调试端口就绪(超时参数化,便于单测)。
async fn wait_for_debug_port_with_timeout(
    port: u16,
    timeout: Duration,
) -> Option<DebugEndpointInfo> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(info) = probe_debug_endpoint(port).await {
            return Some(info);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(RELAUNCH_POLL_INTERVAL).await;
    }
}

// ===================== Chromium 系浏览器进程检测(第 144 轮:只检测,不退出) =====================

/// Chromium 系浏览器进程名(按平台)。
pub fn chromium_process_names() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &["Google Chrome", "Microsoft Edge", "Chromium", "Brave Browser"]
    }
    #[cfg(target_os = "windows")]
    {
        &["chrome.exe", "msedge.exe", "chromium.exe", "brave.exe"]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "brave-browser",
        ]
    }
}

/// 是否检测到 Chromium 系浏览器进程在跑(macOS/Linux `pgrep`,Windows `tasklist`)。
pub fn chromium_running() -> bool {
    chromium_process_names().iter().any(|name| process_alive(name))
}

/// 单进程名存活探测(纯函数入口 + 真实环境探测,便于单测)。
pub fn process_alive(name: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        // tasklist /FI "IMAGENAME eq {name}" 输出表头 + 进程行;命中即存活。
        std::process::Command::new("tasklist")
            .arg("/FI")
            .arg(format!("IMAGENAME eq {name}"))
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.to_ascii_lowercase().contains(&name.to_ascii_lowercase()))
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // pgrep -x 精确匹配进程名;无 pgrep 时回退 /proc 扫描(Linux 极简环境)。
        let hit = std::process::Command::new("pgrep")
            .arg("-x")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if hit {
            return true;
        }
        #[cfg(target_os = "linux")]
        {
            proc_name_from_proc(name)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

/// Linux 无 pgrep 时扫 /proc/*/comm 兜底(纯函数,便于单测)。
#[cfg(target_os = "linux")]
fn proc_name_from_proc(name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|e| {
        let comm = e.path().join("comm");
        std::fs::read_to_string(comm)
            .map(|s| s.trim() == name)
            .unwrap_or(false)
    })
}

// ===================== 调试 profile 与登录态复制 =====================

/// 调试 profile 目录:`~/.laew/chrome_debug_profile(跨平台)。
pub fn debug_profile_dir() -> PathBuf {
    home_dir().join(".laew").join("chrome_debug_profile")
}

/// 用户默认 Chrome profile 根目录(登录态复制源;探测不到返回 None)。
pub fn default_profile_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(
            home_dir()
                .join("Library")
                .join("Application Support")
                .join("Google")
                .join("Chrome"),
        )
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("Google").join("Chrome").join("User Data"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux 服务器无登录态可复用,返回 None(auto_relaunch 跳过复制)。
        None
    }
}

/// 用户主目录(HOME / USERPROFILE,取不到返回当前目录兜底)。
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 登录态复制清单(相对路径;文件/目录由运行时 `is_dir()` 判断)。
///
/// 只复制登录态核心文件,不复制整个 profile(几百 MB 且含缓存/历史等
/// 与登录态无关内容)。单个文件失败跳过(best-effort)。第 144 轮起用户
/// Chrome **运行中**直接复制(不再为一致性退出用户浏览器),补 `-wal` 变体
/// 降低 WAL 模式撕裂读概率(缺失即跳过,零成本)。
const SECRET_COPIES: &[&str] = &[
    "Default/Cookies",
    "Default/Cookies-journal",
    "Default/Cookies-wal",
    "Default/Local Storage",
    "Default/Session Storage",
    "Default/Login Data",
    "Default/Login Data-journal",
    "Default/Login Data-wal",
    "Default/Web Data",
    "Local State",
];

/// 复制默认 profile 的登录态关键文件到调试 profile(先清空目标)。
///
/// 第 144 轮:不再要求 Chrome 退出 —— 用户浏览器运行中直接复制(best-effort,
/// 单条失败跳过;登录态可能略旧,详见 doc 08 §3.3)。返回
/// (成功复制数, 跳过数);源目录不存在返回 (0, 0),不 panic。
pub fn copy_profile_secrets(src: &Path, dst: &Path) -> (usize, usize) {
    if !src.is_dir() {
        return (0, 0);
    }
    // 先清空目标(上次残留的过期登录态),失败也继续(复制会覆盖)。
    let _ = std::fs::remove_dir_all(dst);
    if std::fs::create_dir_all(dst).is_err() {
        return (0, 0);
    }
    let mut copied = 0usize;
    let mut skipped = 0usize;
    for rel in SECRET_COPIES {
        let from = src.join(rel);
        let to = dst.join(rel);
        if from.is_dir() {
            if copy_dir_recursive(&from, &to).is_ok() {
                copied += 1;
            } else {
                skipped += 1;
            }
        } else if from.is_file() {
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&from, &to).is_ok() {
                copied += 1;
            } else {
                skipped += 1;
            }
        } else {
            // 源缺失(全新 profile 没有该文件)不算失败。
            skipped += 1;
        }
    }
    (copied, skipped)
}

/// 递归复制目录(best-effort,单文件失败跳过)。
fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            let _ = std::fs::copy(&path, &target);
        }
    }
    Ok(())
}

// ===================== 调试重启(auto_relaunch 第 3 步) =====================

/// 调试启动参数(纯函数,便于单测)。
pub fn relaunch_args(port: u16, profile_dir: &Path, headless: bool) -> Vec<String> {
    let mut args = vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", profile_dir.display()),
    ];
    if headless {
        args.push("--headless=new".to_string());
        args.push("--no-sandbox".to_string());
        args.push("--disable-gpu".to_string());
    }
    args
}

/// Windows 下可执行文件路径含空格时加引号(纯函数,便于单测)。
pub fn quote_exe_path(exe: &str) -> String {
    if cfg!(target_os = "windows") && exe.contains(' ') {
        format!("\"{exe}\"")
    } else {
        exe.to_string()
    }
}

/// 给用户复制执行的完整重启命令(纯函数,便于单测)。
///
/// 平台差异:macOS 必须调用 .app 包内可执行文件;Windows 路径含空格时
/// 用引号包裹;Linux 直接可执行文件名。
pub fn relaunch_command_string(port: u16, profile_dir: &Path) -> String {
    let exe = crate::agent::browser::detect_browser()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| default_browser_command_name().to_string());
    let exe = quote_exe_path(&exe);
    format!(
        "{} --remote-debugging-port={} --user-data-dir={}",
        exe,
        port,
        profile_dir.display()
    )
}

/// 探测不到浏览器时的命令名兜底(纯函数,便于单测)。
pub fn default_browser_command_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    }
    #[cfg(target_os = "windows")]
    {
        "chrome.exe"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "google-chrome"
    }
}

/// 以调试参数启动 Chrome(auto_relaunch 第 3 步)。
///
/// 返回 spawn 的 Child(调用方持有,进程独立运行);失败返回 None。
pub fn launch_debug_browser(
    port: u16,
    profile_dir: &Path,
    headless: bool,
) -> Option<std::process::Child> {
    let exe = crate::agent::browser::detect_browser()?;
    let mut cmd = std::process::Command::new(exe);
    for arg in relaunch_args(port, profile_dir, headless) {
        cmd.arg(arg);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().ok()
}

/// auto_relaunch 结果(第 144 轮):端点信息 + 登录态复制计数(供响应回报对账)。
#[derive(Debug, Clone)]
pub struct RelaunchOutcome {
    pub info: DebugEndpointInfo,
    /// 成功复制的登录态条目数(仅桌面平台;Linux 恒 0)。
    pub login_state_copied: usize,
    /// 跳过的登录态条目数(源缺失/复制失败)。
    pub login_state_skipped: usize,
}

/// auto_relaunch 全流程(第 144 轮起**不退出用户 Chrome**):
/// 复制登录态(运行中 best-effort)→ 独立调试 profile 另启实例 → 等端口就绪。
///
/// 多实例并存机制:新实例用独立 `--user-data-dir`(Chrome 多实例官方隔离方式),
/// 与用户正在运行的浏览器互不干扰 —— 这是删除旧「退出用户 Chrome」步骤后的
/// 正确替代(旧 `pkill`/`taskkill` 按进程名杀掉用户全部浏览器窗口,实测事故)。
/// 每步 fail-open:任一步失败返回 None(工具层回落 3002 + 原因)。
/// 仅桌面平台(macOS/Windows)复制登录态;Linux 服务器跳过复制。
pub async fn relaunch_and_wait(port: u16) -> Option<RelaunchOutcome> {
    let profile = debug_profile_dir();
    // 1. 复制登录态(仅桌面平台;default_profile_dir 在 Linux 返回 None)。
    //    用户 Chrome 运行中直接读复制(best-effort),绝不退出用户浏览器。
    let (copied, skipped) = {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            default_profile_dir()
                .map(|d| copy_profile_secrets(&d, &profile))
                .unwrap_or((0, 0))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            (0, 0)
        }
    };
    // 2. 以调试参数另启独立 Chrome 实例(无 GUI 会话 → 无头)。
    let headless = !crate::agent::browser_mode::has_gui_session();
    launch_debug_browser(port, &profile, headless)?;
    // 3. 等端口就绪。
    let info = wait_for_debug_port(port).await?;
    Some(RelaunchOutcome {
        info,
        login_state_copied: copied,
        login_state_skipped: skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_port_defaults_to_9222() {
        // 持 env 锁串行:与 debug_port_env_override 并发时 set_var 会互相踩。
        let _g = env_lock();
        unsafe { std::env::remove_var(DEBUG_PORT_ENV) };
        assert_eq!(debug_port(), DEFAULT_DEBUG_PORT);
    }

    /// 环境变量类测试用互斥锁串行(对齐 browser_mode.rs 第 79 轮既有模式):
    /// 多个测试并行跑时 set_var/remove_var 互相踩,全量 cargo test 偶发失败。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn debug_port_env_override() {
        let _g = env_lock();
        unsafe { std::env::set_var(DEBUG_PORT_ENV, "9333") };
        assert_eq!(debug_port(), 9333);
        unsafe { std::env::set_var(DEBUG_PORT_ENV, "not-a-number") };
        assert_eq!(debug_port(), DEFAULT_DEBUG_PORT, "非法值回退默认");
        unsafe { std::env::set_var(DEBUG_PORT_ENV, "0") };
        assert_eq!(debug_port(), DEFAULT_DEBUG_PORT, "0 回退默认");
        unsafe { std::env::remove_var(DEBUG_PORT_ENV) };
        assert_eq!(debug_port(), DEFAULT_DEBUG_PORT);
    }

    #[test]
    fn relaunch_args_port_and_profile() {
        let args = relaunch_args(9222, Path::new("/tmp/profile"), false);
        assert_eq!(
            args,
            vec![
                "--remote-debugging-port=9222".to_string(),
                "--user-data-dir=/tmp/profile".to_string(),
            ]
        );
    }

    #[test]
    fn relaunch_args_headless_appends_flags() {
        let args = relaunch_args(9222, Path::new("/tmp/profile"), true);
        assert!(args.contains(&"--headless=new".to_string()));
        assert!(args.contains(&"--no-sandbox".to_string()));
        assert!(args.contains(&"--disable-gpu".to_string()));
    }

    #[test]
    fn relaunch_command_string_contains_port_and_profile() {
        let cmd = relaunch_command_string(9222, Path::new("/home/u/.laew/chrome_debug_profile"));
        assert!(cmd.contains("--remote-debugging-port=9222"), "实际: {cmd}");
        assert!(
            cmd.contains("--user-data-dir=/home/u/.laew/chrome_debug_profile"),
            "实际: {cmd}"
        );
    }

    #[test]
    fn quote_exe_path_wraps_windows_space_path() {
        // Windows 路径常含空格(Program Files),必须引号包裹;
        // 非 Windows 平台原样返回(平台门控与 relaunch_command_string 一致)。
        let spaced = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
        if cfg!(target_os = "windows") {
            assert_eq!(quote_exe_path(spaced), format!("\"{spaced}\""));
        } else {
            assert_eq!(quote_exe_path(spaced), spaced);
        }
        // 无空格路径任何平台都不包裹。
        assert_eq!(quote_exe_path("chrome.exe"), "chrome.exe");
    }

    #[test]
    fn default_browser_command_name_per_platform() {
        let name = default_browser_command_name();
        assert!(!name.is_empty());
        #[cfg(target_os = "macos")]
        assert!(name.ends_with("Google Chrome"));
        #[cfg(target_os = "windows")]
        assert_eq!(name, "chrome.exe");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(name, "google-chrome");
    }

    #[test]
    fn chromium_process_names_non_empty() {
        let names = chromium_process_names();
        assert!(!names.is_empty());
        for n in names {
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn probe_candidates_order_and_format() {
        let cands = probe_candidates(9222);
        assert_eq!(cands.len(), 3);
        // localhost 优先(Chrome 154+ 只服务 IPv6 loopback,localhost 多解析到 ::1)
        assert_eq!(cands[0], "http://localhost:9222");
        assert_eq!(cands[1], "http://[::1]:9222");
        assert_eq!(cands[2], "http://127.0.0.1:9222");
    }

    #[test]
    fn copy_profile_secrets_copies_files_and_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        // 构造源:文件 + 目录 + 清单外文件(不应被复制)。
        std::fs::create_dir_all(src.join("Default/Local Storage")).unwrap();
        std::fs::write(src.join("Default/Cookies"), "cookie-db").unwrap();
        std::fs::write(src.join("Default/Local Storage/LOG"), "leveldb").unwrap();
        std::fs::write(src.join("Default/History"), "should-not-copy").unwrap();

        let (copied, skipped) = copy_profile_secrets(&src, &dst);
        assert!(copied >= 2, "Cookies 与 Local Storage 应被复制: {copied}");
        assert!(dst.join("Default/Cookies").is_file());
        assert_eq!(
            std::fs::read_to_string(dst.join("Default/Cookies")).unwrap(),
            "cookie-db"
        );
        assert!(dst.join("Default/Local Storage/LOG").is_file());
        assert!(
            !dst.join("Default/History").exists(),
            "清单外文件不应被复制"
        );
        // 源缺失的条目算跳过,不 panic。
        assert!(skipped > 0);
    }

    #[test]
    fn copy_profile_secrets_clears_dst_first() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(dst.join("Default")).unwrap();
        std::fs::write(dst.join("Default/stale"), "stale").unwrap();

        copy_profile_secrets(&src, &dst);
        assert!(
            !dst.join("Default/stale").exists(),
            "目标目录应先清空(过期登录态不残留)"
        );
    }

    #[test]
    fn copy_profile_secrets_missing_src_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let (copied, skipped) = copy_profile_secrets(&tmp.path().join("nope"), &tmp.path().join("dst"));
        assert_eq!((copied, skipped), (0, 0));
        assert!(!tmp.path().join("dst").exists(), "源缺失不应创建目标");
    }

    #[tokio::test]
    async fn probe_parses_json_version_response() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = r#"{"Browser":"Chrome/131.0.6778.87","webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/browser/abc123"}"#;
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        let info = probe_debug_endpoint(port).await.expect("应解析成功");
        assert_eq!(info.port, port);
        assert_eq!(info.browser_version, "Chrome/131.0.6778.87");
        assert_eq!(
            info.web_socket_debugger_url,
            "ws://127.0.0.1:9222/devtools/browser/abc123"
        );
        // connect_url 为实际命中的候选地址(mock 绑 127.0.0.1,经候选链兜底命中)
        assert!(
            info.connect_url.contains(&format!(":{port}")),
            "connect_url 应含端口: {}",
            info.connect_url
        );
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn probe_returns_none_when_port_closed() {
        // 绑定后立即关闭,拿到一个几乎必然无监听的端口。
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(
            probe_debug_endpoint(port).await.is_none(),
            "无监听端口应返回 None"
        );
    }

    #[tokio::test]
    async fn probe_returns_none_on_non_json_response() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nnot json!!";
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        assert!(probe_debug_endpoint(port).await.is_none());
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn wait_for_debug_port_times_out() {
        // 无监听端口 + 短超时:轮询至超时返回 None(不真等 15s)。
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let start = std::time::Instant::now();
        let r = wait_for_debug_port_with_timeout(port, Duration::from_millis(350)).await;
        assert!(r.is_none(), "超时后应返回 None");
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(300), "应轮询到超时");
        assert!(elapsed < Duration::from_secs(5), "不应真等 15s");
    }

    /// 真机集成(环境自适应):本机有调试模式 Chrome 时,验证完整复用链路
    /// (探测 → Browser::connect 接管 → 打开页面 → 读标题 → 关闭断连);
    /// 无调试端口时 skip(不失败)。
    ///
    /// 本测试持有 BrowserManager 单例,收尾用 close_page 复位单例
    /// (connect 模式 close 只断连,不影响用户 Chrome 进程)。
    #[tokio::test]
    async fn connect_real_chrome_when_debug_port_open() {
        let port = debug_port();
        let Some(info) = probe_debug_endpoint(port).await else {
            eprintln!("skip: 本机无调试模式 Chrome(端口 {port} 无监听)");
            return;
        };
        assert!(!info.browser_version.is_empty());
        assert!(info.web_socket_debugger_url.starts_with("ws://"));
        let mgr = crate::agent::browser::BrowserManager::global();
        let (page_id, _title, _final_url) = mgr
            .new_page(
                "https://example.com",
                crate::agent::browser::BrowserMode::Headed,
                Some(&info.connect_url),
                None,
                None,
                false,
                // 第 143 轮:new_page 第 7 参改为管控配置(connect 模式会强制 open)
                crate::agent::browser_overlay::PageGuardConfig::open(),
            )
            .await
            .expect("connect 接管应成功");
        assert!(page_id.starts_with("p_"));
        assert!(mgr.is_connect_mode().await, "connect 模式应可见");
        // 收尾:关闭页面(末页时 connect 模式只断连,单例复位,用户 Chrome 不受影响)
        assert!(mgr.close_page(&page_id).await);
        assert!(
            !mgr.is_connect_mode().await,
            "close 末页后单例应复位(不影响用户 Chrome 进程)"
        );
    }

    /// Chromium 系进程 PID 快照(unix:pgrep;Windows 恒空,断言退化为只看端口)。
    fn chromium_pids() -> Vec<u32> {
        #[cfg(not(target_os = "windows"))]
        {
            let mut out = Vec::new();
            for name in chromium_process_names() {
                if let Ok(o) = std::process::Command::new("pgrep").arg("-x").arg(name).output() {
                    for line in String::from_utf8_lossy(&o.stdout).lines() {
                        if let Ok(pid) = line.trim().parse::<u32>() {
                            out.push(pid);
                        }
                    }
                }
            }
            out
        }
        #[cfg(target_os = "windows")]
        {
            Vec::new()
        }
    }

    /// 真机集成(第 144 轮 #[ignore]):auto_relaunch 全链路**不退出用户 Chrome**。
    ///
    /// 断言:① 另启独立调试实例后端口就绪;② 启动前已存在的全部 Chromium 系
    /// 进程(用户浏览器)仍然存活(核心验收);③ 登录态复制计数 ≥1(桌面平台);
    /// ④ 收尾对调试实例发 CDP Browser.close 优雅回收(只关我们另启的实例)。
    /// 跑 `cargo test --lib relaunch_never_kills -- --ignored --nocapture`。
    #[tokio::test]
    #[ignore = "真机 Chrome 冒烟:复制用户登录态 + 弹独立调试 Chrome 窗口约 15s"]
    async fn relaunch_never_kills_user_chrome() {
        // 用 9333 避开默认 9222(用户可能真开着调试 Chrome);端口已占用则 skip。
        let port = 9333u16;
        if probe_debug_endpoint(port).await.is_some() {
            eprintln!("skip: 端口 {port} 已有服务,另选端口重跑");
            return;
        }
        let before = chromium_pids();
        let out = relaunch_and_wait(port)
            .await
            .expect("复制登录态 + 另启调试实例应成功(本机是否装有 Chrome?)");
        let after = chromium_pids();
        // ① 核心验收:用户既有进程必须全部存活
        for pid in &before {
            assert!(after.contains(pid), "用户浏览器进程 {pid} 被误杀(第 144 轮红线)");
        }
        // ② 调试实例确实新起(进程数只增不减)
        assert!(after.len() > before.len(), "应另启独立调试实例:{before:?} → {after:?}");
        // ③ 登录态复制计数(桌面平台清单非空)
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(
            out.login_state_copied + out.login_state_skipped >= 1,
            "复制清单至少统计到条目:copied={} skipped={}",
            out.login_state_copied,
            out.login_state_skipped
        );
        eprintln!(
            "[smoke] login_state copied={} skipped={}",
            out.login_state_copied, out.login_state_skipped
        );
        // ④ 优雅回收调试实例(CDP Browser.close;只影响我们另启的实例)
        {
            use chromiumoxide::browser::Browser;
            use futures::StreamExt;
            if let Ok((mut browser, mut handler)) = Browser::connect(out.info.connect_url.clone()).await {
                let driver = tokio::spawn(async move {
                    while let Some(msg) = handler.next().await {
                        if msg.is_err() {
                            break;
                        }
                    }
                });
                let _ = browser.close().await;
                drop(browser);
                let _ = driver.await;
            }
        }
        // 等调试实例退出后,用户进程仍全部存活
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let final_pids = chromium_pids();
        for pid in &before {
            assert!(final_pids.contains(pid), "回收调试实例后用户进程 {pid} 仍应存活");
        }
    }
}
