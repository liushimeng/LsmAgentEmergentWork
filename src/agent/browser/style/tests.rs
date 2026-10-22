//! 样式引擎单元测试:级联、特异性、继承、组合符、单位、媒体查询、自定义属性。

use super::{MediaCtx, StyleEngine};
use crate::agent::browser::document::Document;
use crate::agent::browser::style::values::ComputedStyle;

fn doc_engine(html: &str, width: f64) -> (Document, StyleEngine) {
    let doc = Document::parse(html, "https://a.test/");
    let engine = StyleEngine::from_document(&doc, MediaCtx::new(width, 720.0));
    (doc, engine)
}

fn style_of(doc: &Document, engine: &StyleEngine, selector: &str) -> ComputedStyle {
    let node = doc
        .first(selector)
        .unwrap_or_else(|e| panic!("选择器非法 {selector}:{e}"))
        .unwrap_or_else(|| panic!("选择器未命中:{selector}"));
    engine.computed_chain(&node)
}

#[test]
fn 特异性更高的规则获胜() {
    let (doc, engine) = doc_engine(
        "<head><style>
            p { color: red; }
            .card p { color: blue; }
            p { color: green; }
         </style></head><body><div class='card'><p>Hi</p></div></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, "p").color, [0, 0, 255]);
}

#[test]
fn important_覆盖普通声明() {
    let (doc, engine) = doc_engine(
        "<head><style>
            p { color: red !important; }
            .card p { color: blue; }
         </style></head><body><div class='card'><p>Hi</p></div></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, "p").color, [255, 0, 0]);
}

#[test]
fn 内联样式优先于普通作者样式() {
    let (doc, engine) = doc_engine(
        "<head><style>#x { color: red; }</style></head>\
         <body><p id='x' style='color:#000000'>Hi</p></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, "p").color, [0, 0, 0]);
}

#[test]
fn 颜色与字号沿_dom_继承() {
    let (doc, engine) = doc_engine(
        "<head><style>body { color:#00ff00; font-size:20px; }</style></head>\
         <body><div><span>Child</span></div></body>",
        1280.0,
    );
    let st = style_of(&doc, &engine, "span");
    assert_eq!(st.color, [0, 255, 0]);
    assert!((st.font_size - 20.0).abs() < 0.01);
}

#[test]
fn 子代组合符不匹配深层后代() {
    let (doc, engine) = doc_engine(
        "<head><style>.a > .b { color:red; }</style></head>\
         <body><div class='a'><span class='b' id='direct'>D</span>\
         <div><span class='b' id='deep'>X</span></div></div></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, "#direct").color, [255, 0, 0]);
    assert_ne!(style_of(&doc, &engine, "#deep").color, [255, 0, 0]);
}

#[test]
fn 相邻兄弟与一般兄弟组合符() {
    let (doc, engine) = doc_engine(
        "<head><style>
            h1 + p { color:red; }
            h1 ~ span { color:blue; }
         </style></head><body><h1>T</h1><p id='p1'>A</p><p id='p2'>B</p>\
         <span id='s'>S</span></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, "#p1").color, [255, 0, 0]);
    assert_ne!(style_of(&doc, &engine, "#p2").color, [255, 0, 0]);
    assert_eq!(style_of(&doc, &engine, "#s").color, [0, 0, 255]);
}

#[test]
fn nth_child_与_not_伪类() {
    let (doc, engine) = doc_engine(
        "<head><style>
            li:nth-child(2n+1) { color:red; }
            li:not(.skip) { font-weight:bold; }
         </style></head><body><ul>\
         <li id='a'>1</li><li id='b'>2</li><li id='c' class='skip'>3</li></ul></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, "#a").color, [255, 0, 0]);
    assert_ne!(style_of(&doc, &engine, "#b").color, [255, 0, 0]);
    assert_eq!(style_of(&doc, &engine, "#c").color, [255, 0, 0]);
    assert_eq!(style_of(&doc, &engine, "#a").font_weight, 700);
    assert_eq!(style_of(&doc, &engine, "#c").font_weight, 400);
}

