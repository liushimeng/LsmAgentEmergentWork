//! 行内排版第二层回归(第 47 轮):行内块独立格式化上下文 + 行内元素片段盒
//! + 窄容器换行正确性。
//!
//! 覆盖:
//! - **行内块容器**(`display:inline-block`/`inline-flex`)子代进独立行内缓冲、
//!   按盒内容宽换行、盒高由内容决定(此前子代文本漏进父行、多行内容溢出盒外、
//!   盒高恒为一行高);
//! - **行内片段盒**:普通行内元素的背景/边框按**行盒片段**绘制 —— 位置跟随
//!   文本(不再画在行首)、高度含上下内边距、多行时每行一段;
//! - **窄容器换行**:断行后清零待入段宽 + 词界回退(此前从第二行起逐字断行);
//! - `<br>` 作为块容器直接子级时真实换行(此前是静默 no-op)。

use super::super::document::Document;
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

fn layout(html: &str) -> super::Layout {
    let doc = Document::parse(html, "https://a.test/");
    compute(&doc, 800.0, &font())
}

fn layout_wide(html: &str) -> super::Layout {
    let doc = Document::parse(html, "https://a.test/");
    compute(&doc, 1920.0, &font())
}

/// 取文本行(含指定子串)。
fn line_of(lay: &super::Layout, needle: &str) -> super::TextLine {
    lay.lines
        .iter()
        .find(|l| l.text.contains(needle))
        .unwrap_or_else(|| panic!("缺少文本行 {needle}"))
        .clone()
}

/// 按**稳定选择器**取盒(与 `Layout::rect_for` 同源)。
fn 盒_of(lay: &super::Layout, selector: &str) -> super::ElementBox {
    let (x, y, w, h) = lay
        .rect_for(selector)
        .unwrap_or_else(|| panic!("缺少盒子 {selector}"));
    super::ElementBox {
        selector: selector.to_string(),
        tag: String::new(),
        x,
        y,
        w,
        h,
        background: None,
        background_alpha: 255,
        visible: true,
        z_index: 0,
        out_of_flow: false,
    }
}

/// 按**标签名**取首个盒。
fn box_of(lay: &super::Layout, tag: &str) -> super::ElementBox {
    lay.boxes
        .iter()
        .find(|b| b.tag.eq_ignore_ascii_case(tag))
        .unwrap_or_else(|| panic!("缺少 {tag} 盒"))
        .clone()
}

/// `<body>` 下首个 `<p>` 的稳定选择器。
const P_SEL: &str = "html > body > p";

// ===================== 行内块独立格式化上下文 =====================

#[test]
fn 行内块_子代文本不再漏进父行() {
    // 第 47 轮之前:子代文本进**父**行内缓冲,定位取 `flow_x.max(run.x)`,
    // 段中链接前的文字一旦把 flow_x 推过盒左缘,子代文本就落到盒外。
    let lay = layout(
        "<body><p>Hello <span id='ib' style='display:inline-block;width:60px'>x</span> World</p></body>",
    );
    let x = line_of(&lay, "x");
    let ib = 盒_of(&lay, "#ib");
    assert!(
        x.x >= ib.x - 0.5 && x.x < ib.x + ib.w - 0.5,
        "子代文本应落在盒内:x={} 盒={:?}",
        x.x,
        (ib.x, ib.w)
    );
    // 后续兄弟仍排在盒之后,不与盒重叠。
    let world = line_of(&lay, "World");
    assert!(
        world.x >= ib.x + ib.w - 0.5,
        "盒后文本应排在盒右缘之后:world={} 盒右={}",
        world.x,
        ib.x + ib.w
    );
}

