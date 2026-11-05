//! 第 48 轮:媒体/嵌入控件几何、占位与语义装饰回归。

use super::*;
use crate::agent::browser::document::Document;
use crate::agent::browser::images::collect_image_urls;
use crate::agent::browser::render::FontSet;

fn font() -> FontSet {
    FontSet::fallback()
}

fn box_of(lay: &Layout, tag: &str) -> (f64, f64, f64, f64) {
    lay.boxes
        .iter()
        .find(|b| b.tag.eq_ignore_ascii_case(tag))
        .map(|b| (b.x, b.y, b.w, b.h))
        .unwrap_or_else(|| panic!("缺盒子:{tag}"))
}

#[test]
fn video缺省几何300x150() {
    let html = "<html><body><video id='v' controls></video></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let v = box_of(&lay, "video");
    assert!(
        (v.2 - 300.0).abs() < 1.0 && (v.3 - 150.0).abs() < 1.0,
        "video 缺省几何应为 300×150,实际 {v:?}"
    );
}

#[test]
fn video显式宽高优先于缺省固有尺寸() {
    let html = "<html><body><video id='v' width='320' height='180'></video></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let v = box_of(&lay, "video");
    assert!(
        (v.2 - 320.0).abs() < 1.0 && (v.3 - 180.0).abs() < 1.0,
        "video 应取 HTML width/height,实际 {v:?}"
    );
}

#[test]
fn object与embed按替换元素取属性尺寸() {
    let html = "<html><body><object id='o' width='64' height='48'></object><embed id='e' width='32' height='24'></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let o = box_of(&lay, "object");
    let e = box_of(&lay, "embed");
    assert!(
        (o.2 - 64.0).abs() < 1.0 && (o.3 - 48.0).abs() < 1.0,
        "object 应取 width/height,实际 {o:?}"
    );
    assert!(
        (e.2 - 32.0).abs() < 1.0 && (e.3 - 24.0).abs() < 1.0,
        "embed 应取 width/height,实际 {e:?}"
    );
}

#[test]
fn 媒体空盒进入MediaFrameNote占位() {
    let html = "<html><body><iframe id='f' width='300' height='150'></iframe></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        lay.display
            .iter()
            .any(|it| matches!(&it.kind, super::super::render::display::DisplayKind::MediaFrameNote { tag, .. } if tag == "iframe")),
        "iframe 空盒应发射 MediaFrameNote 占位"
    );
}

#[test]
fn video海报与object数据参与图片预取() {
    let html = r#"<html><body>
        <video poster="https://img.example/poster.png"></video>
        <object data="https://img.example/obj.png"></object>
        <embed src="https://img.example/embed.png">
    </body></html>"#;
    let urls = collect_image_urls(html, "https://a.test/", 800.0);
    for want in [
        "https://img.example/poster.png",
        "https://img.example/obj.png",
        "https://img.example/embed.png",
    ] {
        assert!(urls.iter().any(|u| u == want), "预取应含 {want}:{urls:?}");
    }
}

#[test]
fn del与ins文本装饰进入显示列表() {
    let html = "<html><body><p id='p'><del>旧</del><ins>新</ins></p></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let decor: Vec<_> = lay
        .display
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                super::super::render::display::DisplayKind::TextDecor { .. }
            )
        })
        .collect();
    assert!(decor.len() >= 2, "del/ins 应各产生装饰线,实际 {}", decor.len());
}

#[test]
fn hr不再塌成0高() {
    let html = "<html><body>上<hr id='h'>下</body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let h = box_of(&lay, "hr");
    assert!(h.3 >= 2.0, "hr 应至少 2px 高,实际 {h:?}");
}

#[test]
fn mark语义高亮产生背景显示项() {
    let html = "<html><body><p id='p'>a<mark id='m'>b</mark>c</p></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        lay.display
            .iter()
            .any(|it| matches!(it.kind, DisplayKind::Background { .. })),
        "mark 应产生高亮背景显示项"
    );
}

