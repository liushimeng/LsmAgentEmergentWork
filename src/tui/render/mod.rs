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
    pub attrs: u8,
}

impl Span {
    pub fn new(text: impl Into<String>, fg: Color, attrs: u8) -> Self {
        Self { text: text.into(), fg, attrs }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), fg: theme::FG, attrs: attr::NONE }
    }

    pub fn with_attrs(text: impl Into<String>, fg: Color, attrs: u8) -> Self {
        Self::new(text, fg, attrs)
    }
}

/// 渲染结果:多行 span 列表。
pub type RenderLines = Vec<Vec<Span>>;
