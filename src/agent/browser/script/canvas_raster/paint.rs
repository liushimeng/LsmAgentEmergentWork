//! 颜色解析 / 画笔(纯色·渐变) / 直行 RGBA 合成 / 填充覆盖率(扫描线超采样)。
//! 从 canvas_raster.rs 第 56 轮拆分(机械搬移)。

use super::*;

thread_local! {
    pub(super) static GRADS: RefCell<Vec<Option<GradDef>>> = const { RefCell::new(Vec::new()) };
}


pub(super) struct GradDef {
    pub(super) linear: bool,
    pub(super) x0: f64,
    pub(super) y0: f64,
    pub(super) r0: f64,
    pub(super) x1: f64,
    pub(super) y1: f64,
    pub(super) r1: f64,
    pub(super) stops: Vec<(f64, [f64; 4])>,
}


// ---------- 颜色与画笔 ----------

/// 解析 CSS 颜色为直行 rgba(0..=1)。覆盖 canvas 常见输入:命名色子集、
/// `#rgb/#rgba/#rrggbb/#rrggbbaa`、`rgb()/rgba()`(逗号或斜杠、百分比)、
/// `hsl()/hsla()`。非法输入返回 `None`(调用方回退黑)。
pub(super) fn parse_color(input: &str) -> Option<[f64; 4]> {
    let s = input.trim().to_ascii_lowercase();
    let named = |n: &str| -> Option<[f64; 4]> {
        let (r, g, b, a) = match n {
            "black" => (0.0, 0.0, 0.0, 1.0),
            "white" => (1.0, 1.0, 1.0, 1.0),
            "red" => (1.0, 0.0, 0.0, 1.0),
            "green" => (0.0, 128.0 / 255.0, 0.0, 1.0),
            "lime" => (0.0, 1.0, 0.0, 1.0),
            "blue" => (0.0, 0.0, 1.0, 1.0),
            "yellow" => (1.0, 1.0, 0.0, 1.0),
            "cyan" | "aqua" => (0.0, 1.0, 1.0, 1.0),
            "magenta" | "fuchsia" => (1.0, 0.0, 1.0, 1.0),
            "gray" | "grey" => (0.5, 0.5, 0.5, 1.0),
            "orange" => (1.0, 165.0 / 255.0, 0.0, 1.0),
            "purple" => (128.0 / 255.0, 0.0, 128.0 / 255.0, 1.0),
            "pink" => (1.0, 192.0 / 255.0, 203.0 / 255.0, 1.0),
            "brown" => (165.0 / 255.0, 42.0 / 255.0, 42.0 / 255.0, 1.0),
            "gold" => (1.0, 215.0 / 255.0, 0.0, 1.0),
            "silver" => (192.0 / 255.0, 192.0 / 255.0, 192.0 / 255.0, 1.0),
            "transparent" => (0.0, 0.0, 0.0, 0.0),
            _ => return None,
        };
        Some([r, g, b, a])
    };
    if let Some(c) = named(&s) {
        return Some(c);
    }
    if let Some(h) = s.strip_prefix('#') {
        let hx = |t: &str| u8::from_str_radix(t, 16).ok().map(|b| b as f64 / 255.0);
        return match h.len() {
            3 => Some([
                hx(&h[0..1].repeat(2))?,
                hx(&h[1..2].repeat(2))?,
                hx(&h[2..3].repeat(2))?,
                1.0,
            ]),
            4 => Some([
                hx(&h[0..1].repeat(2))?,
                hx(&h[1..2].repeat(2))?,
                hx(&h[2..3].repeat(2))?,
                hx(&h[3..4].repeat(2))?,
            ]),
            6 => Some([hx(&h[0..2])?, hx(&h[2..4])?, hx(&h[4..6])?, 1.0]),
            8 => Some([hx(&h[0..2])?, hx(&h[2..4])?, hx(&h[4..6])?, hx(&h[6..8])?]),
            _ => None,
        };
    }
    let paren = |name: &str| -> Option<&str> {
        s.strip_prefix(name)
            .and_then(|r| r.strip_prefix('('))
            .and_then(|r| r.strip_suffix(')'))
            .map(str::trim)
    };
    let parts_of = |body: &str| -> Vec<String> {
        body.replace('/', ",")
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    };
    // 通道分量:0..255 或百分比;alpha 例外,按 0..1(或百分比)。
    let chan = |p: &str| -> Option<f64> {
        if let Some(pct) = p.strip_suffix('%') {
            pct.trim().parse::<f64>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
        } else {
            p.trim().parse::<f64>().ok().map(|v| (v / 255.0).clamp(0.0, 1.0))
        }
    };
    let alpha_chan = |p: &str| -> Option<f64> {
        if let Some(pct) = p.strip_suffix('%') {
            pct.trim().parse::<f64>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
        } else {
            p.trim().parse::<f64>().ok().map(|v| v.clamp(0.0, 1.0))
        }
    };
    // 长前缀先匹配(rgba 会先命中 rgb 的前缀)。
    if let Some(body) = paren("rgba").or_else(|| paren("rgb")) {
        let parts = parts_of(body);
        if parts.len() >= 3 {
            let a = parts.get(3).and_then(|p| alpha_chan(p)).unwrap_or(1.0);
            return Some([chan(&parts[0])?, chan(&parts[1])?, chan(&parts[2])?, a]);
        }
        return None;
    }
    if let Some(body) = paren("hsla").or_else(|| paren("hsl")) {
        let parts = parts_of(body);
        if parts.len() >= 3 {
            let h_deg: f64 = parts[0].trim().trim_end_matches("deg").parse().ok()?;
            let sat = parts_of_pct(&parts[1])?;
            let lig = parts_of_pct(&parts[2])?;
            let a = parts.get(3).and_then(|p| alpha_chan(p)).unwrap_or(1.0);
            return Some(hsl_to_rgb(h_deg, sat, lig, a));
        }
        return None;
    }
    None
}

