//! DevTools 一级菜单栏 + Console 面板(58 号设计文档,macOS AppKit FFI)。
//!
//! 平台无关的状态/纯逻辑在 `view::devtools_common`(快照合并/过滤/配色/
//! 面板翻转);本模块只做 macOS UI 装配与绘制:
//! * **一级菜单栏**(恒驻,顶部一行):`PANEL_NAMES` 十个按钮;仅 `Console`
//!   有真实功能,其余点击显示「尚未实现」占位文案;
//! * **面板停靠底部**(dock-to-bottom):展开时页面区高度收缩,经既有 `resize`
//!   命令上报新视口,宿主重排重渲染——页面不被遮挡;
//! * **Console 面板**:级别过滤(信息/警告/错误)+ 关键词搜索 + Preserve log
//!   + 清空 + 关闭;日志区 NSTextView 分色可滚动;eval 输入框回车执行
//!   (`console_eval` 命令 → 宿主页面全局求值 → 结果随快照回来显示)。
//!
//! 数据链路 = **整份快照**(幂等):宿主把 console 台账全量推成
//! `{"kind":"console_sync","page_id":..,"entries":[…]}`(`'d'` 帧),查看器经
//! `devtools_common::take_slot_and_merge` 合并;`input/result/eval_error` 三种
//! eval 回写级别分色显示(`eval_error` 刻意不复用 `error`,不污染 Agent 侧
//! 错误统计)。

use super::super::devtools_common as dt;
use super::*;

// ---------- 布局常量(与 mod.rs 几何共用) ----------
/// 一级菜单栏高度(点)。
pub(super) const MENU_H: f64 = 28.0;
/// Console/占位面板高度(点)。
pub(super) const CONSOLE_H: f64 = 260.0;

// ---------- FFI 扩展消息(仅本模块用,命名不与 mod.rs 消息助手冲突) ----------
#[repr(C)]
#[derive(Clone, Copy)]
struct NSRange {
    location: usize,
    length: usize,
}
// dictionaryWithObject:forKey: / initWithString:attributes:(2 个对象参数)。
msg!(send_id_obj_obj, (a: Id, b: Id) -> Id);
// colorWithCalibratedRed:green:blue:alpha:(4 浮点参数)。
msg!(send_id_ffff, (a: f64, b: f64, c: f64, d: f64) -> Id);
// scrollRangeToVisible:(NSRange 按值传参,2 字宽结构体走寄存器,同 NSPoint 依据)。
msg!(send_void_range, (r: NSRange) -> ());

// ---------- UI 运行态(主线程读写) ----------

/// 菜单栏按钮(按 `dt::PANEL_NAMES` 顺序)。
static MENU_BTNS: once_cell::sync::Lazy<Mutex<Vec<IdCell>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

/// 面板子控件句柄(主线程读写;IdCell 保证 static 可 Send)。
#[derive(Clone, Copy)]
struct UiRefs {
    container: IdCell,
    text: IdCell,
    input: IdCell,
    search: IdCell,
    preserve_btn: IdCell,
    clear_btn: IdCell,
    close_btn: IdCell,
    filter_btns: [IdCell; 3],
}
impl Default for IdCell {
    fn default() -> Self {
        IdCell(std::ptr::null_mut())
    }
}
impl Default for UiRefs {
    fn default() -> Self {
        UiRefs {
            container: IdCell::default(),
            text: IdCell::default(),
            input: IdCell::default(),
            search: IdCell::default(),
            preserve_btn: IdCell::default(),
            clear_btn: IdCell::default(),
            close_btn: IdCell::default(),
            filter_btns: [IdCell::default(); 3],
        }
    }
}
static UI: once_cell::sync::Lazy<Mutex<UiRefs>> =
    once_cell::sync::Lazy::new(|| Mutex::new(UiRefs::default()));

// ---------- 面板状态与视口联动 ----------

/// 面板展开时页面区需额外扣除的高度(点)。
pub(super) fn panel_extra_h() -> f64 {
    if dt::panel_open() {
        CONSOLE_H
    } else {
        0.0
    }
}

