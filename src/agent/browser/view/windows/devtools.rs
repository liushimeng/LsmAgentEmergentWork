//! DevTools 一级菜单栏 + Console 面板(58 号设计文档,Win32 实现)。
//!
//! 平台无关的状态/纯逻辑在 `view::devtools_common`(快照合并/过滤/配色/
//! 面板翻转);本模块只做 Win32 UI 装配/绘制/消息分发:
//! 菜单栏 BUTTON 子控件 + 面板(过滤三开关/搜索/Preserve/清空/关闭/eval 输入)
//! + 日志区自绘(等宽字体分色行,滚轮翻行)。视口语义与 macOS 侧一致:
//! 面板展开页面区收缩,经既有 `resize` 通道上报重渲染(dock-to-bottom)。

use super::super::devtools_common as dt;
use super::*;

// ---------- DevTools 一级菜单栏 + Console 面板(58 号设计文档) ----------
/// 一级菜单栏高度(像素)。
pub(super) const MENU_H: i32 = 30;
/// Console/占位面板高度(像素)。
pub(super) const CONSOLE_H: i32 = 260;
/// 十个菜单按钮 ID(顺序 = `dt::PANEL_NAMES`)。
pub(super) const ID_MENU_BASE: i32 = 200;
/// 过滤三开关(信息/警告/错误)。
pub(super) const ID_FILTER_INFO: i32 = 220;
pub(super) const ID_FILTER_WARN: i32 = 221;
pub(super) const ID_FILTER_ERR: i32 = 222;
pub(super) const ID_PRESERVE: i32 = 223;
pub(super) const ID_CLEAR: i32 = 224;
pub(super) const ID_CLOSE_PANEL: i32 = 225;
pub(super) const ID_SEARCH: i32 = 226;
pub(super) const ID_EVAL: i32 = 227;

/// DevTools 面板子控件句柄(重排/显隐用)。
#[derive(Clone, Copy, Default)]
struct DtUi {
    menu: [HwndCell; 10],
    filter: [HwndCell; 3],
    search: HwndCell,
    preserve: HwndCell,
    clear: HwndCell,
    close: HwndCell,
    eval_edit: HwndCell,
}
impl Default for HwndCell {
    fn default() -> Self {
        HwndCell(HWND(std::ptr::null_mut()))
    }
}
static DT_UI: Mutex<DtUi> = Mutex::new(DtUi {
    menu: [HwndCell(HWND(std::ptr::null_mut())); 10],
    filter: [HwndCell(HWND(std::ptr::null_mut())); 3],
    search: HwndCell(HWND(std::ptr::null_mut())),
    preserve: HwndCell(HWND(std::ptr::null_mut())),
    clear: HwndCell(HWND(std::ptr::null_mut())),
    close: HwndCell(HWND(std::ptr::null_mut())),
    eval_edit: HwndCell(HWND(std::ptr::null_mut())),
});
/// Console 日志滚动行偏移(滚轮在面板区滚动)。
static CONSOLE_SCROLL: Mutex<i32> = Mutex::new(0);
/// HFONT 是裸指针,静态存储需 Send 容器(与 HwndCell 同理;只在窗口线程读写)。
#[derive(Clone, Copy)]
struct FontCell(HFONT);
unsafe impl Send for FontCell {}
impl Default for FontCell {
    fn default() -> Self {
        FontCell(HFONT(std::ptr::null_mut()))
    }
}
/// 日志等宽字体(惰性创建,进程退出随进程回收)。
static LOG_FONT: Mutex<Option<FontCell>> = Mutex::new(None);
/// 面板展开高度(58 号:页面区收缩 = Chrome dock-to-bottom 语义)。
pub(super) fn panel_h() -> i32 {
    if dt::panel_open() {
        CONSOLE_H
    } else {
        0
    }
}

/// 光标是否在 DevTools 面板区(滚轮路由判定)。
fn wheel_over_panel(hwnd: HWND) -> bool {
    unsafe {
        let mut pt = std::mem::zeroed::<windows::Win32::Foundation::POINT>();
        if GetCursorPos(&mut pt).is_err() {
            return false;
        }
        if ScreenToClient(hwnd, &mut pt).0 == 0 {
            return false;
        }
        let Some((_, ch)) = client_size(hwnd) else {
            return false;
        };
        pt.y >= ch - CONSOLE_H
    }
}


