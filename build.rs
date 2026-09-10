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

fn main() {
    // 编译时间(本地时区), 失败时退化为 Unix 时间戳
    let build_time = Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S %Z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("unix:{secs}")
        });
    println!("cargo:rustc-env=LAEW_BUILD_TIME={build_time}");

    // Git 短哈希(尽力而为):HEAD^{commit} 优先,失败回退 HEAD
    let git_hash = resolve_git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LAEW_GIT_HASH={git_hash}");

    // 源文件变化时重新运行
    println!("cargo:rerun-if-changed=build.rs");
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
        .and_then(|o| if o.status.success() { Some(o.stdout) } else { None })
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
        .and_then(|o| if o.status.success() { Some(o.stdout) } else { None })
        .and_then(|o| String::from_utf8(o).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
