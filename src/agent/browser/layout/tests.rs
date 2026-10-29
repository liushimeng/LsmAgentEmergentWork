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

// ===================== Grid(阶段三第二步) =====================

#[test]
fn grid_无模板单列堆叠() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:300px'>\
         <div id=a>A</div><div id=b>B</div><div id=c>C</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    let c = layout.rect_for("#c").unwrap();
    // 浏览器一致:无模板单列堆叠,条目 stretch 满宽。
    assert!((a.0 - b.0).abs() < 1.0 && (b.0 - c.0).abs() < 1.0, "x: {} {} {}", a.0, b.0, c.0);
    assert!((a.2 - 300.0).abs() < 2.0, "a.w={}", a.2);
    assert!(a.1 < b.1 && b.1 < c.1, "y: {} {} {}", a.1, b.1, c.1);
}

#[test]
fn grid_固定列宽_px轨道() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:300px;grid-template-columns:100px 200px'>\
         <div id=a>A</div><div id=b>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!((a.2 - 100.0).abs() < 1.0, "a.w={}", a.2);
    assert!((b.2 - 200.0).abs() < 1.0, "b.w={}", b.2);
    assert!((b.0 - (a.0 + 100.0)).abs() < 1.0, "b.x={} a.x={}", b.0, a.0);
    // 同一行:y 一致。
    assert!((a.1 - b.1).abs() < 1.0, "a.y={} b.y={}", a.1, b.1);
}

#[test]
fn grid_fr轨道均分剩余() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:600px;grid-template-columns:1fr 2fr'>\
         <div id=a>A</div><div id=b>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // 1fr:2fr → 200:400。
    assert!((a.2 - 200.0).abs() < 2.0, "a.w={}", a.2);
    assert!((b.2 - 400.0).abs() < 2.0, "b.w={}", b.2);
}

#[test]
fn grid_repeat_展开多列() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:300px;grid-template-columns:repeat(3, 1fr)'>\
         <div id=a>A</div><div id=b>B</div><div id=c>C</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    let c = layout.rect_for("#c").unwrap();
    assert!((a.2 - 100.0).abs() < 2.0, "a.w={}", a.2);
    assert!((b.0 - (a.0 + 100.0)).abs() < 2.0, "b.x={}", b.0);
    assert!((c.0 - (a.0 + 200.0)).abs() < 2.0, "c.x={}", c.0);
    assert!((a.1 - c.1).abs() < 1.0, "同行: a.y={} c.y={}", a.1, c.1);
}

#[test]
fn grid_minmax_钳制轨道() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:420px;grid-template-columns:minmax(80px, 120px) 1fr'>\
         <div id=a>hi</div><div id=b>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // 内容极窄时 min 下限 80 兜底;1fr 吃掉剩余。
    assert!(a.2 >= 79.0 && a.2 <= 121.0, "a.w={}", a.2);
    assert!((b.0 + b.2 - (a.0 + 420.0)).abs() < 2.0, "b 右缘贴容器: {}", b.0 + b.2);
}

#[test]
fn grid_gap_行列间隔() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:220px;gap:20px;grid-template-columns:100px 100px'>\
         <div id=a>A</div><div id=b>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!((b.0 - (a.0 + 120.0)).abs() < 1.0, "gap 列间距: b.x={} a.x={}", b.0, a.0);
}

#[test]
fn grid_span_跨列() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:200px;grid-template-columns:100px 100px'>\
         <div id=a style='grid-column:span 2'>A</div>\
         <div id=b>B</div><div id=c>C</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    let c = layout.rect_for("#c").unwrap();
    assert!((a.2 - 200.0).abs() < 2.0, "a.w={}", a.2);
    // 后两个条目进入第二行。
    assert!(b.1 > a.1, "b.y={} a.y={}", b.1, a.1);
    assert!((b.0 - a.0).abs() < 1.0 && (c.0 - (a.0 + 100.0)).abs() < 1.0);
}

#[test]
fn grid_显式放置行列线() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:300px;grid-template-columns:100px 100px 100px;\
         grid-template-rows:50px 50px'>\
         <div id=a style='grid-column:3;grid-row:2'>A</div>\
         <div id=b>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // a 落在第 3 列第 2 行(1 基线号 → 0 基 2,1);容器 x=8(body 边距)。
    assert!((a.0 - 208.0).abs() < 2.0, "a.x={}", a.0);
    assert!(a.1 >= 50.0, "a.y={}", a.1);
    // b 自动放置避开 a,落第一行第一列。
    assert!((b.0 - 0.0).abs() < 2.0 || b.0 < a.0, "b.x={}", b.0);
    assert!(b.1 < a.1, "b.y={} a.y={}", b.1, a.1);
}

#[test]
fn grid_auto_flow_column_列优先() {
    let doc = Document::parse(
        "<body><div style='display:grid;grid-auto-flow:column;width:300px'>\
         <div id=a>A</div><div id=b>B</div><div id=c>C</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    let c = layout.rect_for("#c").unwrap();
    // 无显式行模板:单行横排,列优先。
    assert!((a.1 - b.1).abs() < 1.0 && (b.1 - c.1).abs() < 1.0);
    assert!(a.0 < b.0 && b.0 < c.0, "x: {} {} {}", a.0, b.0, c.0);
}

#[test]
fn grid_隐式行高由内容决定() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:200px;grid-template-columns:200px'>\
         <div id=a>x</div>\
         <div id=b>one two three four five six seven eight nine ten</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    // b 多行文本,行高 > a 单行;b 从第二行起。
    assert!(b.1 > a.1, "b.y={} a.y={}", b.1, a.1);
    assert!(b.3 > a.3, "b.h={} a.h={}", b.3, a.3);
}

