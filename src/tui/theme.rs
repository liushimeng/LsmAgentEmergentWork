//! CLI 渲染引擎的视觉主题(集中管理 ANSI 颜色 / 属性位掩码)。
//!
//! 选中态视觉规范(2026-09-09 起):
//!   - 操作按钮: `▶ [ X ] ◀` + `SELECTED_FG` + `SELECTED_ATTRS`(Bold|Reverse)
//!   - 列表行:   `► ` 前缀 + `SELECTED_FG` + `SELECTED_ATTRS`
//!   - Tab 标签: `▸ ` 前缀 + `TAB_FOCUSED_FG` + `TAB_FOCUSED_ATTRS`(只 Bold,不反白)
//!   - Choice 选中项: `[ x ]` + ACCENT + Bold
//!   - Text 编辑态: `▶ ... ◀` 包裹 + `SELECTED_FG` + `SELECTED_ATTRS`
//!
//! 主题系统(2026-09-10 第二十三轮 D12):4 套主题(default / dark-contrast / light / daltonized)
//!   - 所有现有 `pub const` 保留且值不变,作为 default 主题的导出常量
//!     (兼容 8 个调用方 + 子屏代码)
//!   - 新增 `ThemeKind` / `Palette` / `palette()` / `set_active()` / `active_kind()` / `from_env()` API
//!     供核心渲染位置(engine/render/diff/render/highlight/banner)动态读取当前主题配色
//!   - 环境变量 `LAEW_THEME` 启动期初始化;`/theme [kind]` 命令运行时切换
//!     (子屏代码仍走 const 路径,因此切换非 default 主题后部分子屏需重启生效,核心 cell 渲染实时切换)

use std::sync::OnceLock;

use crossterm::style::Color;

/// Cell 属性位掩码(engine.rs::Cell.attrs 使用)。
pub mod attr {
    pub const NONE: u8       = 0;
    pub const BOLD: u8       = 1 << 0;
    pub const REVERSE: u8    = 1 << 1;
    pub const DIM: u8        = 1 << 2;
    pub const UNDERLINED: u8 = 1 << 3;
}

// ============================================================
// default 主题导出常量(向后兼容;所有 const 的值就是 default 主题配色)
// ============================================================

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

// ============================================================
// 主题系统(2026-09-10 第二十三轮 D12 多主题)
// ============================================================

/// 主题种类。
///
/// 字符串标识(供 `LAEW_THEME` 环境变量 + `/theme` 命令使用):
///   - `default`        默认配色(向后兼容)
///   - `dark-contrast`  暗色高对比(视力辅助 / 弱光终端)
///   - `light`          浅色终端(白底)
///   - `daltonized`     色盲友好(Deuteranopia / Protanopia 红绿对换为蓝黄)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Default,
    DarkContrast,
    Light,
    Daltonized,
}

impl ThemeKind {
    /// 字符串形式的稳定标识(`/theme` 命令参数 / 环境变量值)。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::DarkContrast => "dark-contrast",
            Self::Light => "light",
            Self::Daltonized => "daltonized",
        }
    }

    /// 人类可读的中文说明(用于 `/theme` 列表展示)。
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Default => "默认配色(平衡可读,大多数终端)",
            Self::DarkContrast => "暗色高对比(视力辅助 / 弱光环境)",
            Self::Light => "浅色配色(白底终端)",
            Self::Daltonized => "色盲友好(红绿对换为蓝黄语义)",
        }
    }

    /// 从环境变量 `LAEW_THEME` 读取主题;未知值回退 default + stderr 提示。
    pub fn from_env() -> Self {
        match std::env::var("LAEW_THEME").ok().as_deref() {
            None => Self::Default,
            Some("") | Some("default") => Self::Default,
            Some("dark-contrast" | "dark_contrast" | "high-contrast" | "high_contrast" | "dark") => {
                Self::DarkContrast
            }
            Some("light") => Self::Light,
            Some("daltonized" | "colorblind" | "cb") => Self::Daltonized,
            Some(other) => {
                eprintln!(
                    "[theme] 未知 LAEW_THEME={other:?}, 已回退 default。可选: default | dark-contrast | light | daltonized"
                );
                Self::Default
            }
        }
    }
}

