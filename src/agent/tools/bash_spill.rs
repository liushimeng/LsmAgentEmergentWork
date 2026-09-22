//! Bash 输出落盘模块(D17 大对象溢出与外部产物存储)。
//!
//! 第二十轮 L2051+ D17:对齐 claudecode `BashTool/utils.ts:101` `full output spilled to disk`、
//! deepseek `spill/`(spill-local/store/cleanup + spill-policy, 2529 行)。
//!
//! 行为:当 stdout/stderr 字符数 > 阈值(默认 30K)时,自动写入
//! `<root_dir>/BashSpill/bash_{YYYYMMDD}_{HHMMSS}_{rand6}.{stream}.log`,
//! 工具返回带 spill 路径的精简摘要(头部 5K + 路径 + 尾部 5K),
//! LLM 可用 Read 工具回读完整内容。
//!
//! 设计要点:
//! - 阈值:`LAEW_BASH_SPILL_THRESHOLD` 环境变量可覆盖(默认 30000 字符)
//! - 头部/尾部:`HEAD_BYTES = 5000`, `TAIL_BYTES = 5000` 字符
//! - 失败降级:目录创建 / 文件写入失败时,自动回退到原 truncate 行为 + `[spill failed: <reason>]` 警告
//! - 路径隔离:`BashSpill/` 与 `CrashReport/` / `AuditTrail/` 同级,均 gitignore
//! - 不引入新 crate:`std::fs` + `time` crate(已有依赖)
//! - 静态单例:同一进程内路径生成共享原子计数器防并发冲突

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 默认阈值(字符数),与 bash.rs::MAX_OUTPUT_CHARS 对齐。
pub const DEFAULT_THRESHOLD: usize = 30_000;

/// 落盘时头部保留字符数。
const HEAD_BYTES: usize = 5_000;

/// 落盘时尾部保留字符数。
const TAIL_BYTES: usize = 5_000;

/// 落盘子目录名(根目录下)。
pub const SPILL_SUBDIR: &str = "BashSpill";

/// 落盘结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpillOutcome {
    /// 未触发落盘(总字符数 <= 阈值)。`text` 为原文,`omitted` 永远为 0。
    NotSpilled {
        text: String,
        omitted: usize,
    },
    /// 已落盘成功。`rel_path` 是面向 LLM 的相对路径(`BashSpill/bash_*.log`),
    /// LLM 可用 Read 工具打开;`head` / `tail` 为截取片段;`total_chars` 是原始总字符数。
    Spilled {
        rel_path: String,
        full_path: PathBuf,
        head: String,
        tail: String,
        total_chars: usize,
        omitted: usize,
    },
    /// 落盘失败,回退到 truncate 行为。`reason` 写入返回文本供调试。
    FailedFallback {
        text: String,
        omitted: usize,
        reason: String,
    },
}

/// Bash 输出流类型,用于命名 spill 文件(`stdout.log` / `stderr.log`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashStream {
    Stdout,
    Stderr,
}

impl BashStream {
    pub fn as_str(&self) -> &'static str {
        match self {
            BashStream::Stdout => "stdout",
            BashStream::Stderr => "stderr",
        }
    }
}

/// 读取当前生效的 spill 阈值(`LAEW_BASH_SPILL_THRESHOLD` 环境变量覆盖,
/// 解析失败时回退默认 30000)。
pub fn threshold_chars() -> usize {
    match std::env::var("LAEW_BASH_SPILL_THRESHOLD") {
        Ok(v) => v.trim().parse::<usize>().unwrap_or(DEFAULT_THRESHOLD),
        Err(_) => DEFAULT_THRESHOLD,
    }
}

