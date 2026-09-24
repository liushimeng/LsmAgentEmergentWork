//! laew 运行日志(`--debug` / `--info` 输出 log 文件,2026-09-17 第 69 轮)。
//!
//! 功能:命令行给出 `--debug` / `--info`(或单横线 / 任意大小写变体)时,
//! 在**工作目录**(启动命令所在目录)生成 `llaew_{YYYYMMDD_HHMMSS}.log`
//! (时间戳 = laew 进程启动时刻,本地时区,精确到秒),记录全部 Agent 的
//! 感知 / 决策 / 执行 / 思考与工具调用事件,便于排查问题。
//!
//! 传输:复用 tracing —— main.rs 组装 registry(原控制台 fmt 层行为不变 +
//! 本模块的文件 fmt 层按 flag 定级:`--debug` → DEBUG 级,`--info` → INFO 级)。
//! 文件层 ANSI 关闭 + 本地时间戳,级别由 flag 决定,不受 `RUST_LOG` 干扰。
//!
//! 容错:文件创建失败时静默降级为不写日志(仅 stderr 提示),绝不影响主流程
//! (对齐 tui_log_writer 的容错哲学,见 `tui/mod.rs` F5)。
//!
//! 设计见 `tmpPlan/2026-09-17_02-输出log文件功能与全链路日志埋点方案.md`。

use std::fs::{File, OpenOptions};
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::Layer;

/// 日志文件名前缀(需求口径:`llaew_YYYYMMDD_HHMMSS.log`)。
pub const LOG_FILE_PREFIX: &str = "llaew_";

/// 单字段默认截断长度(字符数,非字节数;CJK 按 1 字符计)。
const DEFAULT_CLIP_LIMIT: usize = 4000;

/// 生成启动时刻命名的日志文件路径:`{work_dir}/llaew_{YYYYMMDD_HHMMSS}.log`。
///
/// 时间戳取「调用此刻」——main.rs 在进程启动最早期调用,即为 laew 运行启动时间;
/// 复用 `session::now_readable()` 的本地时区方案(取不到本地偏移回退 UTC)。
pub fn startup_log_path(work_dir: &Path) -> PathBuf {
    startup_log_path_at(work_dir, &crate::session::now_readable())
}

/// 按给定启动时刻(`YYYYMMDD-HHMMSS`,session::now_readable 形态)生成日志文件路径。
///
/// 第 72 轮(2026-09-17):main 最早期捕获一次启动时刻,同时用于日志文件命名与
/// TUI 横幅「启动时间」行 —— 保证两者严格同刻,不因各自取 now 在跨秒边界差 1 秒。
pub fn startup_log_path_at(work_dir: &Path, ts: &str) -> PathBuf {
    work_dir.join(format!("{LOG_FILE_PREFIX}{}.log", ts.replace('-', "_")))
}

/// 运行日志文件元信息(第 72 轮):TUI 横幅「日志文件」行展示用。
///
/// main 构造日志层时顺手克隆一份(路径 + 级别)传给 TUI,让用户在会话内
/// 随时(/clear /new 重印横幅)可见日志落盘位置,不再只靠启动时一闪而过的 stderr 提示。
#[derive(Debug, Clone)]
pub struct AgentLogInfo {
    /// 日志文件完整路径(工作目录下 `llaew_YYYYMMDD_HHMMSS.log`)
    pub path: PathBuf,
    /// 日志级别字符串:"DEBUG"(`--debug`) / "INFO"(`--info`)。
    ///
    /// 注意:这是**级别名**,不是显示宽度,也不是同文件里
    /// `LAEW_LOG_CLIP`(`log_clip_limit()`,日志超长字段的**字符**截断长度)那一套。
    /// 本值只用于横幅展示日志级别;TUI 侧的显示列宽计算一律走 `tui::textfit`。
    pub level: &'static str,
}