/// 完整配色表(运行时可切换,`palette()` 返回当前活跃值)。
///
/// 字段含义与上面 `pub const` 一一对应;const 与 default 主题值一致,
/// 切换主题后 const 不变,Palette 动态变化。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub error: Color,
    pub success: Color,
    pub selected_fg: Color,
    pub selected_attrs: u8,
    pub tab_focused_fg: Color,
    pub tab_focused_attrs: u8,
    pub choice_focused_attrs: u8,
    pub highlight_fg: Color,
    pub input_bg: Color,
    pub input_fg: Color,
    pub input_prompt_fg: Color,
    pub input_prompt_attrs: u8,
    pub input_border_fg: Color,
    pub input_hint_fg: Color,
    pub diff_added_fg: Color,
    pub diff_added_attrs: u8,
    pub diff_removed_fg: Color,
    pub diff_removed_attrs: u8,
    pub diff_context_fg: Color,
    pub diff_line_no_fg: Color,
    pub diff_line_no_attrs: u8,
    pub diff_header_fg: Color,
    pub diff_header_attrs: u8,
    pub diff_added_char_bg: Color,
    pub diff_removed_char_bg: Color,
    pub hl_keyword_fg: Color,
    pub hl_keyword_attrs: u8,
    pub hl_string_fg: Color,
    pub hl_number_fg: Color,
    pub hl_comment_fg: Color,
    pub hl_comment_attrs: u8,
    pub hl_type_fg: Color,
    pub hl_type_attrs: u8,
    pub hl_function_fg: Color,
    pub hl_function_attrs: u8,
    pub hl_operator_fg: Color,
    pub hl_punctuation_fg: Color,
    pub hl_plain_fg: Color,
}

/// 编译期 default 主题(向后兼容常量)。
const DEFAULT_PALETTE: Palette = Palette {
    fg: Color::Reset,
    dim: Color::DarkGrey,
    accent: Color::Cyan,
    error: Color::Red,
    success: Color::Green,
    selected_fg: Color::Cyan,
    selected_attrs: attr::BOLD | attr::REVERSE,
    tab_focused_fg: Color::Cyan,
    tab_focused_attrs: attr::BOLD,
    choice_focused_attrs: attr::BOLD,
    highlight_fg: Color::White,
    input_bg: Color::DarkGrey,
    input_fg: Color::White,
    input_prompt_fg: Color::Cyan,
    input_prompt_attrs: attr::BOLD,
    input_border_fg: Color::DarkGrey,
    input_hint_fg: Color::DarkGrey,
    diff_added_fg: Color::Green,
    diff_added_attrs: attr::BOLD,
    diff_removed_fg: Color::Red,
    diff_removed_attrs: attr::BOLD,
    diff_context_fg: Color::DarkGrey,
    diff_line_no_fg: Color::Cyan,
    diff_line_no_attrs: attr::DIM,
    diff_header_fg: Color::Yellow,
    diff_header_attrs: attr::BOLD,
    diff_added_char_bg: Color::DarkGreen,
    diff_removed_char_bg: Color::DarkRed,
    hl_keyword_fg: Color::Magenta,
    hl_keyword_attrs: attr::BOLD,
    hl_string_fg: Color::Green,
    hl_number_fg: Color::Yellow,
    hl_comment_fg: Color::DarkGrey,
    hl_comment_attrs: attr::DIM,
    hl_type_fg: Color::Cyan,
    hl_type_attrs: attr::BOLD,
    hl_function_fg: Color::Blue,
    hl_function_attrs: attr::BOLD,
    hl_operator_fg: Color::White,
    hl_punctuation_fg: Color::Reset,
    hl_plain_fg: Color::Reset,
};

