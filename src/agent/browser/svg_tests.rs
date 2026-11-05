//! SVG 矢量解析与光栅化回归(第 50 轮)。
//!
//! 覆盖:path 指令(绝对/相对/弧线/闭合)、viewBox/aspect 映射、
//! use/defs 引用、绘制属性继承、形状进入显示列表且撤占位、真实像素。

use super::document::Document;
use super::layout;
use super::render::display::DisplayKind;
use super::render::FontSet;
use super::svg::{collect_shapes, parse_path, parse_points, transform_str};
use super::style::transform::Affine;
use std::collections::HashMap;

use super::images::ImageStore;
use super::render::display::StickyInfo;

fn font() -> FontSet {
    FontSet::fallback()
}

fn layout_html(html: &str) -> layout::Layout {
    let doc = Document::parse(html, "https://a.test/");
    layout::compute(&doc, 800.0, &font())
}

fn svg_shapes(html: &str) -> Vec<super::svg::SvgShape> {
    let doc = Document::parse(html, "https://a.test/");
    let root = doc
        .root()
        .select("svg")
        .expect("svg 选择器")
        .next()
        .expect("页面必须有 svg");
    let root = root.as_node().clone();
    collect_shapes(&root, 100.0, 100.0, [10u8, 20, 30])
}

// ---------- path / points 解析 ----------

#[test]
fn path绝对坐标M_L_Z闭合() {
    let sp = parse_path("M 0 0 L 10 0 L 10 10 Z").expect("应解析出子路径");
    assert_eq!(sp.len(), 1, "Z 闭合单子路径");
    let (pts, closed) = &sp[0];
    assert!(*closed, "Z 后子路径应标记闭合");
    // M 起点 + 两个 L 终点(Z 回起点不另加点,闭合由描边补)。
    assert_eq!(pts.len(), 3, "实际点数 {pts:?}");
    assert_eq!(pts[0], (0.0, 0.0));
    assert_eq!(pts[2], (10.0, 10.0));
}

#[test]
fn path相对坐标与HV指令() {
    let sp = parse_path("m 10 10 h 20 v 20 h -20 z").expect("应解析");
    let (pts, closed) = &sp[0];
    assert!(*closed);
    // m(10,10) → h(30,10) → v(30,30) → h(10,30)
    assert_eq!(pts[0], (10.0, 10.0));
    assert_eq!(pts[1], (30.0, 10.0));
    assert_eq!(pts[2], (30.0, 30.0));
    assert_eq!(pts[3], (10.0, 30.0));
}

#[test]
fn path弧线指令端点正确() {
    let sp = parse_path("M 0 0 A 10 10 0 0 1 20 0").expect("弧线应解析");
    let (pts, _) = &sp[0];
    let last = pts.last().expect("弧线应有采样点");
    assert!((last.0 - 20.0).abs() < 1e-6 && last.1.abs() < 1e-6, "弧线终点 (20,0),实际 {last:?}");
    assert!(pts.len() >= 6, "半圆弧至少 6 个采样点,实际 {}", pts.len());
    // sweep=1(屏幕坐标顺时针):左点→12 点→右点,弧在 y<0 半平面(向上凸)。
    let min_y = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    assert!(min_y < -5.0, "顺时针半圆应向上凸(圆心 y=0),最小 y={min_y}");
}

#[test]
fn path立方贝塞尔端点与中点() {
    let sp = parse_path("M 0 0 C 0 10 10 10 10 0").expect("C 应解析");
    let (pts, _) = &sp[0];
    let last = pts.last().unwrap();
    assert!((last.0 - 10.0).abs() < 1e-6 && last.1.abs() < 1e-6, "终点 (10,0),实际 {last:?}");
    // t=0.5:y = 0.125·0 + 0.375·10 + 0.375·10 + 0.125·0 = 7.5;x = 5。
    // 注意 pts[0] 是 M 起点,flatten 点从 t>0 开始。
    let mid = pts[pts.len() / 2];
    assert!((mid.0 - 5.0).abs() < 0.3 && (mid.1 - 7.5).abs() < 0.3, "贝塞尔中点 (5,7.5),实际 {mid:?}");
}

#[test]
fn points属性负号粘连分隔() {
    let pts = parse_points("1,2 3-4 5,6");
    assert_eq!(pts, vec![(1.0, 2.0), (3.0, -4.0), (5.0, 6.0)]);
}

#[test]
fn transform链复合() {
    let m = transform_str("translate(10,0) scale(2)");
    let (x, y) = m.apply(1.0, 1.0);
    assert!((x - 12.0).abs() < 1e-9 && (y - 2.0).abs() < 1e-9, "先 scale 再 translate,实际 ({x},{y})");
}

// ---------- 形状收集与映射 ----------

