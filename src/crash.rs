//! 全局崩溃取证（Panic Hook + CrashDump）。
//!
//! laew 是单进程 CLI，不做 daemon 自动重启；但当进程内任意线程 panic 时，
//! 这里会把最小可诊断现场落盘到根目录 `CrashReport/`，避免 TUI 崩溃后只剩
//! 一闪而过的终端输出。对应第十七轮 atomcode 调研 gap L1166/L1168/L1169。
//!
//! 设计原则：
//! - hook 自身 fail-open：报告写失败只提示，不掩盖原始 panic；
//! - 强制捕获 backtrace，不依赖用户设置 `RUST_BACKTRACE`；
//! - 命令行与 panic message 先脱敏再截断，防止 API Key / 超长提示词进入报告。

use std::backtrace::Backtrace;
use std::io;
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};

use sha2::{Digest, Sha256};

const MAX_ARGS: usize = 64;
const MAX_ARG_CHARS: usize = 512;
const MAX_MESSAGE_CHARS: usize = 8192;
const MAX_BACKTRACE_CHARS: usize = 64 * 1024;
const VERSION_INFO: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("LAEW_BUILD_TIME"),
    ", git ",
    env!("LAEW_GIT_HASH"),
    ")"
);

static INSTALL_ONCE: Once = Once::new();
static REPORT_DIR: OnceLock<PathBuf> = OnceLock::new();
static PREVIOUS_HOOK: OnceLock<Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>> =
    OnceLock::new();

/// 崩溃现场快照。独立成结构便于单元测试报告渲染与脱敏逻辑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashRecord {
    pub timestamp: String,
    pub pid: u32,
    pub thread: String,
    pub message: String,
    pub location: String,
    pub backtrace: String,
    pub executable: String,
    pub root_dir: String,
    pub work_dir: String,
    pub args: Vec<String>,
    pub rust_backtrace: Option<String>,
    pub os: &'static str,
    pub arch: &'static str,
    pub family: &'static str,
    pub version: &'static str,
}

/// 按显式报告目录安装全局 panic hook。
pub fn install_panic_hook(report_dir: impl Into<PathBuf>) {
    INSTALL_ONCE.call_once(|| {
        let report_dir = report_dir.into();
        let previous = panic::take_hook();
        let _ = REPORT_DIR.set(report_dir);
        let _ = PREVIOUS_HOOK.set(previous);

        panic::set_hook(Box::new(|info| {
            if let Some(dir) = REPORT_DIR.get() {
                let record = capture_crash_record(info);
                match write_crash_report(dir, &record) {
                    Ok(path) => {
                        eprintln!("[laew] Crash report 已写入: {}", path.display());
                    }
                    Err(err) => {
                        eprintln!("[laew] Crash report 写入失败: {err}");
                    }
                }
            }

            if let Some(previous) = PREVIOUS_HOOK.get() {
                previous(info);
            }
        }));
    });
}

/// 恢复 Unix 默认 SIGPIPE 行为。
///
/// Rust 运行时启动时会忽略 SIGPIPE，导致 `laew provider list | head` 这类
/// 下游提前关闭管道的常规 CLI 用法被 `println!` 转成 panic。CLI 工具应沿用
/// Unix 惯例：对端关闭时进程收到 SIGPIPE 并退出，而不是生成 CrashDump。
pub fn restore_sigpipe_default() {
    #[cfg(unix)]
    unsafe {
        // `signal` 返回 SIG_ERR 表示安装失败；此时保持 Rust 默认行为即可，
        // 不能在初始化阶段因信号设置失败中断 CLI。
        let _ = libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// 按 laew 根目录安装全局 panic hook。
pub fn install_panic_hook_from_root(root_dir: &Path) {
    install_panic_hook(root_dir.join("CrashReport"));
}

/// 从 `PanicHookInfo` 提取并脱敏崩溃现场。
pub fn capture_crash_record(info: &PanicHookInfo<'_>) -> CrashRecord {
    let payload = info.payload();
    let raw_message = if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload (Box<dyn Any>)".to_string()
    };

    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "unknown".to_string());

    let executable = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let root_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.display().to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    let work_dir = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    CrashRecord {
        timestamp: crate::session::now_readable(),
        pid: std::process::id(),
        thread: std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string(),
        message: truncate_chars(
            &crate::agent::debug::scrub_secrets(&raw_message),
            MAX_MESSAGE_CHARS,
        ),
        location,
        backtrace: truncate_chars(&Backtrace::force_capture().to_string(), MAX_BACKTRACE_CHARS),
        executable,
        root_dir,
        work_dir,
        args: normalize_args(std::env::args_os()),
        rust_backtrace: std::env::var("RUST_BACKTRACE").ok(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        family: std::env::consts::FAMILY,
        version: VERSION_INFO,
    }
}

fn normalize_args<I>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    args.into_iter()
        .take(MAX_ARGS)
        .map(|arg| {
            let rendered = arg.to_string_lossy().to_string();
            truncate_chars(
                &crate::agent::debug::scrub_secrets(&rendered),
                MAX_ARG_CHARS,
            )
        })
        .collect()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str("...[truncated]");
    }
    out
}

/// 生成 `crash_report_{YYYYMMDD}_{HHMMSS}_{rand6}.md`。
fn crash_report_file_name(record: &CrashRecord) -> String {
    let compact = record.timestamp.replace('-', "_");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(record.pid.to_le_bytes());
    hasher.update(record.thread.as_bytes());
    let bytes = hasher.finalize();
    format!(
        "crash_report_{}_{:02x}{:02x}{:02x}.md",
        compact, bytes[0], bytes[1], bytes[2]
    )
}

