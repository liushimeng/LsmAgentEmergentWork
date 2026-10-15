//! Windows 弹窗 macOS 风格主题(唯一真源,2026-10-14)。
//!
//! 设计见 `docs/MCP_Web_Use/27-Windows弹窗macOS风格界面美化.md`。职责边界:
//! - 本模块**只管观感**:配色板(浅/深双档,随系统外观)、owner-draw 控件自绘图元
//!   (圆角主/次按钮、macOS 开关)、WM_CTLCOLOR* 文本配色、DWM 窗口 chrome
//!   (标题栏同色/沉浸式深色/圆角)、**本进程全部顶层窗口自遍历补挂**;
//! - 窗口代码(`windows_dialog.rs` 及日后的新弹窗)只做**接入**:窗口类背景刷取自
//!   本模块、按钮样式追加 `BS_OWNERDRAW`、`WM_DRAWITEM`/`WM_CTLCOLOR*` 分发到本模块,
//!   **禁止在窗口代码里硬编码颜色** —— 配色只存在于此一处,改主题 = 改这一个文件。
//!
//! 杀开关 `LAEW_WIN_THEME=off`(回退系统原生观感);`light`/`dark` 强制档位
//! (跳过注册表检测);缺省 `auto` 读
//! `HKCU\...\Themes\Personalize\AppsUseLightTheme` 随系统浅/深外观。
//! 本模块只在 `laew __hitl-dialog` 子进程内使用(UI 线程独占)。

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, Ellipse, GetStockObject, RoundRect,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, BACKGROUND_MODE, DT_CENTER, DT_END_ELLIPSIS,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, DRAW_TEXT_FORMAT, HBRUSH, HDC, HGDIOBJ, NULL_BRUSH,
    NULL_PEN, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};

// ---------- 常量(数值取自 SDK 头文件/macOS 系统色板,勿改) ----------

/// 按钮圆角半径(px);控件高度不足时按高/2 钳制(见 [`corner_radius_for`])。
const BTN_CORNER_RADIUS: i32 = 6;
/// macOS NSSwitch 名义轨道尺寸 38×22(见 [`switch_geometry`])。
const SWITCH_TRACK_W_NOMINAL: i32 = 38;
const SWITCH_TRACK_H_NOMINAL: i32 = 22;
/// 开关滑块与轨道的内边距(滑块直径 = 轨道高 − 2×本值)。
const SWITCH_KNOB_INSET: i32 = 2;
/// 开关轨道与选项文案的间距。
const SWITCH_TEXT_GAP: i32 = 10;

// ---------- 配色板 ----------

/// RGB → GDI `COLORREF`(内存序 `0x00BBGGRR`)。
fn rgb_from(red: u32, green: u32, blue: u32) -> COLORREF {
    COLORREF(red | (green << 8) | (blue << 16))
}

/// Windows 弹窗观感主题(配色 + 背景刷;进程生命周期内不可变)。
///
/// 色值取自 macOS 系统色板(Apple HIG / Sonoma):
/// 浅色 = 窗口灰 #F2F2F7 / 卡片白 / systemBlue #007AFF;
/// 深色 = 窗口 #1E1E20 / 卡片 #2C2C2E / systemBlue(dark) #0A84FF。
pub(super) struct WinTheme {
    /// 主题是否启用(杀开关 `LAEW_WIN_THEME=off` 时为 false,窗口代码回退系统观感)。
    pub(super) enabled: bool,
    /// 是否深色配色板。
    pub(super) dark: bool,
    /// 窗口底色(同时用作标题栏同色,实现 macOS 统一标题栏观感)。
    pub(super) bg: COLORREF,
    /// 卡片底色(说明区/输入框/触控板)。
    pub(super) card: COLORREF,
    /// 主文本色。
    pub(super) text: COLORREF,
    /// 次要文本色(时间轴/倒计时/触控板提示)。
    pub(super) text_secondary: COLORREF,
    /// 强调色(主按钮/开关开启态/焦点环;= macOS systemBlue)。
    pub(super) accent: COLORREF,
    pub(super) accent_hover: COLORREF,
    pub(super) accent_press: COLORREF,
    /// 强调色之上的文本色(主按钮白字)。
    pub(super) on_accent: COLORREF,
    /// 次按钮底/描边/悬停/按下。
    pub(super) btn_bg: COLORREF,
    pub(super) btn_border: COLORREF,
    pub(super) btn_hover: COLORREF,
    pub(super) btn_press: COLORREF,
    /// 开关未选中轨道/描边/滑块。
    pub(super) switch_track_off: COLORREF,
    pub(super) switch_border: COLORREF,
    pub(super) knob: COLORREF,
    /// 窗口底刷(注册窗口类 `hbrBackground` 用;创建后常驻进程,不 DeleteObject)。
    pub(super) bg_brush: HBRUSH,
    /// 卡片刷(WM_CTLCOLOR* 返回用)。
    pub(super) card_brush: HBRUSH,
}

