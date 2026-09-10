//! Diff 渲染 —— 行级 + 字符级 diff 计算与 ANSI 着色。
//!
//! 算法:使用 `similar` crate(claudecode 同款),行级 `TextDiff::from_lines` +
//! 字符级 `TextDiff::from_chars`(对相邻删/增行对做二次分析)。
//!
//! 参考:claudecode `utils/dts.ts` + Rust ColorFile、pi word-level diff inverse。

use crossterm::style::Color;
use similar::{ChangeTag, TextDiff};

use crate::tui::render::{RenderLines, Span};
use crate::tui::theme::{self, attr};

/// Diff 行标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Added,
    Removed,
    Context,
}

/// 单行 diff 条目。
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffTag,
    /// 旧文件行号(原样保留,None 表示新增行)。
    pub line_no_old: Option<usize>,
    /// 新文件行号(原样保留,None 表示删除行)。
    pub line_no_new: Option<usize>,
    /// 行文本(不含换行)。
    pub text: String,
    /// 字符级变更区间 [(start,end),...],仅 Added/Removed 行有效。
    pub char_spans: Vec<(usize, usize)>,
}

/// 完整 diff 结果(单 hunk)。
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_path: String,
    pub new_path: String,
    pub lines: Vec<DiffLine>,
}

/// 行号宽度计算:取旧/新最大行号的位数,至少 2。
fn line_no_width(n: usize) -> usize {
    if n == 0 { 2 } else { ((n as f64).log10() as usize) + 1 }
}

/// 计算两个文本的行级 + 字符级 diff。
///
/// # 参数
/// - `old_text`:旧文件全文
/// - `new_text`:新文件全文
/// - `old_path`:旧文件路径(仅作标题显示)
/// - `new_path`:新文件路径(仅作标题显示)
pub fn compute_diff(old_text: &str, new_text: &str, old_path: &str, new_path: &str) -> DiffHunk {
    let diff = TextDiff::from_lines(old_text, new_text);

    let mut lines: Vec<DiffLine> = Vec::new();
    let mut old_no: usize = 1;
    let mut new_no: usize = 1;

    // 收集 changes 以便做相邻删/增行对的字符级分析
    let changes: Vec<_> = diff.iter_all_changes().collect();

    let mut i = 0;
    while i < changes.len() {
        let change = &changes[i];
        match change.tag() {
            ChangeTag::Equal => {
                lines.push(DiffLine {
                    tag: DiffTag::Context,
                    line_no_old: Some(old_no),
                    line_no_new: Some(new_no),
                    text: change.value().to_string(),
                    char_spans: vec![],
                });
                old_no += 1;
                new_no += 1;
                i += 1;
            }
            ChangeTag::Delete => {
                // 查看下一个 change 是否是 Insert,构成删→增对做字符级 diff
                let mut char_spans = vec![];
                if let Some(next) = changes.get(i + 1) {
                    if next.tag() == ChangeTag::Insert {
                        char_spans = compute_char_spans(change.value(), next.value());
                    }
                }
                lines.push(DiffLine {
                    tag: DiffTag::Removed,
                    line_no_old: Some(old_no),
                    line_no_new: None,
                    text: change.value().to_string(),
                    char_spans,
                });
                old_no += 1;
                i += 1;
            }
            ChangeTag::Insert => {
                // 查看前一个是否已配对,未配对则单独处理
                let mut char_spans = vec![];
                if i > 0 && changes[i - 1].tag() == ChangeTag::Delete {
                    // 已在前一轮配对计算,此处只收集 Insert 侧的对称区间
                    let prev = &changes[i - 1];
                    char_spans = compute_char_spans_rev(prev.value(), change.value());
                }
                lines.push(DiffLine {
                    tag: DiffTag::Added,
                    line_no_old: None,
                    line_no_new: Some(new_no),
                    text: change.value().to_string(),
                    char_spans,
                });
                new_no += 1;
                i += 1;
            }
        }
    }

    DiffHunk {
        old_path: old_path.to_string(),
        new_path: new_path.to_string(),
        lines,
    }
}

/// 字符级变更区间计算:对 removed→added 文本对分析变更区间。
/// 返回 removed 侧的变更区间。
fn compute_char_spans(removed: &str, added: &str) -> Vec<(usize, usize)> {
    let diff = TextDiff::from_chars(removed, added);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut pos: usize = 0;

    for change in diff.iter_all_changes() {
        let len = change.value().chars().count();
        match change.tag() {
            ChangeTag::Equal => {
                pos += len;
            }
            ChangeTag::Delete => {
                spans.push((pos, pos + len));
                pos += len;
            }
            ChangeTag::Insert => {
                // added 侧插入,removed 侧位置不变
            }
        }
    }
    spans
}