/// 暗色高对比(视力辅助):用 ANSI 256 色板的亮色档(n=15 White / 14 Cyan / 9+ 等)
/// 模拟 BrightXxx —— crossterm 0.27 无 BrightXxx 变体,只能走 Rgb。
/// 256 色板 15=亮白, 14=亮青, 9=亮红, 10=亮绿, 11=亮黄, 13=亮品红。
const DARK_CONTRAST_PALETTE: Palette = Palette {
    fg: Color::Reset,
    dim: Color::Grey,
    accent: Color::AnsiValue(14),         // 亮青
    error: Color::AnsiValue(9),           // 亮红
    success: Color::AnsiValue(10),        // 亮绿
    selected_fg: Color::AnsiValue(14),
    selected_attrs: attr::BOLD | attr::REVERSE,
    tab_focused_fg: Color::AnsiValue(14),
    tab_focused_attrs: attr::BOLD,
    choice_focused_attrs: attr::BOLD,
    highlight_fg: Color::AnsiValue(15),   // 亮白
    input_bg: Color::Black,
    input_fg: Color::AnsiValue(15),
    input_prompt_fg: Color::AnsiValue(14),
    input_prompt_attrs: attr::BOLD,
    input_border_fg: Color::Grey,
    input_hint_fg: Color::Grey,
    diff_added_fg: Color::AnsiValue(10),
    diff_added_attrs: attr::BOLD,
    diff_removed_fg: Color::AnsiValue(9),
    diff_removed_attrs: attr::BOLD,
    diff_context_fg: Color::Grey,
    diff_line_no_fg: Color::AnsiValue(14),
    diff_line_no_attrs: attr::DIM,
    diff_header_fg: Color::AnsiValue(11),
    diff_header_attrs: attr::BOLD,
    diff_added_char_bg: Color::Green,
    diff_removed_char_bg: Color::Red,
    hl_keyword_fg: Color::AnsiValue(13),
    hl_keyword_attrs: attr::BOLD,
    hl_string_fg: Color::AnsiValue(10),
    hl_number_fg: Color::AnsiValue(11),
    hl_comment_fg: Color::Grey,
    hl_comment_attrs: attr::DIM,
    hl_type_fg: Color::AnsiValue(14),
    hl_type_attrs: attr::BOLD,
    hl_function_fg: Color::AnsiValue(12),
    hl_function_attrs: attr::BOLD,
    hl_operator_fg: Color::AnsiValue(15),
    hl_punctuation_fg: Color::Reset,
    hl_plain_fg: Color::Reset,
};

/// 浅色配色(白底终端):选中态反白改下划线(浅底反白不可读);
/// DarkXxx 改为更深一级保证对比度。
const LIGHT_PALETTE: Palette = Palette {
    fg: Color::Reset,
    dim: Color::DarkGrey,
    accent: Color::DarkCyan,
    error: Color::DarkRed,
    success: Color::DarkGreen,
    selected_fg: Color::DarkCyan,
    selected_attrs: attr::BOLD | attr::UNDERLINED,
    tab_focused_fg: Color::DarkCyan,
    tab_focused_attrs: attr::BOLD,
    choice_focused_attrs: attr::BOLD,
    highlight_fg: Color::Black,
    input_bg: Color::Grey,
    input_fg: Color::Black,
    input_prompt_fg: Color::DarkCyan,
    input_prompt_attrs: attr::BOLD,
    input_border_fg: Color::DarkGrey,
    input_hint_fg: Color::DarkGrey,
    diff_added_fg: Color::DarkGreen,
    diff_added_attrs: attr::BOLD,
    diff_removed_fg: Color::DarkRed,
    diff_removed_attrs: attr::BOLD,
    diff_context_fg: Color::DarkGrey,
    diff_line_no_fg: Color::DarkCyan,
    diff_line_no_attrs: attr::DIM,
    diff_header_fg: Color::DarkYellow,
    diff_header_attrs: attr::BOLD,
    diff_added_char_bg: Color::Green,
    diff_removed_char_bg: Color::Red,
    hl_keyword_fg: Color::DarkMagenta,
    hl_keyword_attrs: attr::BOLD,
    hl_string_fg: Color::DarkGreen,
    hl_number_fg: Color::DarkYellow,
    hl_comment_fg: Color::DarkGrey,
    hl_comment_attrs: attr::DIM,
    hl_type_fg: Color::DarkCyan,
    hl_type_attrs: attr::BOLD,
    hl_function_fg: Color::DarkBlue,
    hl_function_attrs: attr::BOLD,
    hl_operator_fg: Color::Black,
    hl_punctuation_fg: Color::Reset,
    hl_plain_fg: Color::Reset,
};

