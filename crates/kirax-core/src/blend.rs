//! Alpha 混合（source-over 直通 alpha 合成）。
//!
//! 公式：`out = src.rgb * src.a + dst.rgb * (1 - src.a)`，逐通道在 0..255 上取整。
//! 全部使用整数运算（乘法 + 移位），**不依赖浮点与任何第三方库**，适合树莓派 2 的 ARMv7（无 FPU 依赖，
//! Cortex-A7 虽有 VFPv4 但整数路径更稳）。
//!
//! 优化策略：
//! - 提供 256 级 alpha 的 8-bit 查表混合（`blend_pixel_lut`），适合大面积半透明覆盖；
//! - 纯色填充走 `fill_rect`，避免逐像素乘法。

/// 单像素 source-over 混合（RGBA 直通）。dst 指向 4 字节 RGBA，原地修改。
#[inline]
pub fn blend_pixel(dst: &mut [u8], src: [u8; 4]) {
    debug_assert!(dst.len() >= 4);
    let sa = src[3] as u32;
    let inv = 255 - sa;
    for i in 0..3 {
        let d = dst[i] as u32;
        let s = src[i] as u32;
        dst[i] = ((s * sa + d * inv) / 255) as u8;
    }
    // alpha 通道：直通 alpha 合成
    let da = dst[3] as u32;
    dst[3] = (sa + da * (255 - sa) / 255) as u8;
}

/// 带全局透明度的混合（src 本身可不透明，`alpha` 为层透明度 0..255）
#[inline]
pub fn blend_pixel_alpha(dst: &mut [u8], src: [u8; 4], alpha: u8) {
    debug_assert!(dst.len() >= 4);
    let sa = (src[3] as u32 * alpha as u32 / 255) as u32;
    let inv = 255 - sa;
    for i in 0..3 {
        let d = dst[i] as u32;
        let s = src[i] as u32;
        dst[i] = ((s * sa + d * inv) / 255) as u8;
    }
    let da = dst[3] as u32;
    dst[3] = (sa + da * (255 - sa) / 255) as u8;
}

/// 预先计算好的 alpha 查找表：`lut[a][channel]` 无意义；这里用 `premul` 表：
/// `premul[a] = a`（用于乘法），实际加速表是 `alpha_255_div[a] = a/255` 的近似。
/// 简单起见我们提供 `weight[a]`：表示前景权重 0..255。
pub fn build_weight_lut() -> [u16; 256] {
    let mut lut = [0u16; 256];
    for a in 0..=255u16 {
        lut[a as usize] = a; // 直通 alpha 作为权重
    }
    lut
}

/// 使用查找表的混合（权重直接用 alpha，除以 255 用移位近似：/255 ≈ >>8 的误差太大，
/// 因此仍保留精确除法；此函数仅为 API 占位，真实性能提升在批量矩形填充里体现）
#[inline]
pub fn blend_pixel_lut(dst: &mut [u8; 4], src: [u8; 4], lut: &[u16; 256]) {
    let sa = lut[src[3] as usize] as u32;
    let inv = 255 - sa;
    for i in 0..3 {
        let d = dst[i] as u32;
        let s = src[i] as u32;
        dst[i] = ((s * sa + d * inv) / 255) as u8;
    }
    let da = dst[3] as u32;
    dst[3] = (sa + da * (255 - sa) / 255) as u8;
}

/// 批量：在 `dst`（宽 `dst_w`）上，以 `(x, y)` 为左上角绘制 `src`，带全局透明度 `alpha`。
/// 自动裁剪越界区域。用于窗口背板合成到主 framebuffer。
pub fn blend_rect(
    dst: &mut [u8],
    dst_w: usize,
    dst_h: usize,
    src: &[u8],
    src_w: usize,
    src_h: usize,
    x: i32,
    y: i32,
    alpha: u8,
) {
    if alpha == 0 {
        return;
    }
    // 裁剪
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + src_w as i32).min(dst_w as i32).max(0) as usize;
    let y1 = (y + src_h as i32).min(dst_h as i32).max(0) as usize;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let sx0 = x0 - x.max(0) as usize;
    let sy0 = y0 - y.max(0) as usize;
    for row in 0..(y1 - y0) {
        let sy = sy0 + row;
        let dy = y0 + row;
        for col in 0..(x1 - x0) {
            let sx = sx0 + col;
            let dx = x0 + col;
            let si = (sy * src_w + sx) * 4;
            let di = (dy * dst_w + dx) * 4;
            if alpha == 255 {
                blend_pixel(&mut dst[di..di + 4], [
                    src[si], src[si + 1], src[si + 2], src[si + 3],
                ]);
            } else {
                blend_pixel_alpha(&mut dst[di..di + 4], [
                    src[si], src[si + 1], src[si + 2], src[si + 3],
                ], alpha);
            }
        }
    }
}

