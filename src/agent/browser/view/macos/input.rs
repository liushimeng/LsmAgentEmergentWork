//! 人类键鼠输入链路(39 号设计文档,macOS AppKit FFI)。
//!
//! 从 `macos.rs` 按职责机械搬移(58 号设计文档拆分),逻辑零改写;
//! 共享基础设施在 `super`(mod.rs),本模块经 `use super::*` 取用。

use super::*;

// ---------- 人类键鼠输入(39 号设计文档) ----------
//
// 页面视图(NSImageView 子类 `LsmBrowserPageView`)捕获原始鼠标/键盘事件,
// 坐标换算成视口坐标后经 stdout 命令通道上报宿主;宿主 `human_input` 管线
// 做控制权闸门 + 命中 + 复用 Agent 侧交互语义。`LAEW_HUMAN_INPUT=off` 时
// 查看器停止上报(宿主侧同时拒绝,双保险)。

/// 修饰键位(NSEvent modifierFlags)。
const NSEVENT_SHIFT: u64 = 0x0002_0000;
const NSEVENT_CONTROL: u64 = 0x0004_0000;
const NSEVENT_OPTION: u64 = 0x0008_0000;
const NSEVENT_COMMAND: u64 = 0x0010_0000;
/// NSTrackingArea 选项:进入/离开 + 移动 + 始终活跃 + 自动跟随 visibleRect。
pub(super) const TRACKING_OPTS: u64 = 0x01 | 0x02 | 0x80 | 0x100;

pub(super) fn input_disabled() -> bool {
    static OFF: once_cell::sync::Lazy<bool> = once_cell::sync::Lazy::new(|| {
        matches!(
            std::env::var("LAEW_HUMAN_INPUT")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    });
    *OFF
}

/// macOS 虚拟键码 → 浏览器键名(特殊键);可打印字符走
/// `charactersIgnoringModifiers`(0x20..=0x7E,IME 组合输入属阶段二)。
pub(super) fn mac_special_key(code: u64) -> Option<&'static str> {
    Some(match code {
        36 | 76 => "Enter",
        48 => "Tab",
        53 => "Escape",
        51 => "Backspace",
        117 => "Delete",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        115 => "Home",
        119 => "End",
        116 => "PageUp",
        121 => "PageDown",
        _ => return None,
    })
}