#[test]
fn viewBox等比映射与居中() {
    let shapes = svg_shapes(
        "<svg viewBox='0 0 100 100' width='100' height='100'><circle cx='50' cy='50' r='40'/></svg>",
    );
    assert_eq!(shapes.len(), 1, "circle 应产出 1 个形状");
    let s = &shapes[0];
    let (pts, _) = &s.subpaths[0];
    let xs: Vec<f64> = pts.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
    let (minx, maxx) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
    let (miny, maxy) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
    // 内容盒 100×100 == viewBox 100×100 → 1:1,圆心 (50,50) r40。
    assert!((minx - 10.0).abs() < 1.0 && (maxx - 90.0).abs() < 1.0, "x 范围应 10..90,实际 {minx}..{maxx}");
    assert!((miny - 10.0).abs() < 1.0 && (maxy - 90.0).abs() < 1.0, "y 范围应 10..90,实际 {miny}..{maxy}");
    assert!(s.fill.is_some(), "SVG 缺省 fill 黑");
}

#[test]
fn viewBox宽盒meet居中缩放() {
    // viewBox 100×100 → 内容盒 200×100:meet 取 min 缩放 1.0,x 居中偏移 50。
    let doc = Document::parse(
        "<svg viewBox='0 0 100 100' width='200' height='100'><rect x='0' y='0' width='100' height='100'/></svg>",
        "https://a.test/",
    );
    let root = doc.root().select("svg").unwrap().next().unwrap();
    let root = root.as_node().clone();
    let shapes = collect_shapes(&root, 200.0, 100.0, [0u8, 0, 0]);
    let (pts, _) = &shapes[0].subpaths[0];
    let minx = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let maxx = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    assert!((minx - 50.0).abs() < 1.5, "meet 居中左边 50,实际 {minx}");
    assert!((maxx - 150.0).abs() < 1.5, "meet 居中右边 150,实际 {maxx}");
}

#[test]
fn defs不直渲use内联引用() {
    let shapes = svg_shapes(
        "<svg><defs><rect id='r' x='0' y='0' width='30' height='30'/></defs><use href='#r' x='10' y='10'/></svg>",
    );
    assert_eq!(shapes.len(), 1, "defs 内不直渲,use 内联产出 1 形状");
    let (pts, _) = &shapes[0].subpaths[0];
    let minx = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    assert!((minx - 10.0).abs() < 1e-6, "use x=10 应平移形状,实际 minx={minx}");
}

#[test]
fn g属性继承与none覆盖() {
    let shapes = svg_shapes(
        "<svg><g fill='red' stroke='blue' stroke-width='2'><rect x='0' y='0' width='10' height='10'/><circle cx='5' cy='5' r='4' fill='none'/></g></svg>",
    );
    assert_eq!(shapes.len(), 2);
    assert!(shapes[0].fill.is_some(), "rect 继承 g 的 fill");
    assert!(shapes[0].stroke.is_some(), "rect 继承 g 的 stroke");
    assert!(shapes[0].stroke_w > 1.5, "stroke-width 继承,实际 {}", shapes[0].stroke_w);
    assert!(shapes[1].fill.is_none(), "circle fill=none 覆盖继承");
}

#[test]
fn currentColor取CSS颜色() {
    let shapes = svg_shapes(
        "<svg><path d='M 0 0 L 10 0 L 10 10 Z' fill='currentColor'/></svg>",
    );
    let s = &shapes[0];
    let (c, _) = s.fill.expect("currentColor 应解析为具体色");
    assert_eq!(c, [10, 20, 30], "currentColor 取传入的 CSS color");
}

#[test]
fn evenodd填充规则透传() {
    let shapes = svg_shapes(
        "<svg><path d='M 0 0 L 100 0 L 100 100 Z M 30 30 L 70 30 L 70 70 Z' fill-rule='evenodd'/></svg>",
    );
    assert!(shapes[0].evenodd, "fill-rule=evenodd 应透传");
}

// ---------- 布局接入 ----------

#[test]
fn svg形状进显示列表且撤占位() {
    let lay = layout_html(
        "<body><svg id='i' width='60' height='60' viewBox='0 0 24 24'><path d='M12 2 L22 12 L12 22 L2 12 Z' fill='#e63946'/></svg></body>",
    );
    assert!(
        lay.display.iter().any(|it| matches!(it.kind, DisplayKind::SvgShape { .. })),
        "svg path 必须产出 SvgShape 显示项"
    );
    assert!(
        !lay.display.iter().any(|it| matches!(&it.kind, DisplayKind::MediaFrameNote { tag, .. } if tag == "svg")),
        "有形状时不得再画 svg 空盒占位"
    );
}

#[test]
fn 空svg保留能力占位() {
    let lay = layout_html("<body><svg id='i' width='60' height='60'></svg></body>");
    assert!(
        lay.display.iter().any(|it| matches!(&it.kind, DisplayKind::MediaFrameNote { tag, .. } if tag == "svg")),
        "无形状 svg 应保留 MediaFrameNote 占位"
    );
}

// ---------- 像素 ----------