impl WinTheme {
    /// 按浅/深档构建配色板(GDI 刷子随板创建)。
    fn palette(dark: bool) -> WinTheme {
        let (bg, card, text, text_secondary, accent, accent_hover, accent_press, on_accent) =
            if dark {
                (
                    rgb_from(0x1E, 0x1E, 0x20),
                    rgb_from(0x2C, 0x2C, 0x2E),
                    rgb_from(0xF5, 0xF5, 0xF7),
                    rgb_from(0x98, 0x98, 0x9D),
                    rgb_from(0x0A, 0x84, 0xFF),
                    rgb_from(0x24, 0x91, 0xFF),
                    rgb_from(0x08, 0x68, 0xC8),
                    rgb_from(0xF5, 0xF5, 0xF7),
                )
            } else {
                (
                    rgb_from(0xF2, 0xF2, 0xF7),
                    rgb_from(0xFF, 0xFF, 0xFF),
                    rgb_from(0x1D, 0x1D, 0x1F),
                    rgb_from(0x6E, 0x6E, 0x73),
                    rgb_from(0x00, 0x7A, 0xFF),
                    rgb_from(0x2D, 0x8C, 0xFF),
                    rgb_from(0x00, 0x62, 0xD6),
                    rgb_from(0xFF, 0xFF, 0xFF),
                )
            };
        let (btn_bg, btn_border, btn_hover, btn_press) = if dark {
            (
                rgb_from(0x3A, 0x3A, 0x3C),
                rgb_from(0x48, 0x48, 0x4A),
                rgb_from(0x45, 0x45, 0x47),
                rgb_from(0x51, 0x51, 0x54),
            )
        } else {
            (
                rgb_from(0xFF, 0xFF, 0xFF),
                rgb_from(0xD2, 0xD2, 0xD7),
                rgb_from(0xF5, 0xF5, 0xF7),
                rgb_from(0xE8, 0xE8, 0xED),
            )
        };
        let (switch_track_off, switch_border, knob) = if dark {
            (
                rgb_from(0x39, 0x39, 0x3D),
                rgb_from(0x48, 0x48, 0x4A),
                rgb_from(0xF5, 0xF5, 0xF7),
            )
        } else {
            (
                rgb_from(0xE9, 0xE9, 0xEA),
                rgb_from(0xD2, 0xD2, 0xD7),
                rgb_from(0xFF, 0xFF, 0xFF),
            )
        };
        // SAFETY:GDI 刷子创建只依赖进程 GDI 句柄表;常驻进程生命周期,不释放。
        let bg_brush = unsafe { CreateSolidBrush(bg) };
        let card_brush = unsafe { CreateSolidBrush(card) };
        WinTheme {
            enabled: true,
            dark,
            bg,
            card,
            text,
            text_secondary,
            accent,
            accent_hover,
            accent_press,
            on_accent,
            btn_bg,
            btn_border,
            btn_hover,
            btn_press,
            switch_track_off,
            switch_border,
            knob,
            bg_brush,
            card_brush,
        }
    }

