//! 文字:FontSet 真实度量、对齐/基线、字形覆盖率蒙版、fill/stroke_text。
//! 从 canvas_raster.rs 第 56 轮拆分(机械搬移)。

use super::*;


// ---------- 文字 ----------

pub(super) struct FontSpec {
    pub(super) px: f64,
    pub(super) bold: bool,
}

pub(super) fn font_spec_of(op: &Value) -> FontSpec {
    let f = op.get("f").cloned().unwrap_or_else(|| json!({}));
    FontSpec {
        px: f_of(&f, "px", 10.0).clamp(1.0, 512.0),
        bold: f.get("bold").and_then(Value::as_bool).unwrap_or(false),
    }
}

pub(super) fn text_width_of(text: &str, px: f64) -> f64 {
    with_fonts(|f| {
        if f.has_font() {
            f.text_width(text, px)
        } else {
            text.chars().count() as f64 * px * 0.55
        }
    })
}

/// 文字对齐与基线的像素偏移(近似比率,与宿主页面文本渲染同源)。
fn text_align_offset(align: &str, total_w: f64) -> f64 {
    match align {
        "center" => -total_w / 2.0,
        "right" | "end" => -total_w,
        _ => 0.0,
    }
}
fn baseline_offset(base: &str, size: f64) -> f64 {
    match base {
        "top" => 0.8 * size,
        "hanging" => 0.65 * size,
        "middle" => 0.35 * size,
        "ideographic" => -0.05 * size,
        "bottom" => -0.2 * size,
        _ => 0.0,
    }
}

/// 文字覆盖率蒙版:真实字形光栅(无字体回退占位色条)。
/// 返回 (CovBuf, 笔宽 total_w)。
fn text_cov(
    text: &str,
    spec: &FontSpec,
    x: f64,
    y: f64,
    align: &str,
    base: &str,
    max_w: f64,
    ctm: &Affine,
    w: usize,
    h: usize,
) -> Option<(CovBuf, f64)> {
    if text.is_empty() {
        return None;
    }
    let scale = ctm_scale(ctm);
    let size = (spec.px * scale).clamp(1.0, 512.0);
    let measured = text_width_of(text, size);
    let x_scale = if max_w > 0.0 && measured > max_w { max_w / measured } else { 1.0 };
    let total_w = measured * x_scale;
    let (dx, dy) = ctm.apply(x, y);
    let pen_x = dx + text_align_offset(align, total_w);
    let base_y = dy + baseline_offset(base, size);
    // 包围盒:字形纵向范围按 size 近似(ascender ~1.0,descender ~0.25)。
    let bb = (pen_x - 2.0, base_y - size * 1.05, pen_x + total_w + 2.0, base_y + size * 0.3);
    let (bx, by, bw, bh) = clip_aabb(bb, 1.0, w, h)?;
    let mut cov = vec![0u8; bw * bh];
    let stamp = |cov: &mut [u8], gx: f64, gy: f64, gw: usize, gh: usize, bmp: &[u8]| {
        for j in 0..gh {
            let py = gy as i64 + j as i64;
            if py < by || py >= by + bh as i64 {
                continue;
            }
            for i in 0..gw {
                let px = gx as i64 + i as i64;
                if px < bx || px >= bx + bw as i64 {
                    continue;
                }
                let v = bmp[j * gw + i];
                let idx = (py - by) as usize * bw + (px - bx) as usize;
                if v > cov[idx] {
                    cov[idx] = v;
                }
            }
        }
    };
    let has_font = fonts_available();
    if !has_font {
        // 无字体:占位色条(与页面文本退化行为一致)。
        let y0 = (base_y - size * 0.35) as i64;
        let y1 = (base_y + size * 0.35) as i64;
        for py in y0.max(by)..y1.min(by + bh as i64) {
            for px in (pen_x as i64).max(bx)..((pen_x + total_w) as i64).min(bx + bw as i64) {
                cov[(py - by) as usize * bw + (px - bx) as usize] = 255;
            }
        }
        return Some((CovBuf { x: bx, y: by, w: bw, h: bh, cov }, total_w));
    }
    with_fonts(|fonts| {
        let mut pen = pen_x;
        for ch in text.chars() {
            let Some((metrics, bmp)) = fonts.rasterize_glyph(ch, size as f32) else {
                pen += fonts.char_width(ch, size) * x_scale;
                continue;
            };
            let gw = metrics.width;
            let gh = metrics.height;
            if gw > 0 && gh > 0 {
                let gx = pen + metrics.xmin as f64;
                let gy = base_y - gh as f64 - metrics.ymin as f64;
                stamp(&mut cov, gx, gy, gw, gh, &bmp);
                if spec.bold {
                    stamp(&mut cov, gx + 1.0, gy, gw, gh, &bmp);
                }
            }
            pen += metrics.advance_width as f64 * x_scale;
        }
    });
    Some((CovBuf { x: bx, y: by, w: bw, h: bh, cov }, total_w))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn op_text(
    st: &mut CanvasState,
    text: &str,
    x: f64,
    y: f64,
    spec: &FontSpec,
    align: &str,
    base: &str,
    max_w: f64,
    stroke_w: Option<f64>,
    paint: Paint,
    alpha: f64,
    comp: Comp,
    shadow: Option<Shadow>,
) {
    let ctm = st.ctm;
    let Some((mut cov, _)) = text_cov(text, spec, x, y, align, base, max_w, &ctm, st.w, st.h)
    else {
        return;
    };
    if let Some(clip) = st.clip.clone() {
        apply_clip(&mut cov, &clip, st.clip_evenodd);
    }
    if let Some(sh) = shadow.as_ref() {
        paint_shadow(st, &cov, sh, alpha);
    }
    if comp == Comp::Copy {
        st.pixels.fill(0);
    }
    match stroke_w {
        None => paint_cov(st, &cov, &paint, alpha, comp),
        Some(line_w) => {
            // 描边文字:字形蒙版在 ±(line_w/2) 邻域取最大(形态学膨胀近似)。
            let scale = ctm_scale(&ctm);
            let o = ((line_w * scale) / 2.0).round().max(1.0) as i64;
            let mut dil = vec![0u8; cov.cov.len()];
            for j in 0..cov.h {
                for i in 0..cov.w {
                    let mut m = 0u8;
                    for dj in -o..=o {
                        for di in -o..=o {
                            let si = i as i64 + di;
                            let sj = j as i64 + dj;
                            if si >= 0 && sj >= 0 && (si as usize) < cov.w && (sj as usize) < cov.h {
                                let v = cov.cov[sj as usize * cov.w + si as usize];
                                if v > m {
                                    m = v;
                                }
                            }
                        }
                    }
                    dil[j * cov.w + i] = m;
                }
            }
            cov.cov = dil;
            paint_cov(st, &cov, &paint, alpha, comp);
        }
    }
}
