//! 光栅器单元测试(从 canvas_raster.rs 第 56 轮拆分,测试数对账:15)。

// ---------- 单元测试 ----------

use super::*;

fn cv(key: &str, op: Value) -> Value {
    let none: Option<Vec<u8>> = None;
    let fetch = |_: &str| -> Option<Vec<u8>> { None };
    match handle(key, &op, none.as_deref(), &fetch) {
        CvOut::Json(v) => v,
        CvOut::Pixels(_) => json!({"error": "pixels"}),
    }
}

fn px(key: &str, x: i64, y: i64) -> [u8; 4] {
    let none: Option<Vec<u8>> = None;
    let fetch = |_: &str| -> Option<Vec<u8>> { None };
    let op = json!({"op":"get_pixels","x":x,"y":y,"w":1,"h":1});
    if let CvOut::Pixels(p) = handle(key, &op, none.as_deref(), &fetch) {
        [p[0], p[1], p[2], p[3]]
    } else {
        [0, 0, 0, 0]
    }
}

#[test]
fn 填充矩形与像素读取() {
    let k = "t-fill-rect";
    cv(k, json!({"op":"attach","w":20,"h":20}));
    cv(k, json!({"op":"fill_rect","x":2,"y":2,"w":10,"h":10,"style":"#ff0000","alpha":1,"comp":"source-over"}));
    let c = px(k, 5, 5);
    assert_eq!(c, [255, 0, 0, 255], "中心应为纯红:{c:?}");
    let outside = px(k, 15, 15);
    assert_eq!(outside, [0, 0, 0, 0]);
}

#[test]
fn 三角形路径填充() {
    let k = "t-tri";
    cv(k, json!({"op":"attach","w":30,"h":30}));
    for op in [
        json!({"op":"begin_path"}),
        json!({"op":"path_move","x":5,"y":5}),
        json!({"op":"path_line","x":25,"y":5}),
        json!({"op":"path_line","x":5,"y":25}),
        json!({"op":"close_path"}),
        json!({"op":"fill","style":"#00ff00","alpha":1,"comp":"source-over","rule":"nonzero"}),
    ] {
        cv(k, op);
    }
    let inside = px(k, 8, 8);
    assert_eq!(inside[1], 255, "三角形内部应有绿色:{inside:?}");
    let outside = px(k, 25, 25);
    assert_eq!(outside[3], 0, "对角外应透明");
}

#[test]
fn 圆弧填充抗锯齿() {
    let k = "t-arc";
    cv(k, json!({"op":"attach","w":60,"h":60}));
    for op in [
        json!({"op":"begin_path"}),
        json!({"op":"path_arc","cx":30,"cy":30,"r":20,"a0":0,"a1":6.28318,"ccw":false}),
        json!({"op":"fill","style":"#0000ff","alpha":1,"comp":"source-over"}),
    ] {
        cv(k, op);
    }
    assert_eq!(px(k, 30, 30), [0, 0, 255, 255], "圆心应纯蓝");
    // 半径外 3px 应完全透明(AA 带 ≤ 1px)。
    assert_eq!(px(k, 30, 6)[3], 0, "圆外应透明");
}

#[test]
fn 平移变换后绘制() {
    let k = "t-translate";
    cv(k, json!({"op":"attach","w":40,"h":40}));
    cv(k, json!({"op":"xform","kind":"translate","x":10,"y":10}));
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":5,"h":5,"style":"#ffffff","alpha":1,"comp":"source-over"}));
    assert_eq!(px(k, 12, 12), [255, 255, 255, 255], "平移后的矩形");
    assert_eq!(px(k, 2, 2)[3], 0, "原位不应有内容");
}

#[test]
fn 裁剪路径限制绘制() {
    let k = "t-clip";
    cv(k, json!({"op":"attach","w":40,"h":40}));
    for op in [
        json!({"op":"begin_path"}),
        json!({"op":"path_rect","x":10,"y":10,"w":10,"h":10}),
        json!({"op":"clip","rule":"nonzero"}),
        json!({"op":"fill_rect","x":0,"y":0,"w":40,"h":40,"style":"#123456","alpha":1,"comp":"source-over"}),
    ] {
        cv(k, op);
    }
    assert_eq!(px(k, 15, 15)[3], 255, "clip 内应有内容");
    assert_eq!(px(k, 5, 5)[3], 0, "clip 外应被裁掉");
}

#[test]
fn 线性渐变两端取色() {
    let k = "t-lgrad";
    cv(k, json!({"op":"attach","w":20,"h":4}));
    let g = cv(k, json!({"op":"grad_create","kind":"linear","x0":0,"y0":0,"x1":20,"y1":0}));
    let id = g["id"].as_u64().unwrap() as u32;
    cv(k, json!({"op":"grad_stop","id":id,"offset":0,"color":"#ff0000"}));
    cv(k, json!({"op":"grad_stop","id":id,"offset":1,"color":"#0000ff"}));
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":20,"h":4,"style":{"__cvGrad":id},"alpha":1,"comp":"source-over"}));
    let left = px(k, 0, 2);
    let right = px(k, 19, 2);
    assert!(left[0] > 240 && left[2] < 32, "左端应接近红:{left:?}");
    assert!(right[2] > 240 && right[0] < 32, "右端应接近蓝:{right:?}");
}