/// 切换面板:共享层翻状态,平台层做 UI 联动(显示/渲染/视口重报/命令上报)。
pub(super) unsafe fn toggle_panel(name: &str) {
    let next = dt::toggle_panel_state(name);
    let open = next != "none";
    {
        let ui = UI.lock().unwrap();
        if !ui.container.0.is_null() {
            send_void_bool(ui.container.0, sel(b"setHidden:\0"), !open);
        }
    }
    if next == "console" {
        render_log();
    } else if open {
        render_stub(&next);
    }
    // 视口收缩/展开:置零强制变化检测命中 → reflow + resize 上报。
    *CONTENT_SIZE.lock().unwrap() = (0.0, 0.0);
    sync_content_size();
    send_cmd(&format!(
        "{{\"cmd\":\"devtools\",\"panel\":{}}}",
        dt::json_string(&next)
    ));
}

// ---------- 渲染 ----------

/// Console 日志区:按过滤态重建 attributed 文本并滚到最新。
unsafe fn render_log() {
    let ui = UI.lock().unwrap();
    let tv = ui.text.0;
    if tv.is_null() {
        return;
    }
    let search_text = ns_to_string(send_id(ui.search.0, sel(b"stringValue\0")));
    let buf = dt::CONSOLE_BUF.lock().unwrap().clone();
    let mut filter = dt::FILTER.lock().unwrap().clone();
    filter.search = search_text;
    let mutable = send_id_id(
        send_id(cls(b"NSMutableAttributedString\0"), sel(b"alloc\0")),
        sel(b"initWithString:\0"),
        ns_str(""),
    );
    if mutable.is_null() {
        return;
    }
    let mut total = 0usize;
    for (level, text) in &buf {
        if !dt::entry_visible(&filter, level, text) {
            continue;
        }
        let (prefix, r, g, b) = dt::entry_style(level);
        let line = format!("{prefix}{text}\n");
        let piece = styled_line(&line, r, g, b);
        if !piece.is_null() {
            send_void_id(mutable, sel(b"appendAttributedString:\0"), piece);
            total += line.chars().count();
        }
    }
    let storage = send_id(tv, sel(b"textStorage\0"));
    if !storage.is_null() {
        send_void_id(storage, sel(b"setAttributedString:\0"), mutable);
    }
    // 滚到最新(有输出时)。
    if total > 0 {
        send_void_range(
            tv,
            sel(b"scrollRangeToVisible:\0"),
            NSRange {
                location: total.saturating_sub(1),
                length: 1,
            },
        );
    }
}

/// 占位面板文案(九个未实现面板的统一提示)。
unsafe fn render_stub(key: &str) {
    let ui = UI.lock().unwrap();
    let tv = ui.text.0;
    if tv.is_null() {
        return;
    }
    let title = dt::PANEL_NAMES
        .iter()
        .find(|(_, k)| *k == key)
        .map(|(t, _)| *t)
        .unwrap_or(key);
    let msg = format!(
        "「{title}」面板尚未实现。\n\n当前一级菜单栏仅 Console 面板可用;\n其余九个面板(Elements/Sources/Network/Performance/Memory/Application/Security/Lighthouse/Recorder)为占位按钮。\n\n点击 Console 切换到控制台;再次点击当前按钮可收起面板。"
    );
    let piece = styled_line(&msg, 0.45, 0.45, 0.45);
    let mutable = send_id_id(
        send_id(cls(b"NSMutableAttributedString\0"), sel(b"alloc\0")),
        sel(b"initWithString:\0"),
        ns_str(""),
    );
    if !mutable.is_null() && !piece.is_null() {
        send_void_id(mutable, sel(b"appendAttributedString:\0"), piece);
        let storage = send_id(tv, sel(b"textStorage\0"));
        if !storage.is_null() {
            send_void_id(storage, sel(b"setAttributedString:\0"), mutable);
        }
    }
}

/// 单行 attributed 文本(颜色经属性名字面量键,取不到时优雅退化为默认色)。
unsafe fn styled_line(text: &str, r: f64, g: f64, b: f64) -> Id {
    let color = send_id_ffff(
        cls(b"NSColor\0"),
        sel(b"colorWithCalibratedRed:green:blue:alpha:\0"),
        r,
        g,
        b,
        1.0,
    );
    let attrs = if color.is_null() {
        std::ptr::null_mut()
    } else {
        send_id_obj_obj(
            cls(b"NSDictionary\0"),
            sel(b"dictionaryWithObject:forKey:\0"),
            color,
            ns_str("NSForegroundColorAttributeName"),
        )
    };
    send_id_obj_obj(
        send_id(cls(b"NSAttributedString\0"), sel(b"alloc\0")),
        sel(b"initWithString:attributes:\0"),
        ns_str(text),
        attrs,
    )
}

