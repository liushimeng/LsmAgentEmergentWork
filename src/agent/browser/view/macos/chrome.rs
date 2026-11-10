//! 一级 chrome 层:工具栏/地址栏/缩放/主菜单/右键菜单(macOS AppKit FFI)。
//!
//! 从 `macos.rs` 按职责机械搬移(58 号设计文档拆分),逻辑零改写;
//! 共享基础设施(消息发送/静态控件句柄/布局几何)在 `super`(mod.rs),
//! 本模块经 `use super::*` 取用。

use super::*;

/// P2d:标题叠加缩放后缀(≠100% 时 `[125%]`;复位时剥掉旧后缀)。
pub(super) fn zoom_title(raw: &str, z: f64) -> String {
    if (z - 1.0).abs() < 1e-9 {
        raw.to_string()
    } else {
        format!("{} [{}%]", raw, (z * 100.0).round() as i64)
    }
}

/// P2d:调整浏览器级缩放并立即生效(视口按新系数换算重报 + 宿主记账 +
/// 标题后缀)。`dir > 0` 放大、`dir < 0` 缩小、`0` 复位到 100%。
pub(super) unsafe fn apply_zoom(dir: i32) {
    let next = if dir == 0 {
        1.0
    } else {
        super::super::zoom_step(*ZOOM.lock().unwrap(), dir)
    };
    *ZOOM.lock().unwrap() = next;
    send_cmd(&format!("{{\"cmd\":\"zoom\",\"z\":{next}}}"));
    // 视口按新缩放换算重报(置零强制变化检测命中)。
    *CONTENT_SIZE.lock().unwrap() = (0.0, 0.0);
    sync_content_size();
    let w = WINDOW.lock().unwrap().0;
    if !w.is_null() {
        let t = LAST_TITLE.lock().unwrap().clone();
        send_void_id(w, sel(b"setTitle:\0"), ns_str(&zoom_title(&t, next)));
    }
}

/// 地址栏同步:用户不在编辑态才覆盖文本;按钮按可用性置灰。
pub(super) unsafe fn apply_urlbar(url: &str, can_back: bool, can_forward: bool) {
    let field = ADDRESS_FIELD.lock().unwrap().0;
    if std::env::var("LAEW_BROWSER_VIEW_DEBUG").is_ok() {
        eprintln!("[bview] apply_urlbar url={url:?} field={:?} editing={}", field, EDITING.load(Ordering::Relaxed));
    }
    if let Ok(mut last) = LAST_URL.lock() {
        *last = url.to_string();
    }
    if !field.is_null() && !EDITING.load(Ordering::Relaxed) {
        send_void_id(field, sel(b"setStringValue:\0"), ns_str(url));
    }
    let back = BACK_BTN.lock().unwrap().0;
    if !back.is_null() {
        send_void_bool(back, sel(b"setEnabled:\0"), can_back);
    }
    let fwd = FWD_BTN.lock().unwrap().0;
    if !fwd.is_null() {
        send_void_bool(fwd, sel(b"setEnabled:\0"), can_forward);
    }
}

/// 工具栏按钮动作。
macro_rules! button_action {
    ($name:ident, $cmd:expr) => {
        pub(super) extern "C" fn $name(_self: Id, _cmd: Sel, _sender: Id) {
            crate::ffi_guard::guard(stringify!($name), || send_cmd($cmd));
        }
    };
}

button_action!(on_back, "{\"cmd\":\"back\"}");
button_action!(on_forward, "{\"cmd\":\"forward\"}");
button_action!(on_reload, "{\"cmd\":\"reload\"}");
button_action!(on_home, "{\"cmd\":\"home\"}");
// 右键菜单的剪贴板项(39 号设计文档阶段二;宿主侧按聚焦控件整值近似)。
button_action!(on_clip_copy, "{\"cmd\":\"clip\",\"act\":\"copy\"}");
button_action!(on_clip_paste, "{\"cmd\":\"clip\",\"act\":\"paste\"}");

/// 地址栏回车:`control:textView:doCommandBySelector:` 收到 `insertNewline:`。
pub(super) extern "C" fn on_text_command(
    _self: Id,
    _cmd: Sel,
    _control: Id,
    _text_view: Id,
    command: Sel,
) -> i64 {
    let handled = std::cell::Cell::new(0i64);
    crate::ffi_guard::guard("on_text_command", || {
        if command == sel(b"insertNewline:\0") {
            let text = unsafe { address_text() };
            let text = text.trim();
            if !text.is_empty() {
                send_cmd(&cmd_json("navigate", Some(text)));
            }
            handled.set(1);
        }
    });
    handled.get()
}

/// 地址栏开始编辑(此后宿主不同步覆盖文本)。
pub(super) extern "C" fn on_text_begin(_self: Id, _cmd: Sel, _note: Id) {
    crate::ffi_guard::guard("on_text_begin", || {
        EDITING.store(true, Ordering::Relaxed);
    });
}

/// 地址栏结束编辑:回填宿主下发的 URL(清掉误入的外来输入)。
pub(super) extern "C" fn on_text_end(_self: Id, _cmd: Sel, _note: Id) {
    crate::ffi_guard::guard("on_text_end", || {
        EDITING.store(false, Ordering::Relaxed);
        unsafe {
            let field = ADDRESS_FIELD.lock().unwrap().0;
            let url = LAST_URL.lock().map(|s| s.clone()).unwrap_or_default();
            if !field.is_null() {
                send_void_id(field, sel(b"setStringValue:\0"), ns_str(&url));
            }
        }
    });
}