/// NSEvent → JSON 字符串的 key 字段转义(键名/单字符,无控制字符)。
pub(super) fn key_json_str(k: &str) -> String {
    let mut out = String::new();
    for c in k.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// 视图本地坐标(原点左下)→ 视口坐标(原点左上;帧等比居中换算)。
/// 越出画面(拖到工具栏/边框)返回 None。
pub(super) unsafe fn page_point(view: Id, event: Id) -> Option<(f64, f64)> {
    let loc = send_point(event, sel(b"locationInWindow\0"));
    let local = send_point_point_id(view, sel(b"convertPoint:fromView:\0"), loc, std::ptr::null_mut());
    let (cw, ch) = *CONTENT_SIZE.lock().unwrap();
    let (vw, vh) = *VIEWPORT.lock().unwrap();
    if vw <= 0.0 || vh <= 0.0 || cw <= 0.0 || ch <= 0.0 {
        return None;
    }
    // 页面区高度 = 内容区 - 工具栏 - 一级菜单栏 - DevTools 面板(58 号)。
    let view_h = (ch - TOOLBAR_H - super::devtools::MENU_H - super::devtools::panel_extra_h()).max(1.0);
    let s = (cw / vw).min(view_h / vh);
    if s <= 0.0 {
        return None;
    }
    let dx = (cw - vw * s) / 2.0;
    let dy_bottom = (view_h - vh * s) / 2.0;
    let top_edge = dy_bottom + vh * s;
    let px = (local.x - dx) / s;
    let py = (top_edge - local.y) / s;
    if px < 0.0 || py < 0.0 || px >= vw || py >= vh {
        return None;
    }
    Some((px, py))
}

/// 上报一条输入事件(命令通道)。
pub(super) fn send_input(line: &str) {
    if input_disabled() {
        return;
    }
    send_cmd(line);
}

pub(super) fn move_allowed(x: f64, y: f64) -> bool {
    let mut last = LAST_MOVE.lock().unwrap();
    match *last {
        Some((at, lx, ly)) => {
            let due = at.elapsed().as_millis() >= 30
                && ((lx - x).abs() >= 1.0 || (ly - y).abs() >= 1.0);
            if due {
                *last = Some((std::time::Instant::now(), x, y));
            }
            due
        }
        None => {
            *last = Some((std::time::Instant::now(), x, y));
            true
        }
    }
}

/// 指针事件公共出口:坐标换算 + 组 JSON。
pub(super) unsafe fn forward_pointer(view: Id, event: Id, act: &str, btn: Option<&str>, is_move: bool) {
    let Some((x, y)) = page_point(view, event) else {
        return;
    };
    if is_move && !move_allowed(x, y) {
        return;
    }
    let line = match btn {
        Some(b) => format!("{{\"cmd\":\"input\",\"act\":\"{act}\",\"x\":{x:.0},\"y\":{y:.0},\"btn\":\"{b}\"}}"),
        None => format!("{{\"cmd\":\"input\",\"act\":\"{act}\",\"x\":{x:.0},\"y\":{y:.0}}}"),
    };
    send_input(&line);
}

macro_rules! pointer_action {
    ($name:ident, $act:expr, $btn:expr, $is_move:expr) => {
        pub(super) extern "C" fn $name(_self: Id, _cmd: Sel, event: Id) {
            crate::ffi_guard::guard(stringify!($name), || {
                if input_disabled() {
                    return;
                }
                unsafe { forward_pointer(_self, event, $act, $btn, $is_move) };
            });
        }
    };
}

pointer_action!(on_pv_mouse_down, "down", Some("left"), false);
pointer_action!(on_pv_mouse_up, "up", Some("left"), false);
pointer_action!(on_pv_dragged, "move", None, true);
pointer_action!(on_pv_moved, "move", None, true);
pointer_action!(on_pv_other_up, "up", Some("middle"), false);

/// 右键按下:默认弹**原生菜单**(后退/前进/刷新/首页/复制/粘贴);
/// `LAEW_HUMAN_RMENU=off` 回退阶段一的 contextmenu 事件路径。
pub(super) extern "C" fn on_pv_right_down(_self: Id, _cmd: Sel, event: Id) {
    crate::ffi_guard::guard("on_pv_right_down", || {
        if input_disabled() || *RMENU_OFF {
            RMENU_ACTIVE.store(false, Ordering::Relaxed);
            unsafe { forward_pointer(_self, event, "down", Some("right"), false) };
            return;
        }
        unsafe {
            // 弹菜单(阻塞至人工选择/点空白关闭);选择经命令通道驱动引擎。
            show_context_menu(_self, event);
            RMENU_ACTIVE.store(true, Ordering::Relaxed);
        }
    });
}

/// 右键抬起:菜单已接管时不重复上报(contextmenu 由菜单项表达意图)。
pub(super) extern "C" fn on_pv_right_up(_self: Id, _cmd: Sel, event: Id) {
    crate::ffi_guard::guard("on_pv_right_up", || {
        if RMENU_ACTIVE.swap(false, Ordering::Relaxed) {
            return;
        }
        if input_disabled() {
            return;
        }
        unsafe { forward_pointer(_self, event, "up", Some("right"), false) };
    });
}

/// 中键按下:仅 buttonNumber==2 视为中键(侧键忽略,阶段二)。
/// `buttonNumber` 先经 `respond` 守卫(事件子类型相关选择器,防 unrecognized
/// selector),缺失时按非中键处理。
pub(super) extern "C" fn on_pv_other_down(_self: Id, _cmd: Sel, event: Id) {
    crate::ffi_guard::guard("on_pv_other_down", || {
        if input_disabled() {
            return;
        }
        unsafe {
            let btn = if respond(event, b"buttonNumber\0") {
                send_u64(event, sel(b"buttonNumber\0"))
            } else {
                0
            };
            if btn == 2 {
                forward_pointer(_self, event, "down", Some("middle"), false);
            } else if btn == 3 {
                // 侧键 1 = 后退(P2c;Apple 鼠标/触控板辅助键序号)。
                send_cmd("{\"cmd\":\"back\"}");
            } else if btn == 4 {
                // 侧键 2 = 前进。
                send_cmd("{\"cmd\":\"forward\"}");
            }
        }
    });
}

/// 滚轮:精确增量(触控板,像素)归一到 48px/档;离散滚轮每档 ±1。
///
/// 事件子类型相关的选择器(`hasPreciseScrollingDeltas`/`deltaX`/`deltaY`)一律
/// 先经 `respond` 守卫再发:选择器不存在时优雅降级(按离散滚轮/零增量处理),
/// 绝不触发 unrecognized selector(2026-11-04 崩溃根因:误拼 `hasPreciseDeltas`,
/// NSException 穿越 nounwind 回调 abort 查看器)。
pub(super) extern "C" fn on_pv_scroll(_self: Id, _cmd: Sel, event: Id) {
    crate::ffi_guard::guard("on_pv_scroll", || {
        if input_disabled() {
            return;
        }
        unsafe {
            let Some((x, y)) = page_point(_self, event) else {
                return;
            };
            let precise = respond(event, b"hasPreciseScrollingDeltas\0")
                && send_i64(event, sel(b"hasPreciseScrollingDeltas\0")) != 0;
            let raw_dx = if respond(event, b"deltaX\0") {
                send_f64(event, sel(b"deltaX\0"))
            } else {
                0.0
            };
            let raw_dy = if respond(event, b"deltaY\0") {
                send_f64(event, sel(b"deltaY\0"))
            } else {
                0.0
            };
            let norm = |d: f64| -> f64 {
                let v = if precise { d / 48.0 } else { d };
                v.clamp(-20.0, 20.0)
            };
            // P2d:滚轮档位 ÷ 缩放(视觉滚动距离与缩放无关,Chrome 行为:
            // 高倍缩放下每档滚过的 CSS 像素更少)。
            let z = (*ZOOM.lock().unwrap()).max(0.05);
            let (dx, dy) = (norm(raw_dx) / z, norm(raw_dy) / z);
            if dx.abs() < f64::EPSILON && dy.abs() < f64::EPSILON {
                return;
            }
            let line = format!(
                "{{\"cmd\":\"input\",\"act\":\"wheel\",\"x\":{x:.0},\"y\":{y:.0},\"dx\":{dx:.3},\"dy\":{dy:.3}}}"
            );
            send_input(&line);
        }
    });
}

/// 键按下:浏览器级快捷键(⌘R/⌘[/⌘]/Ctrl+R/F5)本地消化,其余转发宿主。
pub(super) extern "C" fn on_pv_key_down(_self: Id, _cmd: Sel, event: Id) {
    crate::ffi_guard::guard("on_pv_key_down", || {
        if input_disabled() {
            return;
        }
        unsafe {
        let flags = send_u64(event, sel(b"modifierFlags\0"));
        let code = send_u64(event, sel(b"keyCode\0"));
        let meta = flags & NSEVENT_COMMAND != 0;
        let ctrl = flags & NSEVENT_CONTROL != 0;
        // 浏览器级快捷键:刷新/后退/前进(转既有命令,不进页面)。
        if (meta || ctrl) && code == 15 {
            send_cmd("{\"cmd\":\"reload\"}");
            return;
        }
        if meta && code == 33 {
            send_cmd("{\"cmd\":\"back\"}");
            return;
        }
        if meta && code == 30 {
            send_cmd("{\"cmd\":\"forward\"}");
            return;
        }
        if code == 96 {
            send_cmd("{\"cmd\":\"reload\"}");
            return;
        }
        // P2e:F11(键码 103)全屏切换 —— AppKit 原生动画与还原,浏览器级
        // 快捷键不进页面(与 Chrome 行为一致;系统"显示桌面"手势优先时本键
        // 不达,属用户设置)。
        if code == 103 {
            let window = send_id(_self, sel(b"window\0"));
            if !window.is_null() && respond(window, b"toggleFullScreen:\0") {
                send_void_id(window, sel(b"toggleFullScreen:\0"), std::ptr::null_mut());
            }
            return;
        }
        // 标签页快捷键(39 号设计文档阶段二):⌘/Ctrl+T 新标签、⌘/Ctrl+W 关标签
        //、Ctrl+Tab 切换(⌘+Tab 是系统级切换器,不占用)。
        if (meta || ctrl) && code == 17 {
            send_cmd("{\"cmd\":\"new_tab\"}");
            return;
        }
        if (meta || ctrl) && code == 13 {
            send_cmd("{\"cmd\":\"close_tab\"}");
            return;
        }
        if ctrl && code == 48 {
            send_cmd("{\"cmd\":\"next_tab\"}");
            return;
        }
        // ⌘/Ctrl+L:聚焦地址栏(39 号 P2b;浏览器级快捷键,不进页面)。
        if (meta || ctrl) && code == 37 {
            send_cmd("{\"cmd\":\"focus_addrbar\"}");
            return;
        }
        // ⌘/Ctrl+F:页面查找(39 号 P2c;转 find_toggle,不进页面)。
        if (meta || ctrl) && code == 3 {
            send_cmd("{\"cmd\":\"find_toggle\"}");
            return;
        }
        // P2d:浏览器级缩放 —— ⌘/Ctrl + =/+ 放大(键码 24)、- 缩小(27)、
        // 0 复位(29);Chrome 语义,查看器侧生效,不进页面。
        if (meta || ctrl) && matches!(code, 24 | 27 | 29) {
            let dir = match code {
                27 => -1,
                29 => 0,
                _ => 1,
            };
            apply_zoom(dir);
            return;
        }
        let shift = flags & NSEVENT_SHIFT != 0;
        let alt = flags & NSEVENT_OPTION != 0;
        let key = match mac_special_key(code) {
            Some(k) => k.to_string(),
            None => {
                let chars = send_id(event, sel(b"charactersIgnoringModifiers\0"));
                if chars.is_null() {
                    return;
                }
                let ptr = send_id_ptr(chars, sel(b"UTF8String\0"));
                if ptr.is_null() {
                    return;
                }
                let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string();
                // 仅 ASCII 可打印(P0 边界:IME 组合输入未接)。
                let Some(c) = s.chars().next() else { return };
                if !((0x20..=0x7e).contains(&(c as u32))) {
                    return;
                }
                if meta || ctrl {
                    // 组合键只转发剪贴板三键(宿主侧按聚焦控件整值处理),
                    // 其余组合键不产生页面文本。
                    if !matches!(c.to_ascii_lowercase(), 'c' | 'v' | 'x') {
                        return;
                    }
                }
                c.to_string()
            }
        };
        let line = format!(
            "{{\"cmd\":\"input\",\"act\":\"key\",\"key\":\"{}\",\"mods\":{{\"shift\":{shift},\"ctrl\":{ctrl},\"alt\":{alt},\"meta\":{meta}}}}}",
            key_json_str(&key)
        );
        send_input(&line);
        }
    });
}

/// 页面视图可成为第一响应者(接收键盘事件;地址栏编辑时不抢)。
pub(super) extern "C" fn on_pv_accepts_first_responder(_self: Id, _cmd: Sel) -> i8 {
    1
}

/// 光标形状 → `NSCursor` 类(缺失选择器时返回 null,由调用方降级)。
///
/// 选择器先 `respondsToSelector:` 守卫:`IBeamCursor` 在较老 SDK 上可能缺失
/// (54 号事故教训:不发不存在的消息,foreign exception 会 abort 查看器)。
pub(super) unsafe fn nscursor_for_kind(kind: &str) -> Id {
    let nsc = cls(b"NSCursor\0");
    if nsc.is_null() {
        return std::ptr::null_mut();
    }
    let sel_name: &'static [u8] = match kind {
        "pointer" => b"pointingHandCursor\0",
        "text" => b"IBeamCursor\0",
        _ => b"arrowCursor\0",
    };
    if respond(nsc, sel_name) {
        send_id(nsc, sel(sel_name))
    } else {
        std::ptr::null_mut()
    }
}

