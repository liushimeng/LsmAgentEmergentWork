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
    doc.apply_widget_state(&values, &HashMap::new());
    let font = FontSet::fallback();
    let lay = layout::compute(&doc, 640.0, &font);
    let pm = font
        .render_pixmap(&lay, 640, 480, 0.0, false, &ImageStore::new())
        .unwrap();
    let img = image::load_from_memory(&pm.encode_png().unwrap()).unwrap().to_rgba8();
    (img, lay)
}

fn dark_count(img: &image::RgbaImage, lay: &layout::Layout, selector: &str) -> usize {
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
            if p[0] < 90 && p[1] < 90 && p[2] < 90 && p[3] > 0 {
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
    assert!(dark_count(&img, &lay, "#ok") > 8, "勾图元应产生深色像素");
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
    assert!(dark_count(&img, &lay, "#s") > 8, "select 箭头/文本应产生深色像素");
}