/// 把 Bash 输出按 spill 策略落盘或保留原文。
///
/// 1. 总字符数 <= 阈值:返回 `NotSpilled`,无变化。
/// 2. 总字符数 > 阈值:尝试写入 `<root_dir>/<SPILL_SUBDIR>/bash_*.*.log`。
///    成功 → `Spilled`;失败 → `FailedFallback`(降级到阈值长度截断 + 警告)。
pub fn maybe_spill(stream: BashStream, full: &str, root_dir: &Path) -> SpillOutcome {
    let threshold = threshold_chars();
    let total = full.chars().count();

    if total <= threshold {
        return SpillOutcome::NotSpilled {
            text: full.to_string(),
            omitted: 0,
        };
    }

    let spill_dir = root_dir.join(SPILL_SUBDIR);
    match write_spill_file(&spill_dir, stream, full) {
        Ok(full_path) => {
            let rel_path = format!(
                "{}/{}",
                SPILL_SUBDIR,
                full_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("spill.log"),
            );
            let head = head_chars(full, HEAD_BYTES);
            let tail = tail_chars(full, TAIL_BYTES);
            let omitted = total.saturating_sub(head.chars().count() + tail.chars().count());
            SpillOutcome::Spilled {
                rel_path,
                full_path,
                head,
                tail,
                total_chars: total,
                omitted,
            }
        }
        Err(reason) => {
            let cut = char_boundary(full, threshold);
            SpillOutcome::FailedFallback {
                text: full[..cut].to_string(),
                omitted: total - cut,
                reason,
            }
        }
    }
}

/// 把 spill 结果格式化为 LLM 可见的 `<stream>` 块文本(用于嵌入到 BashTool 返回)。
///
/// - `NotSpilled`:返回原 `text`,无任何标记
/// - `Spilled`:返回头部 + 路径提示 + 尾部
/// - `FailedFallback`:返回截断文本 + `[spill failed: ...]` 警告
pub fn format_stream_block(stream: BashStream, outcome: &SpillOutcome) -> String {
    let tag = stream.as_str();
    match outcome {
        SpillOutcome::NotSpilled { text, omitted: _ } => {
            if text.is_empty() {
                String::new()
            } else {
                format!("<{}>\n{}\n</{}>", tag, text, tag)
            }
        }
        SpillOutcome::Spilled {
            rel_path,
            head,
            tail,
            total_chars,
            omitted,
            ..
        } => {
            let mut s = String::new();
            s.push_str(&format!("<{}>\n", tag));
            s.push_str(head);
            if !head.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&format!(
                "\n...[{} 截断,省略 {} 字符;完整输出({} 字符)已写入: {}]\n",
                tag, omitted, total_chars, rel_path,
            ));
            s.push_str(tail);
            if !tail.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&format!("</{}>", tag));
            s
        }
        SpillOutcome::FailedFallback { text, omitted, reason } => {
            let mut s = String::new();
            s.push_str(&format!("<{}>\n", tag));
            s.push_str(text);
            if !text.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&format!(
                "\n...[{} 截断,省略 {} 字符;spill failed: {}]\n",
                tag, omitted, reason,
            ));
            s.push_str(&format!("</{}>", tag));
            s
        }
    }
}

/// 当前进程的 spill 根目录(与 `Paths::detect().root_dir` 同义)。
///
/// 独立推导,避免向 BashTool 注入 root_dir 字段(参考 decision_audit.rs::audit_root_dir 模式)。
/// 推导失败(无 current_exe 父目录)时回退到工作目录。
pub fn spill_root_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

// ---------- 私有辅助 ----------

/// 把内容写到 `<dir>/bash_{ts}_{rand6}.{stream}.log`,返回完整路径。
fn write_spill_file(
    dir: &Path,
    stream: BashStream,
    content: &str,
) -> std::result::Result<PathBuf, String> {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Err(format!("create_dir_all({}): {}", dir.display(), e));
    }
    let filename = format!("bash_{}_{}.{}.log", timestamp_compact(), random_hex6(), stream.as_str());
    let path = dir.join(&filename);
    if let Err(e) = std::fs::write(&path, content) {
        return Err(format!("write({}): {}", path.display(), e));
    }
    Ok(path)
}

