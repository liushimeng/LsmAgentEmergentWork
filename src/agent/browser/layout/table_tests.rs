//! 基础表格格式化上下文回归：行列对齐、自动列宽与受控跨距。

use super::super::document::Document;
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn box_by_selector<'a>(lay: &'a super::Layout, sel: &str) -> &'a super::ElementBox {
    lay.boxes
        .iter()
        .find(|b| b.selector == sel)
        .unwrap_or_else(|| panic!("表格单元格缺少几何盒:{sel}"))
}

#[test]
fn 表格同行单元格同高线且列递增() {
    let doc = Document::parse(
        "<body><table><tr><td id='a'>A</td><td id='b'>BBBB</td></tr><tr><td id='c'>C</td><td id='d'>DD</td></tr></table></body>",
        "https://a.test/",
    );
    let lay = compute(&doc, 800.0, &font());
    let a = box_by_selector(&lay, "#a");
    let b = box_by_selector(&lay, "#b");
    let c = box_by_selector(&lay, "#c");
    let d = box_by_selector(&lay, "#d");
    assert!((a.y - b.y).abs() < 0.5, "同行单元格 y 必须一致:{:?} {:?}", a, b);
    assert!((c.y - d.y).abs() < 0.5, "第二行 y 必须一致:{:?} {:?}", c, d);
    assert!(b.x > a.x, "右侧单元格 x 必须递增:{:?} {:?}", a, b);
    assert!(c.y > a.y, "下一行 y 必须递增:{:?} {:?}", a, c);
    assert!(b.w > a.w, "长内容列应更宽:{:?} {:?}", a, b);
}

#[test]
fn colspan合并列宽() {
    let doc = Document::parse(
        "<body><table><tr><td id='span' colspan='2'>跨两列内容</td></tr><tr><td id='x'>x</td><td id='y'>yyyyyyyy</td></tr></table></body>",
        "https://a.test/",
    );
    let lay = compute(&doc, 800.0, &font());
    let span = box_by_selector(&lay, "#span");
    let x = box_by_selector(&lay, "#x");
    let y = box_by_selector(&lay, "#y");
    assert!(
        (span.w - (x.w + y.w)).abs() < 1.5,
        "colspan 单元格宽度应约等于两列之和:{:?} {:?} {:?}",
        span,
        x,
        y
    );
}

#[test]
fn rowspan合并行高() {
    let doc = Document::parse(
        "<body><table><tr><td id='tall' rowspan='2'>跨两行内容</td><td id='a'>a</td></tr><tr><td id='b'>b</td></tr></table></body>",
        "https://a.test/",
    );
    let lay = compute(&doc, 800.0, &font());
    let tall = box_by_selector(&lay, "#tall");
    let a = box_by_selector(&lay, "#a");
    let b = box_by_selector(&lay, "#b");
    assert!(
        (tall.h - (a.h + b.h)).abs() < 2.0,
        "rowspan 单元格高度应约等于两行之和:{:?} {:?} {:?}",
        tall,
        a,
        b
    );
}

#[test]
fn 单元格内块级内容正常布局() {
    let doc = Document::parse(
        "<body><table><tr><td id='cell'><div>块级标题</div><p>段落文本</p></td></tr></table></body>",
        "https://a.test/",
    );
    let lay = compute(&doc, 800.0, &font());
    assert!(lay.lines.iter().any(|l| l.text.contains("块级标题")));
    assert!(lay.lines.iter().any(|l| l.text.contains("段落文本")));
    let cell = box_by_selector(&lay, "#cell");
    assert!(cell.h > 20.0, "单元格应吸收块级子内容高度:{:?}", cell);
}

#[test]
fn 非法跨距被钳制不panic() {
    let doc = Document::parse(
        "<body><table><tr><td id='bad' colspan='0' rowspan='-2'>x</td><td id='ok'>y</td></tr></table></body>",
        "https://a.test/",
    );
    let lay = compute(&doc, 800.0, &font());
    assert!(lay.boxes.iter().any(|b| b.selector == "#bad"));
    assert!(lay.boxes.iter().any(|b| b.selector == "#ok"));
}