/// NSString → Rust String(空对象返回空串)。
pub(super) unsafe fn ns_to_string(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let ptr = send_id_ptr(s, sel(b"UTF8String\0"));
    if ptr.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string()
}

// ---------- 定时器接线 ----------

/// 定时器一拍:消费快照槽 → 合并 → Console 可见时重绘(主线程)。
pub(super) fn tick_apply() {
    if !dt::take_slot_and_merge() {
        return;
    }
    if dt::console_open() {
        unsafe { render_log() };
    }
}

// ---------- 控制器回调(FFI;全部走 ffi_guard 防火墙) ----------

/// 菜单栏按钮:`sender.tag` → 面板 key;同键再点收起。
pub(super) extern "C" fn on_dt_panel(_self: Id, _cmd: Sel, sender: Id) {
    crate::ffi_guard::guard("on_dt_panel", || unsafe {
        let tag = send_i64(sender, sel(b"tag\0"));
        let Some((_, key)) = dt::PANEL_NAMES.get(tag.max(0) as usize) else {
            return;
        };
        toggle_panel(key);
    });
}

/// 过滤按钮(信息/警告/错误/Preserve log):开关型,读 `state` 落过滤态。
pub(super) extern "C" fn on_dt_filter(_self: Id, _cmd: Sel, sender: Id) {
    crate::ffi_guard::guard("on_dt_filter", || unsafe {
        let tag = send_i64(sender, sel(b"tag\0"));
        let on = send_i64(sender, sel(b"state\0")) != 0;
        if let Ok(mut f) = dt::FILTER.lock() {
            match tag {
                0 => f.info = on,
                1 => f.warn = on,
                2 => f.error = on,
                3 => f.preserve = on,
                _ => {}
            }
        }
        if dt::console_open() {
            render_log();
        }
    });
}

/// 搜索框回车:按关键词重渲染。
pub(super) extern "C" fn on_dt_search(_self: Id, _cmd: Sel, _sender: Id) {
    crate::ffi_guard::guard("on_dt_search", || unsafe {
        if dt::console_open() {
            render_log();
        }
    });
}

/// 清空按钮:清本地显示缓冲(快照游标不动,新日志照常追加)。
pub(super) extern "C" fn on_dt_clear(_self: Id, _cmd: Sel, _sender: Id) {
    crate::ffi_guard::guard("on_dt_clear", || {
        dt::clear_buf();
        unsafe {
            if dt::console_open() {
                render_log();
            }
        }
    });
}

/// ✕ 关闭按钮:收起面板(等价再点当前菜单键)。
pub(super) extern "C" fn on_dt_close(_self: Id, _cmd: Sel, _sender: Id) {
    crate::ffi_guard::guard("on_dt_close", || unsafe {
        let cur = dt::PANEL.lock().map(|p| p.clone()).unwrap_or_default();
        if cur != "none" {
            toggle_panel(&cur);
        }
    });
}

/// eval 输入框回车:经命令通道送宿主页面全局求值,结果随快照回来显示。
pub(super) extern "C" fn on_console_eval(_self: Id, _cmd: Sel, _sender: Id) {
    crate::ffi_guard::guard("on_console_eval", || unsafe {
        let ui = UI.lock().unwrap();
        let field = ui.input.0;
        if field.is_null() {
            return;
        }
        let text = ns_to_string(send_id(field, sel(b"stringValue\0")));
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        send_cmd(&format!(
            "{{\"cmd\":\"console_eval\",\"expr\":{}}}",
            dt::json_string(trimmed)
        ));
        send_void_id(field, sel(b"setStringValue:\0"), ns_str(""));
    });
}

// ---------- UI 装配 ----------