#[test]
fn em_和_rem_单位解析() {
    let (doc, engine) = doc_engine(
        "<head><style>
            body { font-size: 20px; }
            .box { font-size: 1.5em; margin-left: 2em; width: 2rem; }
         </style></head><body><div class='box'>X</div></body>",
        1280.0,
    );
    let st = style_of(&doc, &engine, ".box");
    assert!((st.font_size - 30.0).abs() < 0.01);
    assert!((st.margin_left - 60.0).abs() < 0.01);
    assert_eq!(st.width, Some(32.0));
}

#[test]
fn 媒体查询按视口求值() {
    let html = "<head><style>@media (min-width: 900px) { p { color:red; } }</style></head>\
                <body><p>X</p></body>";
    let (doc, engine) = doc_engine(html, 1000.0);
    assert_eq!(style_of(&doc, &engine, "p").color, [255, 0, 0]);
    let (doc2, engine2) = doc_engine(html, 600.0);
    assert_ne!(style_of(&doc2, &engine2, "p").color, [255, 0, 0]);
}

#[test]
fn 自定义属性继承与_var_回退() {
    let (doc, engine) = doc_engine(
        "<head><style>
            :root { --brand: #123456; }
            p { color: var(--brand); background-color: var(--missing, #ff0000); }
         </style></head><body><p>X</p></body>",
        1280.0,
    );
    let st = style_of(&doc, &engine, "p");
    assert_eq!(st.color, [0x12, 0x34, 0x56]);
    assert_eq!(st.background, Some([255, 0, 0]));
}

#[test]
fn display_与_visibility_语义区分() {
    let (doc, engine) = doc_engine(
        "<head><style>
            .gone { display:none; }
            .ghost { visibility:hidden; }
         </style></head>\
         <body><div class='gone'>A</div><div class='ghost'>B</div><div>C</div></body>",
        1280.0,
    );
    assert_eq!(style_of(&doc, &engine, ".gone").display, "none");
    let ghost = style_of(&doc, &engine, ".ghost");
    assert_eq!(ghost.display, "block");
    assert_eq!(ghost.visibility, "hidden");
}

#[test]
fn 样式表观测包含外链来源() {
    let (doc, engine) = doc_engine(
        "<head><style>p{color:red}</style>\
         <style data-lsm-href='https://a.test/a.css' media='screen'>p{font-size:18px}</style></head>\
         <body><p>X</p></body>",
        1280.0,
    );
    assert_eq!(engine.sheets().len(), 2);
    let sheet = engine
        .sheets()
        .iter()
        .find(|s| s.href.is_some())
        .expect("外链样式表元信息");
    assert_eq!(sheet.href.as_deref(), Some("https://a.test/a.css"));
    assert_eq!(sheet.media, "screen");
    assert_eq!(engine.rule_count(), 2);
    assert!((style_of(&doc, &engine, "p").font_size - 18.0).abs() < 0.01);
}

#[test]
fn 选择器特异性与组合符解析() {
    use super::selector::{parse_selector, Combinator};
    let sel = parse_selector(".a > .b#c").unwrap();
    assert_eq!(sel.specificity, (1, 2, 0));
    assert_eq!(sel.parts.len(), 2);
    assert_eq!(sel.parts[1].0, Combinator::Child);
    assert!(parse_selector("a:hover").is_some());
    assert!(parse_selector("a::before").is_none());
}

// ===================== Grid 样式(阶段三第二步) =====================

#[test]
fn 轨道列表解析_fr_minmax_repeat() {
    use super::values::{parse_track_list, TrackSize, ValueCtx};
    let ctx = ValueCtx::default();
    let tracks = parse_track_list("repeat(3, 1fr) minmax(10px, 20px) 100px 20%", 16.0, &ctx);
    assert_eq!(tracks.len(), 6, "tracks={:?}", tracks);
    assert_eq!(tracks[0], TrackSize::Fr(1.0));
    assert_eq!(tracks[2], TrackSize::Fr(1.0));
    assert!(matches!(tracks[3], TrackSize::MinMax(_, _)));
    assert_eq!(tracks[4], TrackSize::Px(100.0));
    assert_eq!(tracks[5], TrackSize::Percent(20.0));
}

#[test]
fn 轨道列表非法词与_repeat_auto_fill_丢弃() {
    use super::values::{parse_track_list, TrackSize, ValueCtx};
    let ctx = ValueCtx::default();
    let tracks = parse_track_list("foo 1fr auto-fill repeat(auto-fill, 2)", 16.0, &ctx);
    assert_eq!(tracks, vec![TrackSize::Fr(1.0)]);
    assert!(parse_track_list("none", 16.0, &ctx).is_empty());
}

#[test]
fn grid_共享关键字双写与专属属性() {
    let (doc, engine) = doc_engine(
        "<head><style>
            #t { display:grid; gap:8px; align-items:center; justify-content:flex-end;
                 grid-template-columns:1fr 2fr; grid-auto-flow:column;
                 justify-items:center; }
            #c { grid-column:1 / 3; }
         </style></head><body><div id=t><span id=c>x</span></div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    // 共享关键字双写到 grid 侧,并归一到 start/end 词表。
    assert_eq!(t.grid.row_gap, 8.0);
    assert_eq!(t.grid.col_gap, 8.0);
    assert_eq!(t.grid.align_items, "center");
    assert_eq!(t.grid.justify_content, "end");
    assert_eq!(t.grid.template_cols.len(), 2);
    assert_eq!(t.grid.auto_flow, super::values::GridFlow::Column);
    assert_eq!(t.grid.justify_items, "center");
    let c = style_of(&doc, &engine, "#c");
    use super::values::GridLine;
    assert_eq!(c.grid_item.col_start, GridLine::Int(1));
    assert_eq!(c.grid_item.col_end, GridLine::Int(3));
}

#[test]
fn grid_计算样式可见_template与放置() {
    let (doc, engine) = doc_engine(
        "<head><style>#t{display:grid;grid-template-columns:100px 1fr}</style></head>\
         <body><div id=t><b id=c style='grid-row:span 2'>x</b></div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    let map = t.to_map();
    assert_eq!(map.get("grid-template-columns").unwrap(), "100px 1fr");
    assert_eq!(map.get("grid-auto-flow").unwrap(), "row");
    let c = style_of(&doc, &engine, "#c");
    assert_eq!(c.grid_item.row_start, super::values::GridLine::Span(2));
}

#[test]
fn object_fit_与_背景图_可见于计算样式() {
    let (doc, engine) = doc_engine(
        "<head><style>#t{object-fit:cover;background-image:url(/a.png)}</style></head>\
         <body><div id=t>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.object_fit, "cover");
    assert_eq!(t.background_image.as_deref(), Some("/a.png"));
    let map = t.to_map();
    assert_eq!(map.get("object-fit").unwrap(), "cover");
    assert_eq!(map.get("background-image").unwrap(), "url(/a.png)");
}

#[test]
fn background_简写解析出图片_位置_与颜色() {
    // 图标/按钮/精灵图普遍用简写:`background: url(a.png) no-repeat -15px -136px`。
    // 此前简写只取颜色、丢掉 url(...) —— 整页 UI 渲染成空白(实测百度地图)。
    let (doc, engine) = doc_engine(
        "<head><style>#t{background:#fff url(/a.png) no-repeat -15px -136px}</style></head>\
         <body><div id=t>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.background_image.as_deref(), Some("/a.png"));
    let pos = t.background_position.expect("应解析出 background-position");
    assert_eq!(pos.x, super::values::BackgroundPos::Px(-15.0));
    assert_eq!(pos.y, super::values::BackgroundPos::Px(-136.0));
    assert_eq!(t.background_size, "auto");
    assert!(t.background.is_some(), "简写里的颜色也应保留");
}

#[test]
fn background_position_关键字与百分比归一() {
    use super::values::{parse_background_position, BackgroundPos};
    let p = parse_background_position("right bottom").unwrap();
    assert_eq!(p.x, BackgroundPos::Percent(100.0));
    assert_eq!(p.y, BackgroundPos::Percent(100.0));
    let p2 = parse_background_position("center").unwrap();
    assert_eq!(p2.x, BackgroundPos::Percent(50.0));
    assert_eq!(p2.y, BackgroundPos::Percent(50.0));
    let p3 = parse_background_position("50% 10px").unwrap();
    assert_eq!(p3.x, BackgroundPos::Percent(50.0));
    assert_eq!(p3.y, BackgroundPos::Px(10.0));
    assert!(parse_background_position("bogus").is_none());
}

#[test]
fn 背景图_none_与_非法_object_fit_回落默认值() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='background-image:none;object-fit:bogus'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert!(t.background_image.is_none());
    assert_eq!(t.object_fit, "fill");
}

#[test]
fn border_速写解析_宽度样式颜色() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='border:2px solid #ff0000'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.border_widths, [2.0; 4]);
    assert_eq!(t.border_drawn, [true; 4]);
    assert_eq!(
        t.border_colors,
        [super::values::BorderColor::Rgba([255, 0, 0], 255); 4]
    );
}

#[test]
fn border_速写_仅样式时线宽回落medium() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='border:solid'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.border_widths, [3.0; 4], "CSS 默认 medium=3px");
    assert_eq!(t.border_drawn, [true; 4]);
}

#[test]
fn border_none_不绘制且四值铺开正确() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='border:0;border-top:5px solid transparent;border-right:1px dashed rgb(0,0,255)'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    // border:0 先把四边清零,再按边覆盖:上 5px 透明可绘制,右 1px 蓝,下/左 0。
    assert_eq!(t.border_widths, [5.0, 1.0, 0.0, 0.0]);
    assert_eq!(t.border_drawn, [true, true, false, false]);
    assert_eq!(t.border_colors[0], super::values::BorderColor::Rgba([0, 0, 0], 0));
    assert_eq!(
        t.border_colors[1],
        super::values::BorderColor::Rgba([0, 0, 255], 255)
    );
}

