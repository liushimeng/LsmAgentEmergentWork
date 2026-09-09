//! CLI 渲染引擎的视觉主题(集中管理 ANSI 颜色 / 属性位掩码)。
//!
//! 选中态视觉规范(2026-09-09 起):
//!   - 操作按钮: `▶ [ X ] ◀` + `SELECTED_FG` + `SELECTED_ATTRS`(Bold|Reverse)
//!   - 列表行:   `► ` 前缀 + `SELECTED_FG` + `SELECTED_ATTRS`
//!   - Tab 标签: `▸ ` 前缀 + `TAB_FOCUSED_FG` + `TAB_FOCUSED_ATTRS`(只 Bold,不反白)
//!   - Choice 选中项: `[ x ]` + ACCENT + Bold
//!   - Text 编辑态: `▶ ... ◀` 包裹 + `SELECTED_FG` + `SELECTED_ATTRS`

use crossterm::style::Color;

/// Cell 属性位掩码(engine.rs::Cell.attrs 使用)。
pub mod attr {
    pub const NONE: u8       = 0;
    pub const BOLD: u8       = 1 << 0;
    pub const REVERSE: u8    = 1 << 1;
    pub const DIM: u8        = 1 << 2;
    pub const UNDERLINED: u8 = 1 << 3;
}

/// 终端默认前景色(Reset 即不覆盖)。
pub const FG: Color = Color::Reset;

/// 弱化文字(说明 / 提示行 / 未选中态)。
pub const DIM: Color = Color::DarkGrey;

/// 主题强调色(标题 / 顶部横幅边框)。
pub const ACCENT: Color = Color::Cyan;

/// 错误提示色。
pub const ERROR: Color = Color::Red;

/// 成功提示色。
pub const SUCCESS: Color = Color::Green;

// ============================================================
// 选中态视觉规范(所有子屏统一遵循)
// ============================================================

/// 选中态前景色(按钮 / 列表行 / Text 编辑态)。
pub const SELECTED_FG: Color = Color::Cyan;

/// 选中态属性组合:加粗 + 反白,确保一眼可辨。
pub const SELECTED_ATTRS: u8 = attr::BOLD | attr::REVERSE;

/// Tab 标签选中态前景色(文字多,只用 Bold,避免大面积反白)。
pub const TAB_FOCUSED_FG: Color = Color::Cyan;

/// Tab 标签选中态属性。
pub const TAB_FOCUSED_ATTRS: u8 = attr::BOLD;

/// Choice 选中项属性。
pub const CHOICE_FOCUSED_ATTRS: u8 = attr::BOLD;

/// 选中行 / 列表项的前缀符号。
pub const SELECTED_PREFIX: &str = "► ";

/// Tab 标签选中态前缀。
pub const TAB_FOCUSED_PREFIX: &str = "▸ ";

/// 操作按钮选中态左括号。
pub const SELECTED_BUTTON_L: &str = "▶ ";

/// 操作按钮选中态右括号。
pub const SELECTED_BUTTON_R: &str = " ◀";

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