/// 测试/调试钩子:`LAEW_DEVTOOLS_AUTO=<panel>` 启动即展开指定面板。
///
/// 真机验证面板 UI 时,自动化环境(无辅助功能权限)无法合成点击菜单按钮,
/// 经此钩子让查看器启动即打开面板(走与人工点击**完全相同**的
/// `toggle_panel` 链路:视口重报 + 命令上报 + 快照同步)。未设置时零行为。
pub(super) unsafe fn maybe_auto_panel() {
    let Ok(name) = std::env::var("LAEW_DEVTOOLS_AUTO") else {
        return;
    };
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return;
    }
    toggle_panel(&name);
}

// ---------- UI 装配(续) ----------

/// 给控制器类挂 DevTools 六个动作(`run()` 注册类方法时调用,全成功返回 1)。
pub(super) unsafe fn register_methods(ctrl_cls: Id) -> i8 {
    let mut ok = 1i8;
    for (name, imp) in [
        (
            &b"on_dt_panel:\0"[..],
            on_dt_panel as extern "C" fn(Id, Sel, Id) as *const c_void,
        ),
        (
            &b"on_dt_filter:\0"[..],
            on_dt_filter as extern "C" fn(Id, Sel, Id) as *const c_void,
        ),
        (
            &b"on_dt_search:\0"[..],
            on_dt_search as extern "C" fn(Id, Sel, Id) as *const c_void,
        ),
        (
            &b"on_dt_clear:\0"[..],
            on_dt_clear as extern "C" fn(Id, Sel, Id) as *const c_void,
        ),
        (
            &b"on_dt_close:\0"[..],
            on_dt_close as extern "C" fn(Id, Sel, Id) as *const c_void,
        ),
        (
            &b"on_console_eval:\0"[..],
            on_console_eval as extern "C" fn(Id, Sel, Id) as *const c_void,
        ),
    ] {
        ok &= class_addMethod(ctrl_cls, sel(name), imp, b"v@:@\0".as_ptr().cast());
    }
    ok
}