/// Console 面板日志区自绘(等宽字体 + 分色行;滚轮经 `CONSOLE_SCROLL` 翻行)。
pub(super) unsafe fn paint_console(hdc: HDC, cw: i32, ch: i32) {
    let panel_top = ch - CONSOLE_H;
    // 面板底(白)+ 分隔线感的顶边灰条。
    let area = RECT {
        left: 0,
        top: panel_top,
        right: cw,
        bottom: ch,
    };
    let brush = CreateSolidBrush(COLORREF(0x00FAFAFA));
    FillRect(hdc, &area, brush);
    let _ = DeleteObject(brush);
    // 占位面板:灰字提示;Console:分色日志。
    let panel = dt::PANEL.lock().unwrap().clone();
    let old_font = {
        let font = ensure_log_font();
        if font.0.0.is_null() {
            HGDIOBJ(std::ptr::null_mut())
        } else {
            SelectObject(hdc, HGDIOBJ(font.0.0))
        }
    };
    let _ = SetBkMode(hdc, TRANSPARENT);
    if panel != "console" {
        let title = dt::PANEL_NAMES
            .iter()
            .find(|(_, k)| *k == panel)
            .map(|(t, _)| *t)
            .unwrap_or("面板");
        let msg = format!(
            "「{title}」面板尚未实现。当前一级菜单栏仅 Console 面板可用;点击 Console 切换到控制台。"
        );
        let _ = SetTextColor(hdc, COLORREF(0x00888888));
        let w: Vec<u16> = msg.encode_utf16().collect();
        let _ = TextOutW(hdc, 12, panel_top + 24, &w);
    } else {
        let buf = dt::CONSOLE_BUF.lock().unwrap().clone();
        let mut filter = dt::FILTER.lock().unwrap().clone();
        if let Some(search) = hwnd_text(search_hwnd()) {
            filter.search = search;
        }
        let visible: Vec<&(String, String)> = buf
            .iter()
            .filter(|(l, t)| dt::entry_visible(&filter, l, t))
            .collect();
        // 行高 18px;日志区 = 过滤行(28)之下、eval 行(30)之上。
        let log_top = panel_top + 30;
        let log_bottom = ch - 32;
        let line_h = 18;
        let rows = ((log_bottom - log_top) / line_h).max(1);
        let total = visible.len() as i32;
        let max_scroll = (total - rows).max(0);
        let scroll = (*CONSOLE_SCROLL.lock().unwrap()).clamp(0, max_scroll);
        *CONSOLE_SCROLL.lock().unwrap() = scroll;
        for (i, (level, text)) in visible
            .iter()
            .skip(scroll as usize)
            .take(rows as usize)
            .enumerate()
        {
            let (prefix, r, g, b) = dt::entry_style(level);
            let color = COLORREF(
                ((r * 255.0) as u32) | (((g * 255.0) as u32) << 8) | (((b * 255.0) as u32) << 16),
            );
            let _ = SetTextColor(hdc, color);
            // 长行截断(自绘不折行)。
            let mut line = format!("{prefix}{text}");
            let max_chars = ((cw - 24) / 8).max(10) as usize;
            if line.chars().count() > max_chars {
                let cut: String = line.chars().take(max_chars.saturating_sub(1)).collect();
                line = format!("{cut}…");
            }
            let w: Vec<u16> = line.encode_utf16().collect();
            let y = log_top + i as i32 * line_h + 2;
            let _ = TextOutW(hdc, 12, y, &w);
        }
    }
    if !old_font.0.is_null() {
        let _ = SelectObject(hdc, old_font);
    }
}

