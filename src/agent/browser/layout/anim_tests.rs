//! 第 57 轮:过渡/关键帧动画值进入布局几何与显示列表的集成回归。
//!
//! 时钟全程注入(`AnimEngine::set_clock_override`),断言确定性。

use super::super::anim::AnimEngine;
use super::super::document::Document;
use super::super::images::ImageSizes;
use super::super::render::FontSet;
use super::super::style::MediaCtx;
use super::compute_full_with_anim;

fn font() -> FontSet {
    FontSet::fallback()
}

fn layout_at(doc: &Document, eng: &AnimEngine) -> super::Layout {
    compute_full_with_anim(
        doc,
        800.0,
        &font(),
        &MediaCtx::new(800.0, 600.0),
        &ImageSizes::default(),
        eng,
    )
}

fn div_box<'a>(lay: &'a super::Layout) -> &'a super::ElementBox {
    lay.boxes
        .iter()
        .find(|b| b.tag == "div")
        .expect("div 应产生几何盒")
}

#[test]
fn 关键帧宽度动画中途进入几何() {
    let html = "<html><head><style>\
        @keyframes grow { from { width: 100px } to { width: 300px } } \
        div { animation: grow 1s linear both; height: 20px; }\
        </style></head><body><div id='a'></div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let eng = AnimEngine::new();
    eng.set_clock_override(Some(1000));
    let lay = layout_at(&doc, &eng);
    let w0 = div_box(&lay).w;
    assert!((w0 - 100.0).abs() < 1.5, "起点宽度应 100,实际 {w0}");
    eng.set_clock_override(Some(1500));
    let lay = layout_at(&doc, &eng);
    let w1 = div_box(&lay).w;
    assert!((w1 - 200.0).abs() < 1.5, "中点宽度应 200,实际 {w1}");
    eng.set_clock_override(Some(2050));
    let lay = layout_at(&doc, &eng);
    let w2 = div_box(&lay).w;
    assert!((w2 - 300.0).abs() < 1.5, "终点+fill 应 300,实际 {w2}");
}

#[test]
fn 关键帧透明度进显示列表() {
    let html = "<html><head><style>\
        @keyframes fade { from { opacity: 0 } to { opacity: 1 } } \
        div { animation: fade 1s linear both; width: 50px; height: 50px; background: #000; }\
        </style></head><body><div id='a'></div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let eng = AnimEngine::new();
    eng.set_clock_override(Some(1000));
    let _ = layout_at(&doc, &eng); // 启动动画
    eng.set_clock_override(Some(1500));
    let lay = layout_at(&doc, &eng);
    assert!(
        lay.display
            .iter()
            .any(|it| (it.opacity - 0.5).abs() < 0.03 && it.w >= 40.0),
        "t+500ms 应有半透明 50px 图元,实际:{:?}",
        lay.display.iter().map(|it| (it.w, it.opacity)).collect::<Vec<_>>()
    );
}

#[test]
fn 过渡在两次布局间补间() {
    let html1 = "<html><head><style>\
        div { transition: width 1s linear; width: 100px; height: 20px; }\
        </style></head><body><div id='a'></div></body></html>";
    let html2 = html1.replace("width: 100px", "width: 300px");
    let doc1 = Document::parse(html1, "https://a.test/");
    let doc2 = Document::parse(&html2, "https://a.test/");
    let eng = AnimEngine::new();
    eng.set_clock_override(Some(0));
    // 基线布局:建立过渡前值。
    let lay = layout_at(&doc1, &eng);
    assert!((div_box(&lay).w - 100.0).abs() < 1.5);
    // 变更即刻:过渡启动,几何仍是旧值。
    let lay = layout_at(&doc2, &eng);
    assert!((div_box(&lay).w - 100.0).abs() < 1.5, "过渡启动帧应回旧值");
    // 半程 500ms:补间中值。
    eng.set_clock_override(Some(500));
    let lay = layout_at(&doc2, &eng);
    let w = div_box(&lay).w;
    assert!((w - 200.0).abs() < 1.5, "半程宽度应 200,实际 {w}");
    // 结束:终值。
    eng.set_clock_override(Some(1200));
    let lay = layout_at(&doc2, &eng);
    let w = div_box(&lay).w;
    assert!((w - 300.0).abs() < 1.5, "终值宽度应 300,实际 {w}");
}

#[test]
fn 无动画页面零覆盖不扰动布局() {
    let html = "<html><body><div id='a' style='width:120px;height:10px'></div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let eng = AnimEngine::new();
    let lay = layout_at(&doc, &eng);
    assert!((div_box(&lay).w - 120.0).abs() < 1.0);
    assert!(!eng.has_active(), "无声明页面不应有活跃动画");
}

#[test]
fn transform关键帧进显示列表仿射() {
    let html = "<html><head><style>\
        @keyframes slide { from { transform: translateX(0px) } to { transform: translateX(40px) } } \
        div { animation: slide 1s linear both; width: 30px; height: 30px; background: #333; }\
        </style></head><body><div id='a'></div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let eng = AnimEngine::new();
    eng.set_clock_override(Some(1000));
    let _ = layout_at(&doc, &eng); // 启动动画
    eng.set_clock_override(Some(1250)); // t+250ms → p=0.25 → 10px
    let lay = layout_at(&doc, &eng);
    let moved = lay.display.iter().any(|it| {
        it.w >= 25.0
            && it.transform
                .map(|m| (m.e - 10.0).abs() < 1.0)
                .unwrap_or(false)
    });
    assert!(
        moved,
        "t+250ms 应有 translateX≈10px 的图元,实际:{:?}",
        lay.display
            .iter()
            .filter_map(|it| it.transform.map(|m| m.e))
            .collect::<Vec<_>>()
    );
}