#[test]
fn 行内块_多行内容盒高由内容决定() {
    // 显式高缺省时,盒高必须由子代内容决定 —— 此前恒为 `font-size × 1.45`
    // 一行高,多行内容整块溢出盒外(「显示不全」里最难自查的一类)。
    let lay = layout(
        "<body><p>Hi <span id='ib' style='display:inline-block;width:60px'>one two three four five six</span> ok</p></body>",
    );
    let ib = 盒_of(&lay, "#ib");
    let lines: Vec<&super::TextLine> = lay
        .lines
        .iter()
        .filter(|l| {
            ["one", "two", "three", "four", "five", "six"]
                .iter()
                .any(|w| l.text.contains(w))
        })
        .collect();
    assert!(lines.len() >= 2, "窄盒内文本应换行,实际行数 {}", lines.len());
    let content_bottom = lines.iter().map(|l| l.y + l.size).fold(0.0f64, f64::max);
    assert!(
        content_bottom <= ib.y + ib.h + 1.0,
        "多行内容不得溢出盒底:内容底={} 盒底={}",
        content_bottom,
        ib.y + ib.h
    );
    // 盒高应显著高于「一行高」(16 × 1.45 = 23.2)。
    assert!(ib.h > 30.0, "多行内容盒高应由内容决定:实际 h={}", ib.h);
}

#[test]
fn 行内块_显式高优先于内容高() {
    let lay = layout(
        "<body><p>Hi <span id='ib' style='display:inline-block;width:60px;height:40px'>one two three</span> ok</p></body>",
    );
    let ib = 盒_of(&lay, "#ib");
    assert!((ib.h - 40.0).abs() < 0.5, "显式高应优先:实际 h={}", ib.h);
}

#[test]
fn 行内块_shrink_to_fit取子树累加宽() {
    // 猎聘行业 tab 同构:图标盒 20 + 间距 8 + 文本。此前取 `text_contents()`
    // 的文本宽,漏掉图标 —— 盒窄了 28px,子代在盒内换行溢出。
    let lay = layout_wide(
        "<body><div style='width:400px'>\
         <span id='tab' style='display:inline-block'>\
         <span id='ico' style='width:20px;display:inline-block'>i</span>\
         <span id='txt' style='margin-left:8px'>人工智能</span>\
         </span></div></body>",
    );
    let tab = 盒_of(&lay, "#tab");
    let ico = 盒_of(&lay, "#ico");
    let txt = 盒_of(&lay, "#txt");
    assert!(
        tab.w >= ico.w + 8.0 + txt.w - 1.0,
        "盒宽应覆盖「图标 + 间距 + 文本」:盒={} 图标={} 文本={}",
        tab.w,
        ico.w,
        txt.w
    );
    // 子代应排在盒内,不溢出右缘。
    assert!(
        txt.x + txt.w <= tab.x + tab.w + 1.0,
        "文本不得溢出盒右缘:文本右={} 盒右={}",
        txt.x + txt.w,
        tab.x + tab.w
    );
    // 图标与文本应排在同一行(猎聘 tab 是横排条目)。
    assert!(
        (ico.y - txt.y).abs() < 1.0,
        "图标与文本应同一行:ico={} txt={}",
        ico.y,
        txt.y
    );
}

#[test]
fn 行内块_段首不因盒高下沉() {
    // 回归第 44 轮:盒高不得把块游标提前推下去。
    let lay = layout(
        "<body><p>Hi <span id='ib' style='display:inline-block;width:60px;height:40px'>one two three</span> ok</p></body>",
    );
    let first = line_of(&lay, "Hi");
    let p = 盒_of(&lay, P_SEL);
    assert!(
        (first.y - p.y).abs() < 0.5,
        "段首不应下沉:first={} p={}",
        first.y,
        p.y
    );
}

#[test]
fn 浮动行内块_与后续浮动同行() {
    // 百度地图搜索卡片同构:行内块内容盒 + 浮动按钮应同一行、按钮贴盒右缘。
    let lay = layout(
        "<body><div id='card' style='width:425px;height:44px'>\
         <div id='content' style='display:inline-block;width:368px;height:40px'>\
         <input id='q' style='float:left;width:329px;height:38px'>\
         <div id='route' style='float:left;width:39px;height:38px'></div>\
         </div>\
         <button id='go' style='float:left;width:57px;height:38px'></button>\
         </div></body>",
    );
    let content = 盒_of(&lay, "#content");
    let go = 盒_of(&lay, "#go");
    assert!((go.y - content.y).abs() < 0.5, "浮动应与行内块同行");
    assert!(
        (go.x - (content.x + content.w)).abs() < 0.5,
        "浮动应贴行内块右缘:go={} 盒右={}",
        go.x,
        content.x + content.w
    );
}