fn render_html(html: &str) -> image::RgbaImage {
    let mut doc = Document::parse(html, "https://a.test/");
    doc.apply_widget_state(&HashMap::new(), &HashMap::new(), &HashMap::new());
    let font = FontSet::fallback();
    let lay = layout::compute(&doc, 400.0, &font);
    let pm = font
        .render_pixmap(&lay, 400, 300, 0.0, 0.0, false, &ImageStore::new())
        .unwrap();
    image::load_from_memory(&pm.encode_png().unwrap()).unwrap().to_rgba8()
}

#[test]
fn 实心圆填充落到真实像素() {
    let img = render_html(
        "<body style='margin:0'><svg width='100' height='100' viewBox='0 0 100 100'><circle cx='50' cy='50' r='30' fill='#ff0000'/></svg></body>",
    );
    let c = img.get_pixel(50, 50).0;
    assert!(c[0] > 200 && c[1] < 80 && c[2] < 80, "圆心应为红色,实际 {c:?}");
    let edge = img.get_pixel(50, 21).0;
    assert!(edge[0] > 200 && edge[1] < 80, "圆上缘(半径30)应为红色,实际 {edge:?}");
    let out = img.get_pixel(5, 5).0;
    assert!(out[0] < 235 || out[1] > 220, "圆外应为白底,实际 {out:?}");
}

#[test]
fn evenodd环中心透明() {
    let img = render_html(
        "<body style='margin:0'><svg width='100' height='100' viewBox='0 0 100 100'><path d='M50 10 L90 50 L50 90 L10 50 Z M50 30 L70 50 L50 70 L30 50 Z' fill='#00aa00' fill-rule='evenodd'/></svg></body>",
    );
    let ring = img.get_pixel(50, 18).0;
    assert!(ring[1] > 140 && ring[0] < 80, "环带应为绿色,实际 {ring:?}");
    let hole = img.get_pixel(50, 50).0;
    assert!(hole[0] > 235 && hole[1] > 235, "evenodd 中心孔应为白底,实际 {hole:?}");
}

#[test]
fn 描边折线真实像素() {
    let img = render_html(
        "<body style='margin:0'><svg width='100' height='100' viewBox='0 0 100 100'><path d='M10 50 L90 50' stroke='#0000ff' stroke-width='8' fill='none'/></svg></body>",
    );
    let on = img.get_pixel(50, 50).0;
    assert!(on[2] > 200 && on[0] < 80, "线中点应为蓝色,实际 {on:?}");
    let off = img.get_pixel(50, 30).0;
    assert!(off[0] > 235 && off[2] > 235, "线外 20px 应为白底,实际 {off:?}");
}

#[test]
fn sticky吸附参数端到端() {
    // sticky 头:静态 y=100,包含块高 600,吸顶阈值 0 → max_shift=600-200=400。
    let lay = layout_html(
        "<body style='margin:0'><div style='height:100px'></div>\
         <div id='h' style='position:sticky;top:0;height:100px;background:#333'></div>\
         <div style='height:400px'></div></body>",
    );
    let sticky_items: Vec<&super::render::display::DisplayItem> =
        lay.display.iter().filter(|it| it.sticky.is_some()).collect();
    assert!(!sticky_items.is_empty(), "sticky 元素子树应带吸附标记");
    let info = sticky_items[0].sticky.unwrap();
    assert_eq!(info.top, Some(0.0), "top:0 阈值");
    assert!((info.static_y - 100.0).abs() < 1.0, "静态 y=100,实际 {}", info.static_y);
    assert!(info.max_shift >= 399.0, "max_shift 应≈400,实际 {}", info.max_shift);
    // 吸附位移:scroll_y=50 → shift=50-100=0(还没滚到);scroll_y=150 → 50。
    assert_eq!(StickyInfo::shift(&info, 50.0), 0.0);
    assert!((StickyInfo::shift(&info, 150.0) - 50.0).abs() < 1e-9);
}

#[test]
fn sticky像素随滚动吸附() {
    let html = "<body style='margin:0'><div style='height:200px'></div>\
         <div id='h' style='position:sticky;top:0;height:60px;background:#111111'></div>\
         <div style='height:600px'></div></body>";
    let doc = Document::parse(html, "https://a.test/");
    let font = FontSet::fallback();
    let lay = layout::compute(&doc, 400.0, &font);
    let mk = |sy: f64| {
        let pm = font
            .render_pixmap(&lay, 400, 200, 0.0, sy, false, &ImageStore::new())
            .unwrap();
        image::load_from_memory(&pm.encode_png().unwrap()).unwrap().to_rgba8()
    };
    let before = mk(0.0);
    // 未滚动:头部在文档 y=200 → 视口外(视口高 200)。滚动 200 后头到视口顶。
    let after = mk(250.0);
    let top_px = after.get_pixel(200, 5).0;
    assert!(top_px[0] < 60 && top_px[1] < 60, "滚动 250 后 sticky 头应钉在视口顶,实际 {top_px:?}");
    let still = before.get_pixel(200, 5).0;
    assert!(still[0] > 235, "未滚动时视口顶应为白底,实际 {still:?}");
}