/// 当前本地可读时间戳 `YYYYMMDD_HHMMSS`(对齐 session.rs::now_readable 风格)。
fn timestamp_compact() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let base = time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let dt = match time::UtcOffset::current_local_offset() {
        Ok(offset) => base.to_offset(offset),
        Err(_) => base,
    };
    dt.format(
        &time::format_description::parse_borrowed::<2>(
            "[year][month][day]_[hour][minute][second]",
        )
        .expect("BashSpill format"),
    )
    .unwrap_or_else(|_| "00000000_000000".to_string())
}

/// 6 位 hex 随机串(pid + nanos + 原子计数器混合),防并发同毫秒覆盖。
fn random_hex6() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id() as u64;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Splitmix64 风格混合,避免低质量 LSB 聚集
    let mut z = pid.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos ^ counter;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    format!("{:06x}", (z as u32) & 0x00FF_FFFF)
}

/// 字符索引到字节索引的转换(CJK 安全)。
fn char_boundary(s: &str, char_count: usize) -> usize {
    s.char_indices()
        .nth(char_count)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// 取前 N 字符(CJK 安全)。
fn head_chars(s: &str, n: usize) -> String {
    let cut = char_boundary(s, n);
    s[..cut].to_string()
}

/// 取后 N 字符(CJK 安全)。
fn tail_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        return s.to_string();
    }
    let start_char = total - n;
    let mut byte_offset = s.len();
    for (idx, (i, _)) in s.char_indices().enumerate() {
        if idx == start_char {
            byte_offset = i;
            break;
        }
    }
    s[byte_offset..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化环境变量读写测试:cargo test 默认并行,涉及 env 的测试需互斥。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 创建一个临时目录用于 spill 测试(测试结束后删除)。
    fn temp_spill_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "laew-bash-spill-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn spill_threshold_under_keeps_inline() {
        let s: String = "x".repeat(1_000);
        let dir = temp_spill_dir();
        let outcome = maybe_spill(BashStream::Stdout, &s, &dir);
        assert!(matches!(outcome, SpillOutcome::NotSpilled { omitted: 0, .. }));
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert!(entries.is_empty(), "未触发 spill 时不应写文件");
    }

    #[test]
    fn spill_threshold_over_writes_file() {
        let s: String = "A".repeat(60_000);
        let dir = temp_spill_dir();
        let outcome = maybe_spill(BashStream::Stdout, &s, &dir);
        match outcome {
            SpillOutcome::Spilled {
                rel_path,
                total_chars,
                head,
                tail,
                ..
            } => {
                assert!(rel_path.starts_with("BashSpill/"));
                assert!(rel_path.ends_with(".stdout.log"));
                assert_eq!(total_chars, 60_000);
                assert_eq!(head.chars().count(), HEAD_BYTES);
                assert_eq!(tail.chars().count(), HEAD_BYTES);
                let fname = rel_path.trim_start_matches(&format!("{}/", SPILL_SUBDIR));
                let full_path = dir.join(SPILL_SUBDIR).join(fname);
                let on_disk = std::fs::read_to_string(&full_path).expect("read spill");
                assert_eq!(on_disk.chars().count(), 60_000);
            }
            other => panic!("expected Spilled, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spill_path_format_matches_pattern() {
        let s: String = "B".repeat(35_000);
        let dir = temp_spill_dir();
        let outcome = maybe_spill(BashStream::Stderr, &s, &dir);
        match outcome {
            SpillOutcome::Spilled { rel_path, .. } => {
                assert!(rel_path.starts_with("BashSpill/bash_"));
                assert!(rel_path.ends_with(".stderr.log"));
                let body = rel_path.trim_start_matches("BashSpill/bash_").trim_end_matches(".stderr.log");
                assert!(body.contains('_'), "body 应含下划线: {body}");
            }
            other => panic!("expected Spilled, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spill_head_tail_format_correct() {
        let s: String = "C".repeat(35_000);
        let dir = temp_spill_dir();
        let outcome = maybe_spill(BashStream::Stdout, &s, &dir);
        let formatted = format_stream_block(BashStream::Stdout, &outcome);
        assert!(formatted.contains("<stdout>\n"), "should have open tag");
        assert!(formatted.contains("</stdout>"), "should have close tag");
        assert!(formatted.contains("完整输出"), "should mention full output");
        assert!(formatted.contains("BashSpill/bash_"), "should contain spill path");
        assert!(formatted.contains("省略"), "should show omitted count");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spill_failure_falls_back_to_truncate() {
        // NUL 字节路径,create_dir_all 必然失败 → 走 FailedFallback 分支
        let bogus = PathBuf::from(" no-such-dir");
        let s: String = "D".repeat(35_000);
        let outcome = maybe_spill(BashStream::Stdout, &s, &bogus);
        match outcome {
            SpillOutcome::FailedFallback { text, omitted, reason } => {
                assert!(text.chars().count() <= DEFAULT_THRESHOLD);
                assert!(omitted > 0);
                assert!(!reason.is_empty());
            }
            other => panic!("expected FailedFallback, got {other:?}"),
        }
    }

    #[test]
    fn spill_threshold_env_override() {
        // 串行锁:与其它 env 读写测试互斥(cargo test 默认并行)。
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("LAEW_BASH_SPILL_THRESHOLD").ok();
        std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", "1000");
        let s: String = "E".repeat(2_000);
        let dir = temp_spill_dir();
        let outcome = maybe_spill(BashStream::Stdout, &s, &dir);
        match prev {
            Some(v) => std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", v),
            None => std::env::remove_var("LAEW_BASH_SPILL_THRESHOLD"),
        }
        assert!(matches!(outcome, SpillOutcome::Spilled { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spill_threshold_env_invalid_falls_back() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("LAEW_BASH_SPILL_THRESHOLD").ok();
        std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", "not-a-number");
        let actual = threshold_chars();
        match prev {
            Some(v) => std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", v),
            None => std::env::remove_var("LAEW_BASH_SPILL_THRESHOLD"),
        }
        assert_eq!(actual, DEFAULT_THRESHOLD);
    }

    #[test]
    fn spill_cjk_boundary_safe() {
        // 4 万中文字符(12 万字节) > 默认 30K 阈值 → 触发 spill;
        // 验证按字符切片而非字节(防止在 UTF-8 中间切坏)。
        let s: String = "中".repeat(40_000);
        let dir = temp_spill_dir();
        let outcome = maybe_spill(BashStream::Stdout, &s, &dir);
        match outcome {
            SpillOutcome::Spilled { head, tail, total_chars, .. } => {
                assert_eq!(total_chars, 40_000);
                assert_eq!(head.chars().count(), HEAD_BYTES);
                assert_eq!(tail.chars().count(), HEAD_BYTES);
                assert!(head.chars().all(|c| c == '中'));
                assert!(tail.chars().all(|c| c == '中'));
            }
            other => panic!("expected Spilled, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_stream_not_spilled_passthrough() {
        let outcome = SpillOutcome::NotSpilled {
            text: "hello".into(),
            omitted: 0,
        };
        let s = format_stream_block(BashStream::Stdout, &outcome);
        assert!(s.contains("<stdout>\nhello\n</stdout>"));
    }

    #[test]
    fn format_stream_empty_not_spilled_returns_empty() {
        let outcome = SpillOutcome::NotSpilled {
            text: "".into(),
            omitted: 0,
        };
        let s = format_stream_block(BashStream::Stdout, &outcome);
        assert_eq!(s, "");
    }

    #[test]
    fn random_hex6_format() {
        for _ in 0..100 {
            let h = random_hex6();
            assert_eq!(h.len(), 6);
            assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
    }

    #[test]
    fn random_hex6_uniqueness_under_contention() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        for _ in 0..1000 {
            set.insert(random_hex6());
        }
        assert!(set.len() > 990, "1000 次生成的 hex6 唯一数 {len} 过低", len = set.len());
    }

    #[test]
    fn bash_stream_as_str() {
        assert_eq!(BashStream::Stdout.as_str(), "stdout");
        assert_eq!(BashStream::Stderr.as_str(), "stderr");
    }

    #[test]
    fn spill_root_dir_returns_existing_path() {
        let p = spill_root_dir();
        assert!(!p.as_os_str().is_empty());
    }
}
