//! Canvas 2D 宿主光栅器(第 56 轮,设计见 56 号文档)。
//!
//! JS 线程内使用:画布像素/路径/变换的**权威状态**放在本模块的 thread_local
//! map(key 与 JS 侧 `__lsmCanvasByKey` 同源:`data-lsm-id` / selector /
//! `__lsmDynKey`),JS 薄壳(`canvas_js.rs`)把绘制命令经原生入口
//! `__lsmCv(key, opJson)` 分发进来。
//!
//! 为什么下沉:旧实现把像素留在 Boa 堆里逐像素循环绘制,2000×200 指纹画布
//! 的 `toDataURL` 实测 >50s;路径/文字/渐变此前全是 noop。本模块复用既有
//! Rust 光栅原语:
//! - 填充:扫描线 2×2 超采样卷绕(nonzero/evenodd);
//! - 描边:逐段距离场 + 虚线弧长分段(butt/round/square cap、round join);
//! - 阴影:`render::blur_mask` 三趟盒式模糊(σ = blur/2);
//! - 文字:`FontSet` 真实字体度量与字形光栅缓存;
//! - 变换:`style::transform::Affine`(路径构建时即变换进设备空间);
//! - PNG 编码:`image` crate,结果按像素版本缓存(同轮多次 flush 不重复编码)。
//!
//! 已知边界(56 号文档):`createPattern` 为哑对象;`Path2D`/`ctx.filter`/
//! 离屏级联未实现;字形不随 CTM 旋转/斜切(仅平移 + 等比缩放近似);
//! drawImage 不参与阴影;radial 渐变按同心圆近似。

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::{json, Value};

use crate::agent::browser::render::{blur_mask, FontSet};
use crate::agent::browser::style::transform::Affine;

type Pt = (f64, f64);
type Subpaths = Vec<(Vec<Pt>, bool)>;

/// 画布单边上限(与 JS 侧 `Math.min(4096, …)` 一致)。
const MAX_SIDE: usize = 4096;
/// 渐变注册表槽位上限(超限复用空位;再满则新渐变退化为纯色)。
const MAX_GRAD_SLOTS: usize = 4096;
/// 已解码图片缓存上限(超出丢最旧;URL 缓存同步清理)。
const MAX_DECODED_IMAGES: usize = 24;
/// 图片源解码字节上限(默认 12MB,与宿主图片管线 `LAEW_WEB_MAX_IMAGE_BYTES` 同源)。
fn max_image_bytes() -> usize {
    std::env::var("LAEW_WEB_MAX_IMAGE_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v >= 1024)
        .unwrap_or(12 * 1024 * 1024)
}

// ---------- 状态 ----------

#[derive(Clone)]
struct SavedState {
    ctm: Affine,
    clip: Option<Subpaths>,
    clip_evenodd: bool,
    dash: Vec<f64>,
}

struct CanvasState {
    w: usize,
    h: usize,
    /// 直行(非预乘)RGBA8,与 `getImageData` 语义一致。
    pixels: Vec<u8>,
    /// 当前路径(设备空间,构建时已按 CTM 变换)。
    path: Subpaths,
    /// 当前子路径的用户空间游标(arcTo/arc 续接语义用)。
    cur: Pt,
    has_cur: bool,
    /// 当前子路径起点(用户空间,close 后游标归位用)。
    sub_start: Option<Pt>,
    ctm: Affine,
    stack: Vec<SavedState>,
    /// 裁剪路径(设备空间,多 clip 交集由逐次求交近似:仅保留最近一次)。
    clip: Option<Subpaths>,
    clip_evenodd: bool,
    dash: Vec<f64>,
    /// 像素版本(每次写像素 +1)与编码版本(最近 PNG 编码时的像素版本)。
    pixel_rev: u64,
    enc_rev: u64,
    cached_url: String,
}

impl CanvasState {
    fn new(w: usize, h: usize) -> Self {
        let (w, h) = (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE));
        Self {
            w,
            h,
            pixels: vec![0u8; w * h * 4],
            path: Vec::new(),
            cur: (0.0, 0.0),
            has_cur: false,
            sub_start: None,
            ctm: Affine::identity(),
            stack: Vec::new(),
            clip: None,
            clip_evenodd: false,
            dash: Vec::new(),
            pixel_rev: 1,
            enc_rev: 0,
            cached_url: String::new(),
        }
    }
    fn touch(&mut self) {
        self.pixel_rev += 1;
    }
}


