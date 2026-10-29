//! 列表 marker 类型与位置回归(第 41 轮)。
//!
//! 覆盖:`list-style-type` 全类型、`list-style-position: inside`
//! (marker 画在内容盒内,内容相应右移)。

use super::super::document::Document;
use super::super::render::display::{DisplayKind, WidgetGlyphKind};
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn layout(html: &str) -> super::Layout {
    let doc = Document::parse(html, "https://a.test/");
    compute(&doc, 800.0, &font())
}

fn has_glyph(lay: &super::Layout, glyph: WidgetGlyphKind) -> bool {
    lay.display
        .iter()
        .any(|it| matches!(it.kind, DisplayKind::WidgetGlyph { glyph: g, .. } if g == glyph))
}

#[test]
fn square类型画方块marker() {
    let lay = layout(
        "<body><ul style='list-style-type:square'><li>项目</li></ul></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::BulletSquare),
        "square 类型必须画方块 marker"
    );
    assert!(
        !has_glyph(&lay, WidgetGlyphKind::Bullet),
        "square 类型不得再画圆点 marker"
    );
}

#[test]
fn circle类型画圆环marker() {
    let lay = layout(
        "<body><ul style='list-style-type:circle'><li>项目</li></ul></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::BulletRing),
        "circle 类型必须画圆环 marker"
    );
}

#[test]
fn none类型不画marker() {
    let lay = layout(
        "<body><ul style='list-style-type:none'><li>项目</li></ul></body>",
    );
    assert!(
        !has_glyph(&lay, WidgetGlyphKind::Bullet)
            && !has_glyph(&lay, WidgetGlyphKind::BulletSquare)
            && !has_glyph(&lay, WidgetGlyphKind::BulletRing),
        "list-style-type:none 不得画任何 marker"
    );
}

#[test]
fn 字母与罗马数字序号() {
    let alpha = layout(
        "<body><ol style='list-style-type:lower-alpha'><li>甲</li><li>乙</li></ol></body>",
    );
    let alpha_texts: Vec<&str> = alpha.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(
        alpha_texts.contains(&"a.") && alpha_texts.contains(&"b."),
        "lower-alpha 必须画 a. / b.,实际:{alpha_texts:?}"
    );
    let roman = layout(
        "<body><ol style='list-style-type:upper-roman'><li>甲</li><li>乙</li><li>丙</li></ol></body>",
    );
    let roman_texts: Vec<&str> = roman.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(
        roman_texts.contains(&"I.") && roman_texts.contains(&"II.") && roman_texts.contains(&"III."),
        "upper-roman 必须画 I. / II. / III.,实际:{roman_texts:?}"
    );
}

#[test]
fn leading_zero补零序号() {
    let lay = layout(
        "<body><ol style='list-style-type:decimal-leading-zero'><li>甲</li><li>乙</li></ol></body>",
    );
    let texts: Vec<&str> = lay.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(
        texts.contains(&"01.") && texts.contains(&"02."),
        "decimal-leading-zero 必须补零,实际:{texts:?}"
    );
}

#[test]
fn inside位置把marker画进内容盒() {
    let outside = layout(
        "<body><ul style='list-style-position:outside'><li>项目</li></ul></body>",
    );
    let inside = layout(
        "<body><ul style='list-style-position:inside'><li>项目</li></ul></body>",
    );
    let bullet_x = |lay: &super::Layout| -> f64 {
        lay.display
            .iter()
            .find(|it| {
                matches!(it.kind, DisplayKind::WidgetGlyph { glyph: WidgetGlyphKind::Bullet, .. })
            })
            .map(|it| it.x)
            .expect("应存在 bullet marker")
    };
    let out_x = bullet_x(&outside);
    let in_x = bullet_x(&inside);
    assert!(
        in_x > out_x,
        "inside 的 marker 必须落在内容盒内(应在 outside 位置右侧):outside={out_x} inside={in_x}"
    );
}

#[test]
fn inside位置内容整体右移() {
    let outside = layout(
        "<body><ul style='list-style-position:outside'><li id='t'>项目</li></ul></body>",
    );
    let inside = layout(
        "<body><ul style='list-style-position:inside'><li id='t'>项目</li></ul></body>",
    );
    let text_x = |lay: &super::Layout| -> f64 {
        lay.lines
            .iter()
            .find(|l| l.text.contains("项目"))
            .map(|l| l.x)
            .expect("应有列表项文本")
    };
    assert!(
        text_x(&inside) > text_x(&outside),
        "inside 模式下内容必须让位给 marker:outside={} inside={}",
        text_x(&outside),
        text_x(&inside)
    );
}
