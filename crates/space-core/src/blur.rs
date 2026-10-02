//! 高斯模糊 —— 三次盒式模糊近似（three-box approximation）。
//!
//! 原理（见 Phase 4 文档）：
//! 连续 3 次宽度为 w 的盒式滤波近似一次 σ 的高斯滤波，其中
//! `w = sqrt(12·σ²/3 + 1)`（取奇整数）。
//! 盒式滤波用“滑动窗口和”实现，每个像素 O(1)，三次合计 O(n)，远快于真高斯 O(n·r)。
//!
//! 针对树莓派 2 的轻量化策略（集成在此模块）：
//! 1. **降采样**：先在 1/`downsample` 分辨率上做模糊（面积缩小 4 倍），再双线性放大；
//! 2. **只模糊壁纸层**：合成器只对“壁纸/窗口底下的内容”做一次预计算模糊并缓存；
//! 3. 面板模糊 = 从预模糊缓存里取一块，避免实时卷积。

use crate::{align_up, fmath};

/// 由 σ 计算 3 次盒式滤波的盒宽（奇整数）
/// 3 次盒式近似公式 w = sqrt(12·σ²/3 + 1) ≈ 2σ；直接取 2σ+1（奇）
pub fn box_width_for_sigma(sigma: f32, _passes: u32) -> u32 {
    let w = fmath::round(2.0 * sigma).max(1.0) as u32;
    if w % 2 == 0 {
        w + 1
    } else {
        w
    }
}

/// 水平盒式模糊（滑动窗口，RGBA，逐通道 u8）
/// `radius` = (盒宽-1)/2
pub fn box_blur_h(src: &[u8], w: usize, h: usize, radius: u32, dst: &mut [u8]) {
    debug_assert!(src.len() >= w * h * 4 && dst.len() >= w * h * 4);
    let r = radius as i64;
    let win = 2 * r + 1;
    for y in 0..h {
        // 初始化窗口：[-r, r]
        let mut acc = [0i64; 4];
        for k in -r..=r {
            let x = clamp_idx(k, w as i64) as usize;
            let i = (y * w + x) * 4;
            for c in 0..4 {
                acc[c] += src[i + c] as i64;
            }
        }
        for x in 0..w {
            let o = (y * w + x) * 4;
            for c in 0..4 {
                dst[o + c] = (acc[c] / win) as u8;
            }
            // 滑动：移出 x-r，移入 x+r+1
            let out_x = clamp_idx(x as i64 - r, w as i64) as usize;
            let in_x = clamp_idx(x as i64 + r + 1, w as i64) as usize;
            for c in 0..4 {
                acc[c] -= src[(y * w + out_x) * 4 + c] as i64;
                acc[c] += src[(y * w + in_x) * 4 + c] as i64;
            }
        }
    }
}

/// 垂直盒式模糊（同样滑动窗口）
pub fn box_blur_v(src: &[u8], w: usize, h: usize, radius: u32, dst: &mut [u8]) {
    let r = radius as i64;
    let win = 2 * r + 1;
    for x in 0..w {
        let mut acc = [0i64; 4];
        for k in -r..=r {
            let y = clamp_idx(k, h as i64) as usize;
            let i = (y * w + x) * 4;
            for c in 0..4 {
                acc[c] += src[i + c] as i64;
            }
        }
        for y in 0..h {
            let o = (y * w + x) * 4;
            for c in 0..4 {
                dst[o + c] = (acc[c] / win) as u8;
            }
            let out_y = clamp_idx(y as i64 - r, h as i64) as usize;
            let in_y = clamp_idx(y as i64 + r + 1, h as i64) as usize;
            for c in 0..4 {
                acc[c] -= src[(out_y * w + x) * 4 + c] as i64;
                acc[c] += src[(in_y * w + x) * 4 + c] as i64;
            }
        }
    }
}

#[inline]
fn clamp_idx(v: i64, len: i64) -> i64 {
    // 边界钳制（反射边缘近似：负值取 0，超出取 len-1）
    v.clamp(0, len - 1)
}