/// 渲染 Markdown 报告。保持字段纯文本化，便于用户直接提交 issue。
pub fn render_crash_report(record: &CrashRecord) -> String {
    let args_json = serde_json::to_string_pretty(&record.args).unwrap_or_else(|_| "[]".into());
    format!(
        r#"# laew CrashDump

> 本报告由全局 panic hook 自动生成。命令行与 panic 信息已经脱敏及截断；backtrace 为强制捕获，不依赖 `RUST_BACKTRACE`。

## 1. 进程概览

- 时间: `{timestamp}`
- PID: `{pid}`
- 线程: `{thread}`
- 版本: `{version}`
- 平台: `{os}/{arch} ({family})`

## 2. Panic

- 消息: `{message}`
- 位置: `{location}`
- `RUST_BACKTRACE`: `{rust_backtrace}`

## 3. 运行路径

- 可执行文件: `{executable}`
- 根目录: `{root_dir}`
- 工作目录: `{work_dir}`

## 4. 命令行参数（脱敏/截断）

```json
{args_json}
```

## 5. Backtrace

```text
{backtrace}
```
"#,
        timestamp = record.timestamp,
        pid = record.pid,
        thread = record.thread,
        version = record.version,
        os = record.os,
        arch = record.arch,
        family = record.family,
        message = record.message,
        location = record.location,
        rust_backtrace = record.rust_backtrace.as_deref().unwrap_or("<unset>"),
        executable = record.executable,
        root_dir = record.root_dir,
        work_dir = record.work_dir,
        args_json = args_json,
        backtrace = record.backtrace,
    )
}

/// 将崩溃现场写入 Markdown 文件。
pub fn write_crash_report(dir: &Path, record: &CrashRecord) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(crash_report_file_name(record));
    std::fs::write(&path, render_crash_report(record))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_record() -> CrashRecord {
        CrashRecord {
            timestamp: "20260909-151515".to_string(),
            pid: 1234,
            thread: "main".to_string(),
            message: "index out of bounds".to_string(),
            location: "src/example.rs:12:8".to_string(),
            backtrace: "0: laew::example".to_string(),
            executable: "/usr/local/laew/laew".to_string(),
            root_dir: "/usr/local/laew".to_string(),
            work_dir: "/tmp/work".to_string(),
            args: vec!["laew".to_string(), "-p".to_string()],
            rust_backtrace: Some("1".to_string()),
            os: "linux",
            arch: "x86_64",
            family: "unix",
            version: VERSION_INFO,
        }
    }

    #[test]
    fn render_report_contains_required_fields() {
        let rendered = render_crash_report(&fixture_record());
        assert!(rendered.contains("# laew CrashDump"));
        assert!(rendered.contains("index out of bounds"));
        assert!(rendered.contains("src/example.rs:12:8"));
        assert!(rendered.contains("0: laew::example"));
        assert!(rendered.contains("\"-p\""));
    }

    #[test]
    fn write_report_creates_markdown_in_target_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_crash_report(dir.path(), &fixture_record()).expect("write crash report");
        let content = std::fs::read_to_string(&path).expect("read crash report");
        assert!(path.starts_with(dir.path()));
        assert!(path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("crash_report_20260909_151515_")));
        assert!(content.contains("PID: `1234`"));
        assert!(content.contains("线程: `main`"));
    }

    #[test]
    fn normalize_args_redacts_secrets_limits_count_and_length() {
        let mut args: Vec<std::ffi::OsString> = vec![
            "laew".into(),
            "Authorization: Bearer abcdefghijklmno".into(),
        ];
        let long = "x".repeat(MAX_ARG_CHARS + 20);
        args.push(long.into());
        for i in 0..MAX_ARGS {
            args.push(format!("arg-{i}").into());
        }

        let normalized = normalize_args(args);
        assert_eq!(normalized.len(), MAX_ARGS);
        assert_eq!(normalized[0], "laew");
        assert!(normalized[1].contains("****REDACTED"));
        assert!(normalized[1].contains("abcdefghijklmno") == false);
        assert!(normalized[2].ends_with("...[truncated]"));
        assert!(normalized[2].chars().count() <= MAX_ARG_CHARS + "...[truncated]".len());
    }

    #[test]
    fn long_text_is_truncated_once() {
        let text = "中".repeat(MAX_MESSAGE_CHARS + 3);
        let truncated = truncate_chars(&text, MAX_MESSAGE_CHARS);
        assert!(truncated.ends_with("...[truncated]"));
        assert!(truncated.chars().count() <= MAX_MESSAGE_CHARS + "...[truncated]".len());
    }

    #[test]
    fn report_write_failure_is_reported_as_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("occupied");
        let mut file = std::fs::File::create(&file_path).expect("create file");
        writeln!(file, "not a directory").expect("write file");
        drop(file);

        let result = write_crash_report(&file_path, &fixture_record());
        assert!(result.is_err());
    }

    #[test]
    fn installed_hook_writes_report_and_keeps_unwind_semantics() {
        let dir = tempfile::tempdir().expect("tempdir");
        install_panic_hook(dir.path());

        let caught = std::panic::catch_unwind(|| {
            panic!("smoke Bearer abcdefghijklmno");
        });

        assert!(caught.is_err());
        let reports = std::fs::read_dir(dir.path())
            .expect("read report dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "md")
            })
            .collect::<Vec<_>>();
        assert!(!reports.is_empty(), "panic hook 应生成 CrashDump");

        let content = std::fs::read_to_string(reports[0].path()).expect("read CrashDump");
        assert!(content.contains("smoke ****REDACTED"));
        assert!(!content.contains("abcdefghijklmno"));
        assert!(content.contains("src/crash.rs"));
    }
}
