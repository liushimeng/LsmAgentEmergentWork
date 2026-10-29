//! Web 控件状态与显示回归：运行时值 → 样式/布局 → 显示列表的垂直闭环。

use std::collections::HashMap;

use super::super::document::Document;
use super::super::render::display::{DisplayKind, WidgetGlyphKind};
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn state_doc(html: &str, values: &[(&str, &str)], open: &[(&str, bool)]) -> Document {
    let mut doc = Document::parse(html, "https://a.test/");
    let values: HashMap<String, String> = values
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let open: HashMap<String, bool> = open
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
    doc.apply_widget_state(&values, &open, &HashMap::new());
    doc
}

#[test]
fn 运行时输入值覆盖DOM并进入绘制文本() {
    let doc = state_doc(
        "<body><input id='q' value='旧值'></body>",
        &[("#q", "新值")],
        &[],
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout.lines.iter().any(|l| l.text.contains("新值")),
        "截图文本必须读取运行时值"
    );
    assert!(
        !layout.lines.iter().any(|l| l.text.contains("旧值")),
        "旧 DOM value 不得继续出现在绘制文本中"
    );
    let input_box = layout
        .boxes
        .iter()
        .find(|b| b.tag == "input")
        .expect("输入框应产生几何盒");
    assert!(input_box.w > 10.0 && input_box.h > 5.0, "输入框几何不应塌缩:{input_box:?}");
}

#[test]
fn checkbox与radio选中态画几何图元() {
    let doc = state_doc(
        "<body><input id='ok' type='checkbox'><input id='r1' type='radio'></body>",
        &[("#ok", "true"), ("#r1", "true")],
        &[],
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::CheckMark,
                ..
            }
        )),
        "checkbox 选中态应画勾图元"
    );
    assert!(
        layout.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::RadioDot,
                ..
            }
        )),
        "radio 选中态应画圆点图元"
    );
}

#[test]
fn select只绘制当前选中项并带箭头() {
    let doc = state_doc(
        "<body><select id='s'><option value='a'>AAAA</option><option value='b'>BBBB</option><option value='c'>CCCC</option></select></body>",
        &[("#s", "b")],
        &[],
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(layout.lines.iter().any(|l| l.text.contains("BBBB")));
    assert!(!layout.lines.iter().any(|l| l.text.contains("AAAA")));
    assert!(!layout.lines.iter().any(|l| l.text.contains("CCCC")));
    assert!(
        layout.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::Caret,
                ..
            }
        )),
        "select 应画下拉箭头"
    );
}

#[test]
fn textarea按盒宽多行换行() {
    let doc = state_doc(
        "<body><textarea id='t' style='width:90px;height:80px'>one two three four five six seven</textarea></body>",
        &[],
        &[],
    );
    let layout = compute(&doc, 800.0, &font());
    let rows = layout
        .lines
        .iter()
        .filter(|l| ["one", "two", "three", "four", "five", "six", "seven"].iter().any(|w| l.text.contains(w)))
        .count();
    assert!(rows >= 2, "textarea 内容应按盒宽换行,实际行数:{rows}");
}

#[test]
fn details关闭时隐藏正文并保留summary() {
    let closed = state_doc(
        "<body><details id='d'><summary>标题</summary><p>正文</p></details></body>",
        &[],
        &[("#d", false)],
    );
    let layout = compute(&closed, 800.0, &font());
    assert!(layout.lines.iter().any(|l| l.text.contains("标题")));
    assert!(!layout.lines.iter().any(|l| l.text.contains("正文")));

    let opened = state_doc(
        "<body><details id='d'><summary>标题</summary><p>正文</p></details></body>",
        &[],
        &[("#d", true)],
    );
    let layout = compute(&opened, 800.0, &font());
    assert!(layout.lines.iter().any(|l| l.text.contains("正文")));
}

#[test]
fn dialog无open不渲染有open显示() {
    let closed = state_doc("<body><dialog id='d'>弹窗</dialog></body>", &[], &[]);
    let layout = compute(&closed, 800.0, &font());
    assert!(!layout.lines.iter().any(|l| l.text.contains("弹窗")));

    let opened = state_doc("<body><dialog id='d'>弹窗</dialog></body>", &[], &[("#d", true)]);
    let layout = compute(&opened, 800.0, &font());
    assert!(layout.lines.iter().any(|l| l.text.contains("弹窗")));
}

#[test]
fn canvas使用HTML固有尺寸() {
    let doc = state_doc("<body><canvas id='c' width='120' height='80'></canvas></body>", &[], &[]);
    let layout = compute(&doc, 800.0, &font());
    let box_ = layout
        .boxes
        .iter()
        .find(|b| b.tag == "canvas")
        .expect("canvas 应产生几何盒");
    assert!((box_.w - 120.0).abs() < 0.5, "canvas 宽应读取 HTML width:{:?}", box_);
    assert!((box_.h - 80.0).abs() < 0.5, "canvas 高应读取 HTML height:{:?}", box_);
}

#[test]
fn 列表绘制disc与decimal_marker() {
    let doc = state_doc(
        "<body><ul><li>项目</li></ul><ol><li>第一</li><li>第二</li></ol></body>",
        &[],
        &[],
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout.display.iter().any(|it| matches!(
            it.kind,
            DisplayKind::WidgetGlyph {
                glyph: WidgetGlyphKind::Bullet,
                ..
            }
        )),
        "ul 应画 disc marker"
    );
    assert!(layout.lines.iter().any(|l| l.text == "1."));
    assert!(layout.lines.iter().any(|l| l.text == "2."));
}

#[test]
fn progress与meter画轨道和比例填充() {
    let doc = state_doc(
        "<body><progress value='3' max='4'></progress><meter value='2' min='0' max='4'></meter></body>",
        &[],
        &[],
    );
    let layout = compute(&doc, 800.0, &font());
    let fills = layout
        .display
        .iter()
        .filter(|it| matches!(it.kind, DisplayKind::Background { .. }))
        .count();
    assert!(fills >= 4, "progress/meter 至少各画轨道与填充,实际背景项:{fills}");
}
