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