#[test]
fn grid_justify_items_center_条目居中() {
    let doc = Document::parse(
        "<body><div style='display:grid;width:300px;justify-items:center;\
         grid-template-columns:300px'>\
         <div id=a style='width:100px'>A</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    // (300-100)/2 = 100 偏移。
    assert!((a.2 - 100.0).abs() < 1.0, "a.w={}", a.2);
    assert!((a.0 - 108.0).abs() < 2.0, "a.x={}", a.0);
}

#[test]
fn grid_容器高度等于行高和() {
    let doc = Document::parse(
        "<body><div id=g style='display:grid;width:200px;gap:10px;\
         grid-template-columns:200px;grid-template-rows:50px 50px'>\
         <div>A</div><div>B</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let g = layout.rect_for("#g").unwrap();
    // 50+10+50 = 110。
    assert!((g.3 - 110.0).abs() < 2.0, "g.h={}", g.3);
}

#[test]
fn grid_嵌套在_flex_内仍布局() {
    let doc = Document::parse(
        "<body><div style='display:flex;width:400px'>\
         <div id=g style='display:grid;width:200px;grid-template-columns:100px 100px'>\
         <div id=a>A</div><div id=b>B</div></div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let g = layout.rect_for("#g").unwrap();
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!((g.2 - 200.0).abs() < 2.0, "g.w={}", g.2);
    assert!((a.2 - 100.0).abs() < 1.0 && (b.0 - (a.0 + 100.0)).abs() < 1.0);
}

// ===================== 替换元素(图片,阶段三第三步) =====================

/// 构造图片固有尺寸索引(URL → 自然宽高)。
fn image_sizes(pairs: &[(&str, f64, f64)]) -> super::super::images::ImageSizes {
    let mut sizes = super::super::images::ImageSizes::default();
    for (url, w, h) in pairs {
        sizes.sizes.insert(url.to_string(), (*w, *h));
    }
    sizes
}

#[test]
fn 图片按固有尺寸成盒() {
    let doc = Document::parse("<body><img src='a.png'></body>", "https://a.test/");
    let sizes = image_sizes(&[("https://a.test/a.png", 120.0, 60.0)]);
    let layout = super::compute_full(
        &doc,
        800.0,
        &font(),
        &super::super::style::MediaCtx::new(800.0, 720.0),
        &sizes,
    );
    let img = layout.boxes.iter().find(|b| b.tag == "img").unwrap();
    assert!((img.w - 120.0).abs() < 1.0, "img.w={}", img.w);
    assert!((img.h - 60.0).abs() < 1.0, "img.h={}", img.h);
    assert_eq!(layout.paints.len(), 1);
    assert_eq!(layout.paints[0].src, "https://a.test/a.png");
}

#[test]
fn 图片_css_宽度按比例推导高度() {
    let doc = Document::parse(
        "<body><img src='a.png' style='width:200px'></body>",
        "https://a.test/",
    );
    let sizes = image_sizes(&[("https://a.test/a.png", 120.0, 60.0)]);
    let layout = super::compute_full(
        &doc,
        800.0,
        &font(),
        &super::super::style::MediaCtx::new(800.0, 720.0),
        &sizes,
    );
    let img = layout.boxes.iter().find(|b| b.tag == "img").unwrap();
    assert!((img.w - 200.0).abs() < 1.0, "img.w={}", img.w);
    assert!((img.h - 100.0).abs() < 1.0, "img.h={}", img.h);
}

#[test]
fn 图片_html_属性尺寸生效() {
    let doc = Document::parse(
        "<body><img src='a.png' width='50' height='20'></body>",
        "https://a.test/",
    );
    let sizes = image_sizes(&[("https://a.test/a.png", 120.0, 60.0)]);
    let layout = super::compute_full(
        &doc,
        800.0,
        &font(),
        &super::super::style::MediaCtx::new(800.0, 720.0),
        &sizes,
    );
    let img = layout.boxes.iter().find(|b| b.tag == "img").unwrap();
    assert!((img.w - 50.0).abs() < 1.0 && (img.h - 20.0).abs() < 1.0);
}

#[test]
fn 未加载图片不占位() {
    let doc = Document::parse("<body><img src='missing.png'></body>", "https://a.test/");
    let layout = super::compute_full(
        &doc,
        800.0,
        &font(),
        &super::super::style::MediaCtx::new(800.0, 720.0),
        &super::super::images::ImageSizes::default(),
    );
    let img = layout.boxes.iter().find(|b| b.tag == "img").unwrap();
    assert_eq!(img.w, 0.0);
    assert!(layout.paints.is_empty());
}

#[test]
fn 背景图生成绘制指令() {
    let doc = Document::parse(
        "<body><div id=d style='width:200px;height:100px;background-image:url(/bg.png)'></div></body>",
        "https://a.test/",
    );
    let layout = super::compute_full(
        &doc,
        800.0,
        &font(),
        &super::super::style::MediaCtx::new(800.0, 720.0),
        &super::super::images::ImageSizes::default(),
    );
    assert_eq!(layout.paints.len(), 1);
    assert_eq!(layout.paints[0].src, "https://a.test/bg.png");
    assert_eq!(layout.paints[0].kind, super::PaintKind::Background);
    assert!((layout.paints[0].w - 200.0).abs() < 1.0);
}

#[test]
fn flex_图片条目按固有宽度排布() {
    let doc = Document::parse(
        "<body><div style='display:flex;width:400px'>\
         <img src='a.png'><img src='b.png'></div></body>",
        "https://a.test/",
    );
    let sizes = image_sizes(&[
        ("https://a.test/a.png", 100.0, 50.0),
        ("https://a.test/b.png", 80.0, 40.0),
    ]);
    let layout = super::compute_full(
        &doc,
        800.0,
        &font(),
        &super::super::style::MediaCtx::new(800.0, 720.0),
        &sizes,
    );
    let contents: Vec<&super::PaintBox> = layout
        .paints
        .iter()
        .filter(|p| p.kind == super::PaintKind::Content)
        .collect();
    assert_eq!(contents.len(), 2);
    let a = contents.iter().find(|p| p.src.ends_with("a.png")).unwrap();
    let b = contents.iter().find(|p| p.src.ends_with("b.png")).unwrap();
    assert!((a.w - 100.0).abs() < 1.0, "a.w={}", a.w);
    assert!((b.w - 80.0).abs() < 1.0, "b.w={}", b.w);
    assert!((b.x - (a.x + 100.0)).abs() < 1.0);
}

// ===================== 行内盒与尺寸(第 174 轮) =====================

#[test]
fn 行内元素自身建盒_不再恒零尺寸() {
    // 真实站点的输入框就是这样挂在 div 下:此前行内元素自身永远拿不到盒子,
    // inspect(elements) 的 rect 恒 0×0,坐标点击被当成"点 (0,0)"。
    let doc = Document::parse(
        "<body><div><input id=a></div><div><span><button id=b>搜</button></span></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").expect("input 应有布局盒");
    assert!(a.2 > 0.0 && a.3 > 0.0, "input rect={a:?}");
    let b = layout.rect_for("#b").expect("button 应有布局盒");
    assert!(b.2 > 0.0 && b.3 > 0.0, "button rect={b:?}");
}

#[test]
fn 行内盒尊重显式宽高() {
    let doc = Document::parse(
        "<body><div><input id=a style='width:329px;height:38px'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    assert!((a.2 - 329.0).abs() < 1.0, "w={}", a.2);
    assert!((a.3 - 38.0).abs() < 1.0, "h={}", a.3);
}

#[test]
fn inline_block_作为原子内联盒建盒() {
    let doc = Document::parse(
        "<body><div><span id=a style='display:inline-block;width:60px;height:20px'>x</span></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").expect("inline-block 应有布局盒");
    assert!((a.2 - 60.0).abs() < 1.0, "w={}", a.2);
    assert!((a.3 - 20.0).abs() < 1.0, "h={}", a.3);
}

#[test]
fn 百分比宽度按父内容宽求解() {
    // 此前百分比一律按视口宽解析:`width:50%` 在 400px 的容器里也算成 400。
    let doc = Document::parse(
        "<body><div style='width:400px'><div id=a style='width:50%'>x</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    assert!((a.2 - 200.0).abs() < 1.0, "w={} (应为父内容宽 400 的一半)", a.2);
}

#[test]
fn 显式高度参与占位() {
    let doc = Document::parse(
        "<body><div id=a style='height:300px'>x</div><p id=b>after</p></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = layout.rect_for("#a").unwrap();
    let b = layout.rect_for("#b").unwrap();
    assert!((a.3 - 300.0).abs() < 2.0, "h={}", a.3);
    assert!(b.1 > a.1 + 250.0, "后续块应按声明高度下移: a.y={} b.y={}", a.1, b.1);
}

#[test]
fn 无盒元素不与零尺寸元素混淆() {
    // 被祖先 display:none 剪枝的元素没有布局盒(rect_for=None),
    // 与"有盒子但宽高为 0"必须能区分(inspect 侧用 laid_out 表达)。
    let doc = Document::parse(
        "<body><div style='display:none'><input id=a></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(layout.rect_for("#a").is_none(), "被剪枝元素不应有布局盒");
}

// ===================== 百分比高度求解 + 视口传播(根高度链) =====================

#[test]
fn 百分比高度沿根链向视口传播() {
    // html,body 100% 链:容器 100% 应解析为视口高(compute 默认视口高 720)。
    // 百度地图类 height:100% 页面此前整链退化为 auto,页面塌陷为头部高度。
    let doc = Document::parse(
        "<head><style>html,body{height:100%;margin:0}#wrap{height:100%}</style></head>\
         <body><div id='wrap'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let wrap = layout.rect_for("#wrap").unwrap();
    assert!(
        (wrap.3 - 720.0).abs() < 2.0,
        "100% 容器应撑满视口高(720),实际 {}",
        wrap.3
    );
}

#[test]
fn 百分比高度遇auto父退化为内容高() {
    // body 无显式高(auto)→ 子代 50% 高度无确定包含块,按 auto(内容高)处理。
    let doc = Document::parse(
        "<body><div style='height:50%'><p>hi</p></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let div = layout
        .boxes
        .iter()
        .find(|b| b.tag == "div")
        .expect("div 应有布局盒");
    assert!(
        div.h < 100.0,
        "auto 父下的 50% 应退化为内容高,实际 {}",
        div.h
    );
}

#[test]
fn 显式父高支撑多级百分比链() {
    // 显式 400px 父 → 子 50% = 200 → 孙 50% = 100;逐代确定高逐代传播。
    let doc = Document::parse(
        "<body><div style='height:400px'>\
            <div id='half' style='height:50%'>\
                <div id='quarter' style='height:50%'></div>\
            </div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let half = layout.rect_for("#half").unwrap();
    let quarter = layout.rect_for("#quarter").unwrap();
    assert!((half.3 - 200.0).abs() < 2.0, "50% of 400 应为 200,实际 {}", half.3);
    assert!(
        (quarter.3 - 100.0).abs() < 2.0,
        "50% of 200 应为 100,实际 {}",
        quarter.3
    );
}

#[test]
fn auto高父不向上透传祖父确定高() {
    // 祖父显式 400px → 父 auto → 子 50% 必须按 auto(内容高),
    // 不得把祖父的 400px 透传成子的包含块高(CSS 百分比高度只相对父内容高)。
    let doc = Document::parse(
        "<body><div style='height:400px'><div><div id='inner' style='height:50%'><p>x</p></div></div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let inner = layout.rect_for("#inner").unwrap();
    assert!(
        inner.3 < 100.0,
        "auto 父的子代 50% 应退化为内容高,实际 {}",
        inner.3
    );
}

#[test]
fn 定位父元素确定高时子代百分比高度可求解() {
    // #app 是 position:relative + 显式高 → 子代 height:100% 必须解析为 720。
    // 此前「定位父」一律按未知包含块高处理,#MapHolder 退化为内容高(8px),
    // 百度地图容器塌缩、地图初始化到 1px 视口。
    let doc = Document::parse(
        "<body><div id='app' style='position:relative;height:720px'>\
            <div id='mh' style='height:100%'></div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let mh = layout.rect_for("#mh").expect("mh 应有布局盒");
    assert!((mh.3 - 720.0).abs() < 2.0, "定位父下 100% 应为 720,实际 {}", mh.3);
}

// ===================== 阶段六:显示列表发射与组效果 =====================

use super::super::render::display::DisplayKind;

#[test]
fn 显示列表_父背景先于子背景() {
    // 父黑底 + 子白底:父的 Background 项 seq 必须小于子(排序后先画)。
    let doc = Document::parse(
        "<body><div style='background:#111111'><div style='background:#eeeeee'>x</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let bgs: Vec<_> = layout
        .display
        .iter()
        .filter(|i| matches!(i.kind, DisplayKind::Background { .. }))
        .collect();
    assert!(bgs.len() >= 2, "父子各应有背景项");
    // 排序稳定:先出现的是父(先序 seq 小)。深色项 w 更大(撑满父宽)。
    assert!(
        bgs[0].w >= bgs[1].w,
        "父背景应先画且更宽:{} vs {}",
        bgs[0].w,
        bgs[1].w
    );
}

#[test]
fn 显示列表_组透明度回补子树() {
    let doc = Document::parse(
        "<body><div style='opacity:0.5'><div style='background:#000000'>x</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let child_bg = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, DisplayKind::Background { fill: super::super::render::display::BgFill::Solid(rgb, _) } if rgb == [0u8, 0, 0]))
        .expect("子黑盒背景项");
    assert!(
        (child_bg.opacity - 0.5).abs() < 1e-9,
        "子项应继承组透明度 0.5,实际 {}",
        child_bg.opacity
    );
}

#[test]
fn 显示列表_溢出裁剪回补子树() {
    let doc = Document::parse(
        "<body><div style='width:60px;height:30px;overflow:hidden'><div style='width:100px;height:100px;background:#ff0000'></div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let child_bg = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, DisplayKind::Background { fill: super::super::render::display::BgFill::Solid(rgb, _) } if rgb == [255u8, 0, 0]))
        .expect("子红盒背景项");
    let clip = child_bg.clip.expect("子项应带裁剪矩形");
    assert!(
        (clip.0 - 8.0).abs() < 1e-6 && (clip.2 - 60.0).abs() < 1e-6,
        "裁剪矩形应为父 padding 盒 (8,8,60,30),实际 {clip:?}"
    );
}

#[test]
fn 显示列表_flex条目边框不再缺失() {
    // 此前 flex 条目从不产出边框/阴影绘制指令(只有容器路径调用
    // push_border_and_shadow);阶段六 emit_element 统一修复。
    let doc = Document::parse(
        "<body><div style='display:flex'><div style='width:50px;height:20px;border:2px solid #ff0000'>a</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout
            .display
            .iter()
            .any(|i| matches!(i.kind, DisplayKind::Border { .. })),
        "flex 条目应有边框显示项"
    );
}

#[test]
fn 显示列表_绝对定位元素边框不再缺失() {
    let doc = Document::parse(
        "<body><div style='position:absolute;left:10px;top:10px;width:50px;height:20px;border:1px solid #00ff00'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout
            .display
            .iter()
            .any(|i| matches!(i.kind, DisplayKind::Border { .. })),
        "absolute 元素应有边框显示项"
    );
}

#[test]
fn 显示列表_渐变背景项发射() {
    let doc = Document::parse(
        "<body><div style='width:100px;height:40px;background:linear-gradient(to right, #ff0000, #0000ff)'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout
            .display
            .iter()
            .any(|i| matches!(i.kind, DisplayKind::Background { fill: super::super::render::display::BgFill::Gradient(_) })),
        "渐变应产出 Background-Gradient 显示项"
    );
}

#[test]
fn 显示列表_模糊阴影携带blur半径() {
    let doc = Document::parse(
        "<body><div style='width:100px;height:50px;box-shadow:2px 4px 12px #000000'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let shadow = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, DisplayKind::Shadow { .. }))
        .expect("应有阴影显示项");
    match &shadow.kind {
        DisplayKind::Shadow { blur, .. } => assert!(
            (*blur - 12.0).abs() < 1e-9,
            "blur 应为 12,实际 {blur}"
        ),
        _ => unreachable!(),
    }
}

// ===================== CSS 浮动(阶段七) =====================

fn 盒<'a>(layout: &'a super::Layout, selector: &str) -> &'a super::ElementBox {
    layout
        .boxes
        .iter()
        .find(|b| b.selector == selector)
        .unwrap_or_else(|| panic!("找不到元素盒 {selector}: {:?}", layout.boxes.iter().map(|b| &b.selector).collect::<Vec<_>>()))
}

#[test]
fn 浮动_左右浮动同一行排列() {
    let doc = Document::parse(
        "<body><div id='c' style='width:400px'>\
         <div id='a' style='float:left;width:100px;height:30px'></div>\
         <div id='b' style='float:left;width:200px;height:30px'></div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = 盒(&layout, "#a");
    let b = 盒(&layout, "#b");
    assert_eq!(a.y, b.y, "两个左浮动应同一行:a={a:?} b={b:?}");
    assert!(b.x >= a.x + a.w, "第二个浮动应排在第一个右侧:a={a:?} b={b:?}");
}

#[test]
fn 浮动_右浮动贴容器右缘() {
    let doc = Document::parse(
        "<body><div id='c' style='width:400px'>\
         <div id='r' style='float:right;width:100px;height:20px'></div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let c = 盒(&layout, "#c");
    let r = 盒(&layout, "#r");
    assert!(
        (r.x + r.w - (c.x + c.w)).abs() < 0.01,
        "右浮动应贴容器右缘:c={c:?} r={r:?}"
    );
}

#[test]
fn 浮动_容器auto高收编浮动底边() {
    let doc = Document::parse(
        "<body><div id='c'>\
         <div id='a' style='float:left;width:100px;height:50px'></div>\
         </div><div id='after'>后续</div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let c = 盒(&layout, "#c");
    let a = 盒(&layout, "#a");
    let after = 盒(&layout, "#after");
    assert!(c.h >= a.h, "容器 auto 高不应塌缩:c={c:?} a={a:?}");
    assert!(
        after.y >= a.y + a.h,
        "后续内容不应与浮动重叠:a={a:?} after={after:?}"
    );
}

#[test]
fn 浮动_clear换行到浮动之下() {
    let doc = Document::parse(
        "<body><div id='c' style='width:400px'>\
         <div id='a' style='float:left;width:100px;height:30px'></div>\
         <div id='b' style='clear:both;width:50px;height:10px'></div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = 盒(&layout, "#a");
    let b = 盒(&layout, "#b");
    assert!(
        b.y >= a.y + a.h,
        "clear:both 应下沉到浮动底边之下:a={a:?} b={b:?}"
    );
}

#[test]
fn 浮动_auto宽内容盒在浮动旁收窄() {
    let doc = Document::parse(
        "<body><div id='c' style='width:400px'>\
         <div id='side' style='float:left;width:100px;height:30px'></div>\
         <div id='main' style='height:40px'></div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let c = 盒(&layout, "#c");
    let side = 盒(&layout, "#side");
    let main = 盒(&layout, "#main");
    assert!(
        main.x >= side.x + side.w,
        "内容盒应位于左浮动右侧:side={side:?} main={main:?}"
    );
    assert!(main.w < c.w, "内容盒应收窄到剩余带:c={c:?} main={main:?}");
}

#[test]
fn 浮动_带宽不足时下沉换行() {
    let doc = Document::parse(
        "<body><div id='c' style='width:400px'>\
         <div id='a' style='float:left;width:300px;height:30px'></div>\
         <div id='b' style='float:left;width:300px;height:30px'></div>\
         <div id='d' style='float:left;width:300px;height:30px'></div>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let a = 盒(&layout, "#a");
    let b = 盒(&layout, "#b");
    let d = 盒(&layout, "#d");
    assert!(b.y > a.y, "第二个 300px 浮动在 400px 容器里应换行:a={a:?} b={b:?}");
    assert!(d.y > b.y, "第三个浮动应再换一行:b={b:?} d={d:?}");
}

#[test]
fn 浮动_行内控件与行内块排成一行() {
    // 百度地图搜索卡片同构:input 329 + 路线 39 = 内容盒 368;内容盒 368 + 搜索 57 = 425。
    let doc = Document::parse(
        "<body><div id='card' style='width:425px;height:44px'>\
         <div id='content' style='display:inline-block;width:368px;height:40px'>\
         <input id='q' style='float:left;width:329px;height:38px'>\
         <div id='route' style='float:left;width:39px;height:38px'></div>\
         </div>\
         <button id='go' style='float:left;width:57px;height:38px'></button>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let content = 盒(&layout, "#content");
    let q = 盒(&layout, "#q");
    let route = 盒(&layout, "#route");
    let go = 盒(&layout, "#go");
    assert_eq!(q.y, route.y, "输入框与路线按钮应同一行:q={q:?} route={route:?}");
    assert!(
        (route.x - (q.x + q.w)).abs() < 0.01,
        "路线按钮应紧贴输入框右侧:q={q:?} route={route:?}"
    );
    assert_eq!(content.y, go.y, "搜索按钮应与内容盒同一行:content={content:?} go={go:?}");
    assert!(
        (go.x - (content.x + content.w)).abs() < 0.01,
        "搜索按钮应紧贴内容盒右侧:content={content:?} go={go:?}"
    );
}

#[test]
fn 浮动_行内右浮动贴容器右缘() {
    let doc = Document::parse(
        "<body><div id='card' style='width:400px;height:44px'>\
         <span id='txt'>说明</span>\
         <button id='go' style='float:right;width:60px;height:30px'>Go</button>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let card = 盒(&layout, "#card");
    let go = 盒(&layout, "#go");
    assert!(
        (go.x + go.w - (card.x + card.w)).abs() < 0.01,
        "行内右浮动应贴容器右缘:card={card:?} go={go:?}"
    );
}

#[test]
fn 浮动_块级浮动容器收缩宽度并与行内浮动混排() {
    // 真实百度结构:#searchbox(425) > #searchbox-container(float:left,auto 宽)
    //   > 内容盒(block) > input(float 329) + 路线按钮(block float 39);
    // 搜索按钮(inline float 57)应紧贴容器右侧。
    let doc = Document::parse(
        "<body><div id='card' style='width:425px;height:44px'>\
         <div id='container' style='float:left'>\
         <div id='content' style='height:40px'>\
         <input id='q' style='float:left;width:329px;height:38px'>\
         <div id='route' style='float:left;width:39px;height:38px'></div>\
         </div></div>\
         <button id='go' style='float:left;width:57px;height:38px'></button>\
         </div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let container = 盒(&layout, "#container");
    let q = 盒(&layout, "#q");
    let route = 盒(&layout, "#route");
    let go = 盒(&layout, "#go");
    assert!(
        (container.w - 368.0).abs() < 0.01,
        "auto 宽浮动容器应收缩为内容宽 368,实际 {container:?}"
    );
    assert_eq!(q.y, route.y, "输入框与路线按钮应同一行:q={q:?} route={route:?}");
    assert!(
        (route.x - (q.x + q.w)).abs() < 0.01,
        "块级路线浮动应排在行内输入框右侧:q={q:?} route={route:?}"
    );
    assert_eq!(container.y, go.y, "搜索按钮应与浮动容器同一行:container={container:?} go={go:?}");
    assert!(
        (go.x - (container.x + container.w)).abs() < 0.01,
        "搜索按钮应紧贴浮动容器右侧:container={container:?} go={go:?}"
    );
}

// ===================== 表单控件文本绘制(阶段七) =====================
#[test]
fn 表单_输入框placeholder被绘制为灰字() {
    let doc = Document::parse(
        "<body><div><input id='q' placeholder='搜地点、查公交' style='width:200px;height:30px'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let line = layout
        .lines
        .iter()
        .find(|l| l.text.contains("搜地点"))
        .expect("placeholder 应产生文本行");
    assert_eq!(line.color, [117, 117, 117], "占位符应为 UA 灰");
}

#[test]
fn 表单_输入框value优先于placeholder() {
    let doc = Document::parse(
        "<body><div><input id='q' value='沙河镇' placeholder='搜地点' style='width:200px;height:30px'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    assert!(
        layout.lines.iter().any(|l| l.text.contains("沙河镇")),
        "value 文本应被绘制"
    );
    assert!(
        !layout.lines.iter().any(|l| l.text.contains("搜地点")),
        "有 value 时不应再画 placeholder"
    );
}

#[test]
fn 表单_密码值按圆点掩码() {
    let doc = Document::parse(
        "<body><div><input type='password' value='abc' style='width:200px;height:30px'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let line = layout
        .lines
        .iter()
        .find(|l| l.text.contains('•'))
        .expect("密码应绘制掩码");
    assert_eq!(line.text, "•••");
    assert!(!layout.lines.iter().any(|l| l.text.contains("abc")));
}


// ===================== z-index 下传 / position:fixed 标记(第 N 轮) =====================
// 层叠上下文近似:父级 z-index 创建层叠上下文后,整个子树都应在兄弟节点之上。
// 旧实现只把自身 z 写进显示项 → 父 z:50 的整棵子树会被兄弟 z:1 压住
// (实测百度地图 #platform/#mask/瓦片层/控件层层叠全乱)。

#[test]
fn z_index_沿祖先链下传给子树() {
    let doc = Document::parse(
        "<body style='margin:0'>\
           <div id='parent' style='position:relative;z-index:50;background:#eee'>\
             <div id='child' style='position:relative'><p>inner</p></div>\
           </div>\
           <div id='sibling' style='position:relative;z-index:1;background:#ddd'><p>sib</p></div>\
         </body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let zs = layout.display.iter().map(|i| (i.x, i.y, i.z, i.w)).collect::<Vec<_>>();
    // 分区按真实盒矩形划分(布局语义修正后不再有 2px/1px 累积漂移,
    // 硬编码 y 边界会把兄弟首项误判进父区)。
    let parent_rect = layout.rect_for("#parent").expect("parent 应有布局盒");
    let sib_rect = layout.rect_for("#sibling").expect("sibling 应有布局盒");
    // parent 区:背景盒 + 文本项的 z 都必须 ≥ 50(祖先下限)。
    let parent_items: Vec<_> = layout
        .display
        .iter()
        .filter(|it| it.y >= parent_rect.1 && it.y < parent_rect.1 + parent_rect.3)
        .collect();
    assert!(!parent_items.is_empty(), "父区应有绘制项:{zs:?}");
    assert!(
        parent_items.iter().all(|it| it.z >= 50),
        "父级 z-index:50 应下传到子树全部项:{zs:?}"
    );
    // 兄弟区:自身 z=1 生效。
    let sib_items: Vec<_> = layout
        .display
        .iter()
        .filter(|it| it.y >= sib_rect.1 && it.y < sib_rect.1 + sib_rect.3)
        .collect();
    assert!(!sib_items.is_empty(), "兄弟区应有绘制项:{zs:?}");
    assert!(
        sib_items.iter().all(|it| it.z == 1),
        "兄弟 z-index:1 应保持:{zs:?}"
    );
}

#[test]
fn position_fixed_子树带标记() {
    // fixed 盒坐标以视口为原点,光栅化必须跳过滚动偏移,否则固定控件随页面滚动平移。
    let doc = Document::parse(
        "<body style='margin:0'>\
           <div id='norm'><p>normal</p></div>\
           <div id='fx' style='position:fixed;top:10px;left:10px;width:100px;height:50px;background:#f00'></div>\
         </body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 800.0, &font());
    let fx = layout.display.iter().map(|i| (i.x, i.y, i.w, i.h, i.fixed)).collect::<Vec<_>>();
    assert!(
        layout.display.iter().any(|it| it.fixed && it.w >= 100.0 && it.h >= 50.0),
        "fixed 盒应带 fixed 标记:{fx:?}"
    );
    assert!(
        layout.display.iter().any(|it| !it.fixed),
        "普通流元素不应带 fixed 标记:{fx:?}"
    );
}

#[test]
fn 视口高度参与百分比高度解析() {
    // 回归:MediaCtx 的视口高此前被写死 720(8 处调用点),视口 1920×1080 时
    // `height:100%` 仍解析成 720 → 页面底部大片空白(显示不全),
    // 百度地图的 #MapHolder 也只有 720 高,地图铺不满窗口。
    use super::super::style::MediaCtx;
    use super::super::images::ImageSizes;
    use super::super::layout::compute_full;
    let doc = Document::parse(
        "<html><head><style>html,body{height:100%;margin:0}#wrap{height:100%}</style></head>\
         <body><div id='wrap'><div id='mh' style='height:100%'></div></div></body></html>",
        "https://a.test/",
    );
    let media = MediaCtx::new(1920.0, 1080.0);
    let layout = compute_full(&doc, 1920.0, &font(), &media, &ImageSizes::default());
    let wrap = layout
        .boxes
        .iter()
        .find(|b| b.selector == "#wrap")
        .unwrap_or_else(|| panic!("应有 #wrap 盒"));
    assert!(
        (wrap.h - 1080.0).abs() < 2.0,
        "100% 容器应解析为真实视口高 1080,实际 {}",
        wrap.h
    );
    let mh = layout
        .boxes
        .iter()
        .find(|b| b.selector == "#mh")
        .unwrap_or_else(|| panic!("应有 #mh 盒"));
    assert!(
        (mh.h - 1080.0).abs() < 2.0,
        "子代 100% 应解析为 1080,实际 {}",
        mh.h
    );
}

#[test]
fn 猎聘卡片_flex列_卡内条目间距与logo尺寸() {
    // 猎聘 home.css 原文关键规则(逐字摘录)。
    let css = r#"
.home-industry-container .seo-jobcard2-common,.home-industry-container .seo-jobcard2-common .jobcard2-detail-box{display:-webkit-box;display:-ms-flexbox;display:flex;-webkit-box-orient:vertical;-webkit-box-direction:normal;-ms-flex-direction:column;flex-direction:column}
.home-industry-container .seo-jobcard2-common .jobcard2-detail-box{gap:8px;padding-bottom:0;text-decoration:none}
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-top{display:-webkit-box;display:-ms-flexbox;display:flex;-webkit-box-align:center;-ms-flex-align:center;align-items:center;-webkit-box-pack:start;-ms-flex-pack:start;justify-content:flex-start}
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-top .jobcard2-title{color:#000;font-size:18px;font-weight:500;line-height:25px;margin-right:10px}
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-top .jobcard2-salary{color:#ff6400;flex-shrink:0;font-size:18px;font-weight:500;line-height:25px;margin-left:auto}
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-labels{font-size:0;height:22px;margin-top:10px;overflow:hidden}
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-labels span{background:#f8f9fb;border-radius:6px;color:#999;display:inline-block;font-size:12px;font-weight:400;height:100%;line-height:16px;margin-right:8px;padding:3px 8px}
.seo-jobcard2-common .jobcard2-company-box{border-top:1px solid #f0f3f7;display:-webkit-box;display:-ms-flexbox;display:flex;padding-top:16px}
.home-industry-container .seo-jobcard2-common .jobcard2-company-logo{border:.5px solid #f2f6fb;border-radius:4px;flex-shrink:0;height:24px;overflow:hidden;width:24px}
.home-industry-container .seo-jobcard2-common .jobcard2-company-logo img{display:block;height:100%;object-fit:cover;width:100%}
.seo-jobcard2-common .jobcard2-company-inline{display:-webkit-box;display:-ms-flexbox;display:flex;align-items:center;gap:8px;min-width:0;overflow:hidden}
"#;
    let html = format!(
        "<head><style>{css}</style></head><body>\
<div class='home-industry-container'><div class='home-industry-content-container'><div class='home-industry-content-list'>\
<a class='seo-jobcard2-common' href='/job/1'><div class='jobcard2-detail-box'>\
<div class='jobcard2-detail-top'><span class='jobcard2-title'>高级算法专家</span><span class='jobcard2-salary'>50-80k</span></div>\
<div class='jobcard2-detail-labels'><span>5-10年</span><span>硕士</span></div>\
<div class='jobcard2-company-box'>\
<div class='jobcard2-company-logo'><img src='https://img.example/logo.png' alt=''></div>\
<div class='jobcard2-company-inline'><span class='jobcard2-company-title'>测试公司</span></div>\
</div>\
</div></a>\
</div></div></div></body>"
    );
    let doc = Document::parse(&html, "https://a.test/");
    let layout = compute(&doc, 1920.0, &font());
    // 找卡片各段文字行,断言卡内 y 连续(同卡内文字最大间隔 < 300)
    let mut card_lines: Vec<(String, f64)> = Vec::new();
    for l in &layout.lines {
        if ["高级算法专家", "50-80k", "5-10年", "测试公司"]
            .iter()
            .any(|t| l.text.contains(t))
        {
            card_lines.push((l.text.clone(), l.y));
        }
    }
    println!("卡内文字行: {card_lines:?}");
    if let Some((max, min)) = card_lines
        .iter()
        .map(|(_, y)| *y)
        .fold(None::<(f64, f64)>, |acc, y| match acc {
            None => Some((y, y)),
            Some((mx, mn)) => Some((mx.max(y), mn.min(y))),
        })
    {
        assert!(
            max - min < 300.0,
            "卡内条目间距异常: 最大y={max} 最小y={min} 跨度={}",
            max - min
        );
    }
    // logo 盒尺寸:24×24(company-logo 的 width/height 固定);
    // stable_selector 不含类名,按 company-box(div:nth-of-type(3)) 内首个 div 定位。
    let logo = layout.boxes.iter().find(|b| {
        b.selector.contains("div:nth-of-type(3)") && b.selector.ends_with("> div")
    });
    let b = logo.unwrap_or_else(|| panic!("找不到 company-logo 盒"));
    assert!((b.w - 24.0).abs() < 2.0, "logo 宽应为 24,实际 {}", b.w);
    assert!((b.h - 24.0).abs() < 2.0, "logo 高应为 24,实际 {}", b.h);
    // logo img 盒不得超出 logo 容器(百分比宽按包含块 24 求解,不再取视口宽)。
    let img_box = layout
        .boxes
        .iter()
        .find(|bch| bch.tag == "img")
        .unwrap_or_else(|| panic!("找不到 img 盒"));
    assert!(
        img_box.w <= 26.0,
        "img 盒宽应受包含块约束(≤26),实际 {}",
        img_box.w
    );
}


#[test]
fn 猎聘tab行_inlineflex条目应横排() {
    // 猎聘行业分类 tab:容器 flex+wrap+center+gap24;条目 inline-flex
    // (前缀连写),内部 svg(20x20 display:block w/h 100%)+ 文本。
    let css = r#"
.home-industry-container .home-industry-header .industry-header-title{display:-webkit-box;display:-ms-flexbox;display:flex;-ms-flex-wrap:wrap;flex-wrap:wrap;justify-content:center;align-items:center;gap:24px;min-height:42px}
.home-industry-container .home-industry-header .industry-header-title .header-title-item{box-sizing:border-box;display:-webkit-inline-box;display:-ms-inline-flexbox;display:inline-flex;flex:none;justify-content:center;align-items:center;background:#fff;border:1px solid transparent;border-radius:100px;height:40px;padding:0 16px}
.home-industry-container .home-industry-header .industry-header-title .header-title-item-svg{height:20px;margin-right:8px;width:20px;flex-shrink:0}
.home-industry-container .home-industry-header .industry-header-title .header-title-item-svg svg{display:block;height:100%;width:100%}
.home-industry-container .home-industry-header .industry-header-title .header-title-item-text{color:#121e29;font-size:16px;height:24px;line-height:24px;white-space:nowrap}
"#;
    let tab = |href: &str, name: &str| {
        format!(
            "<a href='{href}' class='header-title-item'><div class='header-title-item-svg'><svg viewBox='0 0 20 20'><path d='M0 0h20v20H0z'></path></svg></div><div class='header-title-item-text'>{name}</div></a>"
        )
    };
    let tabs = [
        tab("/a/", "AI/互联网/IT"),
        tab("/b/", "电子/通信/半导体"),
        tab("/c/", "新能源"),
        tab("/d/", "医疗健康"),
    ]
    .join("");
    let html = format!(
        "<head><style>{css}</style></head><body>\
<div class='home-industry-container'><div class='home-industry-header'>\
<div class='industry-header-title'>{tabs}</div>\
</div></div></body>"
    );
    let doc = Document::parse(&html, "https://a.test/");
    let layout = compute(&doc, 1920.0, &font());
    let mut tabs_y: Vec<(String, f64, f64)> = Vec::new();
    for l in &layout.lines {
        if ["AI/互联网/IT", "电子/通信/半导体", "新能源", "医疗健康"]
            .iter()
            .any(|t| l.text.contains(t))
        {
            tabs_y.push((l.text.clone(), l.x, l.y));
        }
    }
    println!("tab 文字坐标: {tabs_y:?}");
    let ys: Vec<f64> = tabs_y.iter().map(|(_, _, y)| *y).collect();
    let distinct: Vec<f64> = {
        let mut v = ys.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup();
        v
    };
    assert!(
        distinct.len() <= 2,
        "4 个 tab 应排在 1-2 行内(容器 1184 宽),实际行数 {}:{tabs_y:?}",
        distinct.len()
    );
}

// ===================== 第 174 轮:绝对定位元素不污染内在尺寸 =====================

/// 猎聘头部结构复现:`position:relative` 的 flex 条目内含
/// `position:absolute;right:30px` 的无宽 `<svg>` 徽标(NEW 角标)。
/// 修复前:徽标在 MAX_CONTENT_WIDTH 语境下的盒坐标流进 flex 条目基础宽,
/// 后续条目被 flex 收缩压到 ~1px —— 「登录/注册」入口 0×0、点击坐标落空,
/// 登录链路 16 分钟未破(2026-10-28 真机)。修复后:徽标不参与内在尺寸,
/// 后续条目按内容正常排布,徽标盒照常发射(向左溢出父盒,与真实浏览器一致)。
#[test]
fn 绝对定位徽标不吞噬flex后续条目() {
    let html = concat!(
        "<html><head><style>",
        ".box .menu{display:flex;align-items:center;font-size:14px;line-height:22px}",
        ".box .item{position:relative}",
        ".box .badge{bottom:100%;position:absolute;right:30px;top:auto}",
        "</style></head><body><div class='box'>",
        "<ul class='menu'>",
        "<li class='item'><a href='#'>链接甲</a><svg width='28' height='14' viewBox='0 0 28 14' class='badge'><g><path d='M4 0h23v9H4z' fill='#FF6400'></path></g></svg></li>",
        "<li class='item'><span id='login'>登录/注册</span></li>",
        "</ul></div></body></html>"
    );
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 1920.0, &font());
    let li2 = lay
        .boxes
        .iter()
        .find(|b| b.selector.ends_with("li:nth-of-type(2)"))
        .expect("登录条目盒应存在");
    assert!(
        li2.w > 40.0,
        "登录/注册条目不应被脱流徽标挤没:实际 {:?}",
        (li2.x, li2.w)
    );
    let badge = lay
        .boxes
        .iter()
        .find(|b| b.tag == "svg")
        .expect("脱流徽标盒仍应发射(渲染不受影响)");
    assert!((badge.w - 28.0).abs() < 0.5, "徽标宽度应取固有 28:{:?}", badge.w);
    assert!(badge.out_of_flow, "脱流盒应带 out_of_flow 标记");
}

/// 无尺寸 `<iframe>` 按替换元素建议缺省 300×150 参与布局(真实浏览器语义),
/// 不再塌缩 0×0 导致坐标点击/inspect 失真(猎聘页脚 iframe 实测 0×0)。
#[test]
fn 无尺寸iframe缺省300x150() {
    let html = "<html><body><div><iframe src='https://wow.example/t.html'></iframe></div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let fr = lay
        .boxes
        .iter()
        .find(|b| b.tag == "iframe")
        .expect("iframe 应作为替换元素成盒");
    assert!(
        (fr.w - 300.0).abs() < 1.0 && (fr.h - 150.0).abs() < 1.0,
        "iframe 缺省几何应为 300×150,实际 {:?}",
        (fr.w, fr.h)
    );
}

// ===================== 第 175 轮:body margin / 空行内元素 =====================

/// body margin 重置为 0 后,块级内容应从视口原点全幅排布
/// (百度地图 #MapHolder 被 (8,27) 偏移 + 1904 宽的根因回归)。
#[test]
fn body_margin重置后内容全幅从原点排布() {
    let html = "<html><head><style>body{margin:0;padding:0}#app{width:100%;height:40px}</style></head>\
                <body><div id='app'>x</div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let app = lay.rect_for("#app").expect("#app 应有布局盒");
    assert!(app.0.abs() < 0.5, "margin:0 后 x 应为 0,实际 {app:?}");
    assert!((app.2 - 800.0).abs() < 1.0, "width:100% 应撑满 800,实际 {app:?}");
}

/// 无重置时维持 UA 缺省 8px 观感:布局不再硬编码,而是消费样式层真值
/// (ua_default 给 body 四边 8px;站点 reset 后自然全幅,见上一用例)。
#[test]
fn 无样式时body默认margin为样式真值() {
    let html = "<html><body><div id='only'>x</div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let d = lay.rect_for("#only").expect("div 应有布局盒");
    assert!((d.0 - 8.0).abs() < 0.5, "UA 缺省 margin=8,布局应从 8 起,实际 {d:?}");
    assert!((d.2 - 784.0).abs() < 1.0, "内容宽应为 800-16,实际 {d:?}");
}

/// 空行内元素(仅含 name 属性的锚点)不产生行盒:容器高度≈0,
/// 后续兄弟不被幽灵 17px 下推(百度地图顶部条带根因回归)。
#[test]
fn 空锚点不产生幽灵行高() {
    let html = "<html><body><div id='strip'><a name='script1'></a></div>\
                <div id='after'>y</div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let strip = lay.rect_for("#strip").expect("strip 应有布局盒");
    assert!(strip.3 <= 1.5, "空锚点容器高度应≈0,实际 {strip:?}");
    let after = lay.rect_for("#after").expect("after 应有布局盒");
    assert!(
        (after.1 - strip.1).abs() < 4.0,
        "后续兄弟不应被幽灵行高下推(允许 1px 最小高 + 2px margin 兜底):strip={strip:?} after={after:?}"
    );
    // 寻址诉求保留:空锚点仍有盒(0×0)。
    let anchor = lay.boxes.iter().find(|b| b.tag == "a").expect("空锚点仍应发射寻址盒");
}

/// 带文本的行内元素照常建盒,不因空判定收紧。
#[test]
fn 带文本锚点照常建盒() {
    let html = "<html><body><div><a id='lk' href='/x'>link</a></div></body></html>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let lk = lay.rect_for("#lk").expect("锚点应有布局盒");
    assert!(lk.2 > 0.0 && lk.3 > 0.0, "带文本锚点应非零尺寸,实际 {lk:?}");
}




// ===================== 块级堆叠语义(外边距折叠 / 空块零高 / 行内共享行盒) =====================
// 语义修正回归:此前块级盒有 1px 高度地板 + 2px margin 兜底(全页逐块累积漂移),
// 行内样式段各占一行竖排,文本 x 硬编码 body 边距 8 —— 以下用例钉死 CSS 应然。

/// 相邻兄弟外边距折叠取最大值(CSS 2.1 8.3.1):d1(margin-bottom:30) 与
/// d2(margin-top:10) 间隙 = max(30,10) = 30,不是求和 40;margin:0 相邻块无缝。
#[test]
fn 相邻兄弟外边距折叠取最大值() {
    let html = "<body style='margin:0'>\
        <div id='d0' style='height:10px'></div>\
        <div id='d1' style='height:10px;margin-bottom:30px'></div>\
        <div id='d2' style='height:10px;margin-top:10px'></div>\
        <div id='d3' style='height:10px'></div></body>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let d = |id: &str| lay.rect_for(id).unwrap_or_else(|| panic!("{id} 应有布局盒"));
    assert!((d("#d0").1 - 0.0).abs() < 0.5, "d0 应贴顶:{:?}", d("#d0"));
    assert!((d("#d1").1 - 10.0).abs() < 0.5, "margin:0 相邻块应无缝:{:?}", d("#d1"));
    assert!(
        (d("#d2").1 - 50.0).abs() < 0.5,
        "折叠间隙应为 max(30,10)=30(实际 top 应 50):{:?}",
        d("#d2")
    );
    assert!((d("#d3").1 - 60.0).abs() < 0.5, "d3 无缝衔接:{:?}", d("#d3"));
}

/// 空块 / height:0 / 纯空白块高度全部为 0(CSS):1px 高度地板会逐块累积漂移。
#[test]
fn 空块与零高块不产生幽灵高度() {
    let html = "<body style='margin:0'>\
        <div id='e'></div>\
        <div id='z' style='height:0'></div>\
        <div id='w'> </div>\
        <div id='t'>x</div></body>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let d = |id: &str| lay.rect_for(id).unwrap_or_else(|| panic!("{id} 应有布局盒"));
    assert!((d("#e").3 - 0.0).abs() < 0.5, "空块高应 0:{:?}", d("#e"));
    assert!((d("#z").3 - 0.0).abs() < 0.5, "height:0 应为 0:{:?}", d("#z"));
    assert!((d("#w").3 - 0.0).abs() < 0.5, "纯空白块高应 0:{:?}", d("#w"));
    assert!(d("#t").3 > 8.0, "有文本的块应有行盒高:{:?}", d("#t"));
    assert!((d("#t").1 - 0.0).abs() < 0.5, "零高前置块不应下推文本块:{:?}", d("#t"));
}

/// 负 margin-bottom 参与折叠(CSS:负边距不折叠、按叠加语义),兄弟整体上移。
#[test]
fn 负下边距把兄弟上移() {
    let html = "<body style='margin:0'>\
        <div id='a' style='height:10px;margin-bottom:-5px'></div>\
        <div id='b' style='height:10px'></div></body>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let b = lay.rect_for("#b").expect("#b 应有布局盒");
    assert!((b.1 - 5.0).abs() < 0.5, "margin-bottom:-5 应把兄弟上移到 y=5:实际 {b:?}");
}

/// 行内样式段共享同一行盒:`<p>hello <b>world</b>!</p>` 三段同一视觉行、
/// x 连续流动 —— 此前每 run 各占一行竖排。
#[test]
fn 行内样式段共享同一行盒() {
    let html = "<body style='margin:0'><p id='p1'>hello <b>world</b>!</p></body>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    assert_eq!(lay.lines.len(), 3, "三个样式段:{:?}", lay.lines);
    let y0 = lay.lines[0].y;
    assert!(
        lay.lines.iter().all(|l| (l.y - y0).abs() < 0.5),
        "三段应在同一视觉行:{:?}",
        lay.lines
    );
    assert!(
        lay.lines[0].x < lay.lines[1].x && lay.lines[1].x < lay.lines[2].x,
        "段 x 应连续流动:{:?}",
        lay.lines
    );
    // 单 run 换行不受影响:长文本按可用宽断行,行高一致。
    let html2 = "<body style='margin:0'><div style='width:120px'>\
        aaaa bbbb cccc dddd eeee ffff gggg</div></body>";
    let doc2 = Document::parse(html2, "https://a.test/");
    let lay2 = compute(&doc2, 800.0, &font());
    assert!(lay2.lines.len() >= 2, "窄容器长文本应换行:{:?}", lay2.lines);
    let ys: Vec<f64> = lay2.lines.iter().map(|l| l.y).collect();
    for w in ys.windows(2) {
        assert!(w[1] > w[0], "换行后的行 y 应递增:{:?}", lay2.lines);
    }
}

/// 嵌套块文本 x 按包含块内容盒累积定位(body margin:0 下不得再有 +8 漂移)。
#[test]
fn 嵌套块文本x按包含块内容盒定位() {
    let html = "<body style='margin:0'>\
        <div id='outer' style='margin-left:200px'><div id='inner' style='margin-left:50px'>B</div></div>\
        <div style='margin-left:200px'>A</div></body>";
    let doc = Document::parse(html, "https://a.test/");
    let lay = compute(&doc, 800.0, &font());
    let line_b = lay.lines.iter().find(|l| l.text.contains('B')).expect("应有 B 文本");
    assert!((line_b.x - 250.0).abs() < 0.5, "B 文本 x 应 250(200+50):{:?}", line_b);
    let line_a = lay.lines.iter().find(|l| l.text.contains('A')).expect("应有 A 文本");
    assert!((line_a.x - 200.0).abs() < 0.5, "A 文本 x 应 200:{:?}", line_a);
}

// ===================== 内在尺寸(max-content)测量 =====================

#[test]
fn 内在尺寸测量_居中文字不撑大条目宽度() {
    // flex 条目的基础宽在 MAX_CONTENT_WIDTH(10 万)语境下量 max-content。
    // 若行内居中偏移参与宽度计算,文字行起点会落到 ~5 万处,
    // 条目宽度被撑到 5 万(实测猎聘登录页 tab 条目 50034px),
    // 后续布局全歪。此处断言条目宽度落在「文字宽」量级。
    let doc = Document::parse(
        "<head><style>
            .tabs { display:flex; width:348px; }
            .tab { flex-shrink:0; text-align:center; margin-right:40px; }
         </style></head><body><div class='tabs'><div class='tab'>登录/注册</div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 1920.0, &font());
    let tab = layout
        .boxes
        .iter()
        .find(|b| b.tag == "div" && b.w > 1.0 && b.h > 1.0 && b.w < 5000.0)
        .expect("应存在 tab 盒");
    assert!(
        tab.w < 500.0,
        "居中文字条目宽度被内在尺寸测量撑大:{}",
        tab.w
    );
}

#[test]
fn 绝对定位元素_宽度auto走shrink_to_fit() {
    // `position:absolute; left:50%` 且 width:auto → 宽度按内容收缩,
    // 不是填满包含块(填满会把整块登录框推出屏幕右侧)。
    let doc = Document::parse(
        "<head><style>
            .panel { position:absolute; left:50%; top:50%; display:flex; }
            .a { width:278px; } .b { width:460px; }
         </style></head><body><div class='panel'><div class='a'></div><div class='b'></div></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 1920.0, &font());
    let panel = layout
        .boxes
        .iter()
        .find(|b| b.tag == "div" && b.w > 700.0 && b.w < 1200.0)
        .expect("应存在 panel 盒");
    assert!(
        panel.w <= 800.0,
        "绝对定位元素应按内容收缩宽度,实际 {}",
        panel.w
    );
}

// ===================== 绘制层(定位元素在常规流之上) =====================

#[test]
fn 定位元素绘制在常规流内容之上() {
    // CSS 2.1 附录 E 第 8 步:定位元素及其子树绘制在常规流内容之上。
    // 此前只按 (z, seq) 排序,定位元素若在 DOM 顺序上靠前,会被后面的
    // 常规流背景/图片整块盖住(实测猎聘登录框被 hero 背景图覆盖)。
    let doc = Document::parse(
        "<head><style>
            .abs { position:absolute; left:0; top:0; width:200px; height:100px; background:#fff; }
            .flow { width:1920px; height:600px; background:#000642; }
         </style></head><body><div class='abs'></div><div class='flow'></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 1920.0, &font());
    let abs_seq = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, super::super::render::display::DisplayKind::Background { .. }) && i.layer == 1)
        .map(|i| i.seq);
    let flow_seq = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, super::super::render::display::DisplayKind::Background { .. }) && i.layer == 0 && i.w > 1000.0)
        .map(|i| i.seq);
    assert!(abs_seq.is_some(), "应存在定位元素的背景绘制项");
    assert!(flow_seq.is_some(), "应存在常规流背景绘制项");
    assert!(
        abs_seq > flow_seq,
        "定位元素应排在常规流内容之后绘制(定位 {:?} 常规流 {:?})",
        abs_seq,
        flow_seq
    );
}

#[test]
fn 绝对定位元素背景画在自身子树之下() {
    // 元素背景必须画在**子树内容之下**(浏览器语义)。此前绝对定位元素的
    // 自身视觉在测量子树之后才发射,seq 更大 → 画在子树之上,把子树整块
    // 盖住(实测猎聘首页登录弹层白色背景把整张表单盖成白板)。
    let doc = Document::parse(
        "<head><style>
            .panel { position:absolute; left:0; top:0; width:200px; height:100px; background:#ffffff; }
            .inner { color:#ff0000; }
         </style></head><body><div class='panel'><span class='inner'>表单</span></div></body>",
        "https://a.test/",
    );
    let layout = compute(&doc, 1920.0, &font());
    let bg_seq = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, super::super::render::display::DisplayKind::Background { .. }))
        .map(|i| i.seq);
    let text_seq = layout
        .display
        .iter()
        .find(|i| matches!(i.kind, super::super::render::display::DisplayKind::Text { .. }))
        .map(|i| i.seq);
    assert!(bg_seq.is_some(), "应存在背景绘制项");
    assert!(text_seq.is_some(), "应存在文本绘制项");
    assert!(
        bg_seq < text_seq,
        "背景应早于子树文本绘制(背景 {:?} 文本 {:?})",
        bg_seq,
        text_seq
    );
}

// ===================== 猎聘行业卡片几何(真实 CSS) =====================
// 复现「AI时代新机会」卡片:384x192 定高 + overflow:hidden 的内层,
// 公司行+logo 此前被零高裁剪窗整行裁掉(2026-10-29 真机实测)。

#[test]
fn 猎聘卡片_height100链与显式行高不被内容撑破() {
    let html = r#"<html><head><style>
.home-industry-content-item { background:#fff; box-sizing:border-box; height:192px; width:384px; }
.seo-jobcard2-common { border-radius:6px; box-sizing:border-box; height:100%; overflow:hidden; padding:24px; width:100%; }
.seo-jobcard2-common .jobcard2-detail-box { display:block; padding-bottom:16px; }
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-top { display:flex; align-items:center; justify-content:flex-start; }
.seo-jobcard2-common .jobcard2-detail-top .jobcard2-title { color:#000; font-size:18px; font-weight:500; line-height:25px; margin-right:10px; }
.seo-jobcard2-common .jobcard2-detail-top .jobcard2-salary { color:#ff6400; flex-shrink:0; font-size:18px; font-weight:500; line-height:25px; margin-left:auto; }
.seo-jobcard2-common .jobcard2-detail-box .jobcard2-detail-labels { font-size:0; height:22px; margin-top:10px; overflow:hidden; }
.seo-jobcard2-common .jobcard2-detail-labels span { background:#f8f9fb; border-radius:6px; box-sizing:border-box; color:#999; display:inline-block; font-size:12px; height:100%; line-height:16px; margin-right:8px; padding:3px 8px; }
.seo-jobcard2-common .jobcard2-company-box { border-top:1px solid #f0f3f7; display:flex; padding-top:16px; }
.seo-jobcard2-common .jobcard2-company-box .jobcard2-company-info { box-sizing:border-box; display:flex; padding-left:16px; width:calc(100% - 50px); flex-direction:column; justify-content:center; }
.seo-jobcard2-common .jobcard2-company-info .jobcard2-company-title { color:#666; font-size:14px; font-weight:500; height:20px; line-height:20px; overflow:hidden; }
.seo-jobcard2-common .jobcard2-company-info .jobcard2-company-labels { color:#999; font-size:0; height:17px; line-height:17px; margin-top:6px; overflow:hidden; }
.seo-jobcard2-common .jobcard2-company-info .jobcard2-company-labels span { display:inline-block; font-size:12px; margin-right:6px; }
.home-industry-container .seo-jobcard2-common .jobcard2-company-logo { flex-shrink:0; border-radius:4px; height:24px; overflow:hidden; width:24px; }
.home-industry-container .seo-jobcard2-common .jobcard2-company-logo img { display:block; height:100%; object-fit:cover; width:100%; }
.home-industry-container .home-industry-content-container .home-industry-content-list { display:flex; }
.home-industry-content-item { margin:0; }
</style></head><body>
<section class="home-industry-container"><div class="home-industry-box"><div class="home-industry-content-container"><div class="home-industry-content-list">
<div class="home-industry-content-item">
  <div class="seo-jobcard2-common">
    <a class="jobcard2-detail-box" href="/job/1.shtml">
      <div class="jobcard2-detail-top">
        <div class="jobcard2-title-group">
          <span class="jobcard2-title">重庆城市销售经理</span>
          <span class="jobcard2-location">重庆-大坪</span>
        </div>
        <span class="jobcard2-salary">20-30k·14薪</span>
      </div>
      <div class="jobcard2-detail-labels"><span>5-10年</span><span>大专</span></div>
    </a>
    <a class="jobcard2-company-box" href="/company/1/">
      <div class="jobcard2-company-logo"><img src="https://img.example/logo.png" alt=""/></div>
      <div class="jobcard2-company-info">
        <div class="jobcard2-company-title">瑞幸咖啡(中国)有限公司</div>
        <div class="jobcard2-company-labels"><span>互联网</span><span>10000人以上</span></div>
      </div>
    </a>
  </div>
</div></div></div></section>
</body></html>"#;
    let doc = Document::parse(html, "https://a.test/");
    let layout = compute(&doc, 1200.0, &font());
    let g = |sel: &str| layout.rect_for(sel).unwrap_or_else(|| panic!("缺盒子:{sel}"));
    let card = g("html > body > section > div > div > div > div");
    assert!((card.2 - 384.0).abs() < 0.5, "卡片宽 {card:?}");
    assert!((card.3 - 192.0).abs() < 0.5, "卡片高 {card:?}");
    let seo = g("html > body > section > div > div > div > div > div");
    assert!((seo.3 - 192.0).abs() < 0.5, "seo-jobcard2-common height:100% 应为 192(border-box),实际 {seo:?}");
    let labels = g("html > body > section > div > div > div > div > div > a > div:nth-of-type(2)");
    assert!((labels.3 - 22.0).abs() < 0.5, "labels 显式 height:22px 应钳制行盒,实际 {labels:?}");
    let company = g("html > body > section > div > div > div > div > div > a:nth-of-type(2)");
    let card_content_bottom = card.1 + card.3 - 24.0;
    assert!(
        company.1 + company.3 <= card_content_bottom + 0.5,
        "公司行应整行落在卡片裁剪窗内:company={company:?} 窗底={card_content_bottom}"
    );
}