/// 日志等宽字体(惰性;Consolas 缺失时 GDI 自动字体替换)。
unsafe fn ensure_log_font() -> FontCell {
    let mut slot = LOG_FONT.lock().unwrap();
    if slot.is_none() {
        let face: Vec<u16> = "Consolas".encode_utf16().chain(std::iter::once(0)).collect();
        *slot = Some(FontCell(
            CreateFontW(
                16,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                DEFAULT_QUALITY.0 as u32,
                FF_DONTCARE.0 as u32,
                PCWSTR(face.as_ptr()),
            ),
        ));
    }
    slot.unwrap_or_default()
}

/// 搜索框 HWND(过滤重绘读关键词)。
fn search_hwnd() -> HWND {
    DT_UI
        .lock()
        .map(|u| u.search.0)
        .unwrap_or(HWND(std::ptr::null_mut()))
}


/// DevTools 菜单栏 + 面板子控件重排(宽度自适应;面板坐标按「展开位」统一摆放,
/// 隐藏态不影响)。
pub(super) unsafe fn layout_controls(cw: i32, ch: i32) {
    let ui = DT_UI.lock().unwrap();
    // 菜单栏:十按钮均分。
    let n = dt::PANEL_NAMES.len() as i32;
    let bw = ((cw - 2 * PAD) / n).clamp(36, 140);
    let menu_y = (MENU_H - BTN_H) / 2;
    let mut x = PAD;
    for b in ui.menu.iter() {
        if !b.0.0.is_null() {
            let _ = MoveWindow(b.0, x, menu_y, bw - 4, BTN_H, true);
        }
        x += bw;
    }
    // 面板:过滤行(顶)+ 日志区(自绘)+ eval 行(底)。
    let panel_top = ch - CONSOLE_H;
    let row_y = panel_top + 4;
    let mut fx = 8;
    for b in ui.filter.iter() {
        if !b.0.0.is_null() {
            let _ = MoveWindow(b.0, fx, row_y, 54, 22, true);
        }
        fx += 60;
    }
    if !ui.search.0.0.is_null() {
        let search_w = (cw - fx - 320).max(80);
        let _ = MoveWindow(ui.search.0, fx, row_y, search_w, 22, true);
    }
    if !ui.preserve.0.0.is_null() {
        let _ = MoveWindow(ui.preserve.0, cw - 300, row_y, 118, 22, true);
    }
    if !ui.clear.0.0.is_null() {
        let _ = MoveWindow(ui.clear.0, cw - 174, row_y, 58, 22, true);
    }
    if !ui.close.0.0.is_null() {
        let _ = MoveWindow(ui.close.0, cw - 108, row_y, 44, 22, true);
    }
    if !ui.eval_edit.0.0.is_null() {
        let _ = MoveWindow(ui.eval_edit.0, 8, ch - 32, cw - 16, 26, true);
    }
}

/// DevTools 面板开合(58 号):共享层翻状态 + 控件显隐 + 视口重报 + 命令上报。
pub(super) unsafe fn toggle_panel(hwnd: HWND, name: &str) {
    let next = dt::toggle_panel_state(name);
    let open = next != "none";
    {
        let ui = DT_UI.lock().unwrap();
        let show = if open { SW_SHOWNORMAL } else { SW_HIDE };
        // 菜单栏按钮恒驻;过滤行/搜索/开关/清空/关闭/eval 随面板显隐。
        for b in ui.filter.iter() {
            if !b.0.0.is_null() {
                let _ = ShowWindow(b.0, show);
            }
        }
        for h in [
            ui.search.0,
            ui.preserve.0,
            ui.clear.0,
            ui.close.0,
            ui.eval_edit.0,
        ] {
            if !h.0.is_null() {
                let _ = ShowWindow(h, show);
            }
        }
    }
    // 视口收缩/展开:尺寸未变也要重报 → 复位变化检测再走 handle_resize。
    *LAST_SIZE.lock().unwrap() = None;
    handle_resize(hwnd);
    send_cmd(&format!(
        "{{\"cmd\":\"devtools\",\"panel\":{}}}",
        dt::json_string(&next)
    ));
    let _ = InvalidateRect(hwnd, None, false);
}


