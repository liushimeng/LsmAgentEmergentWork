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

use crossterm::style::Color;

use crate::tui::theme::{self, attr};

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
        Self { text: text.into(), fg, bg: Color::Reset, attrs }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), fg: theme::FG, bg: Color::Reset, attrs: attr::NONE }
    }

    pub fn with_attrs(text: impl Into<String>, fg: Color, attrs: u8) -> Self {
        Self::new(text, fg, attrs)
    }

    /// 带背景色构造(2026-09-10 第二十五轮 F04/B07 测试新增)。
    /// 用于字符级 diff 高亮:让主题表中的 `diff_added_char_bg` / `diff_removed_char_bg` 真正生效。
    pub fn with_bg(text: impl Into<String>, fg: Color, bg: Color, attrs: u8) -> Self {
        Self { text: text.into(), fg, bg, attrs }
    }
}

/// 渲染结果:多行 span 列表。
pub type RenderLines = Vec<Vec<Span>>;