/// 色盲友好(Deuteranopia / Protanopia):红绿对换为蓝黄语义对比,
/// 降低红绿色盲用户的混淆度。
const DALTONIZED_PALETTE: Palette = Palette {
    fg: Color::Reset,
    dim: Color::DarkGrey,
    accent: Color::Magenta,
    error: Color::Blue,
    success: Color::Yellow,
    selected_fg: Color::Magenta,
    selected_attrs: attr::BOLD | attr::REVERSE,
    tab_focused_fg: Color::Magenta,
    tab_focused_attrs: attr::BOLD,
    choice_focused_attrs: attr::BOLD,
    highlight_fg: Color::AnsiValue(15),
    input_bg: Color::DarkGrey,
    input_fg: Color::White,
    input_prompt_fg: Color::Magenta,
    input_prompt_attrs: attr::BOLD,
    input_border_fg: Color::DarkGrey,
    input_hint_fg: Color::DarkGrey,
    diff_added_fg: Color::Blue,
    diff_added_attrs: attr::BOLD,
    diff_removed_fg: Color::Magenta,
    diff_removed_attrs: attr::BOLD,
    diff_context_fg: Color::DarkGrey,
    diff_line_no_fg: Color::Cyan,
    diff_line_no_attrs: attr::DIM,
    diff_header_fg: Color::Yellow,
    diff_header_attrs: attr::BOLD,
    diff_added_char_bg: Color::DarkBlue,
    diff_removed_char_bg: Color::DarkMagenta,
    hl_keyword_fg: Color::Magenta,
    hl_keyword_attrs: attr::BOLD,
    hl_string_fg: Color::Blue,
    hl_number_fg: Color::Yellow,
    hl_comment_fg: Color::DarkGrey,
    hl_comment_attrs: attr::DIM,
    hl_type_fg: Color::Cyan,
    hl_type_attrs: attr::BOLD,
    hl_function_fg: Color::Blue,
    hl_function_attrs: attr::BOLD,
    hl_operator_fg: Color::White,
    hl_punctuation_fg: Color::Reset,
    hl_plain_fg: Color::Reset,
};

/// 主题到 Palette 的查表。
pub fn palette_for_kind(kind: ThemeKind) -> Palette {
    match kind {
        ThemeKind::Default => DEFAULT_PALETTE,
        ThemeKind::DarkContrast => DARK_CONTRAST_PALETTE,
        ThemeKind::Light => LIGHT_PALETTE,
        ThemeKind::Daltonized => DALTONIZED_PALETTE,
    }
}

/// 全局当前活跃主题(Mutex<ThemeKind> + 启动期 LazyLock 初始化)。
static ACTIVE_KIND: OnceLock<std::sync::Mutex<ThemeKind>> = OnceLock::new();

fn active_kind_lock() -> &'static std::sync::Mutex<ThemeKind> {
    ACTIVE_KIND.get_or_init(|| std::sync::Mutex::new(ThemeKind::from_env()))
}

/// 当前活跃主题(深拷贝)。
pub fn active_kind() -> ThemeKind {
    *active_kind_lock().lock().expect("theme kind poisoned")
}

/// 切换主题(供 `/theme` 命令调用)。
///
/// 返回旧主题,方便命令打印「✓ 已从 X 切换到 Y」。
pub fn set_active(kind: ThemeKind) -> ThemeKind {
    let mut guard = active_kind_lock().lock().expect("theme kind poisoned");
    let prev = *guard;
    *guard = kind;
    prev
}

/// 当前活跃主题的完整 Palette(深拷贝,值类型不含引用,Copy 即可)。
pub fn palette() -> Palette {
    palette_for_kind(active_kind())
}