// ===================== 行内片段盒 =====================

#[test]
fn 行内片段盒_背景跟随文本位置() {
    // 修复前:盒画在**行首**(收集期 line_x),链接背景整块盖到「Hello」上。
    let lay = layout(
        "<body><p>Hello <a id='lk' href='#' style='background:#ff0000'>link</a> World</p></body>",
    );
    let lk = 盒_of(&lay, "#lk");
    let link = line_of(&lay, "link");
    let hello = line_of(&lay, "Hello");
    assert!(
        lk.x > hello.x + 1.0,
        "链接背景不得盖住前一个词:盒 x={} Hello x={}",
        lk.x,
        hello.x
    );
    assert!(
        (lk.x - link.x).abs() < 1.0,
        "背景应与链接文字对齐:盒 x={} 文字 x={}",
        lk.x,
        link.x
    );
}

#[test]
fn 行内片段盒_高度计入上下内边距() {
    // CSS 2.1 §10.6.1:上下内边距让背景上下溢出(但不影响行高)。
    let lay = layout("<body><p>x <span id='s' style='background:#00ff00;padding:10px 0'>pad</span> y</p></body>");
    let s = 盒_of(&lay, "#s");
    assert!(s.h > 10.0, "背景高度应含上下内边距:实际 h={}", s.h);
    let p_pad = 盒_of(&lay, P_SEL);
    let plain = layout("<body><p>x <span id='s'>pad</span> y</p></body>");
    let p_plain = 盒_of(&plain, P_SEL);
    // 垂直内边距**不影响行高**:两个段落盒高应接近。
    assert!(
        (p_pad.h - p_plain.h).abs() < 1.0,
        "垂直内边距不应撑高段落:pad={} plain={}",
        p_pad.h,
        p_plain.h
    );
}

#[test]
fn 行内片段盒_多行每行一段() {
    // 跨两行的链接:每行都应有自己的片段盒(此前只有第一行、且画在行首)。
    let lay = layout(
        "<body><p style='width:80px'>aaa \
         <a id='lk' href='#' style='background:#0000ff'>one two three four five six seven eight</a>\
         bbb</p></body>",
    );
    let frags: Vec<&super::ElementBox> = lay
        .boxes
        .iter()
        .filter(|b| b.selector == "#lk")
        .collect();
    assert!(
        frags.len() >= 2,
        "跨行链接应按行片段各画一段:实际段数 {}",
        frags.len()
    );
    // 各片段应分处不同 y。
    let mut ys: Vec<f64> = frags.iter().map(|b| b.y).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    assert!(ys.len() >= 2, "片段应分处不同行:y={ys:?}");
    // 链接文字应跨多行。
    let link_lines: Vec<&super::TextLine> = lay
        .lines
        .iter()
        .filter(|l| l.x >= frags[0].x - 1.0 && l.x <= frags[0].x + 200.0)
        .collect();
    assert!(
        link_lines.len() >= 2,
        "链接文字应跨多行:实际 {} 行",
        link_lines.len()
    );
    // 每个片段都应落在链接文字所在的行内。
    for f in &frags {
        let hit = link_lines
            .iter()
            .any(|l| (f.y - (l.y + l.size * 0.5)).abs() < 24.0);
        assert!(hit, "片段应落在链接文字行内:片段 y={}", f.y);
    }
}

#[test]
fn 行内片段盒_无绘制内容仍保寻址盒() {
    // 纯寻址(无背景/边框)的普通行内元素仍要有盒,否则 rect_for 恒 None。
    let lay = layout("<body><div><a id='lk' href='/x'>link</a></div></body>");
    let lk = 盒_of(&lay, "#lk");
    assert!(lk.w > 0.0 && lk.h > 0.0, "寻址盒应非零:实际 {:?}", (lk.w, lk.h));
    let link = line_of(&lay, "link");
    assert!(
        (lk.x - link.x).abs() < 2.0,
        "寻址盒应与文字对齐:盒 x={} 文字 x={}",
        lk.x,
        link.x
    );
}