/// 日志超长字段截断长度:环境变量 `LAEW_LOG_CLIP`(字符数),默认 4000。
///
/// `0` / 非法值回退默认(0 视为「未配置」而非「不截断」,防止误配置写出超大日志)。
pub fn log_clip_limit() -> usize {
    match std::env::var("LAEW_LOG_CLIP") {
        Ok(v) => v.trim().parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(DEFAULT_CLIP_LIMIT),
        Err(_) => DEFAULT_CLIP_LIMIT,
    }
}

/// 按**字符数**截断字符串(CJK 按 1 字符计),超长时尾部追加省略标注。
///
/// 日志字段走 Display 输出,字节截断会把多字节 UTF-8 字符拦腰截断成非法序列,
/// 因此这里必须按 char 边界截。
pub fn clip_for_log(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str(&format!("…(截断,省略{}字符)", total - max_chars));
    out
}

/// 便捷封装:按 [`log_clip_limit`] 截断。
pub fn clip(s: &str) -> String {
    clip_for_log(s, log_clip_limit())
}

/// 共享文件 writer:`Arc<Mutex<LineWriter<File>>>`,tracing 各线程克隆持有。
///
/// 打开一次、进程内复用(对比 tui_log_writer 的每次写入重开——那边是低频 WARN,
/// 本功能 DEBUG 级含 LLM 全文,高频重开开销不可接受)。
#[derive(Clone)]
pub struct AgentLogMaker(Arc<Mutex<LineWriter<File>>>);

/// 单次写入句柄(tracing MakeWriter 每事件借用一次)。
pub struct AgentLogWriter(Arc<Mutex<LineWriter<File>>>);

impl Write for AgentLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut w) => w.write(buf),
            // 锁中毒:唯一持锁线程 panic 时释放,极端场景下静默丢弃本条,不影响主流程
            Err(_) => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut w) => w.flush(),
            Err(_) => Ok(()),
        }
    }
}

impl<'s> tracing_subscriber::fmt::MakeWriter<'s> for AgentLogMaker {
    type Writer = AgentLogWriter;

    fn make_writer(&'s self) -> Self::Writer {
        AgentLogWriter(Arc::clone(&self.0))
    }
}

/// 打开(创建 + 追加)日志文件并构造共享 writer。
///
/// 父目录(工作目录)必然存在;打开失败返回 `None`,调用方降级为不写日志。
pub fn make_log_maker(path: &Path) -> Option<AgentLogMaker> {
    let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    Some(AgentLogMaker(Arc::new(Mutex::new(LineWriter::new(file)))))
}

/// 本地时区时间戳格式化器(`%Y-%m-%d %H:%M:%S%.3f`,本地偏移取不到回退 UTC)。
///
/// tracing-subscriber 自带 `SystemTime` 计时器只输出 UTC 且需要额外 feature;
/// 这里复用 `time` crate(session.rs 同款)实现 `FormatTime`。
struct LocalTimer;

impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let now = match time::OffsetDateTime::now_local() {
            Ok(t) => t,
            // 多线程进程里 time crate 可能拒绝取本地偏移(session.rs 同款回退)
            Err(_) => time::OffsetDateTime::now_utc(),
        };
        write!(
            w,
            "{}",
            now.format(
                &time::format_description::parse_borrowed::<2>(
                    "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
                )
                .expect("format")
            )
            .expect("fmt")
        )
    }
}