/// 反向字符级区间:对 added 行,找其字符级变更区间(与 removed 对齐)。
fn compute_char_spans_rev(removed: &str, added: &str) -> Vec<(usize, usize)> {
    let diff = TextDiff::from_chars(removed, added);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut pos: usize = 0;

    for change in diff.iter_all_changes() {
        let len = change.value().chars().count();
        match change.tag() {
            ChangeTag::Equal => {
                pos += len;
            }
            ChangeTag::Delete => {
                // removed 侧删除,added 侧位置不变
            }
            ChangeTag::Insert => {
                spans.push((pos, pos + len));
                // added 侧插入,added 位置前进
                pos += len;
            }
        }
    }
    spans
}

/// 渲染 diff hunk 为 span 行(供 stdout 直出或 Frame 绘制)。
pub fn render_diff_hunk(hunk: &DiffHunk) -> RenderLines {
    let mut out: RenderLines = Vec::new();

    // 标题行
    out.push(vec![Span::with_attrs(
        format!("--- {}", hunk.old_path),
        theme::DIFF_HEADER_FG,
        theme::DIFF_HEADER_ATTRS,
    )]);
    out.push(vec![Span::with_attrs(
        format!("+++ {}", hunk.new_path),
        theme::DIFF_HEADER_FG,
        theme::DIFF_HEADER_ATTRS,
    )]);

    // 计算行号宽度
    let max_old = hunk.lines.iter().filter_map(|l| l.line_no_last()).max().unwrap_or(1);
    let max_new = hunk.lines.iter().filter_map(|l| l.line_no_new).max().unwrap_or(1);
    let w_old = line_no_width(max_old);
    let w_new = line_no_width(max_new);

    for line in &hunk.lines {
        out.push(render_diff_line(line, w_old, w_new));
    }

    out
}

/// 渲染单行:行号 + 前缀 + 文本(带字符级着色)。
fn render_diff_line(line: &DiffLine, w_old: usize, w_new: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();

    // 左栏行号(旧)
    let old_str = match line.line_no_old {
        Some(n) => format!("{:>w_old$}", n, w_old = w_old),
        None => " ".repeat(w_old),
    };
    spans.push(Span::with_attrs(old_str, theme::DIFF_LINE_NO_FG, theme::DIFF_LINE_NO_ATTRS));

    // 分隔
    spans.push(Span::with_attrs("│", theme::DIFF_LINE_NO_FG, theme::DIFF_LINE_NO_ATTRS));

    // 右栏行号(新)
    let new_str = match line.line_no_new {
        Some(n) => format!("{:>w_new$}", n, w_new = w_new),
        None => " ".repeat(w_new),
    };
    spans.push(Span::with_attrs(new_str, theme::DIFF_LINE_NO_FG, theme::DIFF_LINE_NO_ATTRS));

    // 前缀符号(+/-/` `)
    let (prefix, prefix_fg, prefix_attrs) = match line.tag {
        DiffTag::Added => ("+ ", theme::DIFF_ADDED_FG, theme::DIFF_ADDED_ATTRS),
        DiffTag::Removed => ("- ", theme::DIFF_REMOVED_FG, theme::DIFF_REMOVED_ATTRS),
        DiffTag::Context => ("  ", theme::DIFF_CONTEXT_FG, attr::NONE),
    };
    spans.push(Span::with_attrs(prefix, prefix_fg, prefix_attrs));

    // 文本(字符级着色)
    append_text_with_char_spans(&mut spans, &line.text, line);

    spans
}

/// 追加文本,按字符级区间着色。
fn append_text_with_char_spans(spans: &mut Vec<Span>, text: &str, line: &DiffLine) {
    if line.char_spans.is_empty() {
        // 无字符级变更,整行统一着色
        let (fg, attrs) = match line.tag {
            DiffTag::Added => (theme::DIFF_ADDED_FG, theme::DIFF_ADDED_ATTRS),
            DiffTag::Removed => (theme::DIFF_REMOVED_FG, theme::DIFF_REMOVED_ATTRS),
            DiffTag::Context => (theme::DIFF_CONTEXT_FG, attr::NONE),
        };
        spans.push(Span::with_attrs(text.to_string(), fg, attrs));
        return;
    }

    // 字符级着色:区间外为行颜色,区间内加背景色
    let (base_fg, base_attrs, _hl_bg) = match line.tag {
        DiffTag::Added => (theme::DIFF_ADDED_FG, theme::DIFF_ADDED_ATTRS, theme::DIFF_ADDED_CHAR_BG),
        DiffTag::Removed => (theme::DIFF_REMOVED_FG, theme::DIFF_REMOVED_ATTRS, theme::DIFF_REMOVED_CHAR_BG),
        DiffTag::Context => (theme::DIFF_CONTEXT_FG, attr::NONE, Color::Reset),
    };

    // 将字符索引区间转换为字节偏移
    let byte_spans = char_to_byte_spans(text, &line.char_spans);

    let mut cursor: usize = 0;
    for (s, e) in byte_spans {
        if cursor < s {
            spans.push(Span::with_attrs(text[cursor..s].to_string(), base_fg, base_attrs));
        }
        spans.push(Span::with_attrs(text[s..e].to_string(), Color::White, base_attrs | attr::REVERSE));
        cursor = e;
    }
    if cursor < text.len() {
        spans.push(Span::with_attrs(text[cursor..].to_string(), base_fg, base_attrs));
    }
}