fn parts_of_pct(p: &str) -> Option<f64> {
    p.trim().strip_suffix('%')?.trim().parse::<f64>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
}

fn hsl_to_rgb(h_deg: f64, s: f64, l: f64, a: f64) -> [f64; 4] {
    let h = (h_deg.rem_euclid(360.0)) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [(r1 + m).clamp(0.0, 1.0), (g1 + m).clamp(0.0, 1.0), (b1 + m).clamp(0.0, 1.0), a]
}

/// 画笔引用:纯色 / 渐变(pattern 等哑对象 → None,不画)。
#[derive(Clone, Copy)]
pub(super) enum Paint {
    Solid([f64; 4]),
    Grad(u32),
    None,
}

pub(super) fn resolve_paint(v: &Value) -> Paint {
    if let Some(id) = v.get("__cvGrad").and_then(Value::as_u64) {
        return Paint::Grad(id as u32);
    }
    if v.get("__cvPattern").is_some() {
        return Paint::None;
    }
    let s = v.as_str().unwrap_or("#000000");
    match parse_color(s) {
        Some(c) => Paint::Solid(c),
        None => Paint::Solid([0.0, 0.0, 0.0, 1.0]),
    }
}

fn eval_gradient(id: u32, x: f64, y: f64) -> [f64; 4] {
    let t = GRADS.with(|g| {
        let defs = g.borrow();
        let Some(Some(d)) = defs.get(id as usize) else {
            return None;
        };
        let t = if d.linear {
            let (dx, dy) = (d.x1 - d.x0, d.y1 - d.y0);
            let l2 = dx * dx + dy * dy;
            if l2 <= f64::EPSILON {
                0.0
            } else {
                ((x - d.x0) * dx + (y - d.y0) * dy) / l2
            }
        } else {
            let dist = ((x - d.x0).powi(2) + (y - d.y0).powi(2)).sqrt();
            let dr = d.r1 - d.r0;
            if dr.abs() <= f64::EPSILON {
                0.0
            } else {
                (dist - d.r0) / dr
            }
        };
        // stops 预排序后的区间插值(预乘分量插值,边缘更自然)。
        let t = t.clamp(0.0, 1.0);
        let stops = &d.stops;
        if stops.is_empty() {
            return Some([0.0, 0.0, 0.0, 0.0]);
        }
        if stops.len() == 1 || t <= stops[0].0 {
            return Some(stops[0].1);
        }
        if t >= stops[stops.len() - 1].0 {
            return Some(stops[stops.len() - 1].1);
        }
        let mut lo = 0usize;
        while lo + 1 < stops.len() && stops[lo + 1].0 < t {
            lo += 1;
        }
        let (t0, c0) = stops[lo];
        let (t1, c1) = stops[lo + 1];
        let span = (t1 - t0).max(f64::EPSILON);
        let k = ((t - t0) / span).clamp(0.0, 1.0);
        let pm = |c: [f64; 4]| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
        let lerp = |a: f64, b: f64| a + (b - a) * k;
        let p0 = pm(c0);
        let p1 = pm(c1);
        let pr = lerp(p0[0], p1[0]);
        let pg = lerp(p0[1], p1[1]);
        let pb = lerp(p0[2], p1[2]);
        let pa = lerp(p0[3], p1[3]);
        if pa <= f64::EPSILON {
            return Some([0.0, 0.0, 0.0, 0.0]);
        }
        Some([pr / pa, pg / pa, pb / pa, pa])
    });
    t.unwrap_or([0.0, 0.0, 0.0, 0.0])
}