/// 三次盒式近似高斯模糊（RGBA）
/// - `sigma`：高斯标准差（像素）
/// - 输出与输入同尺寸
pub fn gaussian_blur_3box(src: &[u8], w: usize, h: usize, sigma: f32, dst: &mut [u8]) {
    let passes = 3;
    if sigma <= 0.5 {
        dst.copy_from_slice(src);
        return;
    }
    let bw = box_width_for_sigma(sigma, passes);
    let radius = (bw - 1) / 2;
    // 两次中转缓冲（一次水平 + 一次垂直为一轮；3 轮）
    let mut tmp1 = vec![0u8; w * h * 4];
    let mut tmp2 = vec![0u8; w * h * 4];
    let mut cur = src;
    for pass in 0..passes {
        // 每轮盒宽略不同更接近高斯，这里简化为同宽
        let _ = pass;
        box_blur_h(cur, w, h, radius, &mut tmp1);
        box_blur_v(&tmp1, w, h, radius, &mut tmp2);
        cur = &tmp2;
        if pass < passes - 1 {
            // 交换继续
            tmp1.copy_from_slice(&tmp2);
        }
    }
    dst.copy_from_slice(cur);
}

/// 降采样（最近邻 + 简单平均 2x2 → 面积缩小 4 倍）
/// 输入 w×h → 输出 (w+1)/ds × (h+1)/ds
pub fn downsample(src: &[u8], w: usize, h: usize, ds: usize, dst: &mut [u8]) -> (usize, usize) {
    let nw = (w + ds - 1) / ds;
    let nh = (h + ds - 1) / ds;
    let ds = ds as u32;
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            let mut cnt = 0u32;
            for dy in 0..ds {
                for dx in 0..ds {
                    let sx = x * ds as usize + dx as usize;
                    let sy = y * ds as usize + dy as usize;
                    if sx < w && sy < h {
                        let i = (sy * w + sx) * 4;
                        for c in 0..4 {
                            acc[c] += src[i + c] as u32;
                        }
                        cnt += 1;
                    }
                }
            }
            if cnt == 0 {
                cnt = 1;
            }
            let o = (y * nw + x) * 4;
            for c in 0..4 {
                dst[o + c] = (acc[c] / cnt) as u8;
            }
        }
    }
    (nw, nh)
}

