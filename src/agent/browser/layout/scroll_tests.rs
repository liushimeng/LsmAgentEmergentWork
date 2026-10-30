//! 元素级滚动容器回归(第 46 轮)。
//!
//! 覆盖:`overflow:auto|scroll` 容器登记最大滚动量、绘制滚动条,
//! 运行时滚动偏移把内容上移并被盒内裁剪。

use super::super::document::Document;
use super::super::render::display::DisplayKind;
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn layout(html: &str) -> super::Layout {
    let doc = Document::parse(html, "https://a.test/");
    compute(&doc, 800.0, &font())
}

const TALL: &str = "<body><div id='s' style='overflow:auto;height:60px;width:200px'>\
<p>第一段</p><p>第二段</p><p>第三段</p><p>第四段</p></div></body>";

#[test]
fn 可滚动容器登记最大滚动量() {
    let lay = layout(TALL);
    let max = lay
        .scroll_max
        .get("#s")
        .copied()
        .expect("可滚动容器应登记最大滚动量");
    assert!(max > 0.0, "内容高于盒高时应可滚动,实际 {max}");
}

#[test]
fn 可滚动容器画滚动条() {
    let lay = layout(TALL);
    // 滚动条 = 两条竖条填充(轨道 + 拇指),都在容器的右缘。
    let bars: Vec<(f64, f64)> = lay
        .display
        .iter()
        .filter_map(|it| match it.kind {
            DisplayKind::Background { .. } => Some((it.x, it.w)),
            _ => None,
        })
        .filter(|(x, w)| *x > 190.0 && *w <= 8.0)
        .collect();
    assert!(bars.len() >= 2, "可滚动容器应画轨道与拇指,实际 {bars:?}");
}

#[test]
fn 滚动偏移把内容上移() {
    let base = layout(TALL);
    let base_p = base
        .boxes
        .iter()
        .find(|b| b.tag == "p")
        .map(|b| b.y)
        .unwrap_or_default();
    // 运行时滚动偏移(第 46 轮:落在临时解析树的 `data-lsm-scroll-top`)。
    let html = TALL.replace(
        "<div id='s' style='overflow:auto;height:60px;width:200px'>",
        "<div id='s' data-lsm-scroll-top='30' style='overflow:auto;height:60px;width:200px'>",
    );
    let scrolled = layout(&html);
    let scrolled_p = scrolled
        .boxes
        .iter()
        .find(|b| b.tag == "p")
        .map(|b| b.y)
        .unwrap_or_default();
    assert!(
        (base_p - scrolled_p - 30.0).abs() < 1.0,
        "内容应按滚动偏移上移 30px:base={base_p} scrolled={scrolled_p}"
    );
}

#[test]
fn 普通裁剪容器不登记滚动量() {
    let lay = layout(
        "<body><div id='s' style='overflow:hidden;height:60px'><p>一</p><p>二</p><p>三</p><p>四</p></div></body>",
    );
    assert!(
        !lay.scroll_max.contains_key("#s"),
        "overflow:hidden 只裁剪不滚动,不应登记滚动量"
    );
}
