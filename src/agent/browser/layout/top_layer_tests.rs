//! 顶层(top layer)与浮层逃逸回归(第 41 轮)。
//!
//! 覆盖:模态对话框的顶层居中 + `::backdrop` 遮罩、`position:fixed`
//! 逃出祖先 `overflow:hidden` 裁剪、`select` 展开浮层面板。

use std::collections::HashMap;

use super::super::document::Document;
use super::super::render::display::{BgFill, DisplayKind};
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn layout(html: &str) -> super::Layout {
    let doc = Document::parse(html, "https://a.test/");
    compute(&doc, 800.0, &font())
}

#[test]
fn 打开的对话框走顶层绘制层() {
    let lay = layout("<body><dialog id='d' open>弹窗内容</dialog></body>");
    assert!(
        lay.lines.iter().any(|l| l.text.contains("弹窗内容")),
        "打开的 dialog 必须渲染内容"
    );
    let top_items = lay
        .display
        .iter()
        .filter(|it| it.layer >= 2)
        .count();
    assert!(top_items > 0, "dialog 的显示项必须在顶层绘制层(layer>=2)");
}

#[test]
fn 模态对话框铺backdrop而非模态对话框不铺() {
    // 浏览器语义:只有 `showModal()` 打开的模态对话框有 `::backdrop`;
    // 单纯 `open` 属性的对话框铺遮罩会把整页盖灰。
    let modal = layout("<body><dialog id='d' open data-lsm-modal>弹窗内容</dialog></body>");
    let backdrop = modal.display.iter().any(|it| {
        it.layer >= 2
            && it.w >= 700.0
            && it.h >= 500.0
            && matches!(
                it.kind,
                DisplayKind::Background {
                    fill: BgFill::Solid([0, 0, 0], a),
                } if a > 0 && a < 255
            )
    });
    assert!(backdrop, "模态 dialog 必须铺 ::backdrop 半透明遮罩");

    let plain = layout("<body><dialog id='d' open>弹窗内容</dialog></body>");
    let plain_backdrop = plain.display.iter().any(|it| {
        it.layer >= 2
            && it.w >= 700.0
            && it.h >= 500.0
            && matches!(
                it.kind,
                DisplayKind::Background {
                    fill: BgFill::Solid([0, 0, 0], a),
                } if a > 0 && a < 255
            )
    });
    assert!(
        !plain_backdrop,
        "非模态 dialog 不得铺 ::backdrop(会把整页盖灰)"
    );
}

#[test]
fn 未打开的对话框不渲染() {
    let lay = layout("<body><dialog id='d'>弹窗内容</dialog></body>");
    assert!(
        !lay.lines.iter().any(|l| l.text.contains("弹窗内容")),
        "无 open 属性的 dialog 不得渲染"
    );
}

#[test]
fn 模态对话框居中于视口() {
    let lay = layout("<body><dialog id='d' open>短</dialog></body>");
    let dlg = lay
        .boxes
        .iter()
        .find(|b| b.selector == "#d")
        .expect("dialog 应产生几何盒");
    let center = dlg.x + dlg.w / 2.0;
    assert!(
        (center - 400.0).abs() < 2.0,
        "dialog 应水平居中于 800 宽视口,实际中心={center}"
    );
}

#[test]
fn 固定定位子元素逃出祖先溢出裁剪() {
    // 祖先 overflow:hidden 会裁剪常规流子元素,但 `position:fixed` 不受其裁剪。
    let lay = layout(
        "<body><div style='overflow:hidden;width:200px;height:100px'><div id='fx' style='position:fixed;left:10px;top:10px;width:50px;height:50px'>F</div></div></body>",
    );
    // 固定定位项带 `fixed` 标记 → 祖先裁剪交集跳过它(裁剪矩形不被压成空)。
    let fixed_items = lay
        .display
        .iter()
        .filter(|it| it.fixed)
        .count();
    assert!(fixed_items > 0, "固定定位子树应带 fixed 标记");
    for it in lay.display.iter().filter(|it| it.fixed) {
        let clipped_empty = it
            .clip
            .map(|(_, _, w, h)| w <= 0.0 || h <= 0.0)
            .unwrap_or(false);
        assert!(
            !clipped_empty,
            "固定定位项不得被祖先 overflow 裁成空:{:?}",
            it.clip
        );
    }
}

