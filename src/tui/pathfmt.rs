//! 路径显示格式化(D05/D07 测试轮,2026-09-10 第 23 轮)
//!
//! 问题背景:debug 报告路径 / 导出文件路径等绝对路径(根目录深达 5 层,60+ 字符)
//! 直接打印会在窄终端折行,**文件名被从中间截断**,无法复制。
//! 本模块提供「相对化 + 终端宽度感知 + 中间省略」的纯显示层工具:
//!
//! - [`terminal_width`]: crossterm 探测,失败回退 80,下限 40;
//! - [`display_path`]: 路径在根目录/工作目录之下时显示为相对路径(取更短者),
//!   与 TUI 横幅「根目录/工作目录」呼应;
//! - [`elide_middle`]: 超宽时中间省略(`…`,头 40% / 尾 60%,**保文件名**);
//! - [`fit_line`]: 整行(prefix + middle + suffix)不超终端宽原样返回,超宽只对 middle 省略。

use crate::database::paths::Paths;
use crate::tui::input::display_width;
use std::path::Path;

/// 当前终端显示列数;探测失败(非 TTY / e2e 管道)回退 80,下限 40。
pub fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(40)
}

/// 显示用路径:在根目录或工作目录之下时转为相对路径(取更短者),否则原样。
///
/// 注意:仅用于展示(横幅已标注根目录/工作目录),落盘/日志仍用绝对路径。
pub fn display_path(paths: &Paths, p: &Path) -> String {
    let abs = p.display().to_string();
    let rel_to = |base: &Path| -> Option<String> {
        p.strip_prefix(base)
            .ok()
            .map(|r| r.display().to_string())
            .filter(|r| !r.is_empty())
    };
    match (rel_to(&paths.root_dir), rel_to(&paths.work_dir)) {
        (Some(a), Some(b)) => {
            if display_width(&a) <= display_width(&b) {
                a
            } else {
                b
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => abs,
    }
}

/// 中间省略到不超过 `max` 显示列:头 30% / 尾 70%(文件名在尾部,优先保尾),
/// 以 `…`(占 1 列)衔接;未超宽原样返回。
pub fn elide_middle(s: &str, max: usize) -> String {
    if display_width(s) as usize <= max || max == 0 {
        return s.to_string();
    }
    let budget = max.saturating_sub(1).max(1); // 留 1 列给 …
    let tail_w = (budget as f64 * 0.7) as usize;
    let head_w = budget - tail_w;
    format!("{}…{}", head_until(s, head_w), tail_from(s, tail_w))
}

/// 整行收敛:`prefix + middle + suffix` 不超终端宽原样返回;超宽时仅对 middle 中间省略。
pub fn fit_line(prefix: &str, middle: &str, suffix: &str) -> String {
    fit_line_with_width(prefix, middle, suffix, terminal_width())
}

/// [`fit_line`] 的可测试内核(显式宽度)。
fn fit_line_with_width(prefix: &str, middle: &str, suffix: &str, width: usize) -> String {
    let full_len =
        display_width(prefix) as usize + display_width(middle) as usize + display_width(suffix) as usize;
    if full_len <= width {
        return format!("{prefix}{middle}{suffix}");
    }
    let rest = width
        .saturating_sub(display_width(prefix) as usize)
        .saturating_sub(display_width(suffix) as usize);
    format!("{prefix}{}{suffix}", elide_middle(middle, rest))
}

/// 从头取不超过 `max` 显示列的子串(不在双宽字符中间截断)。
fn head_until(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = crate::tui::input::char_width(c) as usize;
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

/// 从尾取不超过 `max` 显示列的子串(不在双宽字符中间截断)。
fn tail_from(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars().rev() {
        let cw = crate::tui::input::char_width(c) as usize;
        if w + cw > max {
            break;
        }
        w += cw;
        out.insert(0, c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &str, work: &str) -> Paths {
        Paths {
            root_dir: std::path::PathBuf::from(root),
            work_dir: std::path::PathBuf::from(work),
            db_path: std::path::PathBuf::from("/tmp/x.db"),
        }
    }

    #[test]
    fn elide_keeps_short_string() {
        assert_eq!(elide_middle("short/path.md", 80), "short/path.md");
    }

    #[test]
    fn elide_truncates_middle_keeps_tail() {
        let long = "/usr/local/LsmAgentOpenSource/LsmAgentEmergentWork/DebugReport/debug_report_20260910_123729_d76fb4.md";
        let out = elide_middle(long, 60);
        assert!(display_width(&out) as usize <= 60, "超宽: {out}");
        assert!(out.contains('…'), "应含省略号: {out}");
        // 尾部(文件名)必须完整保留
        assert!(
            out.ends_with("debug_report_20260910_123729_d76fb4.md"),
            "文件名被截: {out}"
        );
    }

    #[test]
    fn elide_cjk_width_aware() {
        // 6 个双宽字符 = 12 列,限 8 列 → 头 2(4 列)+ … + 尾 2(4 列)= 9 列?不:预算 7,头 2 列 + 尾 4 列
        let out = elide_middle("取消任务流程图", 8);
        assert!(display_width(&out) as usize <= 8, "CJK 超宽: {out}");
        assert!(out.contains('…'));
    }

    #[test]
    fn display_path_prefers_shorter_relative() {
        let p = paths(
            "/usr/local/LsmGitOpenSource/LsmAgentEmergentWork",
            "/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/TestWorkSpace/deep/nested/dir",
        );
        let abs = std::path::Path::new(
            "/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/DebugReport/r1.md",
        );
        assert_eq!(display_path(&p, abs), "DebugReport/r1.md");
    }

    #[test]
    fn display_path_falls_back_to_absolute() {
        let p = paths("/root/laew", "/tmp/work");
        let abs = std::path::Path::new("/etc/hosts");
        assert_eq!(display_path(&p, abs), "/etc/hosts");
    }

    #[test]
    fn fit_line_keeps_one_line_with_suffix() {
        let prefix = "  ✓ 已导出 Markdown: ";
        let middle = "/very/long/path/laew-export-20260910-123903.md";
        let suffix = " (12 轮对话)";
        let out = fit_line_with_width(prefix, middle, suffix, 60);
        assert!(display_width(&out) as usize <= 60, "超宽: {out}");
        assert!(out.starts_with(prefix));
        assert!(out.ends_with(suffix), "后缀应保留在同行: {out}");
    }

    #[test]
    fn fit_line_no_change_when_fits() {
        let out = fit_line_with_width("a: ", "b.md", "!", 80);
        assert_eq!(out, "a: b.md!");
    }
}
