//! 样式状态回归：运行时 checked/disabled 与 details/dialog/option 显隐语义。

use std::collections::HashMap;

use super::super::document::Document;
use super::{MediaCtx, StyleEngine};

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
    doc.apply_widget_state(&values, &open);
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
