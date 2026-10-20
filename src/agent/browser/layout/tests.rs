//! 布局引擎单测:块级流(存量)+ Flex + 定位(阶段三)。

use super::super::document::Document;
use super::super::render::FontSet;
use super::compute;

fn font() -> FontSet {
    FontSet::fallback()
}

// ===================== 块级流(存量,自原 layout.rs 搬入) =====================

#[test]
fn 块级元素产生多行() {
    let doc = Document::parse(
        "<body><h1>Title</h1><p>Hello world</p><div><a href='/x'>link</a></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(layout.lines.iter().any(|l| l.text == "Title"));
    assert!(layout.lines.iter().any(|l| l.text.contains("Hello")));
    assert!(layout.boxes.iter().any(|b| b.tag == "h1"));
    assert!(layout.height > 40.0);
}

#[test]
fn 长文本按宽度换行() {
    let text = "word ".repeat(400);
    let html = format!("<body><p>{text}</p></body>");
    let doc = Document::parse(&html, "https://a.test/");
    let layout = compute(&doc, 400.0, &font());
    assert!(layout.lines.len() > 5);
}

#[test]
fn display_none_不参与布局() {
    let doc = Document::parse(
        "<body><div style='display:none'>hidden</div><p>shown</p></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(!layout.lines.iter().any(|l| l.text.contains("hidden")));
    assert!(layout.lines.iter().any(|l| l.text.contains("shown")));
}

#[test]
fn 样式表字号生效() {
    let doc = Document::parse(
        "<head><style>h1{font-size:40px;color:#ff0000}</style></head><body><h1>X</h1></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let h1 = layout.lines.iter().find(|l| l.text == "X").unwrap();
    assert!((h1.size - 40.0).abs() < 0.1);
    assert_eq!(h1.color, [255, 0, 0]);
}

#[test]
fn 继承的颜色与字号作用于子元素() {
    let doc = Document::parse(
        "<head><style>body{color:#00ff00;font-size:24px}</style></head>\
         <body><div><span>Child</span></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let line = layout.lines.iter().find(|l| l.text == "Child").unwrap();
    assert_eq!(line.color, [0, 255, 0]);
    assert!((line.size - 24.0).abs() < 0.1);
}

#[test]
fn visibility_hidden_保留布局空间() {
    let doc = Document::parse(
        "<body><div style='visibility:hidden'>ghost</div><p>after</p></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let ghost = layout.lines.iter().find(|l| l.text.contains("ghost")).unwrap();
    assert!(!ghost.visible);
    assert!(layout.lines.iter().any(|l| l.text.contains("after")));
}

#[test]
fn text_align_center_偏移行起点() {
    let doc = Document::parse(
        "<body><p style='text-align:center;width:400px'>mid</p></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let line = layout.lines.iter().find(|l| l.text == "mid").unwrap();
    assert!(line.x > 100.0);
}

// ===================== Flex(阶段三) =====================

#[test]
fn flex_grow_均分剩余空间() {
    let doc = Document::parse(
        "<body><div style='display:flex;width:600px'>\
         <div id=a style='flex:1'>A</div><div id=b style='flex:1'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // 600px 容器,两个 grow=1 条目各 300。
    assert!((a.2 - 300.0).abs() < 1.0, "a.w={}", a.2);
    assert!((b.2 - 300.0).abs() < 1.0, "b.w={}", b.2);
    // 同行:纵坐标一致,横向相邻。
    assert!((a.1 - b.1).abs() < 1.0);
    assert!(b.0 > a.0);
}

#[test]
fn flex_固定宽度加_grow() {
    let doc = Document::parse(
        "<body><div style='display:flex;width:500px'>\
         <div id=a style='width:100px'>A</div><div id=b style='flex:1'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!((a.2 - 100.0).abs() < 1.0, "a.w={}", a.2);
    assert!((b.2 - 400.0).abs() < 1.0, "b.w={}", b.2);
}

#[test]
fn justify_content_center_条目整体居中() {
    let doc = Document::parse(
        "<body><div style='display:flex;justify-content:center;width:600px'>\
         <div id=a style='width:100px'>A</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    // 内容区起点 x=8,余量 500,居中偏移 250。
    assert!(a.0 > 240.0 && a.0 < 270.0, "a.x={}", a.0);
}

#[test]
fn justify_content_space_between_两端对齐() {
    let doc = Document::parse(
        "<body><div style='display:flex;justify-content:space-between;width:608px'>\
         <div id=a style='width:50px'>A</div><div id=b style='width:50px'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // 内容区 608 宽,条目各 50,间隔 508。
    assert!((a.0 - 8.0).abs() < 2.0, "a.x={}", a.0);
    assert!((b.0 - 566.0).abs() < 2.0, "b.x={}", b.0);
}

#[test]
fn flex_gap_条目间隔生效() {
    let doc = Document::parse(
        "<body><div style='display:flex;gap:20px;width:600px'>\
         <div id=a style='width:100px'>A</div><div id=b style='width:100px'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    let spacing = b.0 - (a.0 + a.2);
    assert!((spacing - 20.0).abs() < 1.0, "spacing={spacing}");
}

#[test]
fn flex_wrap_溢出断行() {
    let doc = Document::parse(
        "<body><div style='display:flex;flex-wrap:wrap;width:300px'>\
         <div id=a style='width:200px;height:40px'>A</div>\
         <div id=b style='width:200px;height:40px'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // 300 内容宽放不下两个 200 条目,B 必须换行到下方。
    assert!(b.1 > a.1 + 30.0, "a.y={} b.y={}", a.1, b.1);
}

#[test]
fn flex_direction_column_纵向排列() {
    let doc = Document::parse(
        "<body><div style='display:flex;flex-direction:column'>\
         <div id=a style='height:40px'>A</div><div id=b style='height:50px'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!(b.1 > a.1 + 35.0, "a.y={} b.y={}", a.1, b.1);
    // 列方向宽度默认 stretch 到容器宽。
    assert!(a.2 > 700.0, "a.w={}", a.2);
}

#[test]
fn flex_容器高度覆盖条目() {
    let doc = Document::parse(
        "<body><div id=f style='display:flex;height:300px;width:200px'>\
         <div style='flex:1'>A</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let f = layout.rect_for("#f").unwrap();
    assert!((f.3 - 300.0).abs() < 2.0, "f.h={}", f.3);
}

#[test]
fn flex_匿名文本条目成行() {
    let doc = Document::parse(
        "<body><div style='display:flex;width:400px'>hello<b>world</b></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let hello = layout.lines.iter().find(|l| l.text == "hello").unwrap();
    let world = layout.lines.iter().find(|l| l.text == "world").unwrap();
    // 匿名文本与 b 元素同行(纵坐标一致)。
    assert!((hello.y - world.y).abs() < 1.0, "hello.y={} world.y={}", hello.y, world.y);
    assert!(world.x > hello.x);
}

#[test]
fn flex_order_重排条目() {
    let doc = Document::parse(
        "<body><div style='display:flex;width:600px'>\
         <div id=a style='order:2;width:100px'>A</div>\
         <div id=b style='order:1;width:100px'>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!(b.0 < a.0, "b.x={} a.x={}", b.0, a.0);
}

// ===================== 定位(阶段三) =====================

#[test]
fn relative_偏移几何不影响占位() {
    let doc = Document::parse(
        "<body><div id=r style='position:relative;left:50px;top:10px;width:200px'>R</div>\
         <p id=after>after</p></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let r = layout.rect_for("#r").unwrap();
    let line = layout.lines.iter().find(|l| l.text == "R").unwrap();
    // 盒与文本一起右移 50。
    assert!((r.0 - (8.0 + 50.0)).abs() < 2.0, "r.x={}", r.0);
    assert!(line.x > 50.0, "line.x={}", line.x);
    // 后续段落仍按原占位接排(y 不受偏移影响)。
    let after = layout.lines.iter().find(|l| l.text == "after").unwrap();
    let r_end = r.1 + r.3;
    assert!(after.y >= r_end - 20.0, "after.y={} r_end={}", after.y, r_end);
}

#[test]
fn absolute_脱离流不占位() {
    let doc = Document::parse(
        "<body><div style='position:relative;width:400px;height:200px'>\
         <div id=a style='position:absolute;left:10px;top:20px;width:100px;height:50px'>A</div>\
         </div><p id=after>after</p></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    // 包含块 = relative 祖先 padding 盒:(8+0, ?, 400)。left=10 → x=8+10=18。
    assert!((a.0 - 18.0).abs() < 2.0, "a.x={}", a.0);
    // after 不受 absolute 影响,紧跟容器之后。
    let after = layout.rect_for("#after").unwrap();
    assert!(after.1 < a.1 + 200.0, "after.y={} a.y={}", after.1, a.1);
}

#[test]
fn absolute_收缩适配宽度() {
    let doc = Document::parse(
        "<body><div style='position:relative;width:600px'>\
         <div id=a style='position:absolute;left:0;top:0'>short</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    // 收缩适配:宽度贴近文本宽度,远小于包含块 600。
    assert!(a.2 > 20.0 && a.2 < 200.0, "a.w={}", a.2);
}

#[test]
fn fixed_以视口为包含块() {
    let doc = Document::parse(
        "<body><div style='margin-left:100px;width:400px'>\
         <div id=f style='position:fixed;right:10px;top:5px;width:50px;height:30px'>F</div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let f = layout.rect_for("#f").unwrap();
    // right:10 → x = 800 - 10 - 50 = 740;不受祖先 margin 影响。
    assert!((f.0 - 740.0).abs() < 2.0, "f.x={}", f.0);
    // fixed 以视口为包含块,top:5 → y=5(不受 body 边距影响)。
    assert!((f.1 - 5.0).abs() < 2.0, "f.y={}", f.1);
}

#[test]
fn z_index_绘制序排序() {
    let doc = Document::parse(
        "<body><div id=low style='position:absolute;z-index:-1;left:0;top:0;width:50px;height:50px'>L</div>\
         <div id=high style='position:absolute;z-index:5;left:0;top:0;width:50px;height:50px'>H</div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let low = layout.boxes.iter().position(|b| b.selector == "#low").unwrap();
    let high = layout.boxes.iter().position(|b| b.selector == "#high").unwrap();
    assert!(low < high);
}