#[test]
fn 径向渐变中心取色() {
    let k = "t-rgrad";
    cv(k, json!({"op":"attach","w":20,"h":20}));
    let g = cv(k, json!({"op":"grad_create","kind":"radial","x0":10,"y0":10,"r0":0,"x1":10,"y1":10,"r1":10}));
    let id = g["id"].as_u64().unwrap() as u32;
    cv(k, json!({"op":"grad_stop","id":id,"offset":0,"color":"#ffffff"}));
    cv(k, json!({"op":"grad_stop","id":id,"offset":1,"color":"#000000"}));
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":20,"h":20,"style":{"__cvGrad":id},"alpha":1,"comp":"source-over"}));
    let c = px(k, 10, 10);
    assert!(c[0] > 225, "中心应接近白(像素中心偏 0.7px,t≈0.07):{c:?}");
}

#[test]
fn 虚线描边有间隔() {
    let k = "t-dash";
    cv(k, json!({"op":"attach","w":60,"h":10}));
    for op in [
        json!({"op":"set_dash","dash":[4,4]}),
        json!({"op":"begin_path"}),
        json!({"op":"path_move","x":0,"y":5}),
        json!({"op":"path_line","x":59,"y":5}),
        json!({"op":"stroke","style":"#ff00ff","line_w":2,"cap":"butt","join":"miter","alpha":1,"comp":"source-over","dash_offset":0}),
    ] {
        cv(k, op);
    }
    assert!(px(k, 1, 5)[3] > 0, "亮段起点应有描边");
    assert!(px(k, 6, 5)[3] == 0, "暗段中点应无描边");
    assert!(px(k, 10, 5)[3] > 0, "第二个亮段应有描边");
}

#[test]
fn destination_out_挖孔() {
    let k = "t-destout";
    cv(k, json!({"op":"attach","w":20,"h":20}));
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":20,"h":20,"style":"#00ffff","alpha":1,"comp":"source-over"}));
    cv(k, json!({"op":"fill_rect","x":5,"y":5,"w":10,"h":10,"style":"#000000","alpha":1,"comp":"destination-out"}));
    assert_eq!(px(k, 10, 10)[3], 0, "挖孔区应透明");
    assert_eq!(px(k, 2, 2)[3], 255, "孔外保留");
}

#[test]
fn save_restore_恢复变换() {
    let k = "t-save";
    cv(k, json!({"op":"attach","w":40,"h":40}));
    cv(k, json!({"op":"save"}));
    cv(k, json!({"op":"xform","kind":"translate","x":20,"y":20}));
    cv(k, json!({"op":"restore"}));
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":4,"h":4,"style":"#ffffff","alpha":1,"comp":"source-over"}));
    assert_eq!(px(k, 1, 1)[3], 255, "restore 后矩形应回到原位");
}

#[test]
fn to_data_url_按像素版本缓存() {
    let k = "t-url";
    cv(k, json!({"op":"attach","w":8,"h":8}));
    let u1 = cv(k, json!({"op":"to_data_url"}))["url"].as_str().unwrap().to_string();
    let u2 = cv(k, json!({"op":"to_data_url"}))["url"].as_str().unwrap().to_string();
    assert_eq!(u1, u2, "未变更时不应重复编码");
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":4,"h":4,"style":"#ffffff","alpha":1,"comp":"source-over"}));
    let u3 = cv(k, json!({"op":"to_data_url"}))["url"].as_str().unwrap().to_string();
    assert_ne!(u1, u3, "像素变更后应重新编码");
    assert!(u3.starts_with("data:image/png;base64,"));
}

#[test]
fn draw_image_画布互贴() {
    let src = "t-blit-src";
    let dst = "t-blit-dst";
    cv(src, json!({"op":"attach","w":10,"h":10}));
    cv(src, json!({"op":"fill_rect","x":0,"y":0,"w":10,"h":10,"style":"#ff8800","alpha":1,"comp":"source-over"}));
    cv(dst, json!({"op":"attach","w":20,"h":20}));
    cv(dst, json!({"op":"draw_image","src":{"canvas":src},"sx":0,"sy":0,"sw":10,"sh":10,"dx":5,"dy":5,"dw":10,"dh":10,"smoothing":false,"alpha":1,"comp":"source-over"}));
    let c = px(dst, 10, 10);
    assert_eq!(c, [255, 136, 0, 255], "贴图目标区中心:{c:?}");
    assert_eq!(px(dst, 2, 2)[3], 0, "贴图外应透明");
}

#[test]
fn measure_text_宽度非负() {
    let r = cv("t-measure", json!({"op":"measure","text":"AB12","f":{"px":16,"bold":false}}));
    let w = r["width"].as_f64().unwrap();
    assert!(w > 0.0, "真实字体或估算都应给出正宽度:{w}");
}

#[test]
fn clear_rect_清零() {
    let k = "t-clear";
    cv(k, json!({"op":"attach","w":10,"h":10}));
    cv(k, json!({"op":"fill_rect","x":0,"y":0,"w":10,"h":10,"style":"#ffffff","alpha":1,"comp":"source-over"}));
    cv(k, json!({"op":"clear_rect","x":2,"y":2,"w":4,"h":4}));
    assert_eq!(px(k, 3, 3)[3], 0, "清除区应透明");
    assert_eq!(px(k, 1, 1)[3], 255);
}

#[test]
fn 颜色解析_常见形态() {
    assert_eq!(parse_color("#f60"), Some([1.0, 102.0 / 255.0, 0.0, 1.0]));
    assert_eq!(parse_color("rgba(255, 0, 0, 0.5)"), Some([1.0, 0.0, 0.0, 0.5]));
    assert_eq!(parse_color("transparent"), Some([0.0, 0.0, 0.0, 0.0]));
    assert!(parse_color("hsl(120, 100%, 50%)").is_some());
    assert!(parse_color("不是颜色").is_none());
}
