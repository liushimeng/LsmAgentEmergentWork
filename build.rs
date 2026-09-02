//! 构建脚本: 注入编译时间 / Git 提交哈希, 供 `laew --version` 使用。

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

    // Git 短哈希(尽力而为)
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LAEW_GIT_HASH={git_hash}");

    // 源文件变化时重新运行
    println!("cargo:rerun-if-changed=build.rs");
}
