//! 桌面应用启动工具(2026-09-17 自 tools/window.rs 拆分)。
//!
//! `WindowOpenTool`:已在运行直接恢复前置、开始菜单快捷方式扫描、固定盘
//! 安装路径探测、ShellExecuteW 兜底;无 macOS AX 权限也能启动并轮询定位窗口。

use super::*;

// ===================== WindowOpen =====================

/// 桌面应用启动 / 激活工具:无 macOS AX 权限也能启动应用并轮询定位窗口。
pub struct WindowOpenTool;

fn safe_desktop_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.contains(['\0', '\r', '\n'])
        && s.chars().count() <= 128
}

// ===================== Windows 启动解析链(2026-09-16 第 67 轮) =====================
//
// 背景:微信 4.x(Weixin.exe)既不在 PATH 也不注册 App Paths,实测装在
// `D:\Program Files (x86)\Tencent\Weixin\`,旧版 `powershell Start-Process WeChat`
// 必然失败。新解析链(全部 OS API,无 PowerShell):
//   a. app_name 是已存在的绝对路径(或带 .lnk)→ 直接 ShellExecuteW;
//   b. `ShellExecuteW("<app>.exe")` —— shell 自动走 App Paths(覆盖多数传统应用);
//   c. 开始菜单快捷方式扫描(%ProgramData% + %APPDATA%,文件名含别名,.lnk 直接打开)
//      —— 本机微信 4.x 唯一可靠发现方式:`C:\ProgramData\...\微信\微信.lnk`;
//   d. 已知安装路径探测(全部固定盘 × Program Files[(x86)] × Tencent\{Weixin,WeChat});
//   e. 裸名 `ShellExecuteW(app_name)` 兜底(协议 / App Paths)。

/// ShellExecuteW "open"(成功返回 Ok;失败带回 hresult 文案)。
#[cfg(windows)]
fn shell_execute_open(target: &str) -> std::result::Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY:标准 ShellExecuteW;参数均为合法宽字符串指针。
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // 返回值 > 32 表示成功(SE_ERR_* 为 ≤32 的错误码)
    let code = r.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW({target}) 失败,错误码 {code}"))
    }
}

/// 枚举全部固定磁盘盘符(如 ["C:\\", "D:\\"])。
#[cfg(windows)]
fn fixed_drive_roots() -> Vec<String> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDriveStringsW};
    use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
    let mut buf = [0u16; 256];
    // SAFETY:标准 GetLogicalDriveStringsW,缓冲区足够容纳所有盘符。
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return vec!["C:\\".to_string()];
    }
    let raw = String::from_utf16_lossy(&buf[..n]);
    let mut out = Vec::new();
    for root in raw.split('\0').filter(|s| !s.is_empty()) {
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY:盘符字符串来自系统返回值。
        let dt = unsafe { GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr())) };
        if dt == DRIVE_FIXED {
            out.push(root.to_string());
        }
    }
    if out.is_empty() {
        out.push("C:\\".to_string());
    }
    out
}

/// 扫描开始菜单快捷方式,返回「文件名含任一别名」的 .lnk 完整路径(深度 ≤4,数量 ≤5)。
#[cfg(windows)]
fn scan_start_menu_shortcuts(aliases: &[String]) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(pd) = std::env::var_os("ProgramData") {
        let p = std::path::PathBuf::from(&pd).join("Microsoft/Windows/Start Menu/Programs");
        if p.is_dir() {
            roots.push(p);
        }
    }
    if let Some(ad) = std::env::var_os("APPDATA") {
        let p = std::path::PathBuf::from(&ad).join("Microsoft/Windows/Start Menu/Programs");
        if p.is_dir() {
            roots.push(p);
        }
    }
    let lower_aliases: Vec<String> = aliases.iter().map(|a| a.to_lowercase()).collect();
    let mut hits = Vec::new();
    fn walk(
        dir: &std::path::Path,
        depth: usize,
        lower_aliases: &[String],
        hits: &mut Vec<std::path::PathBuf>,
    ) {
        if depth > 4 || hits.len() >= 5 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1, lower_aliases, hits);
            } else if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                if !stem.is_empty() && lower_aliases.iter().any(|a| stem.contains(a.as_str())) {
                    hits.push(path);
                }
            }
        }
    }
    for root in roots {
        walk(&root, 0, &lower_aliases, &mut hits);
    }
    hits
}