pub(super) fn eval_paint(paint: &Paint, x: f64, y: f64) -> [f64; 4] {
    match paint {
        Paint::Solid(c) => *c,
        Paint::Grad(id) => eval_gradient(*id, x, y),
        Paint::None => [0.0, 0.0, 0.0, 0.0],
    }
}


// ---------- 合成 ----------

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Comp {
    Over,
    Copy,
    DestOut,
    Xor,
    Lighter,
}

pub(super) fn comp_of(s: &str) -> Comp {
    match s {
        "copy" => Comp::Copy,
        "destination-out" => Comp::DestOut,
        "xor" => Comp::Xor,
        "lighter" => Comp::Lighter,
        _ => Comp::Over,
    }
}

/// 直行 RGBA 单像素合成。`cov` 为几何覆盖率(0..1),`alpha` 为 globalAlpha。
pub(super) fn blend_px(px: &mut [u8], i: usize, src: [f64; 4], cov: f64, alpha: f64, comp: Comp) {
    if cov <= 0.0 {
        if comp == Comp::Copy {
            px[i..i + 4].fill(0);
        }
        return;
    }
    let sa = (src[3] * cov * alpha).clamp(0.0, 1.0);
    let dr = px[i] as f64 / 255.0;
    let dg = px[i + 1] as f64 / 255.0;
    let db = px[i + 2] as f64 / 255.0;
    let da = px[i + 3] as f64 / 255.0;
    let (r, g, b, a) = match comp {
        Comp::Over => {
            let oa = sa + da * (1.0 - sa);
            if oa <= f64::EPSILON {
                (0.0, 0.0, 0.0, 0.0)
            } else {
                (
                    (src[0] * sa + dr * da * (1.0 - sa)) / oa,
                    (src[1] * sa + dg * da * (1.0 - sa)) / oa,
                    (src[2] * sa + db * da * (1.0 - sa)) / oa,
                    oa,
                )
            }
        }
        Comp::Copy => (src[0], src[1], src[2], sa),
        Comp::DestOut => (dr, dg, db, da * (1.0 - sa)),
        Comp::Xor => {
            let oa = sa * (1.0 - da) + da * (1.0 - sa);
            if oa <= f64::EPSILON {
                (0.0, 0.0, 0.0, 0.0)
            } else {
                (
                    (src[0] * sa * (1.0 - da) + dr * da * (1.0 - sa)) / oa,
                    (src[1] * sa * (1.0 - da) + dg * da * (1.0 - sa)) / oa,
                    (src[2] * sa * (1.0 - da) + db * da * (1.0 - sa)) / oa,
                    oa,
                )
            }
        }
        Comp::Lighter => (
            (src[0] * sa + dr * da).min(1.0),
            (src[1] * sa + dg * da).min(1.0),
            (src[2] * sa + db * da).min(1.0),
            (sa + da).min(1.0),
        ),
    };
    px[i] = (r * 255.0).round().clamp(0.0, 255.0) as u8;
    px[i + 1] = (g * 255.0).round().clamp(0.0, 255.0) as u8;
    px[i + 2] = (b * 255.0).round().clamp(0.0, 255.0) as u8;
    px[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
}


// ---------- 填充覆盖率 ----------

pub(super) struct CovBuf {
    pub(super) x: i64,
    pub(super) y: i64,
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) cov: Vec<u8>,
}

