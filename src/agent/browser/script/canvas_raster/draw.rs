//! 阴影(三趟模糊 + 偏移合成)与绘制原语(上色/填充/描边 op)。
//! 从 canvas_raster.rs 第 56 轮拆分(机械搬移)。

use super::*;


// ---------- 阴影 ----------

pub(super) struct Shadow {
    blur: f64,
    color: [f64; 4],
    ox: f64,
    oy: f64,
}

pub(super) fn shadow_of(op: &Value) -> Option<Shadow> {
    let s = op.get("shadow")?;
    let color = s
        .get("color")
        .and_then(Value::as_str)
        .and_then(parse_color)
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    if color[3] <= 0.0 {
        return None;
    }
    Some(Shadow {
        blur: f_of(s, "blur", 0.0).clamp(0.0, 512.0),
        color,
        ox: f_of(s, "ox", 0.0),
        oy: f_of(s, "oy", 0.0),
    })
}

/// 按覆盖率蒙版画阴影:模糊 → 着色 → 偏移合成(先于本体绘制)。
pub(super) fn paint_shadow(st: &mut CanvasState, cov: &CovBuf, shadow: &Shadow, alpha: f64) {
    if cov.w == 0 || cov.h == 0 {
        return;
    }
    let mut mask = cov.cov.clone();
    blur_mask(&mut mask, cov.w, cov.h, shadow.blur / 2.0);
    // 阴影视域 = 原区域 ∪ 偏移区域,再夹回画布。
    let ux0 = cov.x.min(cov.x + shadow.ox as i64).max(0);
    let uy0 = cov.y.min(cov.y + shadow.oy as i64).max(0);
    let ux1 = (cov.x + cov.w as i64)
        .max(cov.x + shadow.ox as i64 + cov.w as i64)
        .min(st.w as i64);
    let uy1 = (cov.y + cov.h as i64)
        .max(cov.y + shadow.oy as i64 + cov.h as i64)
        .min(st.h as i64);
    for py in uy0..uy1 {
        for px in ux0..ux1 {
            let mx = px - shadow.ox as i64 - cov.x;
            let my = py - shadow.oy as i64 - cov.y;
            let m = if mx >= 0 && my >= 0 && (mx as usize) < cov.w && (my as usize) < cov.h {
                mask[my as usize * cov.w + mx as usize] as f64 / 255.0
            } else {
                0.0
            };
            if m > 0.0 {
                let i = (py as usize * st.w + px as usize) * 4;
                blend_px(&mut st.pixels, i, shadow.color, m, alpha, Comp::Over);
            }
        }
    }
    st.touch();
}

// ---------- 绘制原语 ----------

/// 覆盖缓冲整体上色(渐变逐像素求值;copy 先清全画布)。
pub(super) fn paint_cov(st: &mut CanvasState, cov: &CovBuf, paint: &Paint, alpha: f64, comp: Comp) {
    if cov.w == 0 || cov.h == 0 || matches!(paint, Paint::None) {
        return;
    }
    if comp == Comp::Copy {
        st.pixels.fill(0);
    }
    let solid = match paint {
        Paint::Solid(c) => Some(*c),
        _ => None,
    };
    for j in 0..cov.h {
        for i in 0..cov.w {
            let c = cov.cov[j * cov.w + i];
            if c == 0 {
                continue;
            }
            let x = cov.x as f64 + i as f64 + 0.5;
            let y = cov.y as f64 + j as f64 + 0.5;
            let color = solid.unwrap_or_else(|| eval_paint(paint, x, y));
            let px = cov.x + i as i64;
            let py = cov.y + j as i64;
            if px < 0 || py < 0 || px as usize >= st.w || py as usize >= st.h {
                continue;
            }
            let idx = (py as usize * st.w + px as usize) * 4;
            blend_px(&mut st.pixels, idx, color, c as f64 / 255.0, alpha, comp);
        }
    }
    st.touch();
}

/// 上色 + 阴影一步完成(阴影先画在本体之下)。
fn paint_cov_with_shadow(
    st: &mut CanvasState,
    cov: &CovBuf,
    paint: &Paint,
    alpha: f64,
    comp: Comp,
    shadow: Option<&Shadow>,
) {
    if let Some(sh) = shadow {
        paint_shadow(st, cov, sh, alpha);
    }
    paint_cov(st, cov, paint, alpha, comp);
}

pub(super) fn op_fill(st: &mut CanvasState, path: &Subpaths, evenodd: bool, paint: Paint, alpha: f64, comp: Comp, shadow: Option<Shadow>) {
    let Some(bb) = path_bounds(path) else { return };
    let Some((bx, by, bw, bh)) = clip_aabb(bb, 0.0, st.w, st.h) else { return };
    let mut cov = CovBuf { x: bx, y: by, w: bw, h: bh, cov: coverage_over_aabb(path, bx, by, bw, bh, evenodd) };
    if let Some(clip) = st.clip.clone() {
        apply_clip(&mut cov, &clip, st.clip_evenodd);
    }
    paint_cov_with_shadow(st, &cov, &paint, alpha, comp, shadow.as_ref());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn op_stroke(
    st: &mut CanvasState,
    path: &Subpaths,
    paint: Paint,
    line_w: f64,
    cap: &str,
    join: &str,
    dash: &[f64],
    dash_offset: f64,
    alpha: f64,
    comp: Comp,
    shadow: Option<Shadow>,
) {
    let scale = ctm_scale(&st.ctm);
    let eff_w = (line_w * scale).clamp(0.01, 1024.0);
    let segs = build_stroke_segs(path, eff_w / 2.0, cap, join, dash, scale, dash_offset);
    let Some(cov) = stroke_cov_buf(&segs, eff_w / 2.0, st.w, st.h) else { return };
    let mut cov = cov;
    if let Some(clip) = st.clip.clone() {
        apply_clip(&mut cov, &clip, st.clip_evenodd);
    }
    paint_cov_with_shadow(st, &cov, &paint, alpha, comp, shadow.as_ref());
}

pub(super) fn ctm_scale(ctm: &Affine) -> f64 {
    let det = (ctm.a * ctm.d - ctm.b * ctm.c).abs();
    det.sqrt().clamp(0.01, 128.0)
}
