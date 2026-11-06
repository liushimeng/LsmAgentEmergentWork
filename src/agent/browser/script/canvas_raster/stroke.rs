//! 描边:距离场覆盖 + 虚线弧长分段(butt/round/square cap、round join 圆盘)。
//! 从 canvas_raster.rs 第 56 轮拆分(机械搬移)。

use super::*;


// ---------- 描边 ----------

pub(super) struct StrokeSegs {
    segs: Vec<(Pt, Pt)>,
    /// 圆帽/圆拐角的端点圆盘(距离场按点距离计)。
    discs: Vec<Pt>,
}

fn dist_segment(px: f64, py: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= f64::EPSILON {
        0.0
    } else {
        (((px - x0) * dx + (py - y0) * dy) / l2).clamp(0.0, 1.0)
    };
    ((px - x0 - t * dx).powi(2) + (py - y0 - t * dy).powi(2)).sqrt()
}

/// 把折线按虚线图样走段,把「亮段」的子线段全部吐出。
/// `pattern` 长度为偶数(奇数图样按规范重复一份),`offset` 为相位偏移。
fn dash_walk(pts: &[Pt], pattern: &[f64], offset: f64, out: &mut Vec<(Pt, Pt)>) {
    let total: f64 = pattern.iter().sum();
    if pattern.is_empty() || total <= 0.0 || pts.len() < 2 {
        return;
    }
    let mut phase = offset.rem_euclid(total);
    let mut slot = 0usize;
    // 跳过 offset 覆盖掉的长度(相位落入某个 slot 的中段)。
    let mut guard = 0usize;
    while guard < pattern.len() * 2 && phase >= pattern[slot] {
        phase -= pattern[slot];
        slot = (slot + 1) % pattern.len();
        guard += 1;
    }
    let mut remaining = (pattern[slot] - phase).max(0.0);
    let mut on = slot % 2 == 0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg_len = (b.0 - a.0).hypot(b.1 - a.1);
        if seg_len <= f64::EPSILON {
            continue;
        }
        let ux = (b.0 - a.0) / seg_len;
        let uy = (b.1 - a.1) / seg_len;
        let mut done = 0.0f64;
        while done < seg_len {
            let take = remaining.min(seg_len - done);
            let p0 = (a.0 + ux * done, a.1 + uy * done);
            let p1 = (a.0 + ux * (done + take), a.1 + uy * (done + take));
            if on && take > f64::EPSILON {
                out.push((p0, p1));
            }
            done += take;
            remaining -= take;
            if remaining <= f64::EPSILON {
                slot = (slot + 1) % pattern.len();
                remaining = pattern[slot];
                on = slot % 2 == 0;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_stroke_segs(
    path: &Subpaths,
    half: f64,
    cap: &str,
    join: &str,
    dash: &[f64],
    dash_scale: f64,
    dash_offset: f64,
) -> StrokeSegs {
    let mut segs: Vec<(Pt, Pt)> = Vec::new();
    let mut discs: Vec<Pt> = Vec::new();
    let dashing = !dash.is_empty() && dash.iter().all(|d| *d >= 0.0) && dash.iter().sum::<f64>() > 0.0;
    let round_cap = cap == "round";
    let square_cap = cap == "square";
    let round_join = join == "round";
    let extend = |segs: &mut Vec<(Pt, Pt)>, p: Pt, dir: (f64, f64)| {
        let e = (p.0 - dir.0 * half, p.1 - dir.1 * half);
        let q = (p.0 + dir.0 * half, p.1 + dir.1 * half);
        segs.push((e, q));
    };
    for (pts, closed) in path {
        let n = pts.len();
        if n < 2 {
            if n == 1 && round_cap {
                discs.push(pts[0]);
            }
            continue;
        }
        if dashing {
            // 奇数图样重复一份(交替语义)。
            let mut pat: Vec<f64> = dash.iter().map(|d| d * dash_scale).collect();
            if pat.len() % 2 == 1 {
                pat.extend(dash.iter().map(|d| d * dash_scale));
            }
            let mut walk: Vec<Pt> = pts.clone();
            if *closed {
                walk.push(pts[0]);
            }
            let mut visible: Vec<(Pt, Pt)> = Vec::new();
            dash_walk(&walk, &pat, dash_offset * dash_scale, &mut visible);
            for (a, b) in visible {
                segs.push((a, b));
                if round_cap {
                    discs.push(a);
                    discs.push(b);
                } else if square_cap {
                    let len = (b.0 - a.0).hypot(b.1 - a.1);
                    if len > f64::EPSILON {
                        extend(&mut segs, a, ((b.0 - a.0) / len, (b.1 - a.1) / len));
                        extend(&mut segs, b, ((a.0 - b.0) / len, (a.1 - b.1) / len));
                    }
                }
            }
            continue;
        }
        for k in 0..n - 1 {
            segs.push((pts[k], pts[k + 1]));
        }
        if *closed {
            segs.push((pts[n - 1], pts[0]));
            if round_join {
                discs.extend_from_slice(pts);
            }
        } else {
            if round_join {
                discs.extend_from_slice(&pts[1..n - 1]);
            }
            if round_cap {
                discs.push(pts[0]);
                discs.push(pts[n - 1]);
            } else if square_cap {
                let d0 = (pts[1].0 - pts[0].0, pts[1].1 - pts[0].1);
                let l0 = d0.0.hypot(d0.1);
                if l0 > f64::EPSILON {
                    extend(&mut segs, pts[0], (d0.0 / l0, d0.1 / l0));
                }
                let d1 = (pts[n - 1].0 - pts[n - 2].0, pts[n - 1].1 - pts[n - 2].1);
                let l1 = d1.0.hypot(d1.1);
                if l1 > f64::EPSILON {
                    extend(&mut segs, pts[n - 1], (d1.0 / l1, d1.1 / l1));
                }
            }
        }
    }
    StrokeSegs { segs, discs }
}

/// 描边覆盖缓冲:AABB 取全部段与圆盘的并集,逐段在其局部 AABB 内做距离场。
pub(super) fn stroke_cov_buf(segs: &StrokeSegs, half: f64, w: usize, h: usize) -> Option<CovBuf> {
    let mut bb: Option<(f64, f64, f64, f64)> = None;
    let acc = |x0: f64, y0: f64, x1: f64, y1: f64, bb: &mut Option<(f64, f64, f64, f64)>| {
        let next = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
        *bb = Some(match *bb {
            None => next,
            Some((a, b, c, d)) => (a.min(next.0), b.min(next.1), c.max(next.2), d.max(next.3)),
        });
    };
    for (a, b) in &segs.segs {
        acc(a.0, a.1, b.0, b.1, &mut bb);
    }
    for p in &segs.discs {
        acc(p.0, p.1, p.0, p.1, &mut bb);
    }
    let bb = bb?;
    let (bx, by, bw, bh) = clip_aabb(bb, half + 1.5, w, h)?;
    let mut cov = vec![0f32; bw * bh];
    let reach = half + 1.0;
    let paint_seg = |cov: &mut [f32], bx: i64, by: i64, bw: usize, bh: usize, a: Pt, b: Pt| {
        let sx0 = (a.0.min(b.0) - reach).floor().max(bx as f64) as i64;
        let sx1 = (a.0.max(b.0) + reach).ceil().min((bx + bw as i64) as f64) as i64;
        let sy0 = (a.1.min(b.1) - reach).floor().max(by as f64) as i64;
        let sy1 = (a.1.max(b.1) + reach).ceil().min((by + bh as i64) as f64) as i64;
        for py in sy0..sy1 {
            for px in sx0..sx1 {
                let d = dist_segment(px as f64 + 0.5, py as f64 + 0.5, a.0, a.1, b.0, b.1);
                let c = (half + 0.5 - d).clamp(0.0, 1.0);
                if c > 0.0 {
                    let idx = ((py - by) as usize) * bw + (px - bx) as usize;
                    if c as f32 > cov[idx] {
                        cov[idx] = c as f32;
                    }
                }
            }
        }
    };
    for (a, b) in &segs.segs {
        paint_seg(&mut cov, bx, by, bw, bh, *a, *b);
    }
    for p in &segs.discs {
        let sx0 = (p.0 - reach).floor().max(bx as f64) as i64;
        let sx1 = (p.0 + reach).ceil().min((bx + bw as i64) as f64) as i64;
        let sy0 = (p.1 - reach).floor().max(by as f64) as i64;
        let sy1 = (p.1 + reach).ceil().min((by + bh as i64) as f64) as i64;
        for py in sy0..sy1 {
            for px in sx0..sx1 {
                let d = ((px as f64 + 0.5 - p.0).powi(2) + (py as f64 + 0.5 - p.1).powi(2)).sqrt();
                let c = (half + 0.5 - d).clamp(0.0, 1.0);
                if c > 0.0 {
                    let idx = ((py - by) as usize) * bw + (px - bx) as usize;
                    if c as f32 > cov[idx] {
                        cov[idx] = c as f32;
                    }
                }
            }
        }
    }
    let bytes = cov.into_iter().map(|c| (c * 255.0).round() as u8).collect();
    Some(CovBuf { x: bx, y: by, w: bw, h: bh, cov: bytes })
}
