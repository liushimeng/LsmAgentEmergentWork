//! 行内原子盒与基线对齐回归(第 44 轮)。
//!
//! 覆盖:行内控件/替换元素**占宽参与行盒**、**基线对齐**、段首不再下沉、
//! `vertical-align` 子集、`accent-color` 与禁用态灰化、`meter` 分段配色、
//! `progress` 不确定态、`select` 列表形态与 `optgroup` 标题。

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

/// 取标签对应的盒。
fn box_of(lay: &super::Layout, tag: &str) -> super::ElementBox {
    lay.boxes
        .iter()
        .find(|b| b.tag.eq_ignore_ascii_case(tag))
        .unwrap_or_else(|| panic!("缺少 {tag} 盒"))
        .clone()
}

/// 文本行的基线(= 文字顶 + 字号,渲染层即此口径)。
fn baseline(lay: &super::Layout, needle: &str) -> f64 {
    let line = lay
        .lines
        .iter()
        .find(|l| l.text.contains(needle))
        .unwrap_or_else(|| panic!("缺少文本行 {needle}"));
    line.y + line.size
}

#[test]
fn 行内控件与文字同行且不重叠() {
    let lay = layout("<body><p>Hello <input id='i' type='text' value='V'> World</p></body>");
    let input = box_of(&lay, "input");
    let hello = lay.lines.iter().find(|l| l.text.contains("Hello")).unwrap();
    let world = lay.lines.iter().find(|l| l.text.contains("World")).unwrap();
    // 控件盒必须在「Hello 」之后。
    assert!(
        input.x >= hello.x + 30.0,
        "控件应排在文字之后:input.x={} hello.x={}",
        input.x,
        hello.x
    );
    // 「World」必须在控件盒右缘之后(不压字)。
    assert!(
        world.x >= input.x + input.w - 0.5,
        "控件后的文字不应被压住:world.x={} input 右缘={}",
        world.x,
        input.x + input.w
    );
    // 控件与文字在同一行(基线相差在 2px 内)。
    let text_base = baseline(&lay, "Hello");
    let input_base = input.y + input.h * 0.8;
    assert!(
        (text_base - input_base).abs() < 2.0,
        "控件底边应与文字基线对齐:text_base={text_base} input_base={input_base}"
    );
}

#[test]
fn 段首不再因行内盒下沉() {
    // 回归:行内盒曾把块游标提前推下去,段首凭空多出一个盒高的空洞。
    let plain = layout("<body><p>Hello world</p></body>");
    let linked = layout("<body><p>Hello <a href='#'>link</a> World</p></body>");
    let base_y = plain.lines[0].y;
    let link_y = linked
        .lines
        .iter()
        .find(|l| l.text.contains("Hello"))
        .unwrap()
        .y;
    assert!(
        (base_y - link_y).abs() < 0.5,
        "带链接的段落首行不应下沉:plain={base_y} linked={link_y}"
    );
}

#[test]
fn 行内图片占宽并按基线对齐() {
    let lay = layout(
        "<body><p>Hi <img id='im' src='https://a.test/x.png' style='width:40px;height:20px'> there</p></body>",
    );
    let img = box_of(&lay, "img");
    let hi = lay.lines.iter().find(|l| l.text.contains("Hi")).unwrap();
    let there = lay.lines.iter().find(|l| l.text.contains("there")).unwrap();
    assert!(img.x >= hi.x, "图片应排在文字之后");
    assert!(
        there.x >= img.x + img.w - 0.5,
        "图片后的文字不应被压住:there.x={} img 右缘={}",
        there.x,
        img.x + img.w
    );
    // 替换元素底边贴基线(CSS 2.1 §10.8.1)。
    let text_base = baseline(&lay, "Hi");
    assert!(
        (img.y + img.h - text_base).abs() < 2.0,
        "替换元素底边应贴基线:img 底={} 基线={}",
        img.y + img.h,
        text_base
    );
}

#[test]
fn 行内块撑开行盒但不推走段首() {
    let lay = layout(
        "<body><p>Hi <span style='display:inline-block;width:60px;height:40px'>x</span> ok</p></body>",
    );
    let first = lay.lines.iter().find(|l| l.text.contains("Hi")).unwrap();
    let p = box_of(&lay, "p");
    assert!(
        (first.y - p.y).abs() < 0.5,
        "段首不应下沉:first={} p={}",
        first.y,
        p.y
    );
}

#[test]
fn vertical_align_middle_改变原子盒落位() {
    let base = layout("<body><p>Hi <img id='a' src='x.png' style='width:40px;height:20px'></p></body>");
    let mid = layout(
        "<body><p>Hi <img id='a' src='x.png' style='width:40px;height:20px;vertical-align:middle'></p></body>",
    );
    let b = box_of(&base, "img");
    let m = box_of(&mid, "img");
    assert!(
        (m.y - b.y).abs() > 1.0,
        "vertical-align:middle 应改变原子盒的落位:base={} mid={}",
        b.y,
        m.y
    );
}