#[test]
fn 空锚点仍发射寻址盒且不产生幽灵行高() {
    // 第 175 轮回归:空行内元素无文本可挂片段,退回单盒路径。
    let lay = layout(
        "<html><body><div id='strip'><a name='x'></a></div><div id='after'>y</div></body></html>",
    );
    let strip = 盒_of(&lay, "#strip");
    assert!(strip.h <= 1.5, "空锚点容器高度应≈0:实际 {strip:?}");
    assert!(
        lay.boxes.iter().any(|b| b.tag == "a"),
        "空锚点仍应发射寻址盒"
    );
}

#[test]
fn 行内片段盒_绘制真实背景显示项() {
    use super::super::render::display::DisplayKind;
    let lay = layout(
        "<body><p>Hello <span id='s' style='background:#123456'>tag</span> World</p></body>",
    );
    let s = 盒_of(&lay, "#s");
    let bg = lay.display.iter().find(|it| {
        matches!(it.kind, DisplayKind::Background { .. })
            && (it.x - s.x).abs() < 1.0
            && (it.w - s.w).abs() < 1.0
    });
    assert!(bg.is_some(), "片段盒应发射背景显示项");
}

#[test]
fn 行内片段盒_无绘制内容不产生背景项() {
    use super::super::render::display::DisplayKind;
    let lay = layout("<body><p>Hello <span id='s'>tag</span> World</p></body>");
    let s = 盒_of(&lay, "#s");
    let bg = lay.display.iter().find(|it| {
        matches!(it.kind, DisplayKind::Background { .. }) && (it.x - s.x).abs() < 1.0
    });
    assert!(bg.is_none(), "无背景样式的行内元素不应发射背景项");
}

// ===================== 窄容器换行 =====================

#[test]
fn 窄容器_断行后不再逐字断行() {
    // 第 47 轮根因:`seg_w` 断行后未清零,下一行拿上一行的累计宽当基数,
    // 从第二行起**逐字断行** —— 窄容器(侧栏/药丸/代码块)整段不可读。
    let lay = layout("<body><p style='width:80px'>one two three four five six</p></body>");
    assert!(
        lay.lines.len() >= 3,
        "80px 容器内应折成多行,实际 {} 行",
        lay.lines.len()
    );
    for l in &lay.lines {
        assert!(
            l.text.chars().count() >= 3,
            "每行应容纳多个字符,实际 {:?}",
            l.text
        );
    }
}

#[test]
fn 窄容器_按词界断行() {
    let lay = layout("<body><p style='width:80px'>one two three four five six</p></body>");
    // 词界回退:每行末尾不应出现「半个词」(每行都应是完整单词序列)。
    for l in &lay.lines {
        let t = l.text.trim();
        assert!(!t.is_empty(), "不应产生空行");
        // 行内只含完整词与空格(词间单空格)。
        let double_space = t.contains("  ");
        assert!(!double_space, "词界回退不应留下连续空格:{t:?}");
    }
    // 所有行的文本拼接后应覆盖原文(不回退丢失字符)。
    let joined: String = lay
        .lines
        .iter()
        .map(|l| l.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("one") && joined.contains("six"),
        "折行不得丢字:{joined:?}"
    );
}

#[test]
fn 宽容器_单行不误断() {
    // 回归:词界回退不得把不溢出的长 run 拆成多行。
    let lay = layout("<body><p>hello world</p></body>");
    assert_eq!(lay.lines.len(), 1, "宽容器内不应折行:{:?}", lay.lines);
}

// ===================== br 换行 =====================

#[test]
fn br_作为块容器直接子级时换行() {
    // 修复前:`<br>` 只有嵌套进行内元素时才生效,块容器直接子级是静默 no-op。
    let lay = layout("<body><div>a<br>b</div></body>");
    let a = line_of(&lay, "a");
    let b = line_of(&lay, "b");
    assert!(b.y > a.y + 1.0, "`<br>` 应真正换行:a.y={} b.y={}", a.y, b.y);
}

#[test]
fn br_嵌套进行内元素内仍换行() {
    let lay = layout("<body><div><span>a<br>b</span></div></body>");
    let a = line_of(&lay, "a");
    let b = line_of(&lay, "b");
    assert!(b.y > a.y + 1.0, "行内元素内的 `<br>` 应换行");
}
