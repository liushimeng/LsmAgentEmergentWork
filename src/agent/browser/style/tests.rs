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