/// 通用子按钮(DevTools 用:自定位置/尺寸/风格)。
unsafe fn child_button(
    parent: HWND,
    hinst: HINSTANCE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &str,
    id: i32,
    style: u32,
) -> HWND {
    let cls = to_wide("BUTTON");
    let txt = to_wide(text);
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(cls.as_ptr()),
        PCWSTR(txt.as_ptr()),
        WINDOW_STYLE((WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | style) as u32),
        x,
        y,
        w,
        h,
        parent,
        HMENU(id as isize as *mut _),
        hinst,
        None,
    )
    .unwrap_or_default()
}

/// 通用子输入框(DevTools 搜索/eval;带 cue 提示 + 回车子类)。
unsafe fn child_edit(
    parent: HWND,
    hinst: HINSTANCE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    cue: &str,
) -> HWND {
    let cls = to_wide("EDIT");
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(cls.as_ptr()),
        PCWSTR::null(),
        WINDOW_STYLE((WS_CHILD.0 | WS_VISIBLE.0 | WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32) as u32),
        x,
        y,
        w,
        h,
        parent,
        HMENU(id as isize as *mut _),
        hinst,
        None,
    )
    .unwrap_or_default();
    if !hwnd.0.is_null() {
        let _ = SetWindowSubclass(hwnd, Some(edit_subclass), 1, 0);
        // 灰色 cue 提示(EM_SETCUEBANNER,Vista+;失败无碍)。
        let cue_w: Vec<u16> = cue.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SendMessageW(
            hwnd,
            windows::Win32::UI::Controls::EM_SETCUEBANNER,
            WPARAM(1),
            LPARAM(cue_w.as_ptr() as isize),
        );
    }
    hwnd
}