/// 建一级菜单栏 + Console/占位面板(窗口装配时调用一次)。
///
/// `page_h` = 初始页面区高度;菜单行在工具栏之上,面板贴窗口底部(初始隐藏)。
pub(super) unsafe fn build_ui(content: Id, ctrl: Id, w: f64, page_h: f64) {
    // ---- 一级菜单栏(顶部一行,十个按钮均分) ----
    let menu_y = page_h + TOOLBAR_H + (MENU_H - BTN_H) / 2.0;
    let n = dt::PANEL_NAMES.len() as f64;
    let bw = ((w - PAD * 2.0) / n).clamp(36.0, 130.0);
    let mut x = PAD;
    let mut btns = Vec::new();
    for (i, (title, _key)) in dt::PANEL_NAMES.iter().enumerate() {
        let b = make_button(
            ctrl,
            ns_rect(x, menu_y, bw - 4.0, BTN_H),
            title,
            sel(b"on_dt_panel:\0"),
            &format!("{title} 面板"),
        );
        if !b.is_null() {
            send_void_i64(b, sel(b"setTag:\0"), i as i64);
            send_void_id(content, sel(b"addSubview:\0"), b);
        }
        btns.push(IdCell(b));
        x += bw;
    }
    *MENU_BTNS.lock().unwrap() = btns;

    // ---- 面板容器(贴底,初始隐藏) ----
    let container = send_id_rect(
        send_id(cls(b"NSView\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, w, CONSOLE_H),
    );
    if container.is_null() {
        return;
    }
    send_void_bool(container, sel(b"setHidden:\0"), true);
    send_void_id(content, sel(b"addSubview:\0"), container);

    // ---- 过滤行(面板顶部) ----
    let row_y = CONSOLE_H - 30.0;
    let mut fx = 8.0;
    let filter_titles = ["信息", "警告", "错误"];
    let mut filter_btns = [IdCell::default(); 3];
    for (i, title) in filter_titles.iter().enumerate() {
        let b = make_button(
            ctrl,
            ns_rect(fx, row_y, 54.0, 22.0),
            title,
            sel(b"on_dt_filter:\0"),
            &format!("{title}级别日志显示开关"),
        );
        if !b.is_null() {
            // 开关型按钮(checkbox 形态):点击自动翻转 state。
            send_void_i64(b, sel(b"setButtonType:\0"), 3);
            send_void_i64(b, sel(b"setState:\0"), 1);
            send_void_i64(b, sel(b"setTag:\0"), i as i64);
            send_void_id(container, sel(b"addSubview:\0"), b);
        }
        filter_btns[i] = IdCell(b);
        fx += 60.0;
    }
    // 搜索框(回车生效)。
    let search_w = (w - fx - 320.0).max(80.0);
    let search = send_id_rect(
        send_id(cls(b"NSTextField\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(fx, row_y - 1.0, search_w, 24.0),
    );
    if !search.is_null() {
        send_void_id(search, sel(b"setPlaceholderString:\0"), ns_str("搜索日志(回车)"));
        send_void_id(search, sel(b"setTarget:\0"), ctrl);
        send_void_sel(search, sel(b"setAction:\0"), sel(b"on_dt_search:\0"));
        send_void_id(container, sel(b"addSubview:\0"), search);
    }
    // Preserve log 开关。
    let preserve = make_button(
        ctrl,
        ns_rect(w - 300.0, row_y, 118.0, 22.0),
        "Preserve log",
        sel(b"on_dt_filter:\0"),
        "跨导航保留历史日志",
    );
    if !preserve.is_null() {
        send_void_i64(preserve, sel(b"setButtonType:\0"), 3);
        send_void_i64(preserve, sel(b"setState:\0"), 0);
        send_void_i64(preserve, sel(b"setTag:\0"), 3);
        send_void_id(container, sel(b"addSubview:\0"), preserve);
    }
    // 清空 + 关闭。
    let clear = make_button(
        ctrl,
        ns_rect(w - 174.0, row_y, 58.0, 22.0),
        "清空",
        sel(b"on_dt_clear:\0"),
        "清空控制台显示",
    );
    if !clear.is_null() {
        send_void_id(container, sel(b"addSubview:\0"), clear);
    }
    let close = make_button(
        ctrl,
        ns_rect(w - 108.0, row_y, 44.0, 22.0),
        "✕",
        sel(b"on_dt_close:\0"),
        "收起面板",
    );
    if !close.is_null() {
        send_void_id(container, sel(b"addSubview:\0"), close);
    }

    // ---- 日志区(NSScrollView + NSTextView,等宽字体) ----
    let log_y = 36.0;
    let log_h = (row_y - log_y - 6.0).max(40.0);
    let scroll = send_id_rect(
        send_id(cls(b"NSScrollView\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(8.0, log_y, w - 16.0, log_h),
    );
    let tv = send_id_rect(
        send_id(cls(b"NSTextView\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(0.0, 0.0, (w - 40.0).max(80.0), log_h.max(40.0)),
    );
    if !scroll.is_null() && !tv.is_null() {
        send_void_bool(tv, sel(b"setEditable:\0"), false);
        send_void_bool(tv, sel(b"setSelectable:\0"), true);
        if respond(tv, b"setVerticallyResizable:\0") {
            send_void_bool(tv, sel(b"setVerticallyResizable:\0"), true);
        }
        let font = mono_font(11.0);
        if !font.is_null() {
            send_void_id(tv, sel(b"setFont:\0"), font);
        }
        send_void_bool(scroll, sel(b"setHasVerticalScroller:\0"), true);
        send_void_id(scroll, sel(b"setDocumentView:\0"), tv);
        send_void_id(container, sel(b"addSubview:\0"), scroll);
    }

    // ---- eval 输入行 ----
    let input = send_id_rect(
        send_id(cls(b"NSTextField\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        ns_rect(8.0, 4.0, w - 16.0, 28.0),
    );
    if !input.is_null() {
        send_void_id(
            input,
            sel(b"setPlaceholderString:\0"),
            ns_str("输入 JS 表达式后回车执行($_ 上次结果 / $$() 选择 / $0 最近元素)"),
        );
        send_void_id(input, sel(b"setTarget:\0"), ctrl);
        send_void_sel(input, sel(b"setAction:\0"), sel(b"on_console_eval:\0"));
        let font = send_id_f64(cls(b"NSFont\0"), sel(b"systemFontOfSize:\0"), 13.0);
        if !font.is_null() {
            send_void_id(input, sel(b"setFont:\0"), font);
        }
        send_void_id(container, sel(b"addSubview:\0"), input);
    }

    *UI.lock().unwrap() = UiRefs {
        container: IdCell(container),
        text: IdCell(tv),
        input: IdCell(input),
        search: IdCell(search),
        preserve_btn: IdCell(preserve),
        clear_btn: IdCell(clear),
        close_btn: IdCell(close),
        filter_btns,
    };
}

/// 等宽字体(userFixedPitchFontOfSize: 缺失时退化系统字体)。
unsafe fn mono_font(size: f64) -> Id {
    let nsf = cls(b"NSFont\0");
    if respond(nsf, b"userFixedPitchFontOfSize:\0") {
        send_id_f64(nsf, sel(b"userFixedPitchFontOfSize:\0"), size)
    } else {
        send_id_f64(nsf, sel(b"systemFontOfSize:\0"), size)
    }
}

/// 重排面板与菜单栏(mod.rs 的 reflow_controls 收尾调用)。
///
/// `page_h` = 页面区高度(= 内容区高 - 工具栏 - 菜单栏 - 面板)。
pub(super) unsafe fn reflow(cw: f64, ch: f64, page_h: f64) {
    // 菜单栏:按新宽度均分。
    let n = dt::PANEL_NAMES.len() as f64;
    let bw = ((cw - PAD * 2.0) / n).clamp(36.0, 130.0);
    let menu_y = ch - MENU_H + (MENU_H - BTN_H) / 2.0;
    let mut x = PAD;
    if let Ok(btns) = MENU_BTNS.lock() {
        for b in btns.iter() {
            if !b.0.is_null() {
                send_void_rect(b.0, sel(b"setFrame:\0"), ns_rect(x, menu_y, bw - 4.0, BTN_H));
            }
            x += bw;
        }
    }
    // 面板容器与子控件按新宽度重排。
    let ui = UI.lock().unwrap();
    if ui.container.0.is_null() {
        return;
    }
    let panel_h = panel_extra_h();
    send_void_rect(
        ui.container.0,
        sel(b"setFrame:\0"),
        ns_rect(0.0, 0.0, cw, panel_h.max(1.0)),
    );
    let _ = page_h;
    let row_y = panel_h.max(CONSOLE_H) - 30.0;
    // 过滤按钮位置固定宽度序列;搜索框与右侧按钮随宽度推移。
    let mut fx = 8.0;
    for b in ui.filter_btns.iter() {
        if !b.0.is_null() {
            send_void_rect(b.0, sel(b"setFrame:\0"), ns_rect(fx, row_y, 54.0, 22.0));
        }
        fx += 60.0;
    }
    if !ui.search.0.is_null() {
        let search_w = (cw - fx - 320.0).max(80.0);
        send_void_rect(
            ui.search.0,
            sel(b"setFrame:\0"),
            ns_rect(fx, row_y - 1.0, search_w, 24.0),
        );
    }
    if !ui.preserve_btn.0.is_null() {
        send_void_rect(
            ui.preserve_btn.0,
            sel(b"setFrame:\0"),
            ns_rect(cw - 300.0, row_y, 118.0, 22.0),
        );
    }
    if !ui.text.0.is_null() {
        // 日志区与输入行随宽度自适应(text view 藏在 scroll view 里,scroll 重排
        // 即可;text view 宽度同步拉宽避免横向滚动)。
        let log_y = 36.0;
        let log_h = (row_y - log_y - 6.0).max(40.0);
        let scroll = send_id(ui.text.0, sel(b"enclosingScrollView\0"));
        if !scroll.is_null() {
            send_void_rect(
                scroll,
                sel(b"setFrame:\0"),
                ns_rect(8.0, log_y, cw - 16.0, log_h),
            );
        }
        let inner_w = (cw - 40.0).max(80.0);
        let inner_h = send_rect(ui.text.0, sel(b"frame\0")).size.height.max(log_h);
        send_void_rect(
            ui.text.0,
            sel(b"setFrame:\0"),
            ns_rect(0.0, 0.0, inner_w, inner_h),
        );
    }
    if !ui.input.0.is_null() {
        send_void_rect(
            ui.input.0,
            sel(b"setFrame:\0"),
            ns_rect(8.0, 4.0, cw - 16.0, 28.0),
        );
    }
    // 清空/关闭:右对齐随宽度推移(与 build_ui 初始布局同公式)。
    if !ui.clear_btn.0.is_null() {
        send_void_rect(
            ui.clear_btn.0,
            sel(b"setFrame:\0"),
            ns_rect(cw - 174.0, row_y, 58.0, 22.0),
        );
    }
    if !ui.close_btn.0.is_null() {
        send_void_rect(
            ui.close_btn.0,
            sel(b"setFrame:\0"),
            ns_rect(cw - 108.0, row_y, 44.0, 22.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 新增 UI 链路的全部实例/类选择器必须真实存在(unrecognized selector
    /// = NSException 穿越 nounwind 回调 abort 查看器,54 号事故教训)。
    #[test]
    fn devtools选择器全部真实存在() {
        unsafe {
            assert!(ensure_frameworks(), "装载 Foundation/AppKit 失败");
            // 实例选择器。
            for (class_name, sels) in [
                (
                    &b"NSButton\0"[..],
                    vec![
                        &b"setTag:\0"[..],
                        &b"setState:\0"[..],
                        &b"setButtonType:\0"[..],
                    ],
                ),
                (
                    &b"NSTextView\0"[..],
                    vec![
                        &b"textStorage\0"[..],
                        &b"setEditable:\0"[..],
                        &b"setSelectable:\0"[..],
                        &b"setVerticallyResizable:\0"[..],
                        &b"scrollRangeToVisible:\0"[..],
                        &b"enclosingScrollView\0"[..],
                    ],
                ),
                (
                    &b"NSMutableAttributedString\0"[..],
                    vec![&b"appendAttributedString:\0"[..]],
                ),
                (
                    &b"NSScrollView\0"[..],
                    vec![
                        &b"setDocumentView:\0"[..],
                        &b"setHasVerticalScroller:\0"[..],
                    ],
                ),
                (&b"NSTextStorage\0"[..], vec![&b"setAttributedString:\0"[..]]),
                (
                    &b"NSTextField\0"[..],
                    vec![
                        &b"setPlaceholderString:\0"[..],
                        &b"setTarget:\0"[..],
                        &b"setAction:\0"[..],
                    ],
                ),
            ] {
                let cls_obj = cls(class_name);
                assert!(
                    !cls_obj.is_null(),
                    "拿不到类 {:?}",
                    String::from_utf8_lossy(class_name)
                );
                for s in sels {
                    let ok = send_i64_sel(cls_obj, sel(b"instancesRespondToSelector:\0"), sel(s));
                    assert!(
                        ok != 0,
                        "{} 不响应选择器 {:?}",
                        String::from_utf8_lossy(class_name),
                        String::from_utf8_lossy(s)
                    );
                }
            }
            // 类方法(直接对类对象问 respondsToSelector:)。
            assert!(
                respond(cls(b"NSDictionary\0"), b"dictionaryWithObject:forKey:\0"),
                "NSDictionary 类方法缺失"
            );
            assert!(
                respond(
                    cls(b"NSColor\0"),
                    b"colorWithCalibratedRed:green:blue:alpha:\0"
                ),
                "NSColor 类方法缺失"
            );
            assert!(
                respond(cls(b"NSFont\0"), b"userFixedPitchFontOfSize:\0"),
                "NSFont 等宽字体工厂缺失(运行时会退化系统字体)"
            );
            // 类簇(AttributedString)的 init 家族:`instancesRespondToSelector:`
            // 会误报 false(AppKit/Foundation 类簇怪癖),必须对**活实例**问
            // respondsToSelector:(实测活实例 true 且 initWithString: 可用)。
            let obj = send_id(
                send_id(cls(b"NSMutableAttributedString\0"), sel(b"alloc\0")),
                sel(b"init\0"),
            );
            assert!(!obj.is_null(), "NSMutableAttributedString alloc/init 失败");
            for s in [
                &b"initWithString:\0"[..],
                &b"initWithString:attributes:\0"[..],
            ] {
                assert!(
                    respond(obj, s),
                    "活实例不响应 {:?} —— 发出去就是 unrecognized selector 崩溃",
                    String::from_utf8_lossy(s)
                );
            }
        }
    }
}