/// Windows 启动解析链:返回成功执行的目标(用于错误信息回显)。
#[cfg(windows)]
fn launch_windows_app(app: &str, aliases: &[String]) -> std::result::Result<String, String> {
    // a. 绝对路径 / 显式 .exe / .lnk
    let p = std::path::Path::new(app);
    if p.is_absolute() && p.exists() {
        shell_execute_open(app)?;
        return Ok(app.to_string());
    }
    // b. ShellExecuteW("<app>.exe")(App Paths 解析)
    if !app.contains('\\') && !app.contains('/') {
        let exe = if app.to_lowercase().ends_with(".exe") {
            app.to_string()
        } else {
            format!("{app}.exe")
        };
        if let Ok(()) = shell_execute_open(&exe) {
            return Ok(exe);
        }
    }
    // c. 开始菜单快捷方式
    for lnk in scan_start_menu_shortcuts(aliases) {
        if let Some(s) = lnk.to_str() {
            if shell_execute_open(s).is_ok() {
                return Ok(s.to_string());
            }
        }
    }
    // d. 已知安装路径探测(Tencent 家族:微信 4.x=Weixin,3.x=WeChat;QQ 同目录族)
    let lower = app.to_lowercase();
    let family = ["weixin", "wechat", "微信", "qq"];
    if family.iter().any(|f| lower.contains(f)) || aliases.iter().any(|a| {
        let a = a.to_lowercase();
        family.iter().any(|f| a.contains(f))
    }) {
        let mut candidates = Vec::new();
        for root in fixed_drive_roots() {
            for pf in ["Program Files", "Program Files (x86)"] {
                candidates.push(format!("{root}{pf}\\Tencent\\Weixin\\Weixin.exe"));
                candidates.push(format!("{root}{pf}\\Tencent\\WeChat\\WeChat.exe"));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(format!(
                "{}\\Programs\\Tencent\\Weixin\\Weixin.exe",
                local.to_string_lossy()
            ));
            candidates.push(format!(
                "{}\\Programs\\Tencent\\WeChat\\WeChat.exe",
                local.to_string_lossy()
            ));
        }
        let tried: Vec<String> = candidates.clone();
        for c in candidates {
            if std::path::Path::new(&c).exists() {
                if let Ok(()) = shell_execute_open(&c) {
                    return Ok(c);
                }
            }
        }
        // 都不存在 → 带尝试清单失败(排查可见)
        return Err(format!(
            "未找到微信安装路径;已探测: {:?};建议改用开始菜单快捷方式或提供完整 exe 路径作为 app_name",
            tried
                .iter()
                .filter(|c| std::path::Path::new(c.as_str()).exists())
                .cloned()
                .collect::<Vec<_>>()
        ));
    }
    // e. 裸名兜底(协议 / App Paths)
    match shell_execute_open(app) {
        Ok(()) => Ok(app.to_string()),
        Err(e) => Err(format!(
            "{e};建议:1) 提供 app_name 完整路径;2) 先手动打开应用再让 WindowOpen 激活"
        )),
    }
}

fn launch_desktop_app(
    app: &str,
    bundle_id: Option<&str>,
) -> std::result::Result<Vec<String>, String> {
    // 2026-09-16 第 67 轮:Windows 走 OS API 解析链(ShellExecuteW / 快捷方式 / 安装路径)
    #[cfg(windows)]
    {
        let aliases = expand_window_query(app);
        if !safe_desktop_identifier(app) {
            return Err(format!("非法应用标识: {app:?}"));
        }
        return match launch_windows_app(app, &aliases) {
            Ok(target) => Ok(vec![format!("ShellExecuteW(open, {target})")]),
            Err(e) => Err(e),
        };
    }

    // macOS / Linux 维持原 open / gtk-launch 路径
    #[cfg(not(windows))]
    {
        use std::process::{Command, Stdio};
        let aliases = expand_window_query(app);
        let mut candidates = vec![app.to_string()];
        for alias in aliases {
            if !candidates.contains(&alias) {
                candidates.push(alias);
            }
        }
        let mut tried = Vec::new();
        for candidate in candidates {
            if !safe_desktop_identifier(&candidate) {
                return Err(format!("非法应用标识: {candidate:?}"));
            }
            let display = if cfg!(target_os = "macos") {
                bundle_id
                    .filter(|s| safe_desktop_identifier(s))
                    .map(|b| format!("open -b {b}"))
                    .unwrap_or_else(|| format!("open -a {candidate}"))
            } else {
                format!("gtk-launch {}", candidate.trim_end_matches(".desktop"))
            };

            let mut command = if cfg!(target_os = "macos") {
                let mut c = Command::new("open");
                if let Some(b) = bundle_id.filter(|s| safe_desktop_identifier(s)) {
                    c.arg("-b").arg(b);
                } else {
                    c.arg("-a").arg(&candidate);
                }
                c
            } else {
                let mut c = Command::new("gtk-launch");
                c.arg(candidate.trim_end_matches(".desktop"));
                c
            };
            let status = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            tried.push(display);
            match status {
                Ok(s) if s.success() => return Ok(tried),
                Ok(s) => return Err(format!("启动命令退出码异常: {s};已尝试: {tried:?}")),
                Err(e) if cfg!(target_os = "macos") && tried.len() < 3 => {
                    let _ = e;
                    continue;
                }
                Err(e) => return Err(format!("无法执行启动命令: {e};已尝试: {tried:?}")),
            }
        }
        Err("未找到可执行的应用标识".into())
    }
}

#[async_trait]
impl Tool for WindowOpenTool {
    fn name(&self) -> &str {
        "WindowOpen"
    }

    fn description(&self) -> &str {
        "启动/激活桌面应用并等待窗口出现,返回 window_id、匹配别名、窗口数量与权限状态。支持 WeChat ↔ 微信别名。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "query":{"type":"string"},
                "app_name":{"type":"string"},
                "bundle_id":{"type":"string"},
                "wait_seconds":{"type":"integer","minimum":0,"maximum":20}
            },
            "required":["query"],
            "additionalProperties":false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = require_str(&args, "query", self.name())?.to_string();
        let app_name = get_str(&args, "app_name").unwrap_or(&query).to_string();
        let bundle_id = get_str(&args, "bundle_id").map(str::to_string);
        let wait_secs = args
            .get("wait_seconds")
            .and_then(Value::as_u64)
            // 2026-09-16 第 65 轮 P1-C:微信等大型应用首次启动需 8-10s,默认从 6 改 10。
            // 可通过 LAEW_WINDOW_OPEN_WAIT 环境变量调整(默认 10,最大 30)。
            .unwrap_or_else(|| {
                std::env::var("LAEW_WINDOW_OPEN_WAIT")
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(10)
            })
            .min(30);

        run_blocking(self.name(), move || {
            let driver = current_driver();
            let before = driver.list_windows(None)?;
            let aliases = expand_window_query(&query);
            let before_hit = find_window_by_aliases(&before, &aliases);
            let started = std::time::Instant::now();

            // 2026-09-16 第 67 轮:已在运行(含最小化到托盘)→ 恢复 + 前置,不重复启动。
            // 此前无视已有窗口直接再启动一次,既浪费又可能触发单实例冲突。
            if let Some((matched_query, info)) = before_hit.clone() {
                let activated = driver.bring_to_front(&info.id).is_ok();
                std::thread::sleep(std::time::Duration::from_millis(400));
                // 前置后重取一次最新 bounds(恢复最小化后 -32000 会刷新为真实坐标)
                let refreshed = driver
                    .list_windows(None)
                    .ok()
                    .and_then(|ws| ws.into_iter().find(|w| w.id == info.id))
                    .unwrap_or(info);
                let permission_hint = driver.permission_hint();
                let body = json!({
                    "ok":true,
                    "window_id":refreshed.id,
                    "title":refreshed.title,
                    "process_name":refreshed.process_name,
                    "pid":refreshed.pid,
                    "bounds":refreshed.bounds,
                    "query":query,
                    "matched_query":matched_query,
                    "query_aliases":aliases,
                    "already_visible_before_launch":true,
                    "activated_existing":activated,
                    "visible_before":before.len(),
                "launch_commands":[],
                    "wait_ms":started.elapsed().as_millis() as u64,
                    "driver":driver.platform_name(),
                    "inspect_ready":permission_hint.is_none(),
                    "permission_hint":permission_hint,
                    "next_action":"窗口已在运行并已激活;直接 WindowInspect / WindowOCR 继续。若控件树为空(自绘 UI),改用 WindowOCR 视觉路线。"
                });
                return Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()));
            }

            let commands =
                launch_desktop_app(&app_name, bundle_id.as_deref())
                    .map_err(|e| tool_err("WindowOpen", e))?;

            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
            let mut after = before.clone();
            let mut matched = before_hit.clone();
            while matched.is_none() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(250));
                after = driver.list_windows(None)?;
                matched = find_window_by_aliases(&after, &aliases);
            }

            let Some((matched_query, info)) = matched else {
                let titles: Vec<String> = after
                    .iter()
                    .take(10)
                    .map(|w| format!("{} ({})", w.title, w.process_name))
                    .collect();
                return Err(tool_err(
                    "WindowOpen",
                    format!(
                        "启动命令已执行({commands:?},耗时 {:.1}s),但 {wait_secs}s 内未匹配到窗口。尝试别名:{aliases:?};当前可见前 10 个:{titles:?}。\
                         提示:应用可能弹出了登录窗(标题不同)或启动较慢;可用 WindowList(filter=相关词)确认,或加大 wait_seconds 重试。",
                        started.elapsed().as_secs_f32()
                    ),
                ));
            };
            let permission_hint = driver.permission_hint();
            let body = json!({
                "ok":true,
                "window_id":info.id,
                "title":info.title,
                "process_name":info.process_name,
                "pid":info.pid,
                "bounds":info.bounds,
                "query":query,
                "matched_query":matched_query,
                "query_aliases":aliases,
                "already_visible_before_launch":before_hit.is_some(),
                "visible_before":before.len(),
                "visible_after":after.len(),
                "launch_commands":commands,
                "wait_ms":started.elapsed().as_millis() as u64,
                "driver":driver.platform_name(),
                "inspect_ready":permission_hint.is_none(),
                "permission_hint":permission_hint,
                "next_action":"inspect_ready=true 时用 window_id 调 WindowInspect;控件树为空(自绘 UI)时改用 WindowOCR。"
            });
            Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
        })
        .await
    }
}
