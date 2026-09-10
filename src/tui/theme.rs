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

/// 补全菜单选中项前景色(主屏输入组件的斜杠命令补全)。
pub const HIGHLIGHT_FG: Color = Color::White;

// ============================================================
// 固定底部输入组件(2026-09-09 起,方案见 tmpPlan/2026-09-09_03)
// ============================================================

/// 输入行整行背景色(与输出区默认底色一眼可辨)。
pub const INPUT_BG: Color = Color::DarkGrey;

/// 输入行文字前景色。
pub const INPUT_FG: Color = Color::White;

/// 输入行提示符(`>> `)前景色。
pub const INPUT_PROMPT_FG: Color = Color::Cyan;

/// 输入行提示符属性。
pub const INPUT_PROMPT_ATTRS: u8 = attr::BOLD;

/// 输入区顶部分隔线颜色。
pub const INPUT_BORDER_FG: Color = Color::DarkGrey;

/// 输入区快捷键提示行颜色。
pub const INPUT_HINT_FG: Color = Color::DarkGrey;

/// 输入区固定高度(行):分隔线 + 输入行 + 提示行。
pub const INPUT_AREA_HEIGHT: u16 = 3;

// ============================================================
// Diff 渲染颜色(D5/L1436-L1445 P0,第十八/十九轮)
// ============================================================

/// 新增行前景色。
pub const DIFF_ADDED_FG: Color = Color::Green;
/// 新增行属性。
pub const DIFF_ADDED_ATTRS: u8 = attr::BOLD;
/// 删除行前景色。
pub const DIFF_REMOVED_FG: Color = Color::Red;
/// 删除行属性。
pub const DIFF_REMOVED_ATTRS: u8 = attr::BOLD;
/// 上下文行前景色。
pub const DIFF_CONTEXT_FG: Color = Color::DarkGrey;
/// 行号前景色。
pub const DIFF_LINE_NO_FG: Color = Color::Cyan;
/// 行号属性。
pub const DIFF_LINE_NO_ATTRS: u8 = attr::DIM;
/// diff 标题(/diff 命令输出路径行)前景色。
pub const DIFF_HEADER_FG: Color = Color::Yellow;
/// diff 标题属性。
pub const DIFF_HEADER_ATTRS: u8 = attr::BOLD;
/// 新增行内字符级变更背景色。
pub const DIFF_ADDED_CHAR_BG: Color = Color::DarkGreen;
/// 删除行内字符级变更背景色。
pub const DIFF_REMOVED_CHAR_BG: Color = Color::DarkRed;

// ============================================================
// 语法高亮 token 颜色(D10/L1641-L1700 P0,第十九轮)
// ============================================================

/// 关键字(if/for/fn/let/def/class/...)前景色。
pub const HL_KEYWORD_FG: Color = Color::Magenta;
/// 关键字属性。
pub const HL_KEYWORD_ATTRS: u8 = attr::BOLD;
/// 字符串前景色。
pub const HL_STRING_FG: Color = Color::Green;
/// 数字前景色。
pub const HL_NUMBER_FG: Color = Color::Yellow;
/// 注释前景色。
pub const HL_COMMENT_FG: Color = Color::DarkGrey;
/// 注释属性。
pub const HL_COMMENT_ATTRS: u8 = attr::DIM;
/// 类型(struct/enum/大写标识符)前景色。
pub const HL_TYPE_FG: Color = Color::Cyan;
/// 类型属性。
pub const HL_TYPE_ATTRS: u8 = attr::BOLD;
/// 函数调用/定义前景色。
pub const HL_FUNCTION_FG: Color = Color::Blue;
/// 函数属性。
pub const HL_FUNCTION_ATTRS: u8 = attr::BOLD;
/// 操作符前景色。
pub const HL_OPERATOR_FG: Color = Color::White;
/// 标点符号前景色。
pub const HL_PUNCTUATION_FG: Color = Color::Reset;
/// 默认文本前景色。
pub const HL_PLAIN_FG: Color = Color::Reset;

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