#[test]
fn object_position左对齐偏移内容图() {
    let html = "<html><body><img id='i' src='a.png' style='width:200px;height:100px;object-fit:contain;object-position:0% 0%'></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let mut sizes = crate::agent::browser::images::ImageSizes::default();
    sizes
        .sizes
        .insert("https://a.test/a.png".to_string(), (100.0, 100.0));
    let lay = super::compute_full(
        &doc,
        800.0,
        &font(),
        &crate::agent::browser::style::MediaCtx::new(800.0, 720.0),
        &sizes,
    );
    let paint = lay
        .paints
        .iter()
        .find(|p| p.src == "https://a.test/a.png")
        .expect("应有图片绘制盒");
    // contain:100×100 图在 200×100 盒内 → 绘制 100×100,居中偏移 (50,0)。
    // object-position:0% 0% → 期望 (0,0),故 off = (0,0) - (50,0) = (-50,0)。
    assert!(
        (paint.off_x + 50.0).abs() < 0.5,
        "object-position:0% 0% 应左对齐,off_x={}",
        paint.off_x
    );
    assert!(paint.off_y.abs() < 0.5, "off_y={}", paint.off_y);
}

#[test]
fn object_position缺省居中() {
    let html = "<html><body><img id='i' src='a.png' style='width:200px;height:100px;object-fit:contain'></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let mut sizes = crate::agent::browser::images::ImageSizes::default();
    sizes
        .sizes
        .insert("https://a.test/a.png".to_string(), (100.0, 100.0));
    let lay = super::compute_full(
        &doc,
        800.0,
        &font(),
        &crate::agent::browser::style::MediaCtx::new(800.0, 720.0),
        &sizes,
    );
    let paint = lay
        .paints
        .iter()
        .find(|p| p.src == "https://a.test/a.png")
        .expect("应有图片绘制盒");
    assert!(
        paint.off_x.abs() < 0.5 && paint.off_y.abs() < 0.5,
        "缺省 object-position 必须居中,off=({}, {})",
        paint.off_x,
        paint.off_y
    );
}

#[test]
fn 未加载图片画alt文本占位() {
    let html = "<html><body><img id='i' src='missing.png' alt='示例图片' style='width:120px;height:40px'></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        lay.display.iter().any(|it| matches!(
            &it.kind,
            super::super::render::display::DisplayKind::Text { text, .. } if text == "示例图片"
        )),
        "未加载图片应画 alt 文本占位"
    );
}

#[test]
fn 空alt不画占位() {
    let html = "<html><body><img id='i' src='missing.png' alt='' style='width:120px;height:40px'></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        !lay.display.iter().any(|it| matches!(
            &it.kind,
            super::super::render::display::DisplayKind::Text { .. }
        )),
        "空 alt 不应画任何占位文本"
    );
}

// ---------- 第 50 轮:媒体控件条 ----------

#[test]
fn video带controls画播放三角与进度条() {
    let html = "<html><body><video id='v' width='320' height='180' controls></video></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            super::super::render::display::DisplayKind::WidgetGlyph {
                glyph: super::super::render::display::WidgetGlyphKind::PlayTriangle,
                ..
            }
        )),
        "video[controls] 必须画播放三角"
    );
    // 进度条轨道:条底部区域应有半透明白 Background。
    let v = box_of(&lay, "video");
    let track = lay.display.iter().any(|it| match &it.kind {
        super::super::render::display::DisplayKind::Background { fill } => {
            it.y > v.1 + v.3 - 30.0 && it.h < 6.0 && it.w > 40.0
        }
        _ => false,
    });
    assert!(track, "video[controls] 应画进度条轨道");
}

#[test]
fn video无controls不画控件条() {
    let html = "<html><body><video id='v' width='320' height='180'></video></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        !lay.display.iter().any(|it| matches!(
            it.kind,
            super::super::render::display::DisplayKind::WidgetGlyph {
                glyph: super::super::render::display::WidgetGlyphKind::PlayTriangle,
                ..
            }
        )),
        "无 controls 属性不得画控件条"
    );
}

#[test]
fn audio带controls为300x30细条() {
    let html = "<html><body><audio id='a' controls></audio></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let a = box_of(&lay, "audio");
    assert!(
        (a.2 - 300.0).abs() < 1.0 && (a.3 - 30.0).abs() < 1.0,
        "audio[controls] 应为 300×30 控件条,实际 {a:?}"
    );
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            super::super::render::display::DisplayKind::WidgetGlyph {
                glyph: super::super::render::display::WidgetGlyphKind::PlayTriangle,
                ..
            }
        )),
        "audio[controls] 必须画播放三角"
    );
}

#[test]
fn audio无controls不建盒() {
    let html = "<html><body><audio id='a'></audio></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert!(
        !lay.boxes.iter().any(|b| b.tag.eq_ignore_ascii_case("audio")),
        "audio 无 controls 应 UA 隐藏,不建替换盒"
    );
}