/// 双线性升采样（低分辨率模糊图 → 全尺寸）
pub fn upsample_bilinear(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, dst: &mut [u8]) {
    for y in 0..dh {
        // 源坐标（浮点）
        let fy = (y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5;
        let y0 = fmath::floor(fy).max(0.0) as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for x in 0..dw {
            let fx = (x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
            let x0 = fmath::floor(fx).max(0.0) as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            for c in 0..4 {
                let p00 = src[(y0 * sw + x0) * 4 + c] as f32;
                let p01 = src[(y0 * sw + x1) * 4 + c] as f32;
                let p10 = src[(y1 * sw + x0) * 4 + c] as f32;
                let p11 = src[(y1 * sw + x1) * 4 + c] as f32;
                let top = p00 * (1.0 - tx) + p01 * tx;
                let bot = p10 * (1.0 - tx) + p11 * tx;
                dst[(y * dw + x) * 4 + c] = (top * (1.0 - ty) + bot * ty) as u8;
            }
        }
    }
}

/// 壁纸专用“预计算模糊”：降采样 → 三次盒式模糊 → 升采样。
/// 结果缓存后，玻璃面板只需从中拷贝，不再实时卷积。
/// 返回处理后的新尺寸（供测试验证）
pub fn precompute_wallpaper_blur(
    src: &[u8],
    w: usize,
    h: usize,
    sigma: f32,
    ds: usize,
    dst: &mut [u8],
) -> (usize, usize) {
    let small_w = align_up(w, ds) / ds;
    let small_h = align_up(h, ds) / ds;
    let mut small = vec![0u8; small_w * small_h * 4];
    let (nw, nh) = downsample(src, w, h, ds, &mut small);
    let mut blurred_small = vec![0u8; nw * nh * 4];
    gaussian_blur_3box(&small[..nw * nh * 4], nw, nh, sigma / ds as f32, &mut blurred_small);
    upsample_bilinear(&blurred_small, nw, nh, w, h, dst);
    (nw, nh)
}

// 测试环境允许分配 Vec
#[cfg(test)]
extern crate alloc;
#[cfg(test)]
use alloc::vec::Vec;

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_image(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for px in v.chunks_mut(4) {
            px.copy_from_slice(&color);
        }
        v
    }

    #[test]
    fn box_width_odd_and_positive() {
        for sigma in [0.5, 1.0, 2.0, 4.0, 8.0, 16.0] {
            let w = box_width_for_sigma(sigma, 3);
            assert!(w >= 1);
            assert_eq!(w % 2, 1, "盒宽必须是奇数");
        }
    }

    #[test]
    fn blur_preserves_flat_color() {
        // 纯色图模糊后必须仍是纯色（归一化正确）
        let w = 64;
        let h = 48;
        let src = solid_image(w, h, [200, 100, 50, 255]);
        let mut dst = vec![0u8; w * h * 4];
        gaussian_blur_3box(&src, w, h, 4.0, &mut dst);
        for px in dst.chunks(4) {
            assert_eq!(px, &[200, 100, 50, 255]);
        }
    }

    #[test]
    fn blur_impulse_is_symmetric() {
        // 单位冲激响应必须对称（高斯性质）
        let w = 32;
        let h = 1;
        let mut src = vec![0u8; w * h * 4];
        // 中心一个白点
        src[(16 * 4)..(16 * 4 + 4)].copy_from_slice(&[255, 255, 255, 255]);
        let mut dst = vec![0u8; w * h * 4];
        gaussian_blur_3box(&src, w, h, 3.0, &mut dst);
        for dx in 0..15 {
            let left = dst[(16 - dx) * 4];
            let right = dst[(16 + dx) * 4];
            assert!((left as i16 - right as i16).abs() <= 1, "不对称 at dx={dx}");
        }
        // 单调衰减
        for dx in 0..14 {
            let a = dst[(16 - dx) * 4] as u16;
            let b = dst[(16 - dx - 1) * 4] as u16;
            assert!(b <= a, "应单调衰减");
        }
    }

    #[test]
    fn blur_edge_is_monotonic() {
        // 阶跃边缘模糊后单调过渡
        let w = 64;
        let h = 1;
        let mut src = vec![0u8; w * h * 4];
        for x in 0..32 {
            src[x * 4] = 0;
        }
        for x in 32..64 {
            src[x * 4] = 255;
        }
        // 只填 R 通道测试
        let mut dst = vec![0u8; w * h * 4];
        gaussian_blur_3box(&src, w, h, 4.0, &mut dst);
        let mut prev = 0u8;
        let mut saw_rise = false;
        for x in 20..50 {
            let v = dst[x * 4];
            assert!(v >= prev, "边缘过渡应单调");
            if v > prev {
                saw_rise = true;
            }
            prev = v;
        }
        assert!(saw_rise);
    }

    #[test]
    fn tiny_sigma_is_noop() {
        let w = 16;
        let h = 16;
        let mut src = solid_image(w, h, [10, 20, 30, 255]);
        src[0] = 200; // 一点扰动
        let mut dst = vec![0u8; w * h * 4];
        gaussian_blur_3box(&src, w, h, 0.1, &mut dst);
        assert_eq!(dst, src);
    }

    #[test]
    fn downsample_size() {
        let w = 100;
        let h = 80;
        let src = solid_image(w, h, [1, 2, 3, 255]);
        let mut dst = vec![0u8; 50 * 40 * 4];
        let (nw, nh) = downsample(&src, w, h, 2, &mut dst);
        assert_eq!((nw, nh), (50, 40));
    }

    #[test]
    fn wallpaper_pipeline() {
        // 整条预计算管线：尺寸正确 + 结果可写回
        let w = 128;
        let h = 96;
        let src = solid_image(w, h, [30, 40, 60, 255]);
        let mut dst = vec![0u8; w * h * 4];
        let (nw, nh) = precompute_wallpaper_blur(&src, w, h, 4.0, 2, &mut dst);
        assert_eq!((nw, nh), (64, 48));
        // 纯色壁纸模糊后保持纯色
        for px in dst.chunks(4) {
            assert_eq!(px, &[30, 40, 60, 255]);
        }
    }
}
