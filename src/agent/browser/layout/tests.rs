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