    /// 杀开关档:观感回退系统原生(色板保留浅色值仅为防御性兜底,正常路径不使用)。
    fn palette_disabled() -> WinTheme {
        let mut t = Self::palette(false);
        t.enabled = false;
        t
    }
}

// ---------- 主题单例与开关 ----------

/// `LAEW_WIN_THEME` 解析结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThemeSetting {
    /// 缺省:随系统外观(注册表)。
    Auto,
    /// 强制浅色。
    Light,
    /// 强制深色。
    Dark,
    /// 关闭主题(回退系统原生观感)。
    Off,
}

/// 纯函数:环境变量原始值 → 主题档位(容错,未知值回退 Auto)。
fn theme_setting_from_env(raw: &str) -> ThemeSetting {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" | "no" => ThemeSetting::Off,
        "light" | "浅色" => ThemeSetting::Light,
        "dark" | "深色" => ThemeSetting::Dark,
        _ => ThemeSetting::Auto,
    }
}

/// 纯函数:`AppsUseLightTheme` 注册表原始 DWORD → 是否深色(0=深色;异常值回退浅色)。
fn prefers_dark_from_light_flag(light_flag: u32) -> bool {
    light_flag == 0
}

/// 系统外观是否深色(注册表 `AppsUseLightTheme`;读取失败回退浅色)。
fn system_prefers_dark_mode() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let subkey = wide_nul(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide_nul("AppsUseLightTheme");
    let mut light_flag: u32 = 1;
    let mut byte_size: u32 = std::mem::size_of::<u32>() as u32;
    // SAFETY:出参指针均指向本栈变量,同步返回。
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut light_flag as *mut u32 as *mut core::ffi::c_void),
            Some(&mut byte_size),
        )
    };
    code.is_ok() && prefers_dark_from_light_flag(light_flag)
}

// HBRUSH(裸指针)默认 !Send/!Sync,static OnceLock 要求两者。本模块只在
// `__hitl-dialog` 子进程的 UI 线程创建/触碰 THEME(窗口过程同线程分发),
// 与 windows_dialog.rs 的 `unsafe impl Send for WinState` 同构。
unsafe impl Send for WinTheme {}
unsafe impl Sync for WinTheme {}

static THEME: OnceLock<WinTheme> = OnceLock::new();

/// 主题单例(弹窗子进程首次访问时初始化;子进程生命周期 = 弹窗生命周期,不热切)。
pub(super) fn theme() -> &'static WinTheme {
    THEME.get_or_init(|| {
        let raw = std::env::var("LAEW_WIN_THEME").unwrap_or_default();
        match theme_setting_from_env(&raw) {
            ThemeSetting::Off => WinTheme::palette_disabled(),
            ThemeSetting::Light => WinTheme::palette(false),
            ThemeSetting::Dark => WinTheme::palette(true),
            ThemeSetting::Auto => WinTheme::palette(system_prefers_dark_mode()),
        }
    })
}

// ---------- 几何纯函数 ----------

/// 控件圆角半径:名义 6px,矮控件按高/2 钳制(保证不超半高)。
fn corner_radius_for(height: i32) -> i32 {
    (height / 2).min(BTN_CORNER_RADIUS).max(1)
}

/// macOS 开关(NSSwitch)几何:名义轨道 38×22;行高不足时按行高收缩(下限 12)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SwitchGeometry {
    pub(super) track_w: i32,
    pub(super) track_h: i32,
    pub(super) knob_d: i32,
    pub(super) knob_margin: i32,
}