/// DevTools 菜单栏 + 面板控件装配(58 号;窗口创建时一次,初始隐藏)。
pub(super) unsafe fn build_ui(hwnd: HWND, hinstance: HINSTANCE, cw: i32, ch: i32) {
    // 一级菜单栏十按钮(恒驻)。
    let n = dt::PANEL_NAMES.len() as i32;
    let bw = ((cw - 2 * PAD) / n).clamp(36, 140);
    let menu_y = (MENU_H - BTN_H) / 2;
    let mut x = PAD;
    let mut menu = [HwndCell::default(); 10];
    for (i, (title, _)) in dt::PANEL_NAMES.iter().enumerate() {
        let h = child_button(
            hwnd,
            hinstance,
            x,
            menu_y,
            bw - 4,
            BTN_H,
            title,
            ID_MENU_BASE + i as i32,
            BS_PUSHBUTTON as u32,
        );
        menu[i] = HwndCell(h);
        x += bw;
    }
    // 面板:过滤行(顶)+ 日志区(自绘)+ eval 行(底)。
    let panel_top = ch - CONSOLE_H;
    let row_y = panel_top + 4;
    let mut filter = [HwndCell::default(); 3];
    for (i, title) in ["信息", "警告", "错误"].iter().enumerate() {
        let id = match i {
            0 => ID_FILTER_INFO,
            1 => ID_FILTER_WARN,
            _ => ID_FILTER_ERR,
        };
        let h = child_button(
            hwnd,
            hinstance,
            8 + i as i32 * 60,
            row_y,
            54,
            22,
            title,
            id,
            BS_AUTOCHECKBOX as u32,
        );
        // 缺省三级全开。
        SendMessageW(h, BM_SETCHECK, WPARAM(1), LPARAM(0));
        filter[i] = HwndCell(h);
    }
    let search = child_edit(
        hwnd,
        hinstance,
        188,
        row_y,
        (cw - 188 - 320).max(80),
        22,
        ID_SEARCH,
        "搜索日志(回车)",
    );
    let preserve = child_button(
        hwnd,
        hinstance,
        cw - 300,
        row_y,
        118,
        22,
        "Preserve log",
        ID_PRESERVE,
        BS_AUTOCHECKBOX as u32,
    );
    let clear = child_button(
        hwnd,
        hinstance,
        cw - 174,
        row_y,
        58,
        22,
        "清空",
        ID_CLEAR,
        BS_PUSHBUTTON as u32,
    );
    let close = child_button(
        hwnd,
        hinstance,
        cw - 108,
        row_y,
        44,
        22,
        "✕",
        ID_CLOSE_PANEL,
        BS_PUSHBUTTON as u32,
    );
    let eval_edit = child_edit(
        hwnd,
        hinstance,
        8,
        ch - 32,
        cw - 16,
        26,
        ID_EVAL,
        "输入 JS 表达式后回车执行($_ 上次结果 / $$() 选择 / $0 最近元素)",
    );
    // 初始隐藏:面板收起(菜单栏按钮恒驻)。
    for h in [
        filter[0].0,
        filter[1].0,
        filter[2].0,
        search,
        preserve,
        clear,
        close,
        eval_edit,
    ] {
        if !h.0.is_null() {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
    *DT_UI.lock().unwrap() = DtUi {
        menu,
        filter,
        search: HwndCell(search),
        preserve: HwndCell(preserve),
        clear: HwndCell(clear),
        close: HwndCell(close),
        eval_edit: HwndCell(eval_edit),
    };
}


// ---------- 消息分发与测试钩子(主窗口过程调用) ----------

/// `WM_COMMAND` 的 DevTools 分发(菜单/过滤/清空/关闭);返回是否消费。
pub(super) unsafe fn on_command(hwnd: HWND, id: i32) -> bool {
    // 一级菜单栏:十按钮 → 面板切换(同键再点收起)。
    if (ID_MENU_BASE..ID_MENU_BASE + 10).contains(&id) {
        let idx = (id - ID_MENU_BASE) as usize;
        if let Some((_, key)) = dt::PANEL_NAMES.get(idx) {
            toggle_panel(hwnd, key);
        }
        return true;
    }
    // 过滤三开关 + Preserve log(勾选态落共享过滤态)。
    let filter_slot = match id {
        ID_FILTER_INFO => Some(0),
        ID_FILTER_WARN => Some(1),
        ID_FILTER_ERR => Some(2),
        ID_PRESERVE => Some(3),
        _ => None,
    };
    if let Some(slot) = filter_slot {
        let h = DT_UI.lock().ok().map(|u| match slot {
            0 => u.filter[0].0,
            1 => u.filter[1].0,
            2 => u.filter[2].0,
            _ => u.preserve.0,
        });
        let checked = h
            .map(|h| {
                if h.0.is_null() {
                    true
                } else {
                    SendMessageW(h, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0
                }
            })
            .unwrap_or(true);
        if let Ok(mut f) = dt::FILTER.lock() {
            match slot {
                0 => f.info = checked,
                1 => f.warn = checked,
                2 => f.error = checked,
                _ => f.preserve = checked,
            }
        }
        let _ = InvalidateRect(hwnd, None, false);
        return true;
    }
    // 清空显示缓冲(快照游标不动,新日志照常追加)。
    if id == ID_CLEAR {
        dt::clear_buf();
        let _ = InvalidateRect(hwnd, None, false);
        return true;
    }
    // ✕ 收起面板。
    if id == ID_CLOSE_PANEL {
        let cur = dt::PANEL.lock().map(|p| p.clone()).unwrap_or_default();
        if cur != "none" {
            toggle_panel(hwnd, &cur);
        }
        return true;
    }
    false
}

/// 面板区滚轮翻日志(命中面板且面板展开才消费;步进 3 行/档)。
pub(super) unsafe fn scroll_console(hwnd: HWND, delta: i32) -> bool {
    if panel_h() <= 0 || !wheel_over_panel(hwnd) {
        return false;
    }
    let step = if delta > 0 { -3 } else { 3 };
    *CONSOLE_SCROLL.lock().unwrap() += step;
    let _ = InvalidateRect(hwnd, None, false);
    true
}

/// 测试钩子:`LAEW_DEVTOOLS_AUTO=<panel>` 启动即展开面板(与人工点击同一
/// `toggle_panel` 链路;真机像素验证用,未设置零行为)。
pub(super) unsafe fn maybe_auto_panel(hwnd: HWND) {
    let Ok(name) = std::env::var("LAEW_DEVTOOLS_AUTO") else {
        return;
    };
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return;
    }
    toggle_panel(hwnd, &name);
}
