//! `human_input` 单元测试:协议解析、键名归一化、焦点循环纯函数,
//! 以及 data: 页面上的坐标命中(含 fixed/常规流双假设与顶层浮层优先)。

use super::*;
use super::text_edit;
use crate::agent::browser::document::CaretState;
use crate::agent::browser::render::FontSet;
use crate::agent::browser::{BrowserMode, LsmBrowserManager, NavHints};
use crate::agent::web_control::{self, ControlState, Engine};

fn v(s: &str) -> Value {
    serde_json::from_str(s).unwrap()
}

fn html_doc(body: &str) -> String {
    format!("data:text/html,<html><head><title>T</title></head><body>{body}</body></html>")
}

// ---------- 协议解析 ----------

#[test]
fn 解析输入事件_鼠标三动作() {
    assert_eq!(
        parse_event(&v(r#"{"cmd":"input","act":"move","x":10.5,"y":20}"#)),
        Some(HumanInput::Move { x: 10.5, y: 20.0 })
    );
    assert_eq!(
        parse_event(&v(r#"{"cmd":"input","act":"down","x":1,"y":2,"btn":"left"}"#)),
        Some(HumanInput::Down {
            x: 1.0,
            y: 2.0,
            button: MouseButton::Left,
            mods: Mods::default(),
        })
    );
    assert_eq!(
        parse_event(&v(
            r#"{"cmd":"input","act":"up","x":3,"y":4,"btn":"right","mods":{"ctrl":true}}"#
        )),
        Some(HumanInput::Up {
            x: 3.0,
            y: 4.0,
            button: MouseButton::Right,
            mods: Mods { ctrl: true, ..Default::default() },
        })
    );
}

#[test]
fn 解析输入事件_滚轮与键盘() {
    assert_eq!(
        parse_event(&v(r#"{"cmd":"input","act":"wheel","x":5,"y":6,"dx":0,"dy":-1}"#)),
        Some(HumanInput::Wheel { x: 5.0, y: 6.0, dx: 0.0, dy: -1.0 })
    );
    // dx 缺省 = 0(纯纵向滚轮)。
    assert_eq!(
        parse_event(&v(r#"{"cmd":"input","act":"wheel","x":5,"y":6,"dy":1}"#)),
        Some(HumanInput::Wheel { x: 5.0, y: 6.0, dx: 0.0, dy: 1.0 })
    );
    assert_eq!(
        parse_event(&v(r#"{"cmd":"input","act":"key","key":"a","mods":{"shift":true}}"#)),
        Some(HumanInput::Key {
            key: "a".to_string(),
            mods: Mods { shift: true, ..Default::default() },
        })
    );
}

#[test]
fn 解析输入事件_非法行一律忽略() {
    // 非 input 命令 / 缺字段 / 类型错 / 键名超长 / 键位名非法。
    assert_eq!(parse_event(&v(r#"{"cmd":"resize","w":1,"h":1}"#)), None);
    assert_eq!(parse_event(&v(r#"{"cmd":"input"}"#)), None);
    assert_eq!(parse_event(&v(r#"{"cmd":"input","act":"move"}"#)), None);
    assert_eq!(parse_event(&v(r#"{"cmd":"input","act":"move","x":1}"#)), None);
    assert_eq!(parse_event(&v(r#"{"cmd":"input","act":"down","x":1,"y":2,"btn":"side"}"#)), None);
    assert_eq!(parse_event(&v(r#"{"cmd":"input","act":"key","key":""}"#)), None);
    assert_eq!(
        parse_event(&v(r#"{"cmd":"input","act":"key","key":"aaaaaaaaaaaaaaaaaaaaaaaaa"}"#)),
        None
    );
    assert_eq!(parse_event(&v(r#"{"cmd":"input","act":"fly"}"#)), None);
}

// ---------- 键名归一化与字符补正 ----------

#[test]
fn 键名归一化_别名与大小写() {
    assert_eq!(normalize_key("ENTER"), "Enter");
    assert_eq!(normalize_key("esc"), "Escape");
    assert_eq!(normalize_key("ArrowLeft"), "ArrowLeft");
    assert_eq!(normalize_key("left"), "ArrowLeft");
    assert_eq!(normalize_key(" "), "Space");
    assert_eq!(normalize_key("space"), "Space");
    assert_eq!(normalize_key("PageDown"), "PageDown");
    // 单字符原样保留。
    assert_eq!(normalize_key("a"), "a");
    assert_eq!(normalize_key("5"), "5");
}

#[test]
fn shift字符大小写补正() {
    assert_eq!(shift_char('a'), 'A');
    assert_eq!(shift_char('A'), 'A');
    assert_eq!(shift_char('5'), '5');
}

#[test]
fn 数值格式化_整数与浮点() {
    assert_eq!(format_num(50.0), "50");
    assert_eq!(format_num(49.5), "49.500");
}

// ---------- 阶段二:事件载荷与剪贴板 ----------

#[test]
fn 遗留键码映射() {
    assert_eq!(legacy_key_code("Enter"), 13);
    assert_eq!(legacy_key_code("Tab"), 9);
    assert_eq!(legacy_key_code("Escape"), 27);
    assert_eq!(legacy_key_code("Backspace"), 8);
    assert_eq!(legacy_key_code("ArrowLeft"), 37);
    assert_eq!(legacy_key_code("PageDown"), 34);
    assert_eq!(legacy_key_code("Space"), 32);
    // 字母取大写 ASCII、数字原值。
    assert_eq!(legacy_key_code("a"), 65);
    assert_eq!(legacy_key_code("A"), 65);
    assert_eq!(legacy_key_code("5"), 53);
    assert_eq!(legacy_key_code("什么"), 0, "未知键位归 0");
}

#[test]
fn js键名_空格与shift大小写() {
    assert_eq!(js_key_name("Space", &Mods::default()), " ");
    assert_eq!(js_key_name("a", &Mods { shift: true, ..Default::default() }), "A");
    assert_eq!(js_key_name("a", &Mods::default()), "a");
    assert_eq!(js_key_name("Enter", &Mods::default()), "Enter");
}

#[test]
fn 键盘载荷_字段齐全() {
    let p = key_payload("a", &Mods { shift: true, ..Default::default() });
    assert_eq!(p["key"], "A");
    assert_eq!(p["keyCode"], 65);
    assert_eq!(p["which"], 65);
    assert_eq!(p["shiftKey"], true);
    assert_eq!(p["repeat"], false);
    let p = key_payload("Enter", &Mods::default());
    assert_eq!(p["key"], "Enter");
    assert_eq!(p["charCode"], 0, "非可打印键 charCode=0");
}

#[test]
fn 鼠标载荷_坐标与按键位() {
    let p = mouse_payload(10.4, 20.6, MouseButton::Left, true, 1);
    assert_eq!(p["clientX"].as_f64(), Some(10.0));
    assert_eq!(p["clientY"].as_f64(), Some(21.0));
    assert_eq!(p["button"].as_i64(), Some(0));
    assert_eq!(p["buttons"].as_i64(), Some(1));
    assert_eq!(p["detail"].as_i64(), Some(1));
    assert_eq!(
        mouse_payload(0.0, 0.0, MouseButton::Right, false, 0)["button"].as_i64(),
        Some(2)
    );
    assert_eq!(
        mouse_payload(0.0, 0.0, MouseButton::Middle, false, 0)["button"].as_i64(),
        Some(1)
    );
}

#[test]
fn 剪贴板操作判定() {
    let ctrl = Mods { ctrl: true, ..Default::default() };
    let meta = Mods { meta: true, ..Default::default() };
    let plain = Mods::default();
    let alt = Mods { alt: true, ..Default::default() };
    assert_eq!(clip_op_for("c", &ctrl), Some(ClipOp::Copy));
    assert_eq!(clip_op_for("C", &meta), Some(ClipOp::Copy));
    assert_eq!(clip_op_for("v", &ctrl), Some(ClipOp::Paste));
    assert_eq!(clip_op_for("x", &meta), Some(ClipOp::Cut));
    assert_eq!(clip_op_for("c", &plain), None, "无修饰键是普通字符");
    assert_eq!(clip_op_for("c", &alt), None, "纯 Alt 不触发剪贴板");
    assert_eq!(clip_op_for("r", &ctrl), None, "Ctrl+R 在查看器层拦截");
}

// ---------- 焦点循环 ----------

#[test]
fn 焦点循环_前进后退与回绕() {
    let list = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    assert_eq!(next_focusable(&list, None, true).as_deref(), Some("a"));
    assert_eq!(next_focusable(&list, None, false).as_deref(), Some("c"));
    assert_eq!(next_focusable(&list, Some("a"), true).as_deref(), Some("b"));
    assert_eq!(next_focusable(&list, Some("a"), false).as_deref(), Some("c"));
    assert_eq!(next_focusable(&list, Some("c"), true).as_deref(), Some("a"));
    assert_eq!(next_focusable(&list, Some("c"), false).as_deref(), Some("b"));
    // 不在场目标按无焦点处理。
    assert_eq!(next_focusable(&list, Some("zz"), true).as_deref(), Some("a"));
    assert_eq!(next_focusable(&[], Some("a"), true), None);
}

// ---------- 光标形状(P2c) ----------

#[test]
fn 光标形状判定_文本链接按钮与普通元素() {
    let doc = Document::parse(
        "<html><body><a href='/x'>链接</a><button>按钮</button>\
         <input id='t' type='text'><input id='c' type='checkbox'>\
         <p>段落</p><span id='rb' role='button'>角色</span>\
         <div id='oc' onclick='x()'>点我</div></body></html>",
        "https://t.test/",
    );
    let kind = |sel: &str| {
        let node = doc
            .select(sel)
            .ok()
            .and_then(|v| v.into_iter().next())
            .expect(sel);
        let tag = crate::agent::browser::document::tag_name(&node);
        let typ = crate::agent::browser::document::attr(&node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        cursor_kind_for_node(&node, &tag, &typ)
    };
    assert_eq!(kind("a"), CursorKind::Pointer, "链接=手型");
    assert_eq!(kind("button"), CursorKind::Pointer, "按钮=手型");
    assert_eq!(kind("#t"), CursorKind::Text, "文本输入=I 型");
    assert_eq!(kind("#c"), CursorKind::Pointer, "复选框=手型");
    assert_eq!(kind("p"), CursorKind::Default, "段落=箭头");
    assert_eq!(kind("#rb"), CursorKind::Pointer, "role=button=手型");
    assert_eq!(kind("#oc"), CursorKind::Pointer, "onclick=手型");
    // 协议取值固定(帧 kind 'k' 的 payload)。
    assert_eq!(CursorKind::Default.as_str(), "default");
    assert_eq!(CursorKind::Pointer.as_str(), "pointer");
    assert_eq!(CursorKind::Text.as_str(), "text");
}

// ---------- 坐标命中(data: 页面,无外网) ----------

async fn open_hidden(mgr: &LsmBrowserManager, body: &str) -> String {
    let url = html_doc(body);
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(800), Some(600), false, true, 5_000, &NavHints::default())
        .await
        .unwrap();
    out.page_id
}

#[tokio::test]
async fn 命中测试_常规流与嵌套子元素() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<div style='padding:40px'><button id='b1' style='width:120px;height:40px'>OK</button></div>",
    )
    .await;
    let hit = mgr.human_hit_test(&pid, 90.0, 55.0).await.expect("按钮区应命中");
    assert_eq!(hit.tag, "button", "面积最小的候选应是 button 而非外层 div");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 命中测试_fixed元素按视口坐标() {
    let mgr = LsmBrowserManager::new();
    // fixed 头部 + 长内容;先把页面滚下去,再点 fixed 头部区域。
    let pid = open_hidden(
        &mgr,
        "<div id='hd' style='position:fixed;top:0;left:0;width:200px;height:50px;background:#eee'>HD</div>\
         <div style='height:3000px'>BODY</div>",
    )
    .await;
    {
        let mut inner = mgr.inner.lock().await;
        let tab = inner.tabs.get_mut(&pid).unwrap();
        tab.scroll_y = 1200.0;
    }
    // fixed 头部在视口 (10,10):若不做 fixed 双假设,常规流换算会命中正文。
    let hit = mgr.human_hit_test(&pid, 10.0, 10.0).await.expect("fixed 头部应命中");
    assert_eq!(hit.tag, "div");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 命中测试_滚动后的常规流内容() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<div style='height:2000px'></div><a id='lk' href='/x' style='display:block;width:200px;height:30px'>LINK</a>",
    )
    .await;
    {
        let mut inner = mgr.inner.lock().await;
        let tab = inner.tabs.get_mut(&pid).unwrap();
        tab.scroll_y = 1990.0;
    }
    // 链接页坐标约在 y≈2008(body 默认 margin 8 + div 2000);视口坐标 ≈ 18..48。
    let hit = mgr.human_hit_test(&pid, 100.0, 30.0).await.expect("滚动后链接应命中");
    assert_eq!(hit.tag, "a");
    mgr.shutdown().await;
}

// ---------- 控制权闸门 ----------

#[tokio::test]
async fn agent态人工输入被闸门忽略_user态放行() {
    // web_control 进程级注册表:串行执行避免与其它用例竞态。
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(&mgr, "<button id='b1' style='width:100px;height:40px'>OK</button>").await;
    web_control::set_state(Engine::Lsm, &pid, ControlState::Agent, None, "test");
    let out = mgr
        .handle_human_input(&pid, HumanInput::Key { key: "a".to_string(), mods: Mods::default() })
        .await;
    assert_eq!(out["ignored"], true, "agent 态输入应被忽略");
    assert_eq!(out["reason"], "agent_control");

    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    // user 态:输入可进管线(页面无聚焦文本控件,单字符被忽略但原因不同)。
    let out = mgr
        .handle_human_input(&pid, HumanInput::Key { key: "a".to_string(), mods: Mods::default() })
        .await;
    assert!(out.get("ignored").is_none() || out["reason"] != "agent_control");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}

#[tokio::test]
async fn 键入文本进入聚焦控件() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<input id='q' style='width:200px;height:30px' value='ab'>",
    )
    .await;
    // 人工输入只在用户控制态放行(默认 agent 态会被闸门忽略)。
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    // 先聚焦输入框(模拟点击聚焦)。
    let hit = mgr.human_hit_test(&pid, 100.0, 15.0).await.expect("输入框应命中");
    mgr.handle_human_input(
        &pid,
        HumanInput::Down {
            x: 100.0,
            y: 15.0,
            button: MouseButton::Left,
            mods: Mods::default(),
        },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up {
            x: 100.0,
            y: 15.0,
            button: MouseButton::Left,
            mods: Mods::default(),
        },
    )
    .await;
    // 追加 'c'(shift 大写补正走 'd'→'D' 一并验证)。
    mgr.handle_human_input(
        &pid,
        HumanInput::Key { key: "c".to_string(), mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Key { key: "d".to_string(), mods: Mods { shift: true, ..Default::default() } },
    )
    .await;
    // Backspace 删掉最后一个字符。
    mgr.handle_human_input(
        &pid,
        HumanInput::Key { key: "Backspace".to_string(), mods: Mods::default() },
    )
    .await;
    let value = mgr.current_value(&pid, &hit.selector).await;
    assert_eq!(value, "abc", "应为 ab + c(追加)+ D(shift 大写)- D(回删)");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}

// ---------- P2a:文本光标 / 选区 / 编辑语义(text_edit 纯逻辑) ----------

#[test]
fn 光标映射_单行点击取最近字符边界() {
    let font = FontSet::fallback();
    let spans = font.char_spans("abc", 16.0, 1000.0);
    assert_eq!(spans.len(), 3);
    assert_eq!(text_edit::index_from_spans(&spans, -5.0), 0);
    assert_eq!(text_edit::index_from_spans(&spans, spans[0].0), 0);
    assert_eq!(text_edit::index_from_spans(&spans, spans[0].1), 1, "第一字右缘 = 字符间边界");
    assert_eq!(text_edit::index_from_spans(&spans, spans[1].0 + 0.01), 1);
    assert_eq!(text_edit::index_from_spans(&spans, 999.0), 3, "超出文本宽夹到末尾");
}

#[test]
fn 光标映射_textarea行列与中文索引() {
    let font = FontSet::fallback();
    let rows = font.wrap_text_rows("你好\n世界", 16.0, 1000.0, 10);
    assert_eq!(rows, vec![("你好".to_string(), 0), ("世界".to_string(), 3)]);
    let line_h = 16.0 * 1.35;
    // 点第 2 行第 1 字中点 → 索引 4(行首 3 + 列 1)。
    let idx = text_edit::index_from_rows(&font, &rows, 16.0, 8.0, line_h + 2.0, line_h);
    assert_eq!(idx, 4);
    // 点第 1 行空白右侧之外 → 第 1 行行尾。
    let idx = text_edit::index_from_rows(&font, &rows, 16.0, 999.0, 1.0, line_h);
    assert_eq!(idx, 2);
}

#[test]
fn 换行行集_折行与行首索引对齐() {
    let font = FontSet::fallback();
    // 16px 回退字宽 8.8:行宽 20 只容 2 字符 → "abcd" 折两行。
    let rows = font.wrap_text_rows("abcd", 16.0, 20.0, 10);
    assert_eq!(rows, vec![("ab".to_string(), 0), ("cd".to_string(), 2)]);
    // 尾随 \n 产生空尾行(编辑语义:回车后的空行可落光标)。
    let rows = font.wrap_text_rows("a\n", 16.0, 100.0, 10);
    assert_eq!(rows, vec![("a".to_string(), 0), ("".to_string(), 2)]);
    // 行数封顶。
    let rows = font.wrap_text_rows("a\nb\nc", 16.0, 100.0, 2);
    assert_eq!(rows.len(), 2);
}

#[test]
fn 词界_双击选词与词跳() {
    let chars: Vec<char> = "hello world 你好".chars().collect();
    assert_eq!(text_edit::word_bounds(&chars, 1), (0, 5));
    assert_eq!(text_edit::word_bounds(&chars, 4), (0, 5), "词尾点选同词");
    assert_eq!(text_edit::word_bounds(&chars, 7), (6, 11));
    assert_eq!(text_edit::word_bounds(&chars, 5), (6, 11), "双击空白向右选相邻词");
    assert_eq!(text_edit::word_bounds(&chars, 12), (12, 14), "CJK 词按字类连跑");
    // 双击空白:向右优先选相邻词。
    assert_eq!(text_edit::word_bounds(&chars, 11), (12, 14));
    // 词跳:向左跳到词首,向右到下一词首。
    assert_eq!(text_edit::word_jump(&chars, 5, -1), 0);
    assert_eq!(text_edit::word_jump(&chars, 5, 1), 6);
    assert_eq!(text_edit::word_jump(&chars, 11, 1), 12);
}

#[test]
fn 编辑运算_插入替换选区与前后删除() {
    let c = CaretState { pos: 4, anchor: 1 }; // "hello" 选中 "ell"
    let (nv, nc) = text_edit::edit_insert("hello", c, "X");
    assert_eq!(nv, "hXo");
    assert_eq!(nc, CaretState::collapsed(2));
    assert_eq!(text_edit::edit_backspace("abc", CaretState::collapsed(2)).0, "ac");
    assert_eq!(text_edit::edit_delete("abc", CaretState::collapsed(1)).0, "ac");
    assert_eq!(text_edit::edit_backspace("abc", CaretState::collapsed(0)).0, "abc", "0 处 no-op");
    assert_eq!(text_edit::edit_delete("abc", CaretState::collapsed(3)).0, "abc", "尾部 no-op");
    // 有选区:Backspace/Delete 均删选区。
    let c = CaretState { pos: 1, anchor: 3 };
    assert_eq!(text_edit::edit_backspace("abcd", c).0, "ad");
    assert_eq!(text_edit::edit_delete("abcd", c).0, "ad");
    // 中文按 char 切,不裂。
    assert_eq!(text_edit::edit_backspace("你好", CaretState::collapsed(2)).0, "你");
    // Ctrl+Backspace 删词。
    assert_eq!(
        text_edit::edit_delete_word_back("hello world", CaretState::collapsed(11)).0,
        "hello "
    );
    assert_eq!(
        text_edit::edit_delete_word_back("hello world", CaretState::collapsed(5)).0,
        " world"
    );
}

#[test]
fn 光标移动_折叠扩选与词跳() {
    let v = "hello world";
    let c = CaretState { pos: 4, anchor: 1 }; // 选区 1..4
    assert_eq!(text_edit::caret_move(v, c, -1, false, false), CaretState::collapsed(1));
    assert_eq!(text_edit::caret_move(v, c, 1, false, false), CaretState::collapsed(4));
    let m = text_edit::caret_move(v, c, 1, false, true);
    assert_eq!((m.pos, m.anchor), (5, 1), "Shift 扩选锚点不动");
    assert_eq!(text_edit::caret_move(v, CaretState::collapsed(5), -1, true, false).pos, 0);
    assert_eq!(text_edit::caret_move(v, CaretState::collapsed(0), 1, true, false).pos, 6);
}

#[test]
fn 光标移动_上下按行与行首行尾() {
    let font = FontSet::fallback();
    let v = "abc\nde";
    let rows = font.wrap_text_rows(v, 16.0, 1000.0, 10);
    assert_eq!(rows, vec![("abc".to_string(), 0), ("de".to_string(), 4)]);
    // 第 0 行 col 2 → 第 1 行 col 2(超出行尾夹到行尾)。
    let c = text_edit::caret_move_vert(v, CaretState::collapsed(2), &rows, 1, false);
    assert_eq!(c.pos, 6, "第 1 行行尾");
    // 首行向上 no-op。
    assert_eq!(
        text_edit::caret_move_vert(v, CaretState::collapsed(2), &rows, -1, false),
        CaretState::collapsed(2)
    );
    // 行首/行尾(非 doc)。
    assert_eq!(text_edit::caret_to_edge(v, CaretState::collapsed(5), &rows, false, false, false).pos, 4);
    assert_eq!(text_edit::caret_to_edge(v, CaretState::collapsed(5), &rows, true, false, false).pos, 6);
    // doc 边缘与扩选。
    assert_eq!(text_edit::caret_to_edge(v, CaretState::collapsed(5), &rows, false, true, false).pos, 0);
    assert_eq!(
        text_edit::caret_to_edge(v, CaretState::collapsed(2), &rows, true, true, true),
        CaretState { pos: 6, anchor: 2 }
    );
}

#[test]
fn 选区构造_双击选词全选与取串() {
    assert_eq!(text_edit::select_word("hello world", 7), CaretState { pos: 11, anchor: 6 });
    assert_eq!(text_edit::select_all("你好"), CaretState { pos: 2, anchor: 0 });
    assert_eq!(text_edit::selected_text("hello", CaretState { pos: 4, anchor: 1 }).as_deref(), Some("ell"));
    assert_eq!(text_edit::selected_text("hello", CaretState::collapsed(2)), None);
}

#[test]
fn 撤销重做_双栈往返与断链() {
    let mut h = text_edit::EditHistory::default();
    h.push("ab".to_string(), CaretState::collapsed(2));
    h.push("abc".to_string(), CaretState::collapsed(3));
    assert_eq!(h.depth(), (2, 0));
    let (v1, _) = h.undo("abcd".to_string(), CaretState::collapsed(4)).unwrap();
    assert_eq!(v1, "abc");
    assert_eq!(h.depth(), (1, 1));
    let (v2, _) = h.undo("abc".to_string(), CaretState::collapsed(3)).unwrap();
    assert_eq!(v2, "ab");
    assert!(h.undo("ab".to_string(), CaretState::collapsed(2)).is_none(), "栈空撤销 no-op");
    let (v3, _) = h.redo("ab".to_string(), CaretState::collapsed(2)).unwrap();
    assert_eq!(v3, "abc");
    // 新编辑断开 redo 链。
    h.push("abx".to_string(), CaretState::collapsed(3));
    assert_eq!(h.depth().1, 0);
}

#[test]
fn 编辑组合键判定() {
    let ctrl = Mods { ctrl: true, ..Default::default() };
    let shift = Mods { shift: true, ..Default::default() };
    let cs = Mods { ctrl: true, shift: true, ..Default::default() };
    assert_eq!(edit_op_for("a", &ctrl), Some(EditOp::SelectAll));
    assert_eq!(edit_op_for("A", &ctrl), Some(EditOp::SelectAll));
    assert_eq!(edit_op_for("z", &ctrl), Some(EditOp::Undo));
    assert_eq!(edit_op_for("z", &cs), Some(EditOp::Redo));
    assert_eq!(edit_op_for("y", &ctrl), Some(EditOp::Redo));
    assert_eq!(edit_op_for("a", &shift), None, "纯 Shift 是普通字符");
    assert_eq!(edit_op_for("a", &Mods::default()), None);
}

#[tokio::test]
async fn 点击定位光标_键入插入中间() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<input id='q' style='width:200px;height:30px' value='abcd'>",
    )
    .await;
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    let hit = mgr.human_hit_test(&pid, 100.0, 15.0).await.expect("输入框应命中");
    assert!(is_text_like(&hit.tag, &hit.typ), "输入框应判为文本控件");
    // 文本起排 = 盒左 + 3;点在第 1 字符右缘稍右 → 光标落索引 1。
    let x = hit.rect.0 + 3.0 + mgr.font().char_width('a', hit.font_size) + 0.5;
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x, y: 15.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up { x, y: 15.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Key { key: "X".to_string(), mods: Mods::default() })
        .await;
    let value = mgr.current_value(&pid, &hit.selector).await;
    assert_eq!(value, "aXbcd", "键入应插入光标处而非尾部");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}

#[tokio::test]
async fn 拖选与双击选词_后键入替换() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<input id='q' style='width:200px;height:30px' value='hello world'>",
    )
    .await;
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    let hit = mgr.human_hit_test(&pid, 100.0, 15.0).await.expect("输入框应命中");
    let size = hit.font_size;
    let x0 = hit.rect.0 + 3.0;
    let x_of = |s: &str, i: usize| -> f64 {
        let w: f64 = s.chars().take(i).map(|c| mgr.font().char_width(c, size)).sum();
        x0 + w + 0.5
    };
    // 拖选 'e'..'l'(索引 1..3 → 覆盖 "el")。
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: x_of("hello world", 1), y: 15.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Move { x: x_of("hello world", 3), y: 15.0 },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: x_of("hello world", 3), y: 15.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Key { key: "Z".to_string(), mods: Mods::default() })
        .await;
    let value = mgr.current_value(&pid, &hit.selector).await;
    assert_eq!(value, "hZlo world", "拖选段应被键入替换");
    // 双击选词 'world' 并替换。
    for _ in 0..2 {
        mgr.handle_human_input(
            &pid,
            HumanInput::Down { x: x_of("hZlo world", 8), y: 15.0, button: MouseButton::Left, mods: Mods::default() },
        )
        .await;
        mgr.handle_human_input(
            &pid,
            HumanInput::Up { x: x_of("hZlo world", 8), y: 15.0, button: MouseButton::Left, mods: Mods::default() },
        )
        .await;
    }
    mgr.handle_human_input(&pid, HumanInput::Key { key: "Y".to_string(), mods: Mods::default() })
        .await;
    let value = mgr.current_value(&pid, &hit.selector).await;
    assert_eq!(value, "hZlo Y", "双击词应被替换");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}

#[tokio::test]
async fn ctrl组合键_全选替换与撤销重做() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<input id='q' style='width:200px;height:30px' value='abc'>",
    )
    .await;
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    let hit = mgr.human_hit_test(&pid, 100.0, 15.0).await.expect("输入框应命中");
    let x = hit.rect.0 + 3.0 + 1.0;
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x, y: 15.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up { x, y: 15.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    // Ctrl+A 全选 → 键入替换。
    let ctrl = Mods { ctrl: true, ..Default::default() };
    mgr.handle_human_input(&pid, HumanInput::Key { key: "a".to_string(), mods: ctrl }).await;
    mgr.handle_human_input(&pid, HumanInput::Key { key: "X".to_string(), mods: Mods::default() })
        .await;
    assert_eq!(mgr.current_value(&pid, &hit.selector).await, "X");
    // Ctrl+Z 撤销回 abc,Ctrl+Y 重做回 X。
    mgr.handle_human_input(&pid, HumanInput::Key { key: "z".to_string(), mods: ctrl }).await;
    assert_eq!(mgr.current_value(&pid, &hit.selector).await, "abc", "撤销应恢复旧值");
    mgr.handle_human_input(&pid, HumanInput::Key { key: "y".to_string(), mods: ctrl }).await;
    assert_eq!(mgr.current_value(&pid, &hit.selector).await, "X", "重做应恢复新值");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}

// ---------- 39 号 P2b:页面级自由选区与选区脚本化 ----------

#[test]
fn 页面选区归一化_同元素与跨元素() {
    let doc = crate::agent::browser::document::Document::parse(
        "<html><body><p id='a'>甲</p><p id='b'>乙</p><p id='c'>丙</p></body></html>",
        "about:blank",
    );
    // 同元素:索引反向 → 升序。
    let s = crate::agent::browser::document::PageSelection {
        anchor_sel: "#a".to_string(),
        anchor_idx: 3,
        head_sel: "#a".to_string(),
        head_idx: 1,
    };
    assert_eq!(s.ordered(&doc), ("#a".to_string(), 1, "#a".to_string(), 3));
    // 跨元素:head 在 anchor 之前(DOM 序)→ 交换两端。
    let s = crate::agent::browser::document::PageSelection {
        anchor_sel: "#c".to_string(),
        anchor_idx: 0,
        head_sel: "#a".to_string(),
        head_idx: 2,
    };
    assert_eq!(s.ordered(&doc), ("#a".to_string(), 2, "#c".to_string(), 0));
    // 空选区判定。
    let empty = crate::agent::browser::document::PageSelection {
        anchor_sel: "#b".to_string(),
        anchor_idx: 1,
        head_sel: "#b".to_string(),
        head_idx: 1,
    };
    assert!(empty.is_empty());
}

#[test]
fn 页面选区文本_跨元素截取() {
    let doc = crate::agent::browser::document::Document::parse(
        "<html><body><p id='a'>甲甲甲</p><p id='b'>乙乙</p><p id='c'>丙丙丙</p></body></html>",
        "about:blank",
    );
    // 文本 = "甲甲甲\n乙乙\n丙丙丙";取 [1, 7) → "甲甲\n乙乙\n丙"。
    let s = crate::agent::browser::document::PageSelection {
        anchor_sel: "#a".to_string(),
        anchor_idx: 1,
        head_sel: "#c".to_string(),
        head_idx: 1,
    };
    assert_eq!(s.text_in(&doc), "甲甲\n乙乙\n丙");
    // 单向:同元素截取。
    let s = crate::agent::browser::document::PageSelection {
        anchor_sel: "#b".to_string(),
        anchor_idx: 0,
        head_sel: "#b".to_string(),
        head_idx: 1,
    };
    assert_eq!(s.text_in(&doc), "乙");
    // 空选区 → 空串。
    let s = crate::agent::browser::document::PageSelection {
        anchor_sel: "#a".to_string(),
        anchor_idx: 2,
        head_sel: "#a".to_string(),
        head_idx: 2,
    };
    assert_eq!(s.text_in(&doc), "");
}

#[test]
fn 页面可选文本元素判定() {
    assert!(is_page_selectable("p", ""));
    assert!(is_page_selectable("span", ""));
    assert!(is_page_selectable("td", ""));
    assert!(!is_page_selectable("div", ""), "布局容器不参与(文本是子代之和)");
    assert!(!is_page_selectable("input", "text"), "控件内选区归 text_drag");
    assert!(!is_page_selectable("html", ""));
    assert!(!is_page_selectable("body", ""));
}

#[test]
fn 页面选区属性写入与解析() {
    use crate::agent::browser::document::{Document, PageSelection, ATTR_PAGE_SEL};
    let mut doc = Document::parse(
        "<html><body><p id='a'>甲甲甲</p><p id='b'>乙乙</p></body></html>",
        "about:blank",
    );
    doc.apply_page_sel(&PageSelection {
        anchor_sel: "#a".to_string(),
        anchor_idx: 1,
        head_sel: "#a".to_string(),
        head_idx: 2,
    });
    let a = doc.select("#a").unwrap().into_iter().next().unwrap();
    assert_eq!(crate::agent::browser::document::attr(&a, ATTR_PAGE_SEL), Some("1,2".to_string()));
    // 跨元素:首元素 [start,末尾)、尾元素 [0,end)。
    let mut doc = Document::parse(
        "<html><body><p id='a'>甲甲甲</p><p id='b'>乙乙</p></body></html>",
        "about:blank",
    );
    doc.apply_page_sel(&PageSelection {
        anchor_sel: "#a".to_string(),
        anchor_idx: 1,
        head_sel: "#b".to_string(),
        head_idx: 1,
    });
    let a = doc.select("#a").unwrap().into_iter().next().unwrap();
    let b = doc.select("#b").unwrap().into_iter().next().unwrap();
    assert_eq!(crate::agent::browser::document::attr(&a, ATTR_PAGE_SEL), Some("1,-1".to_string()));
    assert_eq!(crate::agent::browser::document::attr(&b, ATTR_PAGE_SEL), Some("0,1".to_string()));
}

#[tokio::test]
async fn 页面自由选区拖选_提交与清空() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<p id='a' style='font-size:16px'>甲甲甲甲甲</p><p id='b' style='font-size:16px'>乙乙乙乙乙</p>",
    )
    .await;
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    // 首个 p 的盒:y≈16..39(body margin 8 + p margin-top 8),中心 y≈27。
    let (y, x0) = {
        let hit = mgr.human_hit_test(&pid, 20.0, 27.0).await.expect("p 应命中");
        assert_eq!(hit.tag, "p");
        (hit.rect.1 + hit.rect.3 / 2.0, hit.rect.0 + 6.0)
    };
    // 按下 → 移动(越过 6px 阈值)→ 抬起:应提交 page_sel。
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: x0, y, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Move { x: x0 + 120.0, y },
    )
    .await;
    let up = mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: x0 + 120.0, y, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    assert_eq!(up["page_selection"], true, "拖选应收官:{up}");
    let sel = {
        let inner = mgr.inner.lock().await;
        inner.tabs.get(&pid).and_then(|t| t.page_sel.clone())
    };
    assert!(sel.is_some(), "page_sel 应已提交");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}

#[tokio::test]
async fn disabled控件人工点击不分发不聚焦() {
    let mgr = LsmBrowserManager::new();
    let pid = open_hidden(
        &mgr,
        "<button id='d' disabled style='width:120px;height:40px'>禁用</button><span id='out'></span>",
    )
    .await;
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    let (x, y) = {
        let hit = mgr.human_hit_test(&pid, 20.0, 20.0).await.expect("按钮应命中");
        (hit.rect.0 + hit.rect.2 / 2.0, hit.rect.1 + hit.rect.3 / 2.0)
    };
    let down = mgr.handle_human_input(
        &pid,
        HumanInput::Down { x, y, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    assert_eq!(down["disabled"], true, "disabled 应被闸住:{down}");
    let focused = {
        let inner = mgr.inner.lock().await;
        inner.tabs.get(&pid).and_then(|t| t.focused.clone())
    };
    assert!(focused.is_none(), "disabled 控件不应被聚焦");
    web_control::clear(Engine::Lsm, &pid);
    mgr.shutdown().await;
}









// ===================== P2d:datalist 过滤与多选语义(纯函数) =====================

#[test]
fn datalist过滤_包含匹配大小写与空查询全量() {
    let opts = vec![
        ("#s1".to_string(), "Apple".to_string(), "苹果".to_string()),
        ("#s2".to_string(), "Banana".to_string(), "香蕉".to_string()),
        ("#s3".to_string(), "app-store".to_string(), String::new()),
    ];
    // 空查询 = 全量。
    assert_eq!(datalist::filter_hits(&opts, "").len(), 3);
    // value 包含匹配,大小写不敏感。
    let hit = datalist::filter_hits(&opts, "APP");
    assert_eq!(hit.len(), 2);
    assert_eq!(hit[0].1, "Apple");
    assert_eq!(hit[1].1, "app-store");
    // label 包含匹配(中文)。
    assert_eq!(datalist::filter_hits(&opts, "香蕉").len(), 1);
    // 无命中。
    assert!(datalist::filter_hits(&opts, "zzz").is_empty());
    // 查询串前后空白不参与匹配。
    assert_eq!(datalist::filter_hits(&opts, " apple ").len(), 1);
}

#[test]
fn datalist过滤_超过上限截断() {
    let opts: Vec<(String, String, String)> = (0..30)
        .map(|i| (format!("#s{i}"), format!("opt-{i}"), String::new()))
        .collect();
    assert_eq!(datalist::filter_hits(&opts, "").len(), datalist::DL_MAX_ROWS);
    assert_eq!(datalist::filter_hits(&opts, "opt-").len(), datalist::DL_MAX_ROWS);
}

#[test]
fn 多选语义_翻转范围与独占() {
    let all: Vec<String> = vec!["a".into(), "b".into(), "c".into(), "d".into()];
    // 普通点击 = 独占。
    assert_eq!(multi_pick(&all, &["b".to_string()], 2, false, false, None), vec!["c"]);
    // Ctrl 翻转:已有 → 移除。
    assert_eq!(
        multi_pick(&all, &["a".to_string(), "b".to_string()], 1, true, false, None),
        vec!["a"]
    );
    // Ctrl 翻转:没有 → 追加(结果按 DOM 序重排)。
    assert_eq!(
        multi_pick(&all, &["c".to_string()], 0, true, false, None),
        vec!["a", "c"]
    );
    // Shift 范围(锚点 1 → 点击 3)。
    assert_eq!(
        multi_pick(&all, &["b".to_string()], 3, false, true, Some(1)),
        vec!["b", "c", "d"]
    );
    // Shift 反向范围(锚点 3 → 点击 1)。
    assert_eq!(
        multi_pick(&all, &[], 1, false, true, Some(3)),
        vec!["b", "c", "d"]
    );
    // Shift 无锚点 = 独占。
    assert_eq!(multi_pick(&all, &[], 1, false, true, None), vec!["b"]);
}

// ---------- P2e:number 微调箭头命中(纯函数) ----------

#[test]
fn number微调箭头命中_上下半区与区外() {
    let rect = (100.0, 50.0, 160.0, 24.0);
    // 右端 13px(x ∈ 247..260):上半(y < 62)= +1、下半 = −1。
    assert_eq!(number_spinner_hit(rect, 250.0, 55.0), Some(1));
    assert_eq!(number_spinner_hit(rect, 250.0, 65.0), Some(-1));
    // 文本区(左端)与盒外:None。
    assert_eq!(number_spinner_hit(rect, 120.0, 55.0), None);
    assert_eq!(number_spinner_hit(rect, 259.9, 55.0), Some(1));
    assert_eq!(number_spinner_hit(rect, 260.5, 55.0), None, "越出右缘");
    assert_eq!(number_spinner_hit(rect, 160.0, 49.0), None, "越出上缘");
    // 控件比箭头区还窄:不判微调(点击整个落在文本区)。
    assert_eq!(number_spinner_hit((0.0, 0.0, 10.0, 24.0), 5.0, 10.0), None);
}