thread_local! {
    static CANVASES: RefCell<HashMap<String, CanvasState>> = RefCell::new(HashMap::new());
    static FONTS: RefCell<Option<FontSet>> = const { RefCell::new(None) };
}


fn with_fonts<R>(f: impl FnOnce(&FontSet) -> R) -> R {
    FONTS.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.is_none() {
            *s = Some(FontSet::load());
        }
        f(s.as_ref().expect("FontSet 已初始化"))
    })
}

fn fonts_available() -> bool {
    with_fonts(|f| f.has_font())
}


// ---------- JSON 参数助手 ----------

fn f_of(v: &Value, k: &str, d: f64) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(d)
}
fn s_of<'a>(v: &'a Value, k: &str, d: &'a str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or(d)
}
fn u_of(v: &Value, k: &str, d: usize) -> usize {
    v.get(k).and_then(Value::as_u64).map(|n| n as usize).unwrap_or(d)
}

mod draw;
mod image;
mod paint;
mod stroke;
mod text;

#[cfg(test)]
mod tests;

// 兄弟模块项经私有 use 重导入,子模块 `use super::*` 可直接取用
// (拆分规范:外部 `canvas_raster::handle/CvOut` 路径零改动)。
use draw::{ctm_scale, op_fill, op_stroke, paint_cov, paint_shadow, shadow_of, Shadow};
use image::{encode_png_url, images_lookup, op_draw_image, store_decoded_image, IMAGE_URLS};
use paint::{
    apply_clip, blend_px, clip_aabb, comp_of, coverage_over_aabb,
    eval_paint, parse_color, path_bounds, resolve_paint, Comp, CovBuf, GradDef, Paint, GRADS,
};
use stroke::{build_stroke_segs, stroke_cov_buf};
use text::{font_spec_of, op_text, text_width_of, FontSpec};

// ---------- 分发入口 ----------

pub(super) enum CvOut {
    /// JSON 对象(JS 侧 `JSON.parse` 回对象)。
    Json(Value),
    /// 像素字节(宿主转 `Uint8Array`)。
    Pixels(Vec<u8>),
}