/// 构建并弹出右键菜单(NSMenu + NSMenuItem,action 挂到控制器)。
pub(super) unsafe fn show_context_menu(view: Id, event: Id) {
    let menu = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str("LAEW"),
    );
    if menu.is_null() {
        return;
    }
    // (标题, 动作选择器, 命令) —— 动作直接走 send_cmd,与工具栏按钮同构。
    let items: [(&str, &'static [u8], &str); 6] = [
        ("后退", b"on_back:\0", "{\"cmd\":\"back\"}"),
        ("前进", b"on_forward:\0", "{\"cmd\":\"forward\"}"),
        ("刷新", b"on_reload:\0", "{\"cmd\":\"reload\"}"),
        ("首页", b"on_home:\0", "{\"cmd\":\"home\"}"),
        ("复制", b"on_clip_copy:\0", "{\"cmd\":\"clip\",\"act\":\"copy\"}"),
        ("粘贴", b"on_clip_paste:\0", "{\"cmd\":\"clip\",\"act\":\"paste\"}"),
    ];
    let ctrl = CTRL.lock().unwrap().0;
    for (title, action, _) in items {
        let item = send_id_id_id_id(
            send_id(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
            sel(b"initWithTitle:action:keyEquivalent:\0"),
            ns_str(title),
            sel(action).cast_mut(),
            ns_str(""),
        );
        if item.is_null() {
            continue;
        }
        if !ctrl.is_null() {
            send_void_id(item, sel(b"setTarget:\0"), ctrl);
        }
        send_void_id(menu, sel(b"addItem:\0"), item);
    }
    let loc = send_point(event, sel(b"locationInWindow\0"));
    let local = send_point_point_id(view, sel(b"convertPoint:fromView:\0"), loc, std::ptr::null_mut());
    send_i8_id_point_id(menu, sel(b"popUpMenuPositioningItem:atLocation:inView:\0"), std::ptr::null_mut(), local, view);
}

/// 建一个工具栏按钮。
pub(super) unsafe fn make_button(
    ctrl: Id,
    frame: NSRect,
    title: &str,
    action: Sel,
    tooltip: &str,
) -> Id {
    let btn = send_id_rect(
        send_id(cls(b"NSButton\0"), sel(b"alloc\0")),
        sel(b"initWithFrame:\0"),
        frame,
    );
    if btn.is_null() {
        return btn;
    }
    send_void_id(btn, sel(b"setTitle:\0"), ns_str(title));
    send_void_id(btn, sel(b"setTarget:\0"), ctrl);
    send_void_sel(btn, sel(b"setAction:\0"), action);
    send_void_i64(btn, sel(b"setBezelStyle:\0"), NS_BEZEL_ROUNDED);
    send_void_id(btn, sel(b"setToolTip:\0"), ns_str(tooltip));
    let font = send_id_f64(cls(b"NSFont\0"), sel(b"systemFontOfSize:\0"), 12.0);
    if !font.is_null() {
        send_void_id(btn, sel(b"setFont:\0"), font);
    }
    btn
}

/// 菜单项(标题/动作/快捷键;无修饰键时 AppKit 默认按 ⌘ 解析)。
pub(super) struct MenuItem {
    title: &'static str,
    action: &'static [u8],
    key: &'static str,
}

/// 构建一个 NSMenu:返回挂好 submenu 的顶层菜单项。
pub(super) unsafe fn build_menu(title: &str, items: &[MenuItem]) -> Id {
    let sub = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str(title),
    );
    for it in items {
        let item = send_id_id_id_id(
            send_id(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
            sel(b"initWithTitle:action:keyEquivalent:\0"),
            ns_str(it.title),
            sel(it.action).cast_mut(),
            ns_str(it.key),
        );
        send_void_id(sub, sel(b"addItem:\0"), item);
    }
    let parent = send_id_id_id_id(
        send_id(cls(b"NSMenuItem\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:action:keyEquivalent:\0"),
        ns_str(title),
        std::ptr::null_mut(),
        ns_str(""),
    );
    send_void_id(parent, sel(b"setSubmenu:\0"), sub);
    parent
}

/// 装主菜单:地址栏复制/粘贴/编辑的**系统路径**(与 `human_ui` 弹窗同款)。
///
/// 查看器此前完全没有 NSMenu:⌘C/⌘V/⌘X/⌘A 的 key equivalent 依赖
/// 「菜单项 → responder chain → first responder(地址栏)」这条路径,
/// 无菜单条时这些快捷键行为不可预期,人工没法往地址栏粘贴网址。
pub(super) unsafe fn install_main_menu() {
    let menu = send_id_id(
        send_id(cls(b"NSMenu\0"), sel(b"alloc\0")),
        sel(b"initWithTitle:\0"),
        ns_str("MainMenu"),
    );
    let app_item = build_menu(
        "LAEW 浏览器",
        &[MenuItem { title: "关于浏览器", action: b"orderFrontStandardAboutPanel:\0", key: "" }],
    );
    send_void_id(menu, sel(b"addItem:\0"), app_item);
    let edit_item = build_menu(
        "编辑",
        &[
            MenuItem { title: "剪切", action: b"cut:\0", key: "x" },
            MenuItem { title: "复制", action: b"copy:\0", key: "c" },
            MenuItem { title: "粘贴", action: b"paste:\0", key: "v" },
            MenuItem { title: "全选", action: b"selectAll:\0", key: "a" },
        ],
    );
    send_void_id(menu, sel(b"addItem:\0"), edit_item);
    let app = send_id(cls(b"NSApplication\0"), sel(b"sharedApplication\0"));
    if !app.is_null() {
        send_void_id(app, sel(b"setMainMenu:\0"), menu);
    }
}