fn switch_geometry(row_h: i32) -> SwitchGeometry {
    let track_h = row_h.min(SWITCH_TRACK_H_NOMINAL).max(12);
    let track_w = track_h * SWITCH_TRACK_W_NOMINAL / SWITCH_TRACK_H_NOMINAL;
    SwitchGeometry {
        track_w,
        track_h,
        knob_d: track_h - SWITCH_KNOB_INSET * 2,
        knob_margin: SWITCH_KNOB_INSET,
    }
}

fn inset_rect(rc: &RECT, by: i32) -> RECT {
    RECT {
        left: rc.left + by,
        top: rc.top + by,
        right: rc.right - by,
        bottom: rc.bottom - by,
    }
}

// ---------- 控件自绘图元(BS_OWNERDRAW → WM_DRAWITEM 分发到这里) ----------

/// 按钮层级(决定配色形态)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BtnKind {
    /// 主按钮(「继续」):强调色圆角填充 + 白字。
    Primary,
    /// 次按钮(取消/延长/管控/侵入式/复制/单选选项行):卡片底 + 描边。
    Secondary,
}

/// macOS 风格圆角按钮。`pressed`/`hot`/`focused` 对应 `ODS_SELECTED`/`ODS_HOTLIGHT`/
/// `ODS_FOCUS`(系统 owner-draw 通知,不自行追踪鼠标)。
pub(super) unsafe fn draw_button(
    hdc: HDC,
    rc: &RECT,
    text: &str,
    kind: BtnKind,
    pressed: bool,
    hot: bool,
    focused: bool,
) {
    let t = theme();
    if !t.enabled {
        return;
    }
    let (fill, border, text_color) = match (kind, pressed, hot) {
        (BtnKind::Primary, true, _) => (t.accent_press, None, t.on_accent),
        (BtnKind::Primary, false, true) => (t.accent_hover, None, t.on_accent),
        (BtnKind::Primary, false, false) => (t.accent, None, t.on_accent),
        (BtnKind::Secondary, true, _) => (t.btn_press, Some(t.btn_border), t.text),
        (BtnKind::Secondary, false, true) => (t.btn_hover, Some(t.btn_border), t.text),
        (BtnKind::Secondary, false, false) => (t.btn_bg, Some(t.btn_border), t.text),
    };
    let radius = corner_radius_for(rc.bottom - rc.top);
    fill_rounded_rect(hdc, rc, radius, fill, border);
    if focused {
        // macOS keyboard focus 形态:内缩 2px 的强调色圆角描边
        outline_rounded_rect(hdc, &inset_rect(rc, 2), (radius - 2).max(1), t.accent);
    }
    draw_text_line(hdc, rc, text, text_color, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
}

/// macOS 开关(NSSwitch 形态)选项行:药丸轨道 + 圆滑块 + 左对齐文案。
pub(super) unsafe fn draw_switch_row(
    hdc: HDC,
    rc: &RECT,
    text: &str,
    checked: bool,
    focused: bool,
) {
    let t = theme();
    if !t.enabled {
        return;
    }
    // 2026-10-15:行底自绘 —— 此前行底依赖 draw_text_line 的 OPAQUE 擦除副作用
    // (白色矩形恰好盖住系统默认按钮面);文字改透明模式绘制后该副作用消失,
    // 行底必须显式填充,否则露出系统默认按钮面(深色板下白字压在浅灰面上
    // 对比度极差)。填充形态与单选选项行(Secondary 按钮)保持一致。
    fill_rounded_rect(hdc, rc, corner_radius_for(rc.bottom - rc.top), t.btn_bg, Some(t.btn_border));
    let geom = switch_geometry(rc.bottom - rc.top);
    let track_top = rc.top + (rc.bottom - rc.top - geom.track_h) / 2;
    let track = RECT {
        left: rc.left,
        top: track_top,
        right: rc.left + geom.track_w,
        bottom: track_top + geom.track_h,
    };
    let (track_fill, knob_left) = if checked {
        (t.accent, track.right - geom.knob_d - geom.knob_margin)
    } else {
        (t.switch_track_off, track.left + geom.knob_margin)
    };
    fill_rounded_rect(hdc, &track, geom.track_h / 2, track_fill, Some(t.switch_border));
    let knob_top = track.top + (geom.track_h - geom.knob_d) / 2;
    let knob = RECT {
        left: knob_left,
        top: knob_top,
        right: knob_left + geom.knob_d,
        bottom: knob_top + geom.knob_d,
    };
    fill_ellipse(hdc, &knob, t.knob);
    let label = RECT {
        left: rc.left + geom.track_w + SWITCH_TEXT_GAP,
        ..*rc
    };
    draw_text_line(hdc, &label, text, t.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    if focused {
        outline_rounded_rect(
            hdc,
            &inset_rect(rc, 2),
            (corner_radius_for(rc.bottom - rc.top) - 2).max(1),
            t.accent,
        );
    }
}

// ---------- WM_CTLCOLOR* 文本配色 ----------

/// 文本类控件的面色(窗口代码按控件 ID 映射后调用)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Surface {
    /// 标题:主文本色,窗口底。
    Title,
    /// 时间轴/倒计时:次要文本色,窗口底。
    Timeline,
    /// 说明区(只读 EDIT)/输入框(EDIT):主文本色,卡片底。
    Card,
    /// 触控板提示:次要文本色,卡片底。
    CardSecondary,
}

/// WM_CTLCOLORSTATIC / WM_CTLCOLOREDIT 统一处理:设置文本/底色并返回背景刷。
///
/// 用 `SetBkColor` 同色实底而非 `TRANSPARENT`:EDIT 拖选高亮与 STATIC 每秒重绘
/// (倒计时行)在透明模式下都易出残影。
pub(super) unsafe fn ctl_color(hdc: HDC, surface: Surface) -> HBRUSH {
    let t = theme();
    let (text_color, back_color, brush) = match surface {
        Surface::Title => (t.text, t.bg, t.bg_brush),
        Surface::Timeline => (t.text_secondary, t.bg, t.bg_brush),
        Surface::Card => (t.text, t.card, t.card_brush),
        Surface::CardSecondary => (t.text_secondary, t.card, t.card_brush),
    };
    SetTextColor(hdc, text_color);
    SetBkColor(hdc, back_color);
    brush
}

// ---------- 窗口 chrome 与进程窗口自遍历 ----------

/// 单窗口 chrome:沉浸式深色标题栏 + 标题栏与客户区同色(macOS 统一标题栏)+
/// Win11 圆角。全部 fail-open(Win10 不支持的属性静默失败,保持系统观感)。
pub(super) fn apply_window_chrome(hwnd: HWND) {
    let t = theme();
    if !t.enabled || hwnd.0.is_null() {
        return;
    }
    // SAFETY:属性值均指向本栈变量;DWM 调用失败仅意味着该属性不可用(如 Win10
    // 不支持标题栏着色),静默忽略保持系统观感,无副作用。
    unsafe {
        let dark_mode: windows::Win32::Foundation::BOOL = windows::Win32::Foundation::BOOL(t.dark as i32);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const windows::Win32::Foundation::BOOL as *const core::ffi::c_void,
            std::mem::size_of::<windows::Win32::Foundation::BOOL>() as u32,
        );
        let caption_color = t.bg;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption_color as *const COLORREF as *const core::ffi::c_void,
            std::mem::size_of::<COLORREF>() as u32,
        );
        let corner_pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&corner_pref) as u32,
        );
    }
}

