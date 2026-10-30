//! 表单控件**内置 UI** 回归(第 41 轮)。
//!
//! 覆盖:滑块(轨道+拇指)、颜色色块、日期/时间图标、数字微调箭头、
//! 文件选择器文案、checkbox 半选横杠、radio 未选中圆环。
//!
//! 铁律:内置 UI **只发射显示项,不改变控件盒几何**。

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
fn 滑块画轨道与拇指且拇指按比例定位() {
    let lay = layout(
        "<body><input id='r' type='range' min='0' max='100' value='25' style='width:200px;height:30px'></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::SliderThumb),
        "滑块必须画拇指图元"
    );
    // 轨道 = 一条浅灰填充(至少两个 Background 项:轨道 + 已填充段)。
    let fills = lay
        .display
        .iter()
        .filter(|it| matches!(it.kind, DisplayKind::Background { .. }))
        .count();
    assert!(fills >= 2, "滑块应画轨道与已填充段,背景项数:{fills}");
    // value=25 → 拇指应落在轨道左侧约 1/4 处。
    let thumb = lay
        .display
        .iter()
        .find(|it| matches!(it.kind, DisplayKind::WidgetGlyph { glyph: WidgetGlyphKind::SliderThumb, .. }))
        .expect("滑块拇指");
    assert!(
        thumb.x < 100.0,
        "value=25/100 的拇指应靠左,实际 x={}",
        thumb.x
    );
}

#[test]
fn 颜色选择器画色块() {
    let lay = layout(
        "<body><input id='c' type='color' value='#ff0000' style='width:60px;height:30px'></body>",
    );
    let red = lay.display.iter().any(|it| {
        matches!(
            it.kind,
            DisplayKind::Background {
                fill: super::super::render::display::BgFill::Solid([255, 0, 0], _),
            }
        )
    });
    assert!(red, "颜色选择器必须按 value 画色块");
}

#[test]
fn 颜色选择器非法值回落黑色() {
    let lay = layout(
        "<body><input id='c' type='color' value='不存在的颜色' style='width:60px;height:30px'></body>",
    );
    let black = lay.display.iter().any(|it| {
        matches!(
            it.kind,
            DisplayKind::Background {
                fill: super::super::render::display::BgFill::Solid([0, 0, 0], _),
            }
        )
    });
    assert!(black, "非法颜色值应回落黑色色块");
}

#[test]
fn 日期控件画日历图标() {
    let lay = layout(
        "<body><input id='d' type='date' value='2026-10-29' style='width:160px;height:30px'></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::CalendarIcon),
        "日期控件必须画日历图标"
    );
    assert!(
        lay.lines.iter().any(|l| l.text.contains("2026-10-29")),
        "日期控件应绘制其值文本"
    );
}

#[test]
fn 时间控件画时钟图标() {
    let lay = layout(
        "<body><input id='t' type='time' value='10:30' style='width:160px;height:30px'></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::ClockIcon),
        "时间控件必须画时钟图标"
    );
}

#[test]
fn 数字输入框画微调箭头() {
    let lay = layout(
        "<body><input id='n' type='number' value='42' style='width:120px;height:30px'></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::CaretUp),
        "数字输入框必须画上箭头"
    );
    assert!(
        lay.lines.iter().any(|l| l.text.contains("42")),
        "数字输入框应绘制其值文本"
    );
}

