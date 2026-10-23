//! 三趟盒式模糊(近似高斯):`box-shadow` blur 半径真实生效的核心。
//!
//! CSS 语义:`box-shadow: <blur>` 的高斯 σ = blur/2。三趟等宽盒式模糊的
//! 方差之和 = σ²(经典近似,Kovesi/ Kutskir 算法族)。滑动窗口求和,单趟
//! O(n);只在蒙版的活跃包围盒上运行,半径与面积双上限防失控(见 22 号文档)。

/// 单趟盒式模糊的最大半宽(px)。CSS blur = 96px 时 σ=48 → 半宽 48 封顶,
/// 更大的模糊按此截断(视觉上阴影更"收",不会崩)。
pub(crate) const MAX_BLUR_HALF: usize = 48;

/// 蒙版像素总量上限(4M px):超限调用方应退化为实心矩形近似。
pub(crate) const MAX_MASK_PIXELS: usize = 4_000_000;

/// σ → 三趟盒式模糊的单趟半宽(等方差分解)。
///
/// 单趟盒宽 W=2hw+1 的方差为 `(W²-1)/12 = hw(hw+1)/3`;水平+垂直各三趟
/// 共 6 趟,令总方差 = σ²:`6·hw(hw+1)/3 = σ²` → `hw = (√(1+2σ²)-1)/2`。
pub(crate) fn half_width_for(sigma: f64) -> usize {
    let s = sigma.max(0.5);
    let hw = ((2.0 * s * s + 1.0).sqrt() - 1.0) / 2.0;
    let w = hw.round().max(1.0) as usize;
    w.min(MAX_BLUR_HALF)
}

/// 就地三趟盒式模糊一张单通道(0..=255)蒙版。`w/h` 为蒙版尺寸。
///
/// 边界处理:clamp(重复边缘),与阴影贴边语义一致。
pub(crate) fn blur_mask(mask: &mut [u8], w: usize, h: usize, sigma: f64) {
    if mask.len() != w * h || w == 0 || h == 0 {
        return;
    }
    let hw = half_width_for(sigma);
    if hw == 0 {
        return;
    }
    // 三趟水平 + 垂直交替;单块暂存区(w*h ≥ max(w,h))避免整张蒙版复制。
    let mut tmp = vec![0u8; mask.len()];
    for _ in 0..3 {
        blur_pass_h(mask, &mut tmp, w, h, hw);
        blur_pass_v(mask, &mut tmp, w, h, hw);
    }
}

/// 水平一趟:滑动窗口均值,边界 clamp。结果写回 `mask`,`tmp` 为暂存区。
fn blur_pass_h(mask: &mut [u8], tmp: &mut [u8], w: usize, h: usize, hw: usize) {
    let win = (hw * 2 + 1) as u32;
    let wmax = w as isize - 1;
    for row in 0..h {
        let base = row * w;
        // x=0 的窗口:j∈[-hw, hw] 逐项 clamp 到 [0, w-1](越界侧重复边缘)。
        let mut sum: u32 = 0;
        for j in -(hw as isize)..=(hw as isize) {
            sum += mask[base + j.clamp(0, wmax) as usize] as u32;
        }
        for x in 0..w {
            tmp[base + x] = (sum / win) as u8;
            // 右缘进入/左缘离开(clamp 后即重复边缘的份数增减)。
            let add = (x as isize + hw as isize + 1).clamp(0, wmax) as usize;
            let sub = (x as isize - hw as isize).clamp(0, wmax) as usize;
            sum = sum
                .saturating_add(mask[base + add] as u32)
                .saturating_sub(mask[base + sub] as u32);
        }
        mask[base..base + w].copy_from_slice(&tmp[base..base + w]);
    }
}

/// 垂直一趟:同水平,按列滑动。
fn blur_pass_v(mask: &mut [u8], tmp: &mut [u8], w: usize, h: usize, hw: usize) {
    let win = (hw * 2 + 1) as u32;
    let hmax = h as isize - 1;
    for col in 0..w {
        let mut sum: u32 = 0;
        for j in -(hw as isize)..=(hw as isize) {
            sum += mask[j.clamp(0, hmax) as usize * w + col] as u32;
        }
        for y in 0..h {
            tmp[y] = (sum / win) as u8;
            let add = (y as isize + hw as isize + 1).clamp(0, hmax) as usize;
            let sub = (y as isize - hw as isize).clamp(0, hmax) as usize;
            sum = sum
                .saturating_add(mask[add * w + col] as u32)
                .saturating_sub(mask[sub * w + col] as u32);
        }
        for y in 0..h {
            mask[y * w + col] = tmp[y];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 中心单点亮源模糊后扩散且守恒量级() {
        // 16x16 蒙版中心一个 8x8 实心块,σ=4 模糊后边缘应出现半透明。
        let (w, h) = (16usize, 16usize);
        let mut mask = vec![0u8; w * h];
        for y in 4..12 {
            for x in 4..12 {
                mask[y * w + x] = 255;
            }
        }
        blur_mask(&mut mask, w, h, 4.0);
        // 中心仍接近不透明(等方差近似后不低于一半)。
        assert!(mask[8 * w + 8] > 120, "中心应仍浓:{}", mask[8 * w + 8]);
        // 边缘远处(2px 外)应为低值但非零(模糊扩散)。
        let far = mask[8 * w + 14];
        assert!(far > 0 && far < 120, "扩散区半透明:{far}");
        // 远角(对角距离 ~5.7px)应接近零(有限支撑 3·hw=6px 内的弱尾)。
        assert!(mask[0] < 100, "远角应接近零:{}", mask[0]);
    }

    #[test]
    fn 全亮蒙版模糊后不变() {
        let (w, h) = (8usize, 8usize);
        let mut mask = vec![255u8; w * h];
        blur_mask(&mut mask, w, h, 6.0);
        assert!(mask.iter().all(|&v| v >= 250), "全亮模糊后应保持全亮");
    }

    #[test]
    fn 半宽钳制与空蒙版安全() {
        let mut mask = vec![];
        blur_mask(&mut mask, 0, 0, 10.0); // 不 panic
        assert!(half_width_for(1000.0) <= MAX_BLUR_HALF);
        assert_eq!(half_width_for(0.3), 1);
    }
}
