//! 样式状态回归：运行时 checked/disabled 与 details/dialog/option 显隐语义。

use std::collections::HashMap;

use super::super::document::{Document, PseudoFlags};
use super::{selector_specificity, MediaCtx, StyleEngine};

fn state_doc(html: &str, values: &[(&str, &str)], open: &[(&str, bool)]) -> Document {
    let mut doc = Document::parse(html, "https://a.test/");
    let values: HashMap<String, String> = values
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let open: HashMap<String, bool> = open
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
    doc.apply_widget_state(&values, &open, &HashMap::new());
    doc
}

fn engine(doc: &Document) -> StyleEngine {
    StyleEngine::from_document(doc, MediaCtx::new(800.0, 600.0))
}

#[test]
fn checked伪类读取运行时选中态() {
    let doc = state_doc(
        "<head><style>input:checked{background:#ff0000}</style></head><body><input id='ok' type='checkbox'></body>",
        &[("#ok", "true")],
        &[],
    );
    let st = engine(&doc).computed_chain(&doc.first("#ok").unwrap().unwrap());
    assert_eq!(st.background, Some([255, 0, 0]), ":checked 必须命中运行时选中态");
}

#[test]
fn disabled伪类命中禁用控件() {
    let doc = state_doc(
        "<head><style>input:disabled{opacity:0.5}</style></head><body><input id='q' disabled></body>",
        &[],
        &[],
    );
    let st = engine(&doc).computed_chain(&doc.first("#q").unwrap().unwrap());
    assert!((st.opacity - 0.5).abs() < 0.01);
}

#[test]
fn indeterminate伪类按属性命中() {
    let doc = state_doc(
        "<head><style>input:indeterminate{color:#ff8800}</style></head><body><input id='half' type='checkbox' indeterminate></body>",
        &[],
        &[],
    );
    let st = engine(&doc).computed_chain(&doc.first("#half").unwrap().unwrap());
    assert_eq!(st.color, [255, 136, 0], ":indeterminate 必须命中半选属性");
}

#[test]
fn 无半选属性时indeterminate不命中() {
    let doc = state_doc(
        "<head><style>input:indeterminate{color:#ff8800}</style></head><body><input id='plain' type='checkbox'></body>",
        &[],
        &[],
    );
    let st = engine(&doc).computed_chain(&doc.first("#plain").unwrap().unwrap());
    assert_ne!(st.color, [255, 136, 0], "无 indeterminate 属性时不得命中");
}

#[test]
fn indeterminate计入伪类特异性() {
    let (a, b, c) = selector_specificity("input:indeterminate");
    assert_eq!((a, b, c), (0, 1, 1), ":indeterminate 必须计入伪类列");
}

#[test]
fn 关闭details抑制非summary子树() {
    let doc = state_doc(
        "<body><details id='d'><summary>标题</summary><p>正文</p></details></body>",
        &[],
        &[("#d", false)],
    );
    let e = engine(&doc);
    let summary = e.computed_chain(&doc.first("summary").unwrap().unwrap());
    let body = e.computed_chain(&doc.first("p").unwrap().unwrap());
    assert_ne!(summary.display, "none", "summary 必须保持可见");
    assert_eq!(body.display, "none", "关闭 details 的正文必须隐藏");
}

#[test]
fn option与datalist默认不参与布局() {
    let doc = state_doc(
        "<body><datalist id='l'><option>候选</option></datalist><select><option>甲</option></select></body>",
        &[],
        &[],
    );
    let e = engine(&doc);
    for sel in ["datalist", "option"] {
        let node = doc.first(sel).unwrap().unwrap();
        assert_eq!(e.computed_chain(&node).display, "none", "{sel} 应默认 display:none");
    }
}

/// 第 41 轮:运行时伪类状态(:hover/:focus/:focus-visible/:active)回归。
fn pseudo_doc(html: &str, states: &[(&str, bool, bool)]) -> Document {
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
fn hover伪类按运行时状态命中() {
    let doc = pseudo_doc(
        "<head><style>a:hover{color:#ff0000}</style></head><body><a id='l' href='#x'>链接</a></body>",
        &[("#l", true, false)],
    );
    let st = engine(&doc).computed_chain(&doc.first("#l").unwrap().unwrap());
    assert_eq!(st.color, [255, 0, 0], ":hover 必须命中运行时悬停态");
}

#[test]
fn 无状态时hover与focus均不命中() {
    let doc = pseudo_doc(
        "<head><style>a:hover{color:#ff0000}a:focus{color:#00ff00}</style></head><body><a id='l' href='#x'>链接</a></body>",
        &[],
    );
    let st = engine(&doc).computed_chain(&doc.first("#l").unwrap().unwrap());
    assert_ne!(st.color, [255, 0, 0], "无悬停态时不得命中 :hover");
    assert_ne!(st.color, [0, 255, 0], "无焦点态时不得命中 :focus");
}

#[test]
fn focus伪类按运行时状态命中() {
    let doc = pseudo_doc(
        "<head><style>a:focus{color:#00ff00}</style></head><body><a id='l' href='#x'>链接</a></body>",
        &[("#l", false, true)],
    );
    let st = engine(&doc).computed_chain(&doc.first("#l").unwrap().unwrap());
    assert_eq!(st.color, [0, 255, 0], ":focus 必须命中运行时焦点态");
}

#[test]
fn 伪类计入特异性不再等同裸类型选择器() {
    // `a:hover` 的特异性应为 (0,1,1) —— 伪类列 +1,可压过同层属性选择器。
    let (a, b, c) = selector_specificity("a:hover");
    assert_eq!((a, b, c), (0, 1, 1), ":hover 必须计入伪类列");
    let (a, b, c) = selector_specificity("a");
    assert_eq!((a, b, c), (0, 0, 1), "裸类型选择器特异性为 (0,0,1)");
}