/// EnumWindows 回调:命中本进程的顶层窗口 → 补挂 chrome(幂等)。
unsafe extern "system" fn sweep_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let target_process_id = lparam.0 as u32;
    let mut process_id: u32 = 0;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    if process_id != 0 && process_id == target_process_id {
        apply_window_chrome(hwnd);
    }
    BOOL(1)
}

/// **本进程全部顶层窗口自遍历补挂 chrome**(「所有弹出的 UI 窗口都要美化」的机械化
/// 实现):只碰本进程窗口,绝不越权;幂等,重复调用无副作用。触发时机见 27 文档 §5
/// (主弹窗创建后 / 每秒 WM_TIMER / WM_SETTINGCHANGE)——日后新增窗口即使创建路径
/// 忘了接入,最多 1s 后被本遍历补挂(控件级主题仍需窗口代码按 §7 规则接入)。
pub(super) fn sweep_process_windows() {
    if !theme().enabled {
        return;
    }
    // SAFETY:枚举同步完成,回调参数为纯值。
    unsafe {
        let process_id = GetCurrentProcessId();
        let _ = EnumWindows(Some(sweep_enum_proc), LPARAM(process_id as isize));
    }
}

// ---------- GDI 底层图元 ----------

fn wide_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 圆角矩形填充(可选 1px 描边)。RoundRect 的椭圆直径 = 半径 × 2。
unsafe fn fill_rounded_rect(hdc: HDC, rc: &RECT, radius: i32, fill: COLORREF, border: Option<COLORREF>) {
    let brush = CreateSolidBrush(fill);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let pen = border.map(|color| CreatePen(PS_SOLID, 1, color));
    let old_pen = pen.map(|pen| SelectObject(hdc, HGDIOBJ(pen.0)));
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius * 2, radius * 2);
    if let Some(old) = old_pen {
        let _ = SelectObject(hdc, old);
    }
    let _ = SelectObject(hdc, old_brush);
    if let Some(pen) = pen {
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

/// 圆角描边(空刷填充,只画边)。
unsafe fn outline_rounded_rect(hdc: HDC, rc: &RECT, radius: i32, color: COLORREF) {
    let null_brush = GetStockObject(NULL_BRUSH);
    let old_brush = SelectObject(hdc, null_brush);
    let pen = CreatePen(PS_SOLID, 1, color);
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius * 2, radius * 2);
    let _ = SelectObject(hdc, old_pen);
    if !old_brush.0.is_null() {
        let _ = SelectObject(hdc, old_brush);
    }
    let _ = DeleteObject(HGDIOBJ(pen.0));
}

