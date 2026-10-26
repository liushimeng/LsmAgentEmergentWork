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
        .find(|i| matches!(i.kind, DisplayKind::Background { fill: super::super::render::display::BgFill::Solid(rgb) } if rgb == [0u8, 0, 0]))
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
        .find(|i| matches!(i.kind, DisplayKind::Background { fill: super::super::render::display::BgFill::Solid(rgb) } if rgb == [255u8, 0, 0]))
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
    // parent 区(首行):背景盒 + 文本项的 z 都必须 ≥ 50(祖先下限)。
    let parent_items: Vec<_> = layout.display.iter().filter(|it| it.y < 40.0).collect();
    assert!(!parent_items.is_empty(), "父区应有绘制项:{zs:?}");
    assert!(
        parent_items.iter().all(|it| it.z >= 50),
        "父级 z-index:50 应下传到子树全部项:{zs:?}"
    );
    // 兄弟区(次行):自身 z=1 生效。
    let sib_items: Vec<_> = layout.display.iter().filter(|it| it.y >= 40.0).collect();
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