#[test]
fn vertical_align_top_贴行盒顶() {
    let lay = layout(
        "<body><p>Hi <img id='a' src='x.png' style='width:40px;height:20px;vertical-align:top'></p></body>",
    );
    let img = box_of(&lay, "img");
    let first = lay.lines.iter().find(|l| l.text.contains("Hi")).unwrap();
    assert!(
        (img.y - first.y).abs() < 1.0,
        "vertical-align:top 应贴行盒顶:img={} 行顶={}",
        img.y,
        first.y
    );
}

#[test]
fn accent_color_接入勾图元颜色() {
    let lay = layout(
        "<body><input id='c' type='checkbox' checked style='accent-color:#ff0000'></body>",
    );
    let glyph = lay
        .display
        .iter()
        .find(|it| {
            matches!(
                it.kind,
                DisplayKind::WidgetGlyph {
                    glyph: WidgetGlyphKind::CheckMark,
                    ..
                }
            )
        })
        .expect("应有勾图元");
    match glyph.kind {
        DisplayKind::WidgetGlyph { color, .. } => {
            assert_eq!(color, [255, 0, 0], "勾图元应取 accent-color");
        }
        _ => unreachable!(),
    }
}

#[test]
fn 禁用态控件灰化() {
    let lay = layout("<body><input id='c' type='checkbox' checked disabled></body>");
    let glyph = lay
        .display
        .iter()
        .find(|it| {
            matches!(
                it.kind,
                DisplayKind::WidgetGlyph {
                    glyph: WidgetGlyphKind::CheckMark,
                    ..
                }
            )
        })
        .expect("应有勾图元");
    match glyph.kind {
        DisplayKind::WidgetGlyph { color, .. } => {
            assert_eq!(
                color,
                [150, 150, 150],
                "禁用态控件图元应灰化,实际 {color:?}"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn meter_按_optimum_分三种色() {
    let good = layout("<body><meter id='m' min='0' max='100' low='25' high='75' optimum='50' value='50'></meter></body>");
    let poor = layout("<body><meter id='m' min='0' max='100' low='25' high='75' optimum='50' value='5'></meter></body>");
    let fills = |lay: &super::Layout| -> Vec<[u8; 3]> {
        lay.display
            .iter()
            .filter_map(|it| match it.kind {
                DisplayKind::Background {
                    fill: super::super::render::display::BgFill::Solid(c, _),
                } => Some(c),
                _ => None,
            })
            .collect()
    };
    let g = fills(&good);
    let p = fills(&poor);
    assert!(
        g.iter().any(|c| *c == [80, 160, 90]),
        "最优区的 meter 应为绿色,实际 {g:?}"
    );
    assert!(
        p.iter().any(|c| *c == [190, 75, 75]),
        "最差区的 meter 应为红色,实际 {p:?}"
    );
}

#[test]
fn progress_无value_为不确定态() {
    let indeterminate = layout("<body><progress id='p' style='width:200px;height:20px'></progress></body>");
    let determinate = layout("<body><progress id='p' value='0.5' style='width:200px;height:20px'></progress></body>");
    // 不确定态:填充段居中(起点 > 0);确定态:填充段从左缘起。
    let fill_x = |lay: &super::Layout| -> Option<f64> {
        lay.display
            .iter()
            .filter_map(|it| match it.kind {
                DisplayKind::Background {
                    fill: super::super::render::display::BgFill::Solid(c, _),
                } if c == [0, 117, 255] => Some(it.x),
                _ => None,
            })
            .min_by(|a, b| a.partial_cmp(b).unwrap())
    };
    let ind = fill_x(&indeterminate).expect("不确定态应有填充段");
    let det = fill_x(&determinate).expect("确定态应有填充段");
    assert!(
        ind > det + 10.0,
        "不确定态的填充段应居中:ind={ind} det={det}"
    );
}

#[test]
fn select_列表形态铺开全部option() {
    let lay = layout(
        "<body><select id='s' size='3'><option>甲</option><option selected>乙</option><option>丙</option></select></body>",
    );
    let sel = box_of(&lay, "select");
    assert!(
        sel.h >= 40.0,
        "列表形态 select 应有多行高度,实际 {}",
        sel.h
    );
    let options: Vec<&super::ElementBox> =
        lay.boxes.iter().filter(|b| b.tag == "option").collect();
    assert_eq!(options.len(), 3, "三个 option 都应有几何盒");
    // 选中项铺高亮底色(浅蓝)。
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::Background {
                fill: super::super::render::display::BgFill::Solid(c, _),
            } if c == [214, 232, 250]
        )),
        "选中 option 行应铺高亮底色"
    );
}

#[test]
fn select_列表形态渲染_optgroup_标题() {
    let lay = layout(
        "<body><select id='s' size='3'><optgroup label='第一组'><option>甲</option></optgroup><optgroup label='第二组'><option>乙</option></optgroup></select></body>",
    );
    assert!(
        lay.lines.iter().any(|l| l.text.contains("第一组")),
        "optgroup 标题应渲染出来"
    );
    assert!(
        lay.lines.iter().any(|l| l.text.contains("第二组")),
        "optgroup 标题应渲染出来"
    );
}
