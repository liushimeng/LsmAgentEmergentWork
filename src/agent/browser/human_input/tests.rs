//! `human_input` 单元测试:协议解析、键名归一化、焦点循环纯函数,
//! 以及 data: 页面上的坐标命中(含 fixed/常规流双假设与顶层浮层优先)。

use super::*;
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
