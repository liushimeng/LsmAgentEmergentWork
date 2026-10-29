//! 定位百分比、尺寸约束与验证卡几何回归。

use super::super::document::Document;
use super::super::images::ImageSizes;
use super::super::render::FontSet;
use super::super::style::MediaCtx;
use super::{compute_full, ElementBox, Layout};

fn layout(html: &str) -> Layout {
    let doc = Document::parse(html, "https://a.test/");
    compute_full(
        &doc,
        1920.0,
        &FontSet::fallback(),
        &MediaCtx::new(1920.0, 1080.0),
        &ImageSizes::default(),
    )
}

fn box_by_id<'a>(layout: &'a Layout, id: &str) -> &'a ElementBox {
    let selector = format!("#{id}");
    layout
        .boxes
        .iter()
        .find(|b| b.selector == selector)
        .unwrap_or_else(|| panic!("应有 {selector} 盒:{:?}", layout.boxes))
}

fn assert_near(actual: f64, expected: f64, label: &str) {
    assert!((actual - expected).abs() < 1.5, "{label}:期望 {expected},实际 {actual}");
}

#[test]
fn 验证卡_flex全屏居中且不拉满视口() {
    let lay = layout(
        r#"<html><head><style>
        html,body{margin:0;height:100%}
        #page{display:flex;flex-direction:column;justify-content:center;align-items:center;min-height:100vh;background:#181818}
        #card{width:100%;max-width:520px;min-height:515px;background:#fff}
        </style></head><body><div id="page"><div id="card">安全验证</div></div></body></html>"#,
    );
    let page = box_by_id(&lay, "page");
    let card = box_by_id(&lay, "card");
    assert_near(page.h, 1080.0, "验证页高度");
    assert_near(card.w, 520.0, "验证卡最大宽度");
    assert_near(card.x, 700.0, "验证卡水平居中");
    assert_near(card.y, 282.5, "验证卡垂直居中");
}

#[test]
fn fixed弹层_top_left百分比按包含块宽高求解() {
    let lay = layout(
        r#"<body style="margin:0"><div id="popup" style="position:fixed;left:50%;top:50%;width:348px;height:445px;max-height:400px;transform:translate(-50%,-50%);background:#fff">验证</div></body>"#,
    );
    let popup = box_by_id(&lay, "popup");
    assert_near(popup.w, 348.0, "弹层宽");
    assert_near(popup.h, 400.0, "max-height 钳制");
    assert_near(popup.x, 960.0, "left:50% 的未变换盒起点");
    assert_near(popup.y, 540.0, "top:50% 应相对 1080 高而非 1920 宽");
    let item = lay
        .display
        .iter()
        .find(|it| it.w == popup.w && it.h == popup.h && it.transform.is_some())
        .expect("弹层应有 transform 显示项");
    let (cx, cy) = item
        .transform
        .expect("已断言有 transform")
        .apply(popup.x + popup.w / 2.0, popup.y + popup.h / 2.0);
    assert_near(cx, 960.0, "变换后弹层中心 x");
    assert_near(cy, 540.0, "变换后弹层中心 y");
}

#[test]
fn fixed尺寸_百分比_content_box与min_max统一生效() {
    let lay = layout(
        r#"<body style="margin:0"><div id="panel" style="box-sizing:content-box;padding:20px;position:fixed;left:25%;top:25%;width:50%;height:50%;min-width:600px;max-width:700px;min-height:300px;max-height:400px"></div></body>"#,
    );
    let panel = box_by_id(&lay, "panel");
    assert_near(panel.w, 700.0, "content-box 最大宽");
    assert_near(panel.h, 400.0, "content-box 最大高");
    assert_near(panel.x, 480.0, "水平百分比");
    assert_near(panel.y, 270.0, "垂直百分比");
}

#[test]
fn 高度约束冲突时_min_height优先() {
    let lay = layout(
        r#"<body style="margin:0"><div id="bounded" style="height:600px;max-height:400px;min-height:450px">内容</div><div id="grid" style="display:grid;height:500px;max-height:320px"><div>格</div></div></body>"#,
    );
    assert_near(box_by_id(&lay, "bounded").h, 450.0, "块级 min/max 钳序");
    assert_near(box_by_id(&lay, "grid").h, 320.0, "Grid max-height");
}
