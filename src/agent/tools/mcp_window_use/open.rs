//! MCP_Window_Use 启动类 action(2026-09-18 第 84 轮自 tools/window/open.rs 迁入):
//! `open` —— 已在运行直接恢复前置、开始菜单快捷方式扫描、固定盘安装路径探测、
//! ShellExecuteW 兜底;无 macOS AX 权限也能启动并轮询定位窗口。

use super::*;

// ===================== action=open =====================

fn safe_desktop_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.contains(['\0', '\r', '\n'])
        && s.chars().count() <= 128
}

// ===================== Windows 启动解析链 =====================
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
            "{e};建议:1) 提供 app_name 完整路径;2) 先手动打开应用再让 MCP_Window_Use(action=open) 激活"
        )),
    }
}

// ===================== 2026-09-18 第 85 轮:已知桌面应用 Bundle ID 映射 =====================
//
// 背景:`open -a <name>` 在 macOS 上只匹配 CFBundleName,对中文 DisplayName(如"微信")
// 直接报 `Unable to find application named '微信'`,exit 1。这是本次微信桌面自动化任务
// 失败的头号根因(llaew_20260918_114507.log 实证:open -a 微信 exit status: 1,后续
// 3 次重试 928s 全部走降级,从未实际启动微信)。
//
// 解决:把"常见中文/英文别名的桌面应用"映射到它们真实的 CFBundleIdentifier,统一走
// `open -b <bundle_id>` 启动;LLM 仍可只传 query="微信",工具自动查表用 bundle id 启动。
//
// 维护来源:Apple Stack Exchange / macadmins Slack / 各厂商官方下载页;
// 版本变更时如 bundle id 改名,需同步更新本表;新增应用直接追加,无需改调用方。
const KNOWN_BUNDLE_IDS: &[(&str, &str)] = &[
    // 微信(macOS 当前主版本 / 历史版本)
    ("微信", "com.tencent.xinWeChat"),
    ("wechat", "com.tencent.xinWeChat"),
    ("WeChat", "com.tencent.xinWeChat"),
    ("Weixin", "com.tencent.xinWeChat"),
    ("weixin", "com.tencent.xinWeChat"),
    // 钉钉
    ("钉钉", "com.laiwang.DingTalk"),
    ("DingTalk", "com.laiwang.DingTalk"),
    ("dingtalk", "com.laiwang.DingTalk"),
    // 飞书 / Lark
    ("飞书", "com.bytedance.feishu"),
    ("Feishu", "com.bytedance.feishu"),
    ("Lark", "com.bytedance.feishu"),
    ("lark", "com.bytedance.feishu"),
    // QQ
    ("QQ", "com.tencent.qq"),
    ("qq", "com.tencent.qq"),
    // 腾讯会议
    ("腾讯会议", "com.tencent.meeting"),
    ("TencentMeeting", "com.tencent.meeting"),
    ("VooV", "com.tencent.meeting"),
    // 腾讯文档
    ("腾讯文档", "com.tencent.tdocument"),
    ("TencentDocs", "com.tencent.tdocument"),
    // 字节系
    ("豆包", "com.bot.pc.doubao"),
    ("Doubao", "com.bot.pc.doubao"),
    // 网易
    ("网易云音乐", "com.netease.amp.mac"),
    ("NetEaseMusic", "com.netease.amp.mac"),
    // 微软(Mac 版)
    ("VSCode", "com.microsoft.VSCode"),
    ("Visual Studio Code", "com.microsoft.VSCode"),
    ("Edge", "com.microsoft.edgemac"),
    // 笔记 / 协作
    ("Notion", "notion.id"),
    ("Obsidian", "md.obsidian"),
    ("Slack", "com.tinyspeck.chatlyio"),
    ("Discord", "com.hnc.Discord"),
    ("Zoom", "us.zoom.xos"),
    ("Telegram", "ru.keepcoder.Telegram"),
    // 系统 / 工具
    ("Terminal", "com.apple.Terminal"),
    ("终端", "com.apple.Terminal"),
    ("Finder", "com.apple.finder"),
    ("访达", "com.apple.finder"),
    ("Safari", "com.apple.Safari"),
    ("System Settings", "com.apple.systempreferences"),
    ("系统设置", "com.apple.systempreferences"),
];