/// 字符索引区间 → 字节偏移区间。
fn char_to_byte_spans(text: &str, char_spans: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut result: Vec<(usize, usize)> = Vec::new();
    let total_chars = text.chars().count();
    for (cs, ce) in char_spans {
        let mut byte_start: Option<usize> = None;
        let mut byte_end: Option<usize> = None;
        let mut ch_idx: usize = 0;
        for (b, _ch) in text.char_indices() {
            if ch_idx == *cs {
                byte_start = Some(b);
            }
            if ch_idx == *ce {
                byte_end = Some(b);
                break;
            }
            ch_idx += 1;
        }
        if let Some(bs) = byte_start {
            // byte_end 为 None 表示 ce 超出字符串末尾或 ce == total_chars
            let be = byte_end.unwrap_or_else(|| {
                if *ce >= total_chars {
                    text.len()
                } else {
                    bs // 兜底
                }
            });
            result.push((bs, be));
        }
    }
    result
}

impl DiffLine {
    /// 辅助:取旧行号或新行号的最大值(用于行号宽度计算)。
    fn line_no_last(&self) -> Option<usize> {
        self.line_no_old.or(self.line_no_new)
    }
}

/// 便捷函数:从两个文件路径读文件并 diff。
pub fn diff_files(old_path: &str, new_path: &str) -> std::io::Result<DiffHunk> {
    let old_text = std::fs::read_to_string(old_path)?;
    let new_text = std::fs::read_to_string(new_path)?;
    Ok(compute_diff(&old_text, &new_text, old_path, new_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_diff_basic_add_remove() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nmodified\nline3\nadded\n";
        let hunk = compute_diff(old, new, "old.txt", "new.txt");

        let tags: Vec<_> = hunk.lines.iter().map(|l| l.tag.clone()).collect();
        assert!(tags.contains(&DiffTag::Context));
        assert!(tags.contains(&DiffTag::Added));
        assert!(tags.contains(&DiffTag::Removed));
    }

    #[test]
    fn compute_diff_char_level() {
        let old = "fn hello() {}\n";
        let new = "fn world() {}\n";
        let hunk = compute_diff(old, new, "old.rs", "new.rs");

        // 应有一删一增行,且字符级区间非空
        let removed = hunk.lines.iter().find(|l| l.tag == DiffTag::Removed).unwrap();
        let added = hunk.lines.iter().find(|l| l.tag == DiffTag::Added).unwrap();
        assert!(!removed.char_spans.is_empty() || !added.char_spans.is_empty());
    }

    #[test]
    fn compute_diff_empty_inputs() {
        let hunk = compute_diff("", "", "a", "b");
        assert!(hunk.lines.is_empty());

        let hunk2 = compute_diff("", "new line\n", "a", "b");
        assert!(hunk2.lines.iter().all(|l| l.tag == DiffTag::Added));
    }

    #[test]
    fn render_diff_hunk_output() {
        let old = "a\nb\nc\n";
        let new = "a\nx\nc\n";
        let hunk = compute_diff(old, new, "old.txt", "new.txt");
        let rendered = render_diff_hunk(&hunk);

        // 标题 2 行 + 数据行
        assert!(rendered.len() >= 4);
        // 标题含 --- / +++
        let title0: String = rendered[0].iter().map(|s| s.text.as_str()).collect();
        assert!(title0.contains("---"));
    }

    #[test]
    fn char_to_byte_spans_basic() {
        let text = "hello世界";
        let spans = char_to_byte_spans(text, &[(0, 2), (5, 7)]);
        assert_eq!(spans, vec![(0, 2), (5, 11)]); // "世"=5..8,"界"=8..11
    }

    #[test]
    fn line_no_width_basic() {
        assert_eq!(line_no_width(1), 1);
        assert_eq!(line_no_width(9), 1);
        assert_eq!(line_no_width(10), 2);
        assert_eq!(line_no_width(100), 3);
        assert_eq!(line_no_width(0), 2);
    }
}