#[test]
fn border_width_颜色四值速写() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='border-style:solid;border-width:1px 2px;border-color:#00ff00'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.border_widths, [1.0, 2.0, 1.0, 2.0]);
    assert_eq!(
        t.border_colors,
        [super::values::BorderColor::Rgba([0, 255, 0], 255); 4]
    );
}

#[test]
fn box_shadow_解析_长度与颜色() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='box-shadow:1px 2px 1px rgba(0,0,0,.15)'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.box_shadow.len(), 1);
    let s = &t.box_shadow[0];
    assert_eq!(s.dx, 1.0);
    assert_eq!(s.dy, 2.0);
    assert_eq!(s.blur, 1.0);
    assert_eq!(s.color, Some(([0, 0, 0], 38)), "0.15×255≈38");
}

#[test]
fn box_shadow_透明层与inset丢弃() {
    let (doc, engine) = doc_engine(
        "<body><div id=t style='box-shadow:inset 1px 1px #000, 2px 2px transparent, 3px 4px #ff0000'>x</div></body>",
        1280.0,
    );
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.box_shadow.len(), 1, "inset 与 transparent 层丢弃");
    assert_eq!(t.box_shadow[0].dx, 3.0);
    assert_eq!(t.box_shadow[0].color, Some(([255, 0, 0], 255)));
}

#[test]
fn 表单控件_ua_默认描边() {
    let (doc, engine) = doc_engine("<body><input id=t></body>", 1280.0);
    let t = style_of(&doc, &engine, "#t");
    assert_eq!(t.border_widths, [1.0; 4]);
    assert!(t.border_drawn[0], "input 默认边框可绘制");
    assert_eq!(t.background, Some([255, 255, 255]));
}