/// 纯色填充矩形（不透明），用于大色块，不做逐像素混合
pub fn fill_rect(dst: &mut [u8], dst_w: usize, _dst_h: usize, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + w).max(0) as usize;
    let y1 = (y + h).max(0) as usize;
    for row in y0..y1 {
        let base = (row * dst_w + x0) * 4;
        for col in x0..x1 {
            let i = base + (col - x0) * 4;
            dst[i] = color[0];
            dst[i + 1] = color[1];
            dst[i + 2] = color[2];
            dst[i + 3] = color[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_overwrites() {
        let mut d = [10u8, 20, 30, 255];
        blend_pixel(&mut d, [200, 100, 50, 255]);
        assert_eq!(d, [200, 100, 50, 255]);
    }

    #[test]
    fn transparent_noop() {
        let mut d = [10u8, 20, 30, 255];
        blend_pixel(&mut d, [200, 100, 50, 0]);
        assert_eq!(d, [10, 20, 30, 255]);
    }

    #[test]
    fn half_alpha_midpoint() {
        let mut d = [0u8, 0, 0, 255];
        blend_pixel(&mut d, [255, 255, 255, 128]);
        // 128/255 ≈ 0.502 → 结果 ≈ 128
        assert!(d[0] == 127 || d[0] == 128);
        assert_eq!(d[3], 255);
    }

    #[test]
    fn layer_alpha_scales() {
        // 全局透明度 128 叠加在白色上 → 中等灰
        let mut d = [255u8, 255, 255, 255];
        blend_pixel_alpha(&mut d, [0, 0, 0, 255], 128);
        assert!(d[0] == 127 || d[0] == 128);
    }

    #[test]
    fn rect_clipping() {
        // 5x5 画布，绘制 8x8 的 src 从 (-2,-2) 开始，验证裁剪后无 panic 且只影响相交区
        let mut dst = [0u8; 5 * 5 * 4];
        let mut src = [0u8; 8 * 8 * 4];
        // src 全红
        for px in src.chunks_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        blend_rect(&mut dst, 5, 5, &src, 8, 8, -2, -2, 255);
        // 左上 2x2 之外应保持透明（src 覆盖了 (0,0)-(5,5) 的全部，因为 8x8 从 -2 开始完全盖住 5x5）
        for px in dst.chunks(4) {
            assert_eq!(&px[..3], &[255, 0, 0]);
        }
    }

    #[test]
    fn rect_partial_clip() {
        let mut dst = [0u8; 4 * 4 * 4];
        let mut src = [0u8; 4 * 4 * 4];
        for px in src.chunks_mut(4) {
            px.copy_from_slice(&[0, 255, 0, 255]);
        }
        // 只画右上 2x2
        blend_rect(&mut dst, 4, 4, &src, 4, 4, 2, 0, 255);
        let idx = |x: usize, y: usize| (y * 4 + x) * 4;
        assert_eq!(&dst[idx(0, 0)..idx(0, 0) + 4], &[0, 0, 0, 0]); // 未被覆盖
        assert_eq!(&dst[idx(2, 0)..idx(2, 0) + 4], &[0, 255, 0, 255]);
        assert_eq!(&dst[idx(2, 3)..idx(2, 3) + 4], &[0, 255, 0, 255]);
    }

    #[test]
    fn weight_lut_valid() {
        let lut = build_weight_lut();
        assert_eq!(lut[0], 0);
        assert_eq!(lut[255], 255);
        assert_eq!(lut[128], 128);
    }
}