/// 在已知 Bundle ID 表里查 query 命中(忽略大小写、全词子串)。
pub(super) fn lookup_known_bundle_id(query: &str) -> Option<&'static str> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    for (alias, bundle_id) in KNOWN_BUNDLE_IDS {
        if q.eq_ignore_ascii_case(alias) || q.contains(alias) {
            return Some(bundle_id);
        }
    }
    None
}

/// 通过 mdfind 在 Spotlight 元数据里查 kind:application 命中的应用路径。
///
/// 兜底:表里没命中且 open -a 失败时,先尝试 mdfind 拿到 .app 完整路径再 open <path>。
/// mdfind 索引可能为空(Spotlight 关闭或首次启动未完成),失败静默返回 None。
#[cfg(target_os = "macos")]
fn mdfind_app_path(query: &str) -> Option<String> {
    use std::process::Command;
    let escaped = query.replace('"', "");
    if escaped.is_empty() {
        return None;
    }
    let output = Command::new("mdfind")
        .arg(format!("kind:application AND name:\"{escaped}\""))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let p = line.trim();
        if p.ends_with(".app") && std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

/// 第 109 轮:mdfind 按 Bundle ID 精确查询真实 .app 路径。
///
/// 用途:`open -b <id>` 失败(LaunchServices 过期缓存)但 bundle id 本身正确时的
/// 兜底 —— 与应用显示名/目录名无关,不受「豆包 → 豆包浏览器」同名解析陷阱影响。
#[cfg(target_os = "macos")]
fn mdfind_app_path_by_bundle_id(bundle_id: &str) -> Option<String> {
    use std::process::Command;
    let bid = bundle_id.replace(['"', '\\'], "");
    if bid.is_empty() {
        return None;
    }
    let output = Command::new("mdfind")
        .arg(format!("kMDItemCFBundleIdentifier == '{bid}'"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let p = line.trim();
        if p.ends_with(".app") && std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

fn launch_desktop_app(
    app: &str,
    bundle_id: Option<&str>,
) -> std::result::Result<Vec<String>, String> {
    // Windows 走 OS API 解析链(ShellExecuteW / 快捷方式 / 安装路径)
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

    // macOS / Linux 启动解析链(2026-09-18 第 85 轮 P0-1 强化):
    // 优先级 a -> b -> c -> d;bundle_id 优先 + 已知映射表 + mdfind 兜底,
    // 解决 open -a 中文名报 Unable to find application named '微信' 的根本问题。
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

        // 优先级 a:LLM 显式给了 bundle_id -> 直接 open -b(最高优先级,不走别名)
        let explicit_bundle = bundle_id
            .filter(|s| safe_desktop_identifier(s))
            .map(str::to_string);
        // 真实安装包探测:硬编码 Bundle 表可能随厂商版本过期。先在常规
        // Applications 目录与 Spotlight 中找实际 .app,并读取真实 Bundle ID。
        #[cfg(target_os = "macos")]
        let installed_apps = discover_installed_mac_apps(&candidates);
        #[cfg(not(target_os = "macos"))]
        let installed_apps: Vec<InstalledMacApp> = Vec::new();

        // 优先级 b:从真实安装包 / 已知映射表反查(微信 / 钉钉 / 飞书 等)
        let mut resolved_bundle: Option<String> = explicit_bundle;
        if resolved_bundle.is_none() {
            if let Some(app) = installed_apps.iter().find(|app| app.bundle_id.is_some()) {
                resolved_bundle = app.bundle_id.clone();
            }
        }
        if resolved_bundle.is_none() {
            for candidate in &candidates {
                if let Some(b) = lookup_known_bundle_id(candidate) {
                    resolved_bundle = Some(b.to_string());
                    break;
                }
            }
        }

        let mut tried: Vec<String> = Vec::new();

        // 优先级 a0:真实安装包路径最可靠,不受 LaunchServices 的过期 Bundle
        // ID 缓存影响; discovery 顺序与候选顺序一致。
        if cfg!(target_os = "macos") {
            for app in &installed_apps {
                let display = format!("open {}", app.path);
                let mut c = Command::new("open");
                c.arg(&app.path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                match c.status() {
                    Ok(s) if s.success() => return Ok(vec![display]),
                    _ => tried.push(display),
                }
            }
        }

        // 优先级 a/b:open -b(走 Bundle ID;最稳定,绕过中文 DisplayName 匹配问题)
        if let Some(ref bid) = resolved_bundle {
            if safe_desktop_identifier(bid) {
                let display = format!("open -b {bid}");
                let mut c = Command::new("open");
                c.arg("-b").arg(bid)
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                match c.status() {
                    Ok(s) if s.success() => return Ok(vec![display]),
                    _ => tried.push(display),
                }
            }
        }

        // 优先级 b2(第 109 轮):mdfind 按 bundle id 精确定位真实 .app 路径。
        // 场景:bundle id 已解析(表命中/LLM 显式)但 open -b 失败(LaunchServices
        // 缓存损坏 / bundle 改名)。Spotlight 按 kMDItemCFBundleIdentifier 查询
        // 与应用显示名无关,不受「豆包 → 豆包浏览器」同名解析陷阱影响。
        #[cfg(target_os = "macos")]
        if let Some(ref bid) = resolved_bundle {
            if let Some(path) = mdfind_app_path_by_bundle_id(bid) {
                let display = format!("open {path}");
                let mut c = Command::new("open");
                c.arg(&path)
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                match c.status() {
                    Ok(s) if s.success() => {
                        tried.push(display);
                        return Ok(tried);
                    }
                    _ => tried.push(display),
                }
            }
        }

        // 优先级 c:open -a 遍历别名(英文优先,中文 DisplayName 一般会失败但试一下无成本)。
        // 第 109 轮:bundle id 已解析时**跳过中文别名** —— LaunchServices 同名陷阱:
        // 「豆包」被解析到 豆包浏览器(com.bot.pc.doubao.linkrouter)而非豆包主应用
        // (com.bot.pc.doubao),open -a 豆包 表面成功实则启动错误应用,窗口永远匹配不上。
        let skip_open_a_aliases = resolved_bundle.is_some();
        if !skip_open_a_aliases {
            for candidate in &candidates {
                if !safe_desktop_identifier(candidate) {
                    continue;
                }
                let display = format!("open -a {candidate}");
                let mut c = Command::new("open");
                c.arg("-a").arg(candidate)
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                match c.status() {
                    Ok(s) if s.success() => {
                        tried.push(display);
                        return Ok(tried);
                    }
                    _ => tried.push(display),
                }
            }
        }

        // 优先级 d:mdfind 兜底 —— 表里没有 + open -a 失败时,Spotlight 查 .app 路径直接 open <path>
        #[cfg(target_os = "macos")]
        {
            for candidate in &candidates {
                if let Some(path) = mdfind_app_path(candidate) {
                    if !safe_desktop_identifier(&path) {
                        continue;
                    }
                    let display = format!("open {path}");
                    let mut c = Command::new("open");
                    c.arg(&path)
                        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    match c.status() {
                        Ok(s) if s.success() => {
                            tried.push(display);
                            return Ok(tried);
                        }
                        _ => tried.push(display),
                    }
                    break; // 只取首个命中
                }
            }
        }

        // 优先级 e:Linux gtk-launch 兜底
        #[cfg(not(target_os = "macos"))]
        {
            for candidate in &candidates {
                if !safe_desktop_identifier(candidate) {
                    continue;
                }
                let display = format!("gtk-launch {}", candidate.trim_end_matches(".desktop"));
                let mut c = Command::new("gtk-launch");
                c.arg(candidate.trim_end_matches(".desktop"))
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                match c.status() {
                    Ok(s) if s.success() => {
                        tried.push(display);
                        return Ok(tried);
                    }
                    _ => tried.push(display),
                }
            }
        }

        // 全部失败 -> 统一错误文案(让 LLM 下次直接给 bundle_id)
        Err(format!(
            "启动 {app:?} 全部失败;已尝试: {tried:?}。建议:1) 在 action=open 时显式传 bundle_id(微信=com.tencent.xinWeChat / 钉钉=com.laiwang.DingTalk / 飞书=com.bytedance.feishu / 豆包=com.bot.pc.doubao);             2) 用 system_profiler SPApplicationsDataType | grep -B1 -A6 bundle 查真实 bundle id;             3) 注意 macOS 同名应用陷阱:「豆包」可被 LaunchServices 解析到 豆包浏览器(com.bot.pc.doubao.linkrouter)而非豆包主应用,勿用 open -a 中文名;             4) 手动启动应用后再 action=open 激活。"
        ))
    }
}

/// macOS 实际安装包记录;非 macOS 仅作空 vector 的类型占位。
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledMacApp {
    path: String,
    bundle_id: Option<String>,
}

/// 从 Info.plist 读取真实 Bundle ID(defaults 失败时返回 None)。
#[cfg(target_os = "macos")]
fn installed_mac_bundle_id(path: &str) -> Option<String> {
    let output = std::process::Command::new("defaults")
        .arg("read")
        .arg(format!("{path}/Contents/Info"))
        .arg("CFBundleIdentifier")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty() && safe_desktop_identifier(&value)).then_some(value)
}

/// 按候选别名发现实际 .app,优先常规目录精确命中,Spotlight 只作兜底。
#[cfg(target_os = "macos")]
fn discover_installed_mac_apps(candidates: &[String]) -> Vec<InstalledMacApp> {
    use std::path::PathBuf;

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let roots = [
        Some(PathBuf::from("/Applications")),
        home.as_ref().map(|h| h.join("Applications")),
        Some(PathBuf::from("/System/Applications")),
    ];
    let mut out: Vec<InstalledMacApp> = Vec::new();
    let mut push_app = |path: String, out: &mut Vec<InstalledMacApp>| {
        if !safe_desktop_identifier(&path) || !std::path::Path::new(&path).exists() {
            return;
        }
        if out.iter().any(|app| app.path == path) {
            return;
        }
        out.push(InstalledMacApp {
            path,
            bundle_id: None,
        });
    };

    for candidate in candidates {
        for root in roots.iter().flatten() {
            let path = root.join(format!("{candidate}.app"));
            push_app(path.to_string_lossy().to_string(), &mut out);
        }
        if out.len() >= 3 {
            break;
        }
    }

    if out.is_empty() {
        for candidate in candidates {
            if let Some(path) = mdfind_app_path(candidate) {
                push_app(path, &mut out);
                break;
            }
        }
    }

    for app in &mut out {
        app.bundle_id = installed_mac_bundle_id(&app.path);
    }
    out
}

/// 启动/激活桌面应用并等待窗口出现,返回 window_id、匹配别名、窗口数量与权限状态。
///
/// 已在运行(含最小化到托盘)→ 恢复 + 前置,不重复启动;已在前台 → 跳过激活
/// (不再 AXRaise/osascript/400ms sleep/bounds 重取),窗口不被反复前置闪烁。
pub(super) async fn run(args: Value) -> Result<String> {
    let query = require_str(&args, "query", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let app_name = get_str(&args, "app_name").unwrap_or(&query).to_string();
    let bundle_id = get_str(&args, "bundle_id").map(str::to_string);
    let wait_secs = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        // 微信等大型应用首次启动需 8-10s,默认 10。
        // 可通过 LAEW_WINDOW_OPEN_WAIT 环境变量调整(默认 10,最大 30)。
        .unwrap_or_else(|| {
            std::env::var("LAEW_WINDOW_OPEN_WAIT")
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(10)
        })
        .min(30);

    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let driver = current_driver();
        let before = driver.list_windows(None)?;
        let aliases = expand_window_query(&query);
        let before_hit = find_window_by_aliases(&before, &aliases);
        let started = std::time::Instant::now();

        // 已在运行(含最小化到托盘)→ 恢复 + 前置,不重复启动。
        if let Some((matched_query, mut info)) = before_hit.clone() {
            let already_frontmost = driver.is_frontmost(&info.id);
            let activated = if already_frontmost {
                true
            } else {
                let ok = driver.bring_to_front(&info.id).is_ok();
                std::thread::sleep(std::time::Duration::from_millis(400));
                // 前置后重取一次最新 bounds(恢复最小化后 -32000 会刷新为真实坐标)
                if let Some(fresh) = driver
                    .list_windows(None)
                    .ok()
                    .and_then(|ws| ws.into_iter().find(|w| w.id == info.id))
                {
                    info = fresh;
                }
                ok
            };
            let permission_hint = driver.permission_hint();
            let perm = crate::agent::window::check_platform_permissions();
            let next_action = if perm.screen_recording {
                "窗口已在运行;直接 action=inspect / action=ocr 继续。若控件树为空(自绘 UI),改用 action=ocr 视觉路线。"
            } else if perm.accessibility {
                "窗口已在运行;屏幕录制未授权 → ocr/screenshot/screencapture 均不可用,直接 action=inspect(AX 主路线)+ 坐标估算 / type_text_submit。"
            } else {
                "窗口已在运行;辅助功能未授权 → 仅可 action=list/action=find,读取/操作 UI 需用户先授权。"
            };
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
                "already_visible_before_launch":true,
                "already_frontmost":already_frontmost,
                "activated_existing":activated,
                "visible_before":before.len(),
                "launch_commands":[],
                "wait_ms":started.elapsed().as_millis() as u64,
                "driver":driver.platform_name(),
                "inspect_ready":permission_hint.is_none(),
                "permission_hint":permission_hint,
                "next_action":next_action,
            });
            return Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()));
        }

        let commands =
            launch_desktop_app(&app_name, bundle_id.as_deref())
                .map_err(|e| tool_err(MCP_WINDOW_USE_TOOL_NAME, e))?;

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
                MCP_WINDOW_USE_TOOL_NAME,
                format!(
                    "启动命令已执行({commands:?},耗时 {:.1}s),但 {wait_secs}s 内未匹配到窗口。尝试别名:{aliases:?};当前可见前 10 个:{titles:?}。\
                     提示:应用可能弹出了登录窗(标题不同)或启动较慢;可用 action=list(filter=相关词)确认,或加大 wait_seconds 重试。",
                    started.elapsed().as_secs_f32()
                ),
            ));
        };
        let permission_hint = driver.permission_hint();
        // next_action 按屏幕录制权限分派(避免把 LLM 推向不可用的 OCR 路线)
        let perm = crate::agent::window::check_platform_permissions();
        let next_action = if perm.screen_recording {
            "inspect_ready=true 时用 window_id 调 action=inspect;控件树为空(自绘 UI)时改用 action=ocr。"
        } else if perm.accessibility {
            "inspect_ready=true 时用 window_id 调 action=inspect(AX 主路线);屏幕录制未授权,ocr/screenshot/screencapture 均不可用,坐标用窗口 bounds 比例估算。"
        } else {
            "辅助功能未授权:仅可 action=list/action=find 定位窗口;读取/操作 UI 需用户先授权。"
        };
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
            "next_action":next_action,
        });
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    })
    .await
}
