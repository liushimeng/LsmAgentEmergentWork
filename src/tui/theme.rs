//! CLI 渲染引擎的视觉主题(集中管理 ANSI 颜色 / 属性)。

use crossterm::style::{Attribute, Color};

/// 终端默认前景色(Reset 即不覆盖)。
pub const FG: Color = Color::Reset;

/// 弱化文字(说明 / 提示行 / Tab 未选中态)。
pub const DIM: Color = Color::DarkGrey;

/// 主题强调色(标题 / 顶部横幅边框)。
pub const ACCENT: Color = Color::Cyan;

/// Tab 选中时的反白前景。
pub const TAB_SELECTED_FG: Color = Color::White;

/// 操作按钮选中时的反白。
pub const BUTTON_FOCUSED: Attribute = Attribute::Reverse;

/// 错误提示色。
pub const ERROR: Color = Color::Red;

/// 成功提示色。
pub const SUCCESS: Color = Color::Green;

/// 把 API Key 末 4 位脱敏展示;长度不足时退化为 `****`。
pub fn mask_key(s: &str) -> String {
    let tail: String = if s.chars().count() >= 4 {
        s.chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        String::new()
    };
    if tail.is_empty() {
        "****".to_string()
    } else {
        format!("****{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_long_key() {
        assert_eq!(mask_key("sk-1234567890abcd"), "****abcd");
    }

    #[test]
    fn mask_short_key() {
        assert_eq!(mask_key("abc"), "****");
        assert_eq!(mask_key(""), "****");
    }
}