pub(super) fn path_bounds(path: &Subpaths) -> Option<(f64, f64, f64, f64)> {
    let mut it = path.iter().flat_map(|(pts, _)| pts.iter()).copied();
    let first = it.next()?;
    let mut bb = (first.0, first.1, first.0, first.1);
    for p in it {
        bb.0 = bb.0.min(p.0);
        bb.1 = bb.1.min(p.1);
        bb.2 = bb.2.max(p.0);
        bb.3 = bb.3.max(p.1);
    }
    Some(bb)
}

/// AABB 区域上的路径填充覆盖率(0..=255)。扫描线 + 2×2 超采样
/// (区域超大时降为 1× 保内存/耗时);填充语义把开放子路径按闭合处理。
pub(super) fn coverage_over_aabb(path: &Subpaths, bx: i64, by: i64, bw: usize, bh: usize, evenodd: bool) -> Vec<u8> {
    if bw == 0 || bh == 0 || path.is_empty() {
        return Vec::new();
    }
    let ss: usize = if bw * bh <= 1_000_000 { 2 } else { 1 };
    let sw = bw * ss;
    let sh = bh * ss;
    let mut mask = vec![0u8; sw * sh];
    for j in 0..sh {
        let y = by as f64 + (j as f64 + 0.5) / ss as f64;
        let mut cross: Vec<(f64, i32)> = Vec::new();
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
                    cross.push((a.0 + t * (b.0 - a.0), if b.1 > a.1 { 1 } else { -1 }));
                }
            }
        }
        if cross.is_empty() {
            continue;
        }
        cross.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
        let row = j * sw;
        let mut wind = 0i32;
        let mut parity = 0i32;
        let mut prev = cross[0].0;
        for &(cx, dir) in &cross {
            let inside = if evenodd { parity % 2 == 1 } else { wind != 0 };
            if inside {
                // 子采样点 x = bx + (i+0.5)/ss ∈ [prev, cx)
                let i0 = (((prev - bx as f64) * ss as f64 - 0.5).ceil() as isize).max(0);
                let i1 = (((cx - bx as f64) * ss as f64 - 0.5).ceil() as isize).min(sw as isize);
                for i in i0..i1.max(i0) {
                    if (i as usize) < sw {
                        mask[row + i as usize] = 255;
                    }
                }
            }
            wind += dir;
            parity += 1;
            prev = cx;
        }
    }
    if ss == 1 {
        return mask;
    }
    let mut out = vec![0u8; bw * bh];
    for py in 0..bh {
        for px in 0..bw {
            let mut sum = 0u32;
            for dj in 0..ss {
                for di in 0..ss {
                    sum += mask[(py * ss + dj) * sw + px * ss + di] as u32;
                }
            }
            out[py * bw + px] = (sum / 4) as u8;
        }
    }
    out
}

/// 与画布求交后的整数 AABB(pad 为外扩像素)。
pub(super) fn clip_aabb(
    bb: (f64, f64, f64, f64),
    pad: f64,
    w: usize,
    h: usize,
) -> Option<(i64, i64, usize, usize)> {
    let x0 = (bb.0 - pad).floor().max(0.0) as i64;
    let y0 = (bb.1 - pad).floor().max(0.0) as i64;
    let x1 = ((bb.2 + pad).ceil().min(w as f64)) as i64;
    let y1 = ((bb.3 + pad).ceil().min(h as f64)) as i64;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, (x1 - x0) as usize, (y1 - y0) as usize))
}

/// 覆盖率乘以 clip 路径(交集近似:逐像素相乘)。
pub(super) fn apply_clip(cov: &mut CovBuf, clip: &Subpaths, clip_evenodd: bool) {
    if cov.w == 0 || cov.h == 0 {
        return;
    }
    let cc = coverage_over_aabb(clip, cov.x, cov.y, cov.w, cov.h, clip_evenodd);
    for (dst, src) in cov.cov.iter_mut().zip(cc.iter()) {
        *dst = ((*dst as u32) * (*src as u32) / 255) as u8;
    }
}