#[test]
fn select展开时铺出浮层面板() {
    let mut doc = Document::parse(
        "<body><select id='s'><option value='a'>甲</option><option value='b'>乙</option></select></body>",
        "https://a.test/",
    );
    let mut open: HashMap<String, bool> = HashMap::new();
    open.insert("#s".to_string(), true);
    doc.apply_widget_state(&HashMap::new(), &open, &HashMap::new());
    let lay = compute(&doc, 800.0, &font());
    let texts: Vec<&str> = lay.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(
        texts.contains(&"甲") && texts.contains(&"乙"),
        "展开面板必须列出全部 option,实际:{texts:?}"
    );
    assert!(
        lay.display.iter().any(|it| it.layer >= 2),
        "展开面板必须走顶层绘制层"
    );
}

#[test]
fn select未展开时不铺面板() {
    let lay = layout(
        "<body><select id='s'><option value='a'>甲</option><option value='b'>乙</option></select></body>",
    );
    let texts: Vec<&str> = lay.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(
        !texts.contains(&"乙"),
        "未展开时不得列出其它 option,实际:{texts:?}"
    );
}

#[test]
fn select面板option产生可寻址几何盒() {
    let mut doc = Document::parse(
        "<body><select id='s'><option value='a'>甲</option><option value='b'>乙</option></select></body>",
        "https://a.test/",
    );
    let mut open: HashMap<String, bool> = HashMap::new();
    open.insert("#s".to_string(), true);
    doc.apply_widget_state(&HashMap::new(), &open, &HashMap::new());
    let lay = compute(&doc, 800.0, &font());
    let option_boxes: Vec<_> = lay
        .boxes
        .iter()
        .filter(|b| b.tag == "option" && b.w > 0.0 && b.h > 0.0)
        .collect();
    assert_eq!(option_boxes.len(), 2, "面板 option 应各自产生几何盒:{lay:?}");
    assert!(
        option_boxes[0].y < option_boxes[1].y,
        "option 盒应按面板行序垂直排列"
    );
}

#[test]
fn select面板重复label按选中项高亮() {
    let mut doc = Document::parse(
        "<body><select id='s'><option value='a'>同名</option><option value='b'>同名</option></select></body>",
        "https://a.test/",
    );
    let mut values: HashMap<String, String> = HashMap::new();
    values.insert("#s".to_string(), "b".to_string());
    let mut open: HashMap<String, bool> = HashMap::new();
    open.insert("#s".to_string(), true);
    doc.apply_widget_state(&values, &open, &HashMap::new());
    let lay = compute(&doc, 800.0, &font());
    let highlights = lay
        .display
        .iter()
        .filter(|it| {
            it.layer >= 2
                && matches!(
                    &it.kind,
                    DisplayKind::Background {
                        fill: BgFill::Solid([0, 120, 215], 255),
                    }
                )
        })
        .count();
    assert_eq!(highlights, 1, "重复 label 时也只能高亮真实选中的一项");
}

#[test]
fn select面板视口下缘不足时向上翻转() {
    let mut doc = Document::parse(
        "<body><div style='height:650px'></div><select id='s'><option value='a'>甲</option><option value='b'>乙</option></select></body>",
        "https://a.test/",
    );
    let mut open: HashMap<String, bool> = HashMap::new();
    open.insert("#s".to_string(), true);
    doc.apply_widget_state(&HashMap::new(), &open, &HashMap::new());
    let lay = compute(&doc, 800.0, &font());
    let select_box = lay
        .boxes
        .iter()
        .find(|b| b.selector == "#s")
        .expect("select 应有几何盒");
    let first_option = lay
        .boxes
        .iter()
        .find(|b| b.tag == "option")
        .expect("展开面板应有 option 盒");
    assert!(
        first_option.y < select_box.y,
        "下方放不下时面板应翻转到 select 上方:select_y={}, option_y={}",
        select_box.y,
        first_option.y
    );
}