/// 构造日志文件 fmt 层:ANSI 关闭 + 无 target + 本地时间戳 + 按级别过滤。
///
/// 级别由调用方传入(`--debug` → DEBUG,`--info` → INFO),确定性输出,
/// 不受 `RUST_LOG` 影响。泛型 `S` 使本层可叠加在任意 registry/Layered 栈上。
pub fn file_fmt_layer<S>(
    maker: AgentLogMaker,
    level: LevelFilter,
) -> impl Layer<S> + Sized
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_timer(LocalTimer)
        .with_writer(maker)
        .with_filter(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_log_path_at_固定时刻与格式() {
        let dir = Path::new("/tmp/some_workdir");
        // YYYYMMDD-HHMMSS(now_readable 形态,'-' 分隔)→ 文件名 '_' 分隔
        let p = startup_log_path_at(dir, "20260917-150412");
        assert_eq!(
            p,
            PathBuf::from("/tmp/some_workdir/llaew_20260917_150412.log"),
            "时刻经 '-' → '_' 归一后拼文件名"
        );
    }

    #[test]
    fn startup_log_path_格式与目录() {
        let dir = Path::new("/tmp/some_workdir");
        let p = startup_log_path(dir);
        assert!(p.starts_with(dir), "必须落在指定工作目录");
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            regex_lite_is_match(&name),
            "文件名应为 llaew_YYYYMMDD_HHMMSS.log,实际: {name}"
        );
    }

    /// 不引入 regex 依赖的轻量校验:^llaew_\d{8}_\d{6}\.log$
    fn regex_lite_is_match(name: &str) -> bool {
        let rest = match name.strip_prefix("llaew_") {
            Some(r) => r,
            None => return false,
        };
        let bytes = rest.as_bytes();
        // YYYYMMDD(8) + '_'(1) + HHMMSS(6) + ".log"(4)
        if bytes.len() != 19 {
            return false;
        }
        for i in 0..8 {
            if !bytes[i].is_ascii_digit() {
                return false;
            }
        }
        if bytes[8] != b'_' {
            return false;
        }
        for i in 9..15 {
            if !bytes[i].is_ascii_digit() {
                return false;
            }
        }
        &rest[15..] == ".log"
    }

    #[test]
    fn clip_for_log_短串原样() {
        assert_eq!(clip_for_log("hello", 10), "hello");
        assert_eq!(clip_for_log("", 10), "");
    }

    #[test]
    fn clip_for_log_长串截断并标注() {
        let s = "a".repeat(100);
        let out = clip_for_log(&s, 10);
        assert!(out.starts_with(&"a".repeat(10)), "前 10 字符保留");
        assert!(out.contains("省略90字符"), "含省略标注: {out}");
    }

    #[test]
    fn clip_for_log_cjk_按字符计() {
        // 6 个 CJK 字符 = 18 字节;截 3 字符应保留完整字符边界
        let s = "你好世界再见";
        let out = clip_for_log(s, 3);
        assert!(out.starts_with("你好世"));
        assert!(out.contains("省略3字符"));
    }

    #[test]
    fn log_clip_limit_环境变量覆盖() {
        // 同进程串行执行,测试后恢复,避免污染其它用例
        std::env::set_var("LAEW_LOG_CLIP", "100");
        assert_eq!(log_clip_limit(), 100);
        std::env::set_var("LAEW_LOG_CLIP", "0");
        assert_eq!(log_clip_limit(), DEFAULT_CLIP_LIMIT, "0 回退默认");
        std::env::set_var("LAEW_LOG_CLIP", "abc");
        assert_eq!(log_clip_limit(), DEFAULT_CLIP_LIMIT, "非法回退默认");
        std::env::remove_var("LAEW_LOG_CLIP");
        assert_eq!(log_clip_limit(), DEFAULT_CLIP_LIMIT);
    }

    #[test]
    fn make_log_maker_创建并续写() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("llaew_test.log");
        let maker = make_log_maker(&path).expect("应能创建日志文件");
        {
            let mut w = tracing_subscriber::fmt::MakeWriter::make_writer(&maker);
            w.write_all(b"first line\n").unwrap();
            w.flush().unwrap();
        }
        {
            let mut w = tracing_subscriber::fmt::MakeWriter::make_writer(&maker);
            w.write_all(b"second line\n").unwrap();
            w.flush().unwrap();
        }
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "first line\nsecond line\n", "追加写不覆盖");
    }

    #[test]
    fn make_log_maker_不存在目录返回none() {
        assert!(make_log_maker(Path::new("/nonexistent_dir_xyz/llaew.log")).is_none());
    }
}