#[test]
fn 文件选择器画文件名或选择文案() {
    let with_value = layout(
        "<body><input id='f' type='file' value='/tmp/报告.pdf' style='width:200px;height:30px'></body>",
    );
    assert!(
        with_value.lines.iter().any(|l| l.text.contains("报告.pdf")),
        "文件选择器应绘制文件名(不含目录):{:?}",
        with_value.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
    let empty = layout("<body><input id='f' type='file' style='width:200px;height:30px'></body>");
    assert!(
        empty.lines.iter().any(|l| l.text.contains("选择文件")),
        "无值时文件选择器应画「选择文件」文案"
    );
}

#[test]
fn checkbox半选画横杠() {
    let lay = layout(
        "<body><input id='i' type='checkbox' indeterminate style='width:20px;height:20px'></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::Indeterminate),
        "半选 checkbox 必须画横杠"
    );
}

#[test]
fn radio未选中画圆环() {
    let lay = layout(
        "<body><input id='r' type='radio' style='width:20px;height:20px'></body>",
    );
    assert!(
        has_glyph(&lay, WidgetGlyphKind::BulletRing),
        "未选中 radio 必须画圆环轮廓"
    );
    assert!(
        !has_glyph(&lay, WidgetGlyphKind::RadioDot),
        "未选中 radio 不得画实心点"
    );
}

#[test]
fn 内置UI不改变控件占位尺寸() {
    // 同尺寸控件,有无内置 UI 的占位必须一致(状态/内置 UI 只影响绘制)。
    let plain = layout("<body><input id='a' type='text' style='width:120px;height:30px'></body>");
    let range = layout(
        "<body><input id='a' type='range' min='0' max='10' value='5' style='width:120px;height:30px'></body>",
    );
    let pa = plain.boxes.iter().find(|b| b.selector == "#a").unwrap();
    let ra = range.boxes.iter().find(|b| b.selector == "#a").unwrap();
    assert!(
        (pa.w - ra.w).abs() < 0.5 && (pa.h - ra.h).abs() < 0.5,
        "内置 UI 不得改变控件占位:{:?} vs {:?}",
        pa,
        ra
    );
}

#[test]
fn 长提示文本参与输入框固有宽() {
    let lay = layout(
        "<body><input id='p' type='text' placeholder='搜地点、查公交、找路线、看周边'></body>",
    );
    let input = lay
        .boxes
        .iter()
        .find(|b| b.selector == "#p")
        .expect("输入框应有几何盒");
    assert!(
        input.w > 140.0,
        "长 placeholder 应参与固有宽,实际只有 {}px",
        input.w
    );
}

#[test]
fn textarea按rows参与固有高() {
    let one = layout("<body><textarea id='t' rows='1'></textarea></body>");
    let four = layout("<body><textarea id='t' rows='4'></textarea></body>");
    let one_h = one.boxes.iter().find(|b| b.selector == "#t").unwrap().h;
    let four_h = four.boxes.iter().find(|b| b.selector == "#t").unwrap().h;
    assert!(
        four_h > one_h * 2.5,
        "rows=4 应显著高于 rows=1:{four_h} vs {one_h}"
    );
}

#[test]
fn 图片按钮绘制alt占位文案() {
    let lay = layout(
        "<body><input id='imgbtn' type='image' alt='登录图标' style='width:90px;height:32px'></body>",
    );
    assert!(
        lay.lines.iter().any(|l| l.text.contains("登录图标")),
        "input[type=image] 应绘制 alt 文案:{:?}",
        lay.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
}

#[test]
fn 行内控件文本只绘制一次() {
    let button = layout("<body><button id='b' style='width:100px'>唯一按钮</button></body>");
    let button_count = button
        .lines
        .iter()
        .filter(|l| l.text.contains("唯一按钮"))
        .count();
    let button_box = button.boxes.iter().find(|b| b.selector == "#b");
    assert_eq!(
        button_count, 1,
        "button 子文本不得与控件文本重复绘制:{:?}, box={button_box:?}",
        button.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );

    let area = layout("<body><textarea id='t' style='width:100px'>唯一多行</textarea></body>");
    let area_count = area
        .lines
        .iter()
        .filter(|l| l.text.contains("唯一多行"))
        .count();
    let area_box = area.boxes.iter().find(|b| b.selector == "#t");
    assert_eq!(
        area_count, 1,
        "textarea 初始文本只能绘制一次:{:?}, box={area_box:?}",
        area.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
}