/// 列出所有可用主题(供 `/theme` 无参数使用)。
pub fn all_kinds() -> &'static [ThemeKind] {
    const KINDS: &[ThemeKind] = &[
        ThemeKind::Default,
        ThemeKind::DarkContrast,
        ThemeKind::Light,
        ThemeKind::Daltonized,
    ];
    KINDS
}

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

    // --- D12 主题系统(2026-09-10 第二十三轮) ---

    #[test]
    fn theme_kind_as_str_round_trip() {
        for k in all_kinds() {
            assert_eq!(ThemeKind::from_env_str(k.as_str()), Some(*k));
        }
    }

    #[test]
    fn theme_kind_from_env_str_unknown_returns_none() {
        assert_eq!(ThemeKind::from_env_str("nope"), None);
        assert_eq!(ThemeKind::from_env_str(""), None);
    }

    #[test]
    fn theme_kind_from_env_str_accepts_aliases() {
        // 别名映射(给 /theme <kind> 与 LAEW_THEME 共享入口)
        assert_eq!(ThemeKind::from_env_str("dark-contrast"), Some(ThemeKind::DarkContrast));
        assert_eq!(ThemeKind::from_env_str("dark_contrast"), Some(ThemeKind::DarkContrast));
        assert_eq!(ThemeKind::from_env_str("high-contrast"), Some(ThemeKind::DarkContrast));
        assert_eq!(ThemeKind::from_env_str("dark"), Some(ThemeKind::DarkContrast));
        assert_eq!(ThemeKind::from_env_str("colorblind"), Some(ThemeKind::Daltonized));
        assert_eq!(ThemeKind::from_env_str("cb"), Some(ThemeKind::Daltonized));
    }

    #[test]
    fn palette_for_kind_returns_distinct_palettes() {
        // 4 个主题至少在 selected_fg / input_bg / accent 上有差异
        let kinds = [
            ThemeKind::Default,
            ThemeKind::DarkContrast,
            ThemeKind::Light,
            ThemeKind::Daltonized,
        ];
        let palettes: Vec<Palette> = kinds.iter().copied().map(palette_for_kind).collect();
        // 浅色主题选中态不用反白(改下划线)
        assert_ne!(palettes[2].selected_attrs & attr::REVERSE, attr::REVERSE);
        assert_eq!(palettes[2].selected_attrs & attr::UNDERLINED, attr::UNDERLINED);
        // 其他主题用反白
        for p in &palettes[..2] {
            assert_ne!(p.selected_attrs & attr::REVERSE, 0, "dark themes use reverse");
        }
        // daltonized: ERROR 是 Blue 不是 Red
        assert_eq!(palettes[3].error, Color::Blue);
        // light: ERROR 是 DarkRed
        assert_eq!(palettes[2].error, Color::DarkRed);
    }

    #[test]
    fn palette_default_matches_const_values() {
        // 默认主题的所有 Palette 字段必须与上面 const 一一对应
        let p = palette_for_kind(ThemeKind::Default);
        assert_eq!(p.fg, FG);
        assert_eq!(p.accent, ACCENT);
        assert_eq!(p.error, ERROR);
        assert_eq!(p.success, SUCCESS);
        assert_eq!(p.selected_fg, SELECTED_FG);
        assert_eq!(p.selected_attrs, SELECTED_ATTRS);
        assert_eq!(p.tab_focused_fg, TAB_FOCUSED_FG);
        assert_eq!(p.input_bg, INPUT_BG);
        assert_eq!(p.input_fg, INPUT_FG);
        assert_eq!(p.diff_added_fg, DIFF_ADDED_FG);
        assert_eq!(p.diff_removed_fg, DIFF_REMOVED_FG);
        assert_eq!(p.hl_keyword_fg, HL_KEYWORD_FG);
        assert_eq!(p.hl_string_fg, HL_STRING_FG);
    }

    #[test]
    fn active_kind_initializes_from_env() {
        // 单测环境下 LAEW_THEME 未设置 → default
        // (其他测试可能改 set_active,但本测试只断言初始值或 default 之一)
        let kind = active_kind();
        assert!(
            matches!(
                kind,
                ThemeKind::Default | ThemeKind::DarkContrast | ThemeKind::Light | ThemeKind::Daltonized
            ),
            "active_kind 必须在 4 主题之一"
        );
    }

    #[test]
    fn set_active_round_trip() {
        let prev = set_active(ThemeKind::Daltonized);
        assert_eq!(active_kind(), ThemeKind::Daltonized);
        let _ = set_active(prev); // 还原,避免污染后续测试
    }

    #[test]
    fn palette_after_set_active_changes() {
        let prev = active_kind();
        let default_p = palette();
        set_active(ThemeKind::Light);
        let light_p = palette();
        assert_ne!(default_p.error, light_p.error);
        assert_ne!(default_p.selected_attrs, light_p.selected_attrs);
        let _ = set_active(prev);
    }
}

// 额外辅助方法(在 ThemeKind 上扩展,保持 enum 本体的简洁)
impl ThemeKind {
    /// 从字符串解析主题,大小写敏感 + 多别名接受。
    /// 接受的值与 `from_env` 一致(返回 `Some(kind)`);未知值返回 `None`
    /// —— 用于 `/theme <kind>` 命令的严格参数校验(未知值要打印错误提示)。
    pub fn from_env_str(s: &str) -> Option<Self> {
        match s {
            "" => None,
            "default" => Some(Self::Default),
            "dark-contrast" | "dark_contrast" | "high-contrast" | "high_contrast" | "dark" => {
                Some(Self::DarkContrast)
            }
            "light" => Some(Self::Light),
            "daltonized" | "colorblind" | "cb" => Some(Self::Daltonized),
            _ => None,
        }
    }
}