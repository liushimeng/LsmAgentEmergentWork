//! 渲染层单测:既有用例(自 render.rs 搬迁对账)+ 阶段六新能力像素断言。

use super::super::images::{DecodedImage, ImageStore};
use super::super::{document::Document, layout};
use super::FontSet;

/// 便捷:布局 + 渲染,直接返回解码后的 RGBA 像素与尺寸。
fn render_html(html: &str, vw: u32, vh: u32) -> (image::RgbaImage) {
    let doc = Document::parse(html, "https://a.test/");
    let font = FontSet::fallback();
    let l = layout::compute(&doc, vw as f64, &font);
    let pm = font
        .render_pixmap(&l, vw, vh, 0.0, false, &ImageStore::new())
        .unwrap();
    decode(&pm)
}

fn decode(pm: &tiny_skia::Pixmap) -> image::RgbaImage {
    let png = pm.encode_png().unwrap();
    image::load_from_memory(&png).unwrap().to_rgba8()
}

fn pixel(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x, y).0
}

/// 构造带文本标签的显示项,便于断言 `sort_items` 的最终顺序。
fn tier_item(text: &str, z: i64, layer: u8, seq: u64) -> super::DisplayItem {
    super::DisplayItem {
        kind: super::DisplayKind::Text {
            text: text.to_string(),
            size: 16.0,
            color: [0, 0, 0],
            bold: false,
        },
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
        radius: [Default::default(); 4],
        z,
        seq,
        opacity: 1.0,
        clip: None,
        transform: None,
        fixed: false,
        layer,
    }
}

#[test]
fn 顶层绘制项压过高z定位内容() {
    let mut items = vec![
        tier_item("positioned", 1000, 1, 20),
        tier_item("top-layer", 0, 2, 30),
    ];
    super::display::sort_items(&mut items);
    let texts: Vec<&str> = items
        .iter()
        .map(|i| match &i.kind {
            super::DisplayKind::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(
        texts,
        vec!["positioned", "top-layer"],
        "top layer 必须最后绘制,压过普通 z-index"
    );
}

#[test]
fn 负z定位内容绘制在常规流之下() {
    let mut items = vec![
        tier_item("normal", 0, 0, 10),
        tier_item("negative", -5, 1, 20),
    ];
    super::display::sort_items(&mut items);
    let texts: Vec<&str> = items
        .iter()
        .map(|i| match &i.kind {
            super::DisplayKind::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(texts, vec!["negative", "normal"], "负 z 定位项应沉回常规流档之下");
}

#[test]
fn paint_tier_归一三层档位() {
    assert_eq!(tier_item("top", 9999, 2, 1).paint_tier(), 2);
    assert_eq!(tier_item("negative", -1, 1, 2).paint_tier(), 0);
    assert_eq!(tier_item("positioned", 10, 1, 3).paint_tier(), 1);
    assert_eq!(tier_item("normal", 0, 0, 4).paint_tier(), 0);
}

// ============ 既有用例(render.rs 搬迁,断言不变) ============

#[test]
fn 渲染输出_png_签名() {
    let doc = Document::parse("<body><h1>Hello</h1><p>World</p></body>", "https://a.test/");
    let font = FontSet::fallback();
    let l = layout::compute(&doc, 640.0, &font);
    let png = font.render(&l, 640, 480, 0.0, false, &ImageStore::new());
    assert!(png.len() > 8);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn 渲染绘制图片() {
    let font = FontSet::fallback();
    let l = layout::Layout {
        paints: vec![layout::PaintBox {
            src: "u".to_string(),
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 20.0,
            fit: "fill".to_string(),
            kind: layout::PaintKind::Content,
            visible: true,
            z_index: 0,
            off_x: 0.0,
            off_y: 0.0,
        }],
        width: 100.0,
        height: 100.0,
        ..Default::default()
    };
    let mut store = ImageStore::new();
    store.insert(
        "u".to_string(),
        std::sync::Arc::new(DecodedImage {
            width: 4,
            height: 4,
            rgba: (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect(),
        }),
    );
    let png = font.render(&l, 100, 100, 0.0, false, &store);
    let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
    let inside = decoded.get_pixel(20, 20).0;
    assert_eq!(&inside[..3], &[255, 0, 0], "图片区域应为红色");
    let outside = decoded.get_pixel(5, 5).0;
    assert_eq!(&outside[..3], &[255, 255, 255], "图片外应为白底");
}

#[test]
fn 字体度量回退可用() {
    let f = FontSet::fallback();
    assert!(f.char_width('中', 16.0) >= 16.0);
    assert!(f.char_width('a', 16.0) > 0.0);
}

#[test]
fn crop_pixmap_裁剪子区域() {
    // 4x2 像素:左半红右半蓝。
    let mut src = tiny_skia::Pixmap::new(4, 2).unwrap();
    let red = tiny_skia::PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
    let blue = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 255, 255).unwrap();
    let pixels = src.pixels_mut();
    for row in 0..2 {
        for col in 0..4 {
            pixels[row * 4 + col] = if col < 2 { red } else { blue };
        }
    }
    // 裁剪右半(x=2, w=2)。
    let cropped = super::crop_pixmap(&src, 2.0, 0.0, 2.0, 2.0).unwrap();
    let decoded = image::load_from_memory(&cropped).unwrap().to_rgba8();
    assert_eq!(decoded.width(), 2);
    assert_eq!(decoded.height(), 2);
    assert_eq!(&decoded.get_pixel(0, 0).0[..3], &[0, 0, 255]);
    assert_eq!(&decoded.get_pixel(1, 1).0[..3], &[0, 0, 255]);
}

#[test]
fn crop_pixmap_越界区域返回_none() {
    let src = tiny_skia::Pixmap::new(4, 2).unwrap();
    assert!(super::crop_pixmap(&src, 10.0, 10.0, 2.0, 2.0).is_none());
    assert!(super::crop_pixmap(&src, -5.0, 0.0, 2.0, 2.0).is_none());
}

#[test]
fn render_pixmap_与_render_一致() {
    let doc = Document::parse("<body><h1>Hi</h1></body>", "https://a.test/");
    let font = FontSet::fallback();
    let l = layout::compute(&doc, 320.0, &font);
    let pm = font
        .render_pixmap(&l, 320, 240, 0.0, false, &ImageStore::new())
        .unwrap();
    let png = pm.encode_png().unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(pm.width(), 320);
    assert_eq!(pm.height(), 240);
}

#[test]
fn 边框绘制在盒沿() {
    // 300×200 白底盒,2px 红边框:盒内白、盒沿红。
    let img = render_html(
        "<body><div style='width:300px;height:200px;border:2px solid #ff0000;background:#ffffff'></div></body>",
        800,
        600,
    );
    let edge = pixel(&img, 8, 8);
    assert!(edge[0] > 200 && edge[1] < 50 && edge[2] < 50, "左上角应为红:{edge:?}");
    let inside = pixel(&img, 30, 30);
    assert!(inside[0] > 200 && inside[1] > 200, "盒内应为白:{inside:?}");
}

#[test]
fn 外阴影绘制在背景之下() {
    // 100×100 白盒 + 1px 2px 纯黑阴影:盒外偏移处应出现黑色像素。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#ffffff;box-shadow:1px 2px 0 #000000'></div></body>",
        800,
        600,
    );
    // 盒 x∈[8,108]、阴影右移 1px 覆盖 x∈[9,109);背景绘制后仅 x=108 一列可见。
    let sp = pixel(&img, 108, 30);
    assert!(sp[0] < 50 && sp[1] < 50 && sp[2] < 50, "盒外偏移处应为黑:{sp:?}");
}

#[test]
fn 输入框_ua_默认边框可见() {
    // 裸 input(无页面样式):UA 1px 灰边框应出现在渲染输出里。
    let img = render_html("<body><input style='width:200px;height:30px'></body>", 800, 600);
    let edge = pixel(&img, 8, 8);
    assert!(
        edge[0] < 200 && edge[1] < 200 && edge[2] < 200,
        "输入框边缘应为灰边框而非纯白:{edge:?}"
    );
}

// ============ 阶段六新能力像素断言 ============

#[test]
fn 线性渐变_水平两端与中点() {
    // to right:左端红、右端蓝、中点约 50% 混合。
    let img = render_html(
        "<body><div style='width:200px;height:80px;background-image:linear-gradient(to right, #ff0000, #0000ff)'></div></body>",
        400,
        300,
    );
    // 盒 x∈[8,208)。
    let left = pixel(&img, 12, 40);
    assert!(left[0] > 200 && left[2] < 60, "左端应偏红:{left:?}");
    let right = pixel(&img, 203, 40);
    assert!(right[2] > 200 && right[0] < 60, "右端应偏蓝:{right:?}");
    let mid = pixel(&img, 107, 40);
    // t≈0.5:R≈127,B≈127。
    assert!(
        (90..170).contains(&mid[0]) && (90..170).contains(&mid[2]),
        "中点应约紫:{mid:?}"
    );
}

#[test]
fn 线性渐变_简写与度数() {
    let img = render_html(
        "<body><div style='width:120px;height:60px;background:linear-gradient(90deg, #ff0000, #0000ff)'></div></body>",
        300,
        200,
    );
    let left = pixel(&img, 12, 30);
    assert!(left[0] > 200, "90deg 左端应红:{left:?}");
    let right = pixel(&img, 123, 30);
    assert!(right[2] > 200, "90deg 右端应蓝:{right:?}");
}

#[test]
fn 径向渐变_中心与边缘() {
    let img = render_html(
        "<body><div style='width:160px;height:160px;background:radial-gradient(circle, #000000, #ffffff)'></div></body>",
        400,
        400,
    );
    let center = pixel(&img, 80, 80);
    assert!(center[0] < 90 && center[1] < 90, "中心应黑:{center:?}");
    let corner = pixel(&img, 165, 165);
    assert!(corner[0] > 200, "远角应白:{corner:?}");
}

#[test]
fn 圆角_角落透明中心填充() {
    // 100×100 黑盒 24px 圆角:角外白、中心黑、边中点黑。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#000000;border-radius:24px'></div></body>",
        300,
        300,
    );
    let corner = pixel(&img, 10, 10);
    assert!(corner[0] > 200, "左上角外应白(圆角裁掉):{corner:?}");
    let center = pixel(&img, 50, 50);
    assert!(center[0] < 50, "中心应黑:{center:?}");
    let edge_mid = pixel(&img, 50, 10);
    assert!(edge_mid[0] < 50, "上边中点应黑:{edge_mid:?}");
}

#[test]
fn 圆角_百分比半径成圆() {
    // border-radius:50% → r = min(w,h)/2 = 40:角落白、中心黑。
    let img = render_html(
        "<body><div style='width:80px;height:80px;background:#000000;border-radius:50%'></div></body>",
        300,
        300,
    );
    let corner = pixel(&img, 10, 10);
    assert!(corner[0] > 200, "角外应白:{corner:?}");
    let center = pixel(&img, 45, 45);
    assert!(center[0] < 50, "中心应黑:{center:?}");
}

#[test]
fn 圆角边框_环带可见() {
    // 80×80 盒 16px 圆角 + 4px 红边:角外白、环带红、内部白。
    let img = render_html(
        "<body><div style='width:80px;height:80px;background:#ffffff;border:4px solid #ff0000;border-radius:16px'></div></body>",
        300,
        300,
    );
    // 盒 (8,8)-(88,88):上边环带 y∈[8,12),x 中点 50。
    let ring = pixel(&img, 48, 9);
    assert!(ring[0] > 200 && ring[1] < 80, "上边环带应红:{ring:?}");
    let inside = pixel(&img, 48, 48);
    assert!(inside[0] > 200 && inside[1] > 200, "内部应白:{inside:?}");
    let corner_out = pixel(&img, 10, 10);
    assert!(corner_out[0] > 200, "角外应白:{corner_out:?}");
}

#[test]
fn 模糊阴影_边缘半透明_中心被背景覆盖() {
    // 100×100 白盒 + 10px 10px 12px 黑阴影:盒外近阴影处应为灰(半透明),
    // 盒内仍白(阴影被背景盖住)。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#ffffff;box-shadow:10px 10px 12px #000000'></div></body>",
        400,
        400,
    );
    let inside = pixel(&img, 50, 50);
    assert!(inside[0] > 240, "盒内应纯白:{inside:?}");
    // 盒右缘 108,阴影中心方向 (115,115):模糊区灰度应介于黑白之间。
    let g = pixel(&img, 115, 115);
    assert!(
        g[0] > 25 && g[0] < 235,
        "模糊区应为半透明灰:{g:?}"
    );
    // 更远处应接近白。
    let far = pixel(&img, 180, 180);
    assert!(far[0] > 240, "远离阴影应白:{far:?}");
}

#[test]
fn 透明度_半透明黑盒混合白底() {
    // opacity 0.5 的黑盒:渲染值 ≈ 127(黑 50% 混白)。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#000000;opacity:0.5'></div></body>",
        300,
        300,
    );
    let p = pixel(&img, 50, 50);
    assert!(
        (100..=160).contains(&p[0]) && (100..=160).contains(&p[1]) && (100..=160).contains(&p[2]),
        "半透明黑应混成中灰:{p:?}"
    );
}

#[test]
fn 溢出裁剪_子盒越界被裁() {
    // 60×30 overflow:hidden 白底容器,子盒 100×100 红:越界区域保持白。
    let img = render_html(
        "<body><div style='width:60px;height:30px;overflow:hidden;background:#ffffff'><div style='width:100px;height:100px;background:#ff0000'></div></div></body>",
        300,
        300,
    );
    let inside = pixel(&img, 20, 20);
    assert!(inside[0] > 200 && inside[1] < 60, "容器内子盒应红:{inside:?}");
    let beyond = pixel(&img, 100, 100);
    assert!(beyond[0] > 200 && beyond[1] > 200, "越界区应被裁成白:{beyond:?}");
    // 容器 60px 宽,右缘 x=68;(80,20) 已越界,应被裁回白底。
    let beyond_x = pixel(&img, 80, 20);
    assert!(beyond_x[1] > 200, "水平越界应被裁:{beyond_x:?}");
}

#[test]
fn 透明度_子树继承折叠() {
    // 父 opacity 0.5 + 子黑盒:子盒也应半透明(祖先组透明度乘进子项)。
    let img = render_html(
        "<body><div style='opacity:0.5'><div style='width:100px;height:100px;background:#000000'></div></div></body>",
        300,
        300,
    );
    let p = pixel(&img, 50, 50);
    assert!(
        (100..=160).contains(&p[0]),
        "子盒应继承组透明度(中灰):{p:?}"
    );
}

#[test]
fn 文本渲染_无字体占位条不崩() {
    let img = render_html("<body><p>示例文本</p></body>", 200, 100);
    // 无字体环境:整图仍可解码(占位条或空白均可,不 panic 即可)。
    assert_eq!(img.width(), 200);
}

// ============ 阶段六第二步:transform / text-shadow / 多背景层 ============

#[test]
fn transform_平移_整体偏移() {
    // 盒 (8,8)-(108,108) 平移 (50,20) → (58,28)-(158,128)。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#000000;transform:translate(50px,20px)'></div></body>",
        300,
        300,
    );
    let inside = pixel(&img, 60, 30);
    assert!(inside[0] < 50, "平移后盒内应黑:{inside:?}");
    let outside = pixel(&img, 10, 10);
    assert!(outside[0] > 200, "原位置应白:{outside:?}");
}

#[test]
fn transform_旋转45度_角外为白() {
    // 绕盒中心 (58,58) 旋转 45°:原四角落在菱形之外。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#000000;transform:rotate(45deg)'></div></body>",
        300,
        300,
    );
    let center = pixel(&img, 58, 58);
    assert!(center[0] < 50, "旋转后中心应黑:{center:?}");
    let axis = pixel(&img, 58, 120);
    assert!(axis[0] < 50, "旋转后竖直轴上应黑:{axis:?}");
    let corner = pixel(&img, 108, 108);
    assert!(corner[0] > 200, "原角外应白:{corner:?}");
}

#[test]
fn transform_缩放_尺寸放大() {
    // scale(2) 绕中心 (58,58):盒放大到 200×200 → (-42,-42)-(158,158)。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#000000;transform:scale(2)'></div></body>",
        300,
        300,
    );
    let inside = pixel(&img, 150, 150);
    assert!(inside[0] < 50, "放大后应黑:{inside:?}");
    let outside = pixel(&img, 170, 170);
    assert!(outside[0] > 200, "放大后仍应白:{outside:?}");
}

#[test]
fn transform_origin_绕右下角旋转() {
    // origin right bottom (108,108) + rotate(90deg) → 盒映射到 (108,8)-(208,108)。
    let img = render_html(
        "<body><div style='width:100px;height:100px;background:#000000;transform:rotate(90deg);transform-origin:right bottom'></div></body>",
        300,
        300,
    );
    let inside = pixel(&img, 150, 50);
    assert!(inside[0] < 50, "绕右下角旋转后应黑:{inside:?}");
    let outside = pixel(&img, 50, 50);
    assert!(outside[0] > 200, "原位置应白:{outside:?}");
}

#[test]
fn transform_嵌套_祖先链合成() {
    // 父 translate(20px,0) + 子 translate(30px,0) → 子盒右移 50px。
    let img = render_html(
        "<body><div style='transform:translate(20px,0)'><div style='width:60px;height:60px;background:#000000;transform:translate(30px,0)'></div></div></body>",
        300,
        300,
    );
    let inside = pixel(&img, 100, 30);
    assert!(inside[0] < 50, "嵌套平移合成后应黑:{inside:?}");
    let outside = pixel(&img, 30, 30);
    assert!(outside[0] > 200, "合成前位置应白:{outside:?}");
}

#[test]
fn text_shadow_阴影色可见() {
    let doc = Document::parse(
        "<body><p style='color:#000000;text-shadow:4px 4px 0 #ff0000'>MMMM</p></body>",
        "https://a.test/",
    );
    let font = FontSet::load();
    let l = layout::compute(&doc, 300.0, &font);
    let pm = font
        .render_pixmap(&l, 300, 300, 0.0, false, &ImageStore::new())
        .unwrap();
    let img = decode(&pm);
    let red = img
        .pixels()
        .filter(|p| p.0[0] > 200 && p.0[1] < 60 && p.0[2] < 60)
        .count();
    assert!(red > 0, "text-shadow 应出现红色阴影像素");
}

#[test]
fn text_shadow_模糊_边缘半透明() {
    let doc = Document::parse(
        "<body><p style='color:#000000;text-shadow:6px 6px 8px rgba(255,0,0,0.6)'>MMMM</p></body>",
        "https://a.test/",
    );
    let font = FontSet::load();
    let l = layout::compute(&doc, 300.0, &font);
    let pm = font
        .render_pixmap(&l, 300, 300, 0.0, false, &ImageStore::new())
        .unwrap();
    let img = decode(&pm);
    // 阴影区应出现红色分量明显高于绿/蓝的像素。
    let shadowed = img
        .pixels()
        .filter(|p| {
            let (r, g, b) = (p.0[0] as i32, p.0[1] as i32, p.0[2] as i32);
            r > 120 && r > g + 40 && r > b + 40
        })
        .count();
    assert!(shadowed > 0, "模糊 text-shadow 应出现红色像素");
}

#[test]
fn 多背景层_首层在上() {
    let red_top = render_html(
        "<body><div style='width:100px;height:100px;background-image:linear-gradient(to right, #ff0000, #ff0000),linear-gradient(to right, #0000ff, #0000ff)'></div></body>",
        300,
        300,
    );
    let p = pixel(&red_top, 50, 50);
    assert!(p[0] > 200 && p[2] < 60, "首层(红)应在上:{p:?}");
    let blue_top = render_html(
        "<body><div style='width:100px;height:100px;background-image:linear-gradient(to right, #0000ff, #0000ff),linear-gradient(to right, #ff0000, #ff0000)'></div></body>",
        300,
        300,
    );
    let p = pixel(&blue_top, 50, 50);
    assert!(p[2] > 200 && p[0] < 60, "首层(蓝)应在上:{p:?}");
}


