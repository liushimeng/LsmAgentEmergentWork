//! 构建脚本: 注入编译时间 / Git 提交哈希, 供 `laew --version` 使用。
//!
//! 设计要点(2026-09-10 第 26 轮加固):
//! 1. 短哈希取自 `git rev-parse --short=8 HEAD^{commit}`,避免 worktree/分离 HEAD/标签指向错误 commit。
//! 2. `rerun-if-changed` 跟踪 `.git/HEAD` / 当前 ref / `.git/packed-refs`(浅 clone / 大仓库常见),
//!    覆盖 git pull 后 mtime 被刷新的常规路径。
//! 3. 「跨机器 git pull 后 `.git/HEAD` mtime 不刷新导致 cargo 不重编」的兜底交给
//!    `rebuild_restart_app.sh`: 它在版本不一致时主动 `touch build.rs` 穿透 cargo 缓存;
//!    也提供 `--force` 选项让用户随时强制重编。

use std::process::Command;
use time::{macros::format_description, OffsetDateTime};

fn main() {
    // macOS: 链接系统框架(AXUIElementRef / CGWindowList 等符号)
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
        println!("cargo:rustc-link-lib=framework=Accessibility");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    // 编译时间(本地时区, 跨平台)
    //
    // 第 90 轮(2026-09-19)修复:之前用 `Command::new("date")` 调用 Unix 日期命令,
    // 在 Windows cmd.exe / PowerShell 上 `date` 行为完全不同:
    //   - cmd.exe 内置 `date` 不接受 `+%Y-%m-%d ...` 参数, 会卡在交互式日期输入
    //   - PowerShell 中 `date` 是 `Get-Date` 的别名, 不接受 Unix 格式字符串
    // 导致 `cargo:rustc-env=LAEW_BUILD_TIME` 退化为 `unix:1789782004` 字面值,
    // `laew --version` 在 Windows 上输出 `unix:xxx` 而不是可读时间。
    //
    // 修复:用 `time` crate 跨平台格式化 `YYYY-MM-DD HH:MM:SS ±HH:MM`。
    // `time` 已在 [build-dependencies] 复用本仓库已引入的同一 crate,
    // 不增加新依赖 / 不增加下载 / 不增加 build 缓存。
    //
    // 本机调试命令(单测):
    //   powershell -NoProfile -Command "Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'"
    //   date '+%Y-%m-%d %H:%M:%S %z'   # Unix / Git Bash
    let build_time = current_local_build_time();
    println!("cargo:rustc-env=LAEW_BUILD_TIME={build_time}");

    // Git 短哈希(尽力而为):HEAD^{commit} 优先,失败回退 HEAD
    let git_hash = resolve_git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LAEW_GIT_HASH={git_hash}");

    // 源文件变化时重新运行
    println!("cargo:rerun-if-changed=build.rs");
    // F6(2026-09-14 第 51 轮):src/ 变化同样重跑本脚本 —— 否则只改源码时
    // LAEW_BUILD_TIME 沿用旧值,--version 显示的编译时间失真(本轮实测
    // 09:51 构建 → 10:49 重编仍显示 09:51)。
    println!("cargo:rerun-if-changed=src/");
    // HEAD ref 文件变化时重新运行(本地提交时 mtime 会被 git 刷新)
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
        if let Some(refname) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=.git/{}", refname);
        }
    }
    // packed-refs 也可能承载 HEAD 引用(浅 clone / 大仓库常见)
    println!("cargo:rerun-if-changed=.git/packed-refs");
}

/// 解析当前 commit 短哈希。
/// 1. `git rev-parse --short=8 HEAD^{commit}` —— 解析 HEAD 最终指向的 commit 对象,
///    避免 worktree/分离 HEAD/tag 指向错误对象。
/// 2. 失败时回退 `git rev-parse --short=8 HEAD`(ref 文件存在但 commit 缺失的场景,
///    git 仍会输出截断的 short hash;不指定 `=8` 会得到默认 7 位,与脚本兜底 8 位
///    比对时会假阳性「不一致 → 强制重编」)。
fn resolve_git_hash() -> Option<String> {
    let primary = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD^{commit}"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(o.stdout)
            } else {
                None
            }
        })
        .and_then(|o| String::from_utf8(o).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if primary.is_some() {
        return primary;
    }

    Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(o.stdout)
            } else {
                None
            }
        })
        .and_then(|o| String::from_utf8(o).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 跨平台获取本地编译时间字符串(第 90 轮 Windows 修复)。
///
/// 优先 `OffsetDateTime::now_local()` + 本地 UTC 偏移;
/// 失败(沙箱/WSL/单线程初始化失败)回退到 UTC + "UTC" 标记;
/// 任何格式化失败回退到 `unix:<secs>`(等价于修复前的 fallback)。
///
/// 输出格式:`YYYY-MM-DD HH:MM:SS ±HH:MM`(例:`2026-09-19 09:51:00 +08:00`)。
/// - TUI 横幅可见
/// - HTTP User-Agent 头可视读(`+` 在 header value 中无需转义, RFC 7230 允许)
/// - crash 报告 / `laew --version` 友好
fn current_local_build_time() -> String {
    const FMT_LOCAL: &[time::format_description::FormatItem<'_>] = format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second] [offset_hour sign:mandatory]:[offset_minute]"
    );
    const FMT_UTC: &[time::format_description::FormatItem<'_>] = format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second] UTC"
    );

    // 1) 本地时间 + 本地偏移(优先)
    if let Ok(local) = OffsetDateTime::now_local() {
        if let Ok(s) = local.format(&FMT_LOCAL) {
            return s;
        }
    }

    // 2) UTC 时间 + "UTC" 标记(本地偏移不可用时)
    if let Ok(s) = OffsetDateTime::now_utc().format(&FMT_UTC) {
        return s;
    }

    // 3) 极端兜底:Unix 时间戳(原行为, 诊断可见)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
