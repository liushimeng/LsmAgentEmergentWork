//! 控件几何图元像素回归：勾/圆点/箭头必须真实落到截图。

use std::collections::HashMap;

use super::super::images::ImageStore;
use super::super::render::display::{DisplayKind, WidgetGlyphKind};
use super::super::{document::Document, layout};
use super::FontSet;

fn render_state(html: &str, values: &[(&str, &str)]) -> (image::RgbaImage, layout::Layout) {
    let mut doc = Document::parse(html, "https://a.test/");
    let values: HashMap<String, String> = values
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    doc.apply_widget_state(&values, &HashMap::new(), &HashMap::new());
    let font = FontSet::fallback();
    let lay = layout::compute(&doc, 640.0, &font);
    let pm = font
        .render_pixmap(&lay, 640, 480, 0.0, 0.0, false, &ImageStore::new())
        .unwrap();
    let img = image::load_from_memory(&pm.encode_png().unwrap()).unwrap().to_rgba8();
    (img, lay)
}

/// 统计控件盒内的**墨迹像素**(非白底像素)。
///
/// 第 44 轮:勾/圆点/箭头等图元改用 `accent-color`(默认 `#0075FF` 蓝),
/// 不再是深灰 —— 判据从「深色」改为「非背景色」。
fn ink_count(img: &image::RgbaImage, lay: &layout::Layout, selector: &str) -> usize {
    let b = lay
        .boxes
        .iter()
        .find(|b| b.selector == selector)
        .expect("控件缺少几何盒");
    let x0 = b.x.max(0.0) as u32;
    let y0 = b.y.max(0.0) as u32;
    let x1 = ((b.x + b.w).max(0.0) as u32).min(img.width());
    let y1 = ((b.y + b.h).max(0.0) as u32).min(img.height());
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            if (p[0] < 235 || p[1] < 235 || p[2] < 235) && p[3] > 0 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn 选中checkbox勾图元进入真实像素() {
    let (img, lay) = render_state(
        "<body><input id='ok' type='checkbox' style='width:40px;height:40px'></body>",
        &[("#ok", "true")],
    );
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::CheckMark,
                ..
            }
        )),
        "布局显示列表应含勾图元"
    );
    assert!(ink_count(&img, &lay, "#ok") > 8, "勾图元应产生可见像素");
}

#[test]
fn select箭头图元进入真实像素() {
    let (img, lay) = render_state(
        "<body><select id='s' style='width:120px;height:32px'><option>甲</option><option>乙</option></select></body>",
        &[("#s", "乙")],
    );
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::Caret,
                ..
            }
        )),
        "布局显示列表应含箭头图元"
    );
    assert!(ink_count(&img, &lay, "#s") > 8, "select 箭头/文本应产生可见像素");
}

/// 第 41 轮:新增控件图元必须落到真实像素。
#[test]
fn 滑块拇指图元进入真实像素() {
    let (img, lay) = render_state(
        "<body><input id='r' type='range' min='0' max='100' value='50' style='width:160px;height:30px'></body>",
        &[],
    );
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::SliderThumb,
                ..
            }
        )),
        "布局显示列表应含滑块拇指图元"
    );
    assert!(ink_count(&img, &lay, "#r") > 8, "滑块拇指应产生可见像素");
}

#[test]
fn 日历图标图元进入真实像素() {
    let (img, lay) = render_state(
        "<body><input id='d' type='date' value='2026-10-29' style='width:160px;height:30px'></body>",
        &[],
    );
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::CalendarIcon,
                ..
            }
        )),
        "布局显示列表应含日历图标图元"
    );
    assert!(ink_count(&img, &lay, "#d") > 8, "日历图标应产生可见像素");
}

#[test]
fn 数字微调箭头图元进入真实像素() {
    let (img, lay) = render_state(
        "<body><input id='n' type='number' value='7' style='width:120px;height:30px'></body>",
        &[],
    );
    assert!(
        lay.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::CaretUp,
                ..
            }
        )),
        "布局显示列表应含上箭头图元"
    );
    assert!(ink_count(&img, &lay, "#n") > 8, "微调箭头应产生可见像素");
}

#[test]
fn 颜色色块进入真实像素() {
    let (img, lay) = render_state(
        "<body><input id='c' type='color' value='#0000ff' style='width:60px;height:30px'></body>",
        &[],
    );
    let blue = img
        .pixels()
        .filter(|p| p[2] > 200 && p[0] < 80 && p[1] < 80)
        .count();
    assert!(blue > 20, "颜色选择器色块应产生蓝色像素,实际:{blue}");
    let _ = lay;
}