#[test]
fn transform_flex条目_整体随变换() {
    // flex 条目 translate(40px,0):条目盒 (8,8)-(108,108) → 绘制在 (48,8)-(148,108)。
    let img = render_html(
        "<body><div style='display:flex'><div style='width:100px;height:100px;background:#0000ff;transform:translate(40px,0)'></div></div></body>",
        300,
        300,
    );
    let inside = pixel(&img, 60, 50);
    assert!(inside[2] > 200 && inside[0] < 60, "flex 条目平移后应蓝:{inside:?}");
    let outside = pixel(&img, 20, 50);
    assert!(outside[0] > 200, "平移前应白:{outside:?}");
}

// ============ 字体回退链 / 缓存(跨平台文本一致性) ============

/// 空链(`FontSet::fallback`):启发式宽度不 panic、栅格返回 None。
#[test]
fn 无字体链退化为启发式宽度() {
    let f = FontSet::fallback();
    assert!(!f.has_font());
    assert!(f.char_width('中', 16.0) > 0.0);
    assert!(f.text_width("abc", 16.0) > 0.0);
    assert!(f.rasterize_glyph('A', 16.0).is_none());
}

/// 回退链按覆盖逐字选字:主字体缺 CJK 时 `中` 落到后续字体
/// (Windows/Linux 中文豆腐块根因回归)。依赖本机字体文件,缺失即跳过。
#[test]
fn 回退链缺字形字符落到后续字体() {
    #[cfg(target_os = "macos")]
    let paths = [
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ];
    #[cfg(target_os = "windows")]
    let paths = [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\msyh.ttc",
    ];
    #[cfg(target_os = "linux")]
    let paths = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    let fonts: Vec<fontdue::Font> = paths
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).ok())
        .collect();
    if fonts.len() < 2 {
        // 本机字体不齐(裁剪环境/精简容器):跳过,不误伤。
        return;
    }
    let chain = FontSet::from_fonts(fonts, String::new());
    assert!(chain.covers('A'), "拉丁应被主字体覆盖");
    assert!(
        chain.covers('中'),
        "'中' 应由回退字体补齐(单字体实现恒 tofu)"
    );
    let w1 = chain.char_width('中', 16.0);
    let w2 = chain.char_width('中', 16.0);
    assert!((w1 - w2).abs() < 1e-9, "字宽缓存命中应一致:{w1} vs {w2}");
}

/// 平台主链加载:CJK 覆盖达标(macOS Arial Unicode / Windows msyh / Linux Noto/文泉驿)。
#[test]
fn 平台字体链覆盖中英文() {
    let f = FontSet::load();
    assert!(f.has_font(), "桌面平台应至少有一个可用字体");
    assert!(f.covers('A'), "拉丁覆盖");
    assert!(
        f.covers('中'),
        "CJK 覆盖(Linux 此前首选 DejaVu 无 CJK → 中文豆腐块)"
    );
}

/// 字形栅格缓存命中一致性:两次栅格化逐字节相同。
#[test]
fn 字形栅格缓存命中一致() {
    let f = FontSet::load();
    if !f.has_font() {
        return;
    }
    let a = f.rasterize_glyph('M', 16.0).expect("应能栅格化 M");
    let b = f.rasterize_glyph('M', 16.0).expect("缓存命中应返回字形");
    assert_eq!(a.0, b.0, "度量应一致");
    assert_eq!(a.1, b.1, "位图应逐字节一致");
}