/// `resetCursorRects`:整幅页面区挂当前光标(P2c 光标形状跟随)。
///
/// 用 cursor rect 而非 `[NSCursor set]`——后者只在鼠标跟踪上下文(移动/按下)
/// 里可靠,静止悬停时不会生效;cursor rect 由 AppKit 在鼠标进入/移动时自动应用,
/// 宿主改形状后 `invalidateCursorRectsForView:` 触发本方法重挂。
pub(super) extern "C" fn on_pv_reset_cursor_rects(_self: Id, _cmd: Sel) {
    crate::ffi_guard::guard("on_pv_reset_cursor_rects", || unsafe {
        let kind = CURSOR_KIND.lock().unwrap().clone();
        let cur = nscursor_for_kind(&kind);
        if cur.is_null() {
            return;
        }
        let bounds = send_rect(_self, sel(b"bounds\0"));
        send_void_rect_id(_self, sel(b"addCursorRect:cursor:\0"), bounds, cur);
    });
}

/// 注册页面视图类(NSImageView 子类:保留 setImage 绘制,叠加事件捕获)。
pub(super) unsafe fn ensure_page_view_class() -> Id {
    {
        let cached = PAGE_VIEW_CLASS.lock().unwrap().0;
        if !cached.is_null() {
            return cached;
        }
    }
    let page_cls = objc_allocateClassPair(
        cls(b"NSImageView\0"),
        b"LsmBrowserPageView\0".as_ptr().cast(),
        0,
    );
    if page_cls.is_null() {
        // 兜底:子类化失败退回原生 NSImageView(无键鼠转发,窗口仍可用)。
        return cls(b"NSImageView\0");
    }
    let add = |name: &'static [u8], imp: *const c_void, enc: &'static [u8]| {
        class_addMethod(page_cls, sel(name), imp, enc.as_ptr().cast())
    };
    let mut ok = 1;
    ok &= add(
        b"acceptsFirstResponder\0",
        on_pv_accepts_first_responder as extern "C" fn(Id, Sel) -> i8 as *const c_void,
        b"B@:\0",
    );
    // P2c:光标形状跟随(cursor rect 整幅页面区)。
    ok &= add(
        b"resetCursorRects\0",
        on_pv_reset_cursor_rects as extern "C" fn(Id, Sel) as *const c_void,
        b"v@:\0",
    );
    for (name, imp) in [
        (b"mouseDown:\0".as_slice(), on_pv_mouse_down as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"mouseUp:\0".as_slice(), on_pv_mouse_up as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"mouseDragged:\0".as_slice(), on_pv_dragged as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"mouseMoved:\0".as_slice(), on_pv_moved as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"rightMouseDown:\0".as_slice(), on_pv_right_down as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"rightMouseUp:\0".as_slice(), on_pv_right_up as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"otherMouseDown:\0".as_slice(), on_pv_other_down as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"otherMouseUp:\0".as_slice(), on_pv_other_up as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"scrollWheel:\0".as_slice(), on_pv_scroll as extern "C" fn(Id, Sel, Id) as *const c_void),
        (b"keyDown:\0".as_slice(), on_pv_key_down as extern "C" fn(Id, Sel, Id) as *const c_void),
    ] {
        ok &= add(name, imp, b"v@:@\0");
    }
    objc_registerClassPair(page_cls);
    let final_cls = if ok == 0 { cls(b"NSImageView\0") } else { page_cls };
    *PAGE_VIEW_CLASS.lock().unwrap() = IdCell(final_cls);
    final_cls
}
