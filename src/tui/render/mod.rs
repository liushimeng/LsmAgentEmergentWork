//! TUI 渲染增强子模块 —— Diff 渲染 + 语法高亮。
//!
//! 第十八/十九轮 P0 候选:
//! - D5 Diff 渲染行级(L1436-L1445)
//! - D10 语法高亮(L1641-L1700)
//!
//! 纯函数 API 设计:输入文本 → 输出 `Vec<(text, fg, attrs)>` 片段,
//! 便于 `engine.rs::Frame` 绘制或 stdout 直出。

pub mod diff;
pub mod highlight;
pub mod markdown;

use crossterm::style::Color;

use crate::tui::theme::{self, attr, attrs_to_ansi, bg_color_ansi, color_to_ansi256};

/// 渲染片段:一段文本 + 颜色 + 属性。
#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub fg: Color,
    /// 背景色;`Color::Reset` 表示不覆盖终端默认底色(2026-09-10 第二十五轮 F04/B07 测试新增)。
    /// 字符级 diff 高亮使用此字段实现主题色背景块(此前 `_hl_bg` 字段被丢弃,主题色成为死代码)。
    pub bg: Color,
    pub attrs: u8,
}

impl Span {
    pub fn new(text: impl Into<String>, fg: Color, attrs: u8) -> Self {
        Self {
            text: text.into(),
            fg,
            bg: Color::Reset,
            attrs,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: theme::FG,
            bg: Color::Reset,
            attrs: attr::NONE,
        }
    }

    pub fn with_attrs(text: impl Into<String>, fg: Color, attrs: u8) -> Self {
        Self::new(text, fg, attrs)
    }

    /// 带背景色构造(2026-09-10 第二十五轮 F04/B07 测试新增)。
    /// 用于字符级 diff 高亮:让主题表中的 `diff_added_char_bg` / `diff_removed_char_bg` 真正生效。
    pub fn with_bg(text: impl Into<String>, fg: Color, bg: Color, attrs: u8) -> Self {
        Self {
            text: text.into(),
            fg,
            bg,
            attrs,
        }
    }
}

/// 渲染结果:多行 span 列表。
pub type RenderLines = Vec<Vec<Span>>;

/// 把单个 Span 编码为完整 ANSI 序列(fg + bg + attrs + text + reset)。
///
/// TUIMarkdown 富文本渲染(2026-09-15)收敛:`dispatch.rs` 的 diff 打印 /
/// 围栏高亮打印与 Markdown 渲染统一走此助手,消除三处重复的 ANSI 拼接。
pub fn span_to_ansi(span: &Span) -> String {
    // 全默认样式(Reset 前景 + 无底色 + 无属性)→ 直接输出原文:
    // 避免 Markdown 渲染的普通文本行逐 span 包 ANSI(Markdown 富文本渲染,2026-09-15)。
    if span.fg == Color::Reset && span.bg == Color::Reset && span.attrs == attr::NONE {
        return span.text.clone();
    }
    format!(
        "\x1b[38;5;{}m{}{}{}\x1b[0m",
        color_to_ansi256(span.fg),
        bg_color_ansi(span.bg),
        attrs_to_ansi(span.attrs),
        span.text
    )
}

/// 把 RenderLines 逐行直出到 stdout(每行前缀固定缩进;空行只输出缩进)。
pub fn print_render_lines(lines: &RenderLines, indent: &str) {
    for spans in lines {
        print!("{indent}");
        for s in spans {
            print!("{}", span_to_ansi(s));
        }
        println!();
    }
}

/// RenderLines → 含 ANSI 的多行字符串(无尾换行;供需要 String 拼接的
/// 渲染路径使用,如 `format_task_result` 的 Styled 模式)。
pub fn render_lines_to_ansi_str(lines: &RenderLines, indent: &str) -> String {
    let mut out = String::new();
    for (i, spans) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(indent);
        for s in spans {
            out.push_str(&span_to_ansi(s));
        }
    }
    out
}

/// 计算一组 Span 的纯文本显示宽度(供表格列宽对齐)。
pub(crate) fn spans_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|s| crate::tui::input::display_width(&s.text) as usize)
        .sum()
}