/// 椭圆填充(空笔,只填不描)。
unsafe fn fill_ellipse(hdc: HDC, rc: &RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let null_pen = GetStockObject(NULL_PEN);
    let old_pen = SelectObject(hdc, null_pen);
    let _ = Ellipse(hdc, rc.left, rc.top, rc.right, rc.bottom);
    if !old_pen.0.is_null() {
        let _ = SelectObject(hdc, old_pen);
    }
    let _ = SelectObject(hdc, old_brush);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

/// 单行文本(透明背景,画在刚绘制的控件面上)。
///
/// 必须 `SetBkMode(TRANSPARENT)`:DC 默认 `OPAQUE` 模式 + 默认背景色纯白,
/// `DrawTextW` 会先把传入矩形**整个用背景色擦白**再绘制文字 —— 实测 HITL
/// 弹窗按钮被整矩形刷白、白字白底「看不清」(2026-10-15 人工报障)。自绘场景
/// 调用处均已先用填充图元铺满控件面,透明模式直接往面上画字即可;画完恢复
/// 旧模式,不影响同 DC 上的后续 OPAQUE 绘制(如 ctl_color 的实底文本)。
unsafe fn draw_text_line(hdc: HDC, rc: &RECT, text: &str, color: COLORREF, format: DRAW_TEXT_FORMAT) {
    if text.is_empty() {
        return;
    }
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    SetTextColor(hdc, color);
    let old_mode = SetBkMode(hdc, TRANSPARENT);
    let mut area = *rc;
    let _ = DrawTextW(hdc, &mut wide, &mut area, format);
    if old_mode != 0 {
        // SetBkMode 返回 i32 旧模式,重设需包回 BACKGROUND_MODE。
        let _ = SetBkMode(hdc, BACKGROUND_MODE(old_mode as u32));
    }
}

// ---------- 纯函数单测 ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_setting_from_env_covers_all_modes() {
        assert_eq!(theme_setting_from_env(""), ThemeSetting::Auto);
        assert_eq!(theme_setting_from_env("auto"), ThemeSetting::Auto);
        assert_eq!(theme_setting_from_env(" garbage "), ThemeSetting::Auto);
        for raw in ["off", "0", "false", "no", "OFF", " No "] {
            assert_eq!(theme_setting_from_env(raw), ThemeSetting::Off, "{raw} 应为 Off");
        }
        for raw in ["light", "LIGHT", "浅色"] {
            assert_eq!(theme_setting_from_env(raw), ThemeSetting::Light, "{raw} 应为 Light");
        }
        for raw in ["dark", "Dark", "深色"] {
            assert_eq!(theme_setting_from_env(raw), ThemeSetting::Dark, "{raw} 应为 Dark");
        }
    }

    #[test]
    fn dark_flag_only_when_zero() {
        // AppsUseLightTheme: 0=深色,1=浅色;异常值一律回退浅色(fail-open)
        assert!(prefers_dark_from_light_flag(0));
        assert!(!prefers_dark_from_light_flag(1));
        assert!(!prefers_dark_from_light_flag(7));
        assert!(!prefers_dark_from_light_flag(u32::MAX));
    }

    #[test]
    fn rgb_packs_bgr() {
        // COLORREF 内存序 0x00BBGGRR(高字节 B)
        assert_eq!(rgb_from(0x12, 0x34, 0x56).0, 0x0056_3412);
        assert_eq!(rgb_from(0xFF, 0x00, 0x00).0, 0x0000_00FF); // 纯红
    }

    #[test]
    fn corner_radius_clamps_to_half_height() {
        assert_eq!(corner_radius_for(30), 6); // 名义半径生效(BTN_H=30)
        assert_eq!(corner_radius_for(12), 6);
        assert_eq!(corner_radius_for(8), 4); // 矮控件按半高钳制
        assert_eq!(corner_radius_for(1), 1);
    }

    #[test]
    fn switch_geometry_matches_nsswitch_proportions() {
        let g = switch_geometry(30); // BTN_H 行高
        assert_eq!((g.track_w, g.track_h), (38, 22));
        assert_eq!(g.knob_d, 18);
        // 滑块在轨道内(左/右两个停靠位)
        let off_right = g.knob_margin + g.knob_d;
        assert!(off_right <= g.track_w - g.knob_margin, "off 位滑块越轨");
        let on_left = g.track_w - g.knob_d - g.knob_margin;
        assert!(on_left >= g.knob_margin, "on 位滑块越轨");
        // 行高不足时整体收缩,滑块仍内嵌
        let small = switch_geometry(14);
        assert_eq!(small.track_h, 14);
        assert!(small.knob_d > 0);
        assert!(small.knob_d + small.knob_margin * 2 <= small.track_h);
    }

    #[test]
    fn palettes_light_and_dark_differ_on_key_slots() {
        let light = WinTheme::palette(false);
        let dark = WinTheme::palette(true);
        assert!(light.enabled && dark.enabled);
        assert_ne!(light.bg, dark.bg);
        assert_ne!(light.card, dark.card);
        assert_ne!(light.accent, dark.accent);
        // 关键对比槽:卡片与底色必须可区分(分层观感的前提)
        assert_ne!(light.bg, light.card);
        assert_ne!(dark.bg, dark.card);
        // 主按钮文字与强调色可区分
        assert_ne!(light.on_accent, light.accent);
        assert_ne!(dark.on_accent, dark.accent);
    }

    #[test]
    fn disabled_palette_keeps_flag() {
        let t = WinTheme::palette_disabled();
        assert!(!t.enabled);
    }
}