/// `fetch_bytes` 由 JS 线程提供(处理 `data:` 与 http(s) 图片源取回)。
pub(super) fn handle(
    key: &str,
    op: &Value,
    extra: Option<&[u8]>,
    fetch_bytes: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> CvOut {
    let name = op.get("op").and_then(Value::as_str).unwrap_or("");
    // 与画布状态无关的 op。
    match name {
        "grad_create" => {
            let id = GRADS.with(|g| {
                let mut defs = g.borrow_mut();
                if defs.len() >= MAX_GRAD_SLOTS {
                    // 找空位复用;没有空位则失败(id 0 = 无效)。
                    return defs.iter().position(|d| d.is_none()).unwrap_or(usize::MAX) as u32;
                }
                defs.push(None);
                (defs.len() - 1) as u32
            });
            if id as usize == usize::MAX {
                return CvOut::Json(json!({"id": 0}));
            }
            let def = GradDef {
                linear: s_of(op, "kind", "linear") == "linear",
                x0: f_of(op, "x0", 0.0),
                y0: f_of(op, "y0", 0.0),
                r0: f_of(op, "r0", 0.0),
                x1: f_of(op, "x1", 0.0),
                y1: f_of(op, "y1", 0.0),
                r1: f_of(op, "r1", 0.0),
                stops: Vec::new(),
            };
            GRADS.with(|g| {
                let mut defs = g.borrow_mut();
                if let Some(slot) = defs.get_mut(id as usize) {
                    *slot = Some(def);
                }
            });
            return CvOut::Json(json!({"id": id}));
        }
        "grad_stop" => {
            let id = op.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
            let offset = f_of(op, "offset", 0.0).clamp(0.0, 1.0);
            let color = op
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_color)
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            GRADS.with(|g| {
                let mut defs = g.borrow_mut();
                if let Some(Some(d)) = defs.get_mut(id as usize) {
                    d.stops.push((offset, color));
                    d.stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                }
            });
            return CvOut::Json(json!({}));
        }
        "image_decode" => {
            let url = s_of(op, "url", "");
            if url.is_empty() {
                return CvOut::Json(json!({"id": 0}));
            }
            let cached = IMAGE_URLS.with(|m| m.borrow().get(url).copied());
            if let Some(id) = cached {
                if let Some((_, w, h)) = images_lookup(id) {
                    return CvOut::Json(json!({"id": id, "w": w, "h": h}));
                }
            }
            let Some(bytes) = fetch_bytes(url) else {
                return CvOut::Json(json!({"id": 0}));
            };
            if bytes.len() > max_image_bytes() {
                return CvOut::Json(json!({"id": 0}));
            }
            return match store_decoded_image(&bytes) {
                Some((id, w, h)) => {
                    IMAGE_URLS.with(|m| {
                        let mut urls = m.borrow_mut();
                        if urls.len() >= MAX_DECODED_IMAGES {
                            if let Some(oldest) = urls.values().copied().min() {
                                urls.retain(|_, v| *v != oldest);
                            }
                        }
                        urls.insert(url.to_string(), id);
                    });
                    CvOut::Json(json!({"id": id, "w": w, "h": h}))
                }
                None => CvOut::Json(json!({"id": 0})),
            }
        }
        "measure" => {
            let spec = font_spec_of(op);
            let text = s_of(op, "text", "");
            let w = text_width_of(text, spec.px);
            return CvOut::Json(json!({"width": w}));
        }
        _ => {}
    }
    // attach/resize 需要接管 map 全量(建/重置条目)。
    if name == "attach" {
        let w = u_of(op, "w", 300).clamp(1, MAX_SIDE);
        let h = u_of(op, "h", 150).clamp(1, MAX_SIDE);
        CANVASES.with(|m| {
            let mut map = m.borrow_mut();
            map.entry(key.to_string()).or_insert_with(|| CanvasState::new(w, h));
        });
        return CvOut::Json(json!({}));
    }
    if name == "resize" {
        let w = u_of(op, "w", 300).clamp(1, MAX_SIDE);
        let h = u_of(op, "h", 150).clamp(1, MAX_SIDE);
        CANVASES.with(|m| {
            m.borrow_mut().insert(key.to_string(), CanvasState::new(w, h));
        });
        return CvOut::Json(json!({}));
    }
    // draw_image 的源可能在本 map:先拷贝,避免同 map 读写借用冲突。
    let src_copy: Option<(Vec<u8>, usize, usize)> = if name == "draw_image" {
        let src = op.get("src").cloned().unwrap_or(Value::Null);
        if let Some(ck) = src.get("canvas").and_then(Value::as_str) {
            CANVASES.with(|m| {
                m.borrow().get(ck).map(|s| (s.pixels.clone(), s.w, s.h))
            })
        } else if let Some(id) = src.get("img").and_then(Value::as_u64) {
            images_lookup(id as u32)
        } else {
            None
        }
    } else {
        None
    };
    CANVASES.with(|m| {
        let mut map = m.borrow_mut();
        let st = map
            .entry(key.to_string())
            .or_insert_with(|| CanvasState::new(300, 150));
        dispatch_state(st, name, op, extra, &src_copy)
    })
}

