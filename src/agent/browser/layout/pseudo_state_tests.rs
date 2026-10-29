//! 运行时伪类状态回归(第 41 轮)。
//!
//! 覆盖:`:hover`/`:focus`/`:focus-visible`/`:active` 命中,
//! 以及「状态只重绘不重排」—— 状态切换不得改变控件占位尺寸。

use std::collections::HashMap;

use super::super::document::{Document, PseudoFlags};
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn state_doc(html: &str, states: &[(&str, bool, bool)]) -> Document {
    let mut doc = Document::parse(html, "https://a.test/");
    let mut map: HashMap<String, PseudoFlags> = HashMap::new();
    for (sel, hover, focus) in states {
        map.insert(
            (*sel).to_string(),
            PseudoFlags {
                hover: *hover,
                focus: *focus,
                focus_visible: *focus,
                active: false,
            },
        );
    }
    doc.apply_widget_state(&HashMap::new(), &HashMap::new(), &map);
    doc
}

#[test]
fn hover伪类命中运行时悬停态() {
    let doc = state_doc(
        "<head><style>button:hover{background:#ff0000}</style></head><body><button id='b'>按钮</button></body>",
        &[("#b", true, false)],
    );
    let lay = compute(&doc, 800.0, &font());
    let btn = lay
        .boxes
        .iter()
        .find(|b| b.selector == "#b")
        .expect("按钮应有几何盒");
    assert_eq!(
        btn.background,
        Some([255, 0, 0]),
        ":hover 必须命中运行时悬停态"
    );
}

#[test]
fn focus伪类命中运行时焦点态() {
    let doc = state_doc(
        "<head><style>input:focus{border-color:#00ff00}</style></head><body><input id='q'></body>",
        &[("#q", false, true)],
    );
    let lay = compute(&doc, 800.0, &font());
    let input = lay
        .boxes
        .iter()
        .find(|b| b.selector == "#q")
        .expect("输入框应有几何盒");
    // UA 描边色被 :focus 覆盖为绿色。
    let green = lay.borders.iter().any(|b| {
        b.colors
            .iter()
            .any(|([r, g, b], _)| *r == 0 && *g == 255 && *b == 0)
    });
    assert!(green, ":focus 必须命中运行时焦点态");
    let _ = input;
}

#[test]
fn 无悬停态时hover伪类不命中() {
    let doc = state_doc(
        "<head><style>button:hover{background:#ff0000}</style></head><body><button id='b'>按钮</button></body>",
        &[],
    );
    let lay = compute(&doc, 800.0, &font());
    let btn = lay
        .boxes
        .iter()
        .find(|b| b.selector == "#b")
        .expect("按钮应有几何盒");
    assert_ne!(
        btn.background,
        Some([255, 0, 0]),
        "无悬停态时不得命中 :hover"
    );
}

#[test]
fn 状态切换不改变控件占位尺寸() {
    // 铁律:交互态只触发重绘,不得让控件占位变化(否则相邻元素会抖动)。
    let plain = state_doc(
        "<body><input id='q' type='checkbox' style='width:120px;height:30px'></body>",
        &[],
    );
    let hovered = state_doc(
        "<body><input id='q' type='checkbox' style='width:120px;height:30px'></body>",
        &[("#q", true, true)],
    );
    let a = compute(&plain, 800.0, &font());
    let b = compute(&hovered, 800.0, &font());
    let ba = a.boxes.iter().find(|x| x.selector == "#q").unwrap();
    let bb = b.boxes.iter().find(|x| x.selector == "#q").unwrap();
    assert!(
        (ba.w - bb.w).abs() < 0.01 && (ba.h - bb.h).abs() < 0.01 && (ba.x - bb.x).abs() < 0.01,
        "状态切换不得改变控件占位:{:?} vs {:?}",
        ba,
        bb
    );
}
