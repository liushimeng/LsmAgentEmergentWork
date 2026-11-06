//! drawImage(逆 CTM 采样贴图)、图片源解码缓存与 PNG data URL 编码。
//! 从 canvas_raster.rs 第 56 轮拆分(机械搬移)。

use super::*;

thread_local! {
    pub(super) static IMAGES: RefCell<HashMap<u32, DecodedImage>> = RefCell::new(HashMap::new());
    pub(super) static IMAGE_URLS: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}


struct DecodedImage {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}


// ---------- drawImage ----------

fn sample_nearest(src: &[u8], sw: usize, sh: usize, u: f64, v: f64) -> [f64; 4] {
    let x = (u.floor() as i64).clamp(0, sw as i64 - 1) as usize;
    let y = (v.floor() as i64).clamp(0, sh as i64 - 1) as usize;
    let i = (y * sw + x) * 4;
    [
        src[i] as f64 / 255.0,
        src[i + 1] as f64 / 255.0,
        src[i + 2] as f64 / 255.0,
        src[i + 3] as f64 / 255.0,
    ]
}

fn sample_bilinear(src: &[u8], sw: usize, sh: usize, u: f64, v: f64) -> [f64; 4] {
    if sw == 0 || sh == 0 {
        return [0.0; 4];
    }
    let x0 = u.floor().clamp(0.0, sw as f64 - 1.0);
    let y0 = v.floor().clamp(0.0, sh as f64 - 1.0);
    let x1 = (x0 + 1.0).min(sw as f64 - 1.0);
    let y1 = (y0 + 1.0).min(sh as f64 - 1.0);
    let fx = u - x0;
    let fy = v - y0;
    let at = |x: f64, y: f64| -> [f64; 4] {
        let i = (y as usize * sw + x as usize) * 4;
        [
            src[i] as f64 / 255.0,
            src[i + 1] as f64 / 255.0,
            src[i + 2] as f64 / 255.0,
            src[i + 3] as f64 / 255.0,
        ]
    };
    let c00 = at(x0, y0);
    let c10 = at(x1, y0);
    let c01 = at(x0, y1);
    let c11 = at(x1, y1);
    let mut out = [0.0; 4];
    for k in 0..4 {
        let top = c00[k] * (1.0 - fx) + c10[k] * fx;
        let bot = c01[k] * (1.0 - fx) + c11[k] * fx;
        out[k] = top * (1.0 - fy) + bot * fy;
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(super) fn op_draw_image(
    st: &mut CanvasState,
    src: Option<&(Vec<u8>, usize, usize)>,
    sx: f64,
    sy: f64,
    sw: f64,
    sh: f64,
    dx: f64,
    dy: f64,
    dw: f64,
    dh: f64,
    smoothing: bool,
    alpha: f64,
    comp: Comp,
) {
    let Some((spix, sw_, sh_)) = src else { return };
    if dw <= 0.0 || dh <= 0.0 || sw <= 0.0 || sh <= 0.0 || *sw_ == 0 || *sh_ == 0 {
        return;
    }
    // 源矩形夹回源界(与浏览器一致,越界部分透明)。
    if sx < 0.0 || sy < 0.0 || sx + sw > *sw_ as f64 || sy + sh > *sh_ as f64 {
        // 简化:整体夹取,不拆子矩形(极端跨界场景少)。
        let cx0 = sx.max(0.0);
        let cy0 = sy.max(0.0);
        let cx1 = (sx + sw).min(*sw_ as f64);
        let cy1 = (sy + sh).min(*sh_ as f64);
        if cx1 <= cx0 || cy1 <= cy0 {
            return;
        }
        let nx = cx0 - sx;
        let ny = cy0 - sy;
        let nsw = cx1 - cx0;
        let nsh = cy1 - cy0;
        let ndx = dx + nx * dw / sw;
        let ndy = dy + ny * dh / sh;
        let ndw = nsw * dw / sw;
        let ndh = nsh * dh / sh;
        return op_draw_image(
            st,
            src,
            cx0,
            cy0,
            nsw,
            nsh,
            ndx,
            ndy,
            ndw,
            ndh,
            smoothing,
            alpha,
            comp,
        );
    }
    // 目标矩形四角过 CTM → AABB(支持旋转/缩放)。
    let corners = [
        st.ctm.apply(dx, dy),
        st.ctm.apply(dx + dw, dy),
        st.ctm.apply(dx, dy + dh),
        st.ctm.apply(dx + dw, dy + dh),
    ];
    let bb = corners.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, p| {
        (a.0.min(p.0), a.1.min(p.1), a.2.max(p.0), a.3.max(p.1))
    });
    let Some((bx, by, bw, bh)) = clip_aabb(bb, 1.0, st.w, st.h) else { return };
    let Some(inv) = st.ctm.invert() else { return };
    let clip = st.clip.clone();
    let clip_evenodd = st.clip_evenodd;
    if comp == Comp::Copy {
        st.pixels.fill(0);
    }
    for py in by..by + bh as i64 {
        for px in bx..bx + bw as i64 {
            let (ux, uy) = inv.apply(px as f64 + 0.5, py as f64 + 0.5);
            let u = sx + (ux - dx) * sw / dw;
            let v = sy + (uy - dy) * sh / dh;
            if u < 0.0 || v < 0.0 || u >= *sw_ as f64 || v >= *sh_ as f64 {
                continue;
            }
            let color = if smoothing {
                sample_bilinear(spix, *sw_, *sh_, u, v)
            } else {
                sample_nearest(spix, *sw_, *sh_, u, v)
            };
            // 几何覆盖率 = clip 折算(源 alpha 在 color[3] 内,blend_px 会相乘)。
            let mut geo = 1.0f64;
            if let Some(cp) = &clip {
                let cc = coverage_over_aabb(cp, px, py, 1, 1, clip_evenodd);
                if cc.is_empty() {
                    continue;
                }
                geo *= cc[0] as f64 / 255.0;
            }
            if geo <= 0.0 || color[3] <= 0.0 {
                continue;
            }
            let i = (py as usize * st.w + px as usize) * 4;
            blend_px(&mut st.pixels, i, color, geo, alpha, comp);
        }
    }
    st.touch();
}

// ---------- PNG 编码 ----------

pub(super) fn encode_png_url(w: usize, h: usize, rgba: &[u8]) -> Result<String, String> {
    use base64::Engine as _;
    use ::image::ImageEncoder as _;
    let expected = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= 64 * 1024 * 1024)
        .ok_or_else(|| "画布尺寸超过 16M 像素上限".to_string())?;
    if rgba.len() != expected {
        return Err(format!("RGBA 长度不匹配:期望 {expected},实际 {}", rgba.len()));
    }
    let mut png = Vec::new();
    ::image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, w as u32, h as u32, ::image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG 编码失败:{e}"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}


pub(super) fn images_lookup(id: u32) -> Option<(Vec<u8>, usize, usize)> {
    IMAGES.with(|m| {
        m.borrow()
            .get(&id)
            .map(|d| (d.rgba.clone(), d.w, d.h))
    })
}

pub(super) fn store_decoded_image(bytes: &[u8]) -> Option<(u32, usize, usize)> {
    let img = ::image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    if w == 0 || h == 0 || w > MAX_SIDE * 4 || h > MAX_SIDE * 4 {
        return None;
    }
    let id = IMAGES.with(|m| {
        let mut imgs = m.borrow_mut();
        if imgs.len() >= MAX_DECODED_IMAGES {
            // 丢最旧(u32 id 单调递增,最小者最旧)。
            if let Some(oldest) = imgs.keys().copied().min() {
                imgs.remove(&oldest);
            }
        }
        let next = imgs.keys().copied().max().unwrap_or(0) + 1;
        imgs.insert(next, DecodedImage { w, h, rgba: rgba.into_raw() });
        next
    });
    Some((id, w, h))
}