fn dispatch_state(
    st: &mut CanvasState,
    name: &str,
    op: &Value,
    extra: Option<&[u8]>,
    src_copy: &Option<(Vec<u8>, usize, usize)>,
) -> CvOut {
    match name {
        "fill_rect" | "stroke_rect" | "clear_rect" => {
            let (x, y, w, h) = (f_of(op, "x", 0.0), f_of(op, "y", 0.0), f_of(op, "w", 0.0), f_of(op, "h", 0.0));
            // 四角各自过 CTM:旋转/斜切下的 fillRect 是变换后的平行四边形。
            let rect: Subpaths = vec![(
                vec![
                    st.ctm.apply(x, y),
                    st.ctm.apply(x + w, y),
                    st.ctm.apply(x + w, y + h),
                    st.ctm.apply(x, y + h),
                ],
                true,
            )];
            if name == "clear_rect" {
                // clearRect 与复合模式无关:覆盖率 > 0 的像素直接置透明。
                if let Some(bb) = path_bounds(&rect) {
                    if let Some((bx, by, bw, bh)) = clip_aabb(bb, 0.0, st.w, st.h) {
                        let cov = coverage_over_aabb(&rect, bx, by, bw, bh, false);
                        let mut touched = false;
                        for j in 0..bh {
                            for i in 0..bw {
                                if cov[j * bw + i] == 0 {
                                    continue;
                                }
                                let px = bx + i as i64;
                                let py = by + j as i64;
                                let idx = (py as usize * st.w + px as usize) * 4;
                                st.pixels[idx..idx + 4].fill(0);
                                touched = true;
                            }
                        }
                        if touched {
                            st.touch();
                        }
                    }
                }
                return CvOut::Json(json!({}));
            }
            if name == "fill_rect" {
                op_fill(st, &rect, false, resolve_paint(op.get("style").unwrap_or(&Value::Null)), f_of(op, "alpha", 1.0), comp_of(s_of(op, "comp", "")), shadow_of(op));
            } else {
                op_stroke(
                    st,
                    &rect,
                    resolve_paint(op.get("style").unwrap_or(&Value::Null)),
                    f_of(op, "line_w", 1.0),
                    s_of(op, "cap", "butt"),
                    "miter",
                    &[],
                    0.0,
                    f_of(op, "alpha", 1.0),
                    comp_of(s_of(op, "comp", "")),
                    shadow_of(op),
                );
            }
            CvOut::Json(json!({}))
        }
        "begin_path" => {
            st.path.clear();
            st.has_cur = false;
            st.sub_start = None;
            CvOut::Json(json!({}))
        }
        "close_path" => {
            if let Some(last) = st.path.last_mut() {
                last.1 = true;
            }
            if let Some(sp) = st.sub_start {
                st.cur = sp;
            }
            CvOut::Json(json!({}))
        }
        "path_move" | "path_line" => {
            let (x, y) = (f_of(op, "x", 0.0), f_of(op, "y", 0.0));
            let dev = st.ctm.apply(x, y);
            if name == "path_move" || !st.has_cur || st.path.is_empty() {
                st.path.push((vec![dev], false));
                st.sub_start = Some((x, y));
            } else if let Some(last) = st.path.last_mut() {
                last.0.push(dev);
            }
            st.cur = (x, y);
            st.has_cur = true;
            CvOut::Json(json!({}))
        }
        "path_quad" | "path_cubic" => {
            let (x, y) = (f_of(op, "x", 0.0), f_of(op, "y", 0.0));
            if !st.has_cur {
                st.path.push((vec![st.ctm.apply(x, y)], false));
                st.cur = (x, y);
                st.has_cur = true;
                st.sub_start = Some((x, y));
                return CvOut::Json(json!({}));
            }
            let p0 = st.cur;
            let steps = 14usize;
            for k in 1..=steps {
                let t = k as f64 / steps as f64;
                let (px, py) = if name == "path_quad" {
                    let (cpx, cpy) = (f_of(op, "cpx", 0.0), f_of(op, "cpy", 0.0));
                    let mt = 1.0 - t;
                    (
                        mt * mt * p0.0 + 2.0 * mt * t * cpx + t * t * x,
                        mt * mt * p0.1 + 2.0 * mt * t * cpy + t * t * y,
                    )
                } else {
                    let (c1x, c1y) = (f_of(op, "cp1x", 0.0), f_of(op, "cp1y", 0.0));
                    let (c2x, c2y) = (f_of(op, "cp2x", 0.0), f_of(op, "cp2y", 0.0));
                    let mt = 1.0 - t;
                    (
                        mt * mt * mt * p0.0 + 3.0 * mt * mt * t * c1x + 3.0 * mt * t * t * c2x + t * t * t * x,
                        mt * mt * mt * p0.1 + 3.0 * mt * mt * t * c1y + 3.0 * mt * t * t * c2y + t * t * t * y,
                    )
                };
                if let Some(last) = st.path.last_mut() {
                    last.0.push(st.ctm.apply(px, py));
                }
            }
            st.cur = (x, y);
            CvOut::Json(json!({}))
        }
        "path_arc" | "path_ellipse" => {
            let (cx, cy) = (f_of(op, "cx", 0.0), f_of(op, "cy", 0.0));
            let (rx, ry) = if name == "path_arc" {
                let r = f_of(op, "r", 0.0).abs();
                (r, r)
            } else {
                (f_of(op, "rx", 0.0).abs(), f_of(op, "ry", 0.0).abs())
            };
            let rot = if name == "path_ellipse" { f_of(op, "rot", 0.0) } else { 0.0 };
            let (a0, a1) = (f_of(op, "a0", 0.0), f_of(op, "a1", 0.0));
            let ccw = op.get("ccw").and_then(Value::as_bool).unwrap_or(false);
            let mut sweep = a1 - a0;
            if ccw {
                while sweep >= 0.0 {
                    sweep -= std::f64::consts::TAU;
                }
            } else {
                while sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
            }
            let point = |a: f64| -> Pt {
                let (s, c) = (a.sin(), a.cos());
                (
                    cx + rx * c * rot.cos() - ry * s * rot.sin(),
                    cy + rx * c * rot.sin() + ry * s * rot.cos(),
                )
            };
            let steps = ((sweep.abs() / 0.15).ceil() as usize).clamp(8, 256);
            let start = point(a0);
            if st.has_cur && name == "path_arc" {
                // 圆弧语义:当前点先连线到弧起点。
                if let Some(last) = st.path.last_mut() {
                    last.0.push(st.ctm.apply(start.0, start.1));
                }
            }
            for k in 0..=steps {
                let p = point(a0 + sweep * k as f64 / steps as f64);
                let dev = st.ctm.apply(p.0, p.1);
                if !st.has_cur {
                    st.path.push((vec![dev], false));
                    st.sub_start = Some(p);
                    st.has_cur = true;
                } else if let Some(last) = st.path.last_mut() {
                    last.0.push(dev);
                }
            }
            st.cur = point(a1);
            st.has_cur = true;
            CvOut::Json(json!({}))
        }
        "path_arc_to" => {
            let (x1, y1) = (f_of(op, "x1", 0.0), f_of(op, "y1", 0.0));
            let (x2, y2) = (f_of(op, "x2", 0.0), f_of(op, "y2", 0.0));
            let r = f_of(op, "r", 0.0).abs();
            let p0 = st.cur;
            let push_dev = |st: &mut CanvasState, p: Pt| {
                if let Some(last) = st.path.last_mut() {
                    last.0.push(st.ctm.apply(p.0, p.1));
                }
            };
            if !st.has_cur {
                return CvOut::Json(json!({}));
            }
            if r <= f64::EPSILON {
                push_dev(st, (x1, y1));
                st.cur = (x1, y1);
                return CvOut::Json(json!({}));
            }
            let d1 = ((x1 - p0.0), (y1 - p0.1));
            let d2 = ((x2 - x1), (y2 - y1));
            let l1 = d1.0.hypot(d1.1);
            let l2 = d2.0.hypot(d2.1);
            if l1 <= f64::EPSILON || l2 <= f64::EPSILON {
                return CvOut::Json(json!({}));
            }
            let u1 = (d1.0 / l1, d1.1 / l1);
            let u2 = (d2.0 / l2, d2.1 / l2);
            let cos_t = (u1.0 * u2.0 + u1.1 * u2.1).clamp(-1.0, 1.0);
            let theta = cos_t.acos();
            if (theta.abs() - std::f64::consts::PI).abs() < 1e-6 || theta < 1e-6 {
                // 共线:退化为直线到拐角。
                push_dev(st, (x1, y1));
                st.cur = (x1, y1);
                return CvOut::Json(json!({}));
            }
            let t_len = r / (theta / 2.0).tan();
            let t1 = (x1 - u1.0 * t_len, y1 - u1.1 * t_len);
            let t2 = (x1 + u2.0 * t_len, y1 + u2.1 * t_len);
            // 圆心:切点法线交点(等腰直角三角形,斜边 = 圆心到拐角距离)。
            let mid = (u1.0 + u2.0, u1.1 + u2.1);
            let ml = mid.0.hypot(mid.1);
            let bisector = if ml > f64::EPSILON { (mid.0 / ml, mid.1 / ml) } else { (0.0, 0.0) };
            let hypot_off = (r * r + t_len * t_len).sqrt();
            let center = (x1 - bisector.0 * hypot_off, y1 - bisector.1 * hypot_off);
            let a_start = (t1.1 - center.1).atan2(t1.0 - center.0);
            let a_end = (t2.1 - center.1).atan2(t2.0 - center.0);
            // 逆时针性:与拐角方向一致。
            let cross = u1.0 * u2.1 - u1.1 * u2.0;
            let ccw = cross < 0.0;
            let mut sweep = a_end - a_start;
            if ccw && sweep > 0.0 {
                sweep -= std::f64::consts::TAU;
            }
            if !ccw && sweep < 0.0 {
                sweep += std::f64::consts::TAU;
            }
            push_dev(st, t1);
            let steps = ((sweep.abs() / 0.15).ceil() as usize).clamp(4, 128);
            for k in 1..=steps {
                let a = a_start + sweep * k as f64 / steps as f64;
                push_dev(st, (center.0 + r * a.cos(), center.1 + r * a.sin()));
            }
            st.cur = t2;
            CvOut::Json(json!({}))
        }
        "path_rect" => {
            let (x, y, w, h) = (f_of(op, "x", 0.0), f_of(op, "y", 0.0), f_of(op, "w", 0.0), f_of(op, "h", 0.0));
            let a = st.ctm.apply(x, y);
            let b = st.ctm.apply(x + w, y);
            let c = st.ctm.apply(x + w, y + h);
            let d = st.ctm.apply(x, y + h);
            st.path.push((vec![a, b, c, d], true));
            st.cur = (x, y);
            st.has_cur = true;
            st.sub_start = Some((x, y));
            CvOut::Json(json!({}))
        }
        "fill" => {
            let evenodd = s_of(op, "rule", "nonzero") == "evenodd";
            op_fill(
                st,
                &st.path.clone(),
                evenodd,
                resolve_paint(op.get("style").unwrap_or(&Value::Null)),
                f_of(op, "alpha", 1.0),
                comp_of(s_of(op, "comp", "")),
                shadow_of(op),
            );
            CvOut::Json(json!({}))
        }
        "stroke" => {
            op_stroke(
                st,
                &st.path.clone(),
                resolve_paint(op.get("style").unwrap_or(&Value::Null)),
                f_of(op, "line_w", 1.0),
                s_of(op, "cap", "butt"),
                s_of(op, "join", "miter"),
                &st.dash.clone(),
                f_of(op, "dash_offset", 0.0),
                f_of(op, "alpha", 1.0),
                comp_of(s_of(op, "comp", "")),
                shadow_of(op),
            );
            CvOut::Json(json!({}))
        }
        "clip" => {
            let evenodd = s_of(op, "rule", "nonzero") == "evenodd";
            let path = st.path.clone();
            if path.is_empty() {
                return CvOut::Json(json!({}));
            }
            st.clip = Some(path);
            st.clip_evenodd = evenodd;
            CvOut::Json(json!({}))
        }
        "save" => {
            st.stack.push(SavedState {
                ctm: st.ctm,
                clip: st.clip.clone(),
                clip_evenodd: st.clip_evenodd,
                dash: st.dash.clone(),
            });
            CvOut::Json(json!({}))
        }
        "restore" => {
            if let Some(s) = st.stack.pop() {
                st.ctm = s.ctm;
                st.clip = s.clip;
                st.clip_evenodd = s.clip_evenodd;
                st.dash = s.dash;
            }
            CvOut::Json(json!({}))
        }
        "xform" => {
            let kind = s_of(op, "kind", "");
            let next = match kind {
                "translate" => Affine::translate(f_of(op, "x", 0.0), f_of(op, "y", 0.0)),
                "rotate" => {
                    let rad = f_of(op, "rad", 0.0);
                    let (s, c) = (rad.sin(), rad.cos());
                    Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
                }
                "scale" => Affine::scale(f_of(op, "sx", 1.0), f_of(op, "sy", 1.0)),
                "mult" | "set" => Affine {
                    a: f_of(op, "a", 1.0),
                    b: f_of(op, "b", 0.0),
                    c: f_of(op, "c", 0.0),
                    d: f_of(op, "d", 1.0),
                    e: f_of(op, "e", 0.0),
                    f: f_of(op, "f", 0.0),
                },
                _ => Affine::identity(),
            };
            if kind == "set" || kind == "reset" {
                st.ctm = if kind == "reset" { Affine::identity() } else { next };
            } else {
                // canvas 变换为后乘:新矩阵作用在用户坐标上,再走旧 CTM
                // (translate 后 rotate = 绕平移后的原点旋转)。
                st.ctm = st.ctm.compose(&next);
            }
            CvOut::Json(json!({}))
        }
        "set_dash" => {
            let dash = op
                .get("dash")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
                .unwrap_or_default();
            st.dash = dash;
            CvOut::Json(json!({}))
        }
        "fill_text" | "stroke_text" => {
            let spec = font_spec_of(op);
            op_text(
                st,
                s_of(op, "text", ""),
                f_of(op, "x", 0.0),
                f_of(op, "y", 0.0),
                &spec,
                s_of(op, "align", "start"),
                s_of(op, "base", "alphabetic"),
                f_of(op, "max_w", 0.0),
                if name == "stroke_text" { Some(f_of(op, "line_w", 1.0)) } else { None },
                resolve_paint(op.get("style").unwrap_or(&Value::Null)),
                f_of(op, "alpha", 1.0),
                comp_of(s_of(op, "comp", "")),
                shadow_of(op),
            );
            CvOut::Json(json!({}))
        }
        "draw_image" => {
            op_draw_image(
                st,
                src_copy.as_ref(),
                f_of(op, "sx", 0.0),
                f_of(op, "sy", 0.0),
                f_of(op, "sw", 0.0),
                f_of(op, "sh", 0.0),
                f_of(op, "dx", 0.0),
                f_of(op, "dy", 0.0),
                f_of(op, "dw", 0.0),
                f_of(op, "dh", 0.0),
                op.get("smoothing").and_then(Value::as_bool).unwrap_or(true),
                f_of(op, "alpha", 1.0),
                comp_of(s_of(op, "comp", "")),
            );
            CvOut::Json(json!({}))
        }
        "put_pixels" => {
            let (dx, dy) = (f_of(op, "x", 0.0) as i64, f_of(op, "y", 0.0) as i64);
            let (iw, ih) = (u_of(op, "w", 0), u_of(op, "h", 0));
            let (rx, ry, rw, rh) = (
                op.get("dx").and_then(Value::as_i64).filter(|v| *v >= 0),
                op.get("dy").and_then(Value::as_i64).filter(|v| *v >= 0),
                op.get("dw").and_then(Value::as_i64).filter(|v| *v > 0),
                op.get("dh").and_then(Value::as_i64).filter(|v| *v > 0),
            );
            let Some(bytes) = extra else { return CvOut::Json(json!({"error": "putImageData 缺像素"})) };
            if iw * ih * 4 != bytes.len() || iw == 0 || ih == 0 {
                return CvOut::Json(json!({"error": "putImageData 尺寸不匹配"}));
            }
            let (x0, y0, x1, y1) = match (rx, ry, rw, rh) {
                (Some(rx), Some(ry), Some(rw), Some(rh)) => (rx, ry, rx + rw, ry + rh),
                _ => (0, 0, iw as i64, ih as i64),
            };
            for iy in y0..y1 {
                for ix in x0..x1 {
                    if ix < 0 || iy < 0 || ix as usize >= iw || iy as usize >= ih {
                        continue;
                    }
                    let cxx = dx + ix;
                    let cyy = dy + iy;
                    if cxx < 0 || cyy < 0 || cxx as usize >= st.w || cyy as usize >= st.h {
                        continue;
                    }
                    let si = (iy as usize * iw + ix as usize) * 4;
                    let di = (cyy as usize * st.w + cxx as usize) * 4;
                    st.pixels[di..di + 4].copy_from_slice(&bytes[si..si + 4]);
                }
            }
            st.touch();
            CvOut::Json(json!({}))
        }
        "get_pixels" => {
            let (x, y) = (f_of(op, "x", 0.0) as i64, f_of(op, "y", 0.0) as i64);
            let (w, h) = (u_of(op, "w", 0), u_of(op, "h", 0));
            let mut out = vec![0u8; w.saturating_mul(h).saturating_mul(4)];
            for j in 0..h {
                for i in 0..w {
                    let sx = x + i as i64;
                    let sy = y + j as i64;
                    if sx < 0 || sy < 0 || sx as usize >= st.w || sy as usize >= st.h {
                        continue;
                    }
                    let si = (sy as usize * st.w + sx as usize) * 4;
                    let di = (j * w + i) * 4;
                    out[di..di + 4].copy_from_slice(&st.pixels[si..si + 4]);
                }
            }
            CvOut::Pixels(out)
        }
        "is_point_in_path" => {
            let (x, y) = st.ctm.apply(f_of(op, "x", 0.0), f_of(op, "y", 0.0));
            let evenodd = s_of(op, "rule", "nonzero") == "evenodd";
            let inside = point_in_paths(&st.path, x, y, evenodd);
            CvOut::Json(json!({"inside": inside}))
        }
        "to_data_url" => {
            if st.pixel_rev != st.enc_rev || st.cached_url.is_empty() {
                match encode_png_url(st.w, st.h, &st.pixels) {
                    Ok(url) => {
                        st.cached_url = url;
                        st.enc_rev = st.pixel_rev;
                    }
                    Err(e) => return CvOut::Json(json!({"error": e})),
                }
            }
            let url = st.cached_url.clone();
            CvOut::Json(json!({"url": url}))
        }
        _ => CvOut::Json(json!({"error": format!("未知 Canvas op:{name}")})),
    }
}


fn point_in_paths(path: &Subpaths, x: f64, y: f64, evenodd: bool) -> bool {
    let mut winding = 0i32;
    let mut crossings = 0i32;
    for (pts, _) in path {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        for k in 0..n {
            let a = pts[k];
            let b = pts[(k + 1) % n];
            if (a.1 <= y) != (b.1 <= y) {
                let t = (y - a.1) / (b.1 - a.1);
                let cx = a.0 + t * (b.0 - a.0);
                if cx > x {
                    crossings += 1;
                    winding += if b.1 > a.1 { 1 } else { -1 };
                }
            }
        }
    }
    if evenodd {
        crossings % 2 == 1
    } else {
        winding != 0
    }
}
