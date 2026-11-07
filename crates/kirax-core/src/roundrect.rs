//! 圆角矩形：带 1px 抗锯齿的 SDF（有向距离场）覆盖率填充与描边。
//!
//! 方法：
//! 对像素中心计算“到圆角矩形的有向距离”：
//! `d = length(max(|p - c| - (half - r), 0)) - r`（r 为圆角半径）。
//! 覆盖率 = clamp(0.5 - d, 0, 1)：在边界 ±0.5px 内做线性抗锯齿。
//!
//! 纯整数/浮点均可，这里用 f32（Cortex-A7 有 VFPv4，单精度够快）。

use crate::fmath;

/// 矩形定义
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// 与另一矩形求交（空矩形返回 None）
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        if x1 <= x0 || y1 <= y0 {
            None
        } else {
            Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
        }
    }
}

/// 像素中心到圆角矩形的有向距离（负 = 内部）
pub fn sdf_round_rect(px: f32, py: f32, rect: &Rect, radius: f32) -> f32 {
    // 中心 = 左上角 + 半宽（(x+w)/2 在 x≠0 时是错的）
    let cx = rect.x as f32 + rect.w as f32 / 2.0;
    let cy = rect.y as f32 + rect.h as f32 / 2.0;
    let half_w = rect.w as f32 / 2.0;
    let half_h = rect.h as f32 / 2.0;
    let r = radius.min(half_w.min(half_h));
    let qx = (px - cx).abs() - (half_w - r);
    let qy = (py - cy).abs() - (half_h - r);
    let outside = fmath::sqrt(qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    outside + inside - r
}

/// 覆盖率（0..1）：0.5 处为边界
pub fn coverage(px: f32, py: f32, rect: &Rect, radius: f32) -> f32 {
    let d = sdf_round_rect(px, py, rect, radius);
    (0.5 - d).clamp(0.0, 1.0)
}

/// 填充圆角矩形到 RGBA 缓冲（带 1px 抗锯齿），混合模式 source-over
/// - `buf`：目标帧缓冲（整幅）
/// - `buf_w`：帧缓冲宽度
/// - `color`：[R,G,B,A]
pub fn fill_round_rect(
    buf: &mut [u8],
    buf_w: usize,
    buf_h: usize,
    rect: &Rect,
    radius: f32,
    color: [u8; 4],
) {
    let Some(clip) = rect.intersect(&Rect::new(0, 0, buf_w as i32, buf_h as i32)) else {
        return;
    };
    for y in clip.y..clip.y + clip.h {
        for x in clip.x..clip.x + clip.w {
            let cov = coverage(x as f32 + 0.5, y as f32 + 0.5, rect, radius);
            if cov <= 0.0 {
                continue;
            }
            let alpha = fmath::round(color[3] as f32 * cov) as u8;
            if alpha == 0 {
                continue;
            }
            let i = (y as usize * buf_w + x as usize) * 4;
            blend_px(&mut buf[i..i + 4], [color[0], color[1], color[2], alpha]);
        }
    }
}

/// 描边圆角矩形（用于“上亮下暗”玻璃高光描边）
/// - `stroke_w`：描边宽度（px）
/// - 支持分段颜色：top/left 亮，bottom/right 暗（传不同颜色即可实现，或调用方对四个边分别画）
pub fn stroke_round_rect(
    buf: &mut [u8],
    buf_w: usize,
    buf_h: usize,
    rect: &Rect,
    radius: f32,
    stroke_w: f32,
    color: [u8; 4],
) {
    // 方法：外框填充 - 内框挖空
    let Some(clip) = rect.intersect(&Rect::new(0, 0, buf_w as i32, buf_h as i32)) else {
        return;
    };
    for y in clip.y..clip.y + clip.h {
        for x in clip.x..clip.x + clip.w {
            let outer = coverage(x as f32 + 0.5, y as f32 + 0.5, rect, radius);
            let inner_rect = Rect::new(
                rect.x + stroke_w as i32,
                rect.y + stroke_w as i32,
                rect.w - 2 * stroke_w as i32,
                rect.h - 2 * stroke_w as i32,
            );
            let inner = if inner_rect.w > 0 && inner_rect.h > 0 {
                coverage(x as f32 + 0.5, y as f32 + 0.5, &inner_rect, (radius - stroke_w).max(0.0))
            } else {
                0.0
            };
            let cov = (outer - inner).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let alpha = fmath::round(color[3] as f32 * cov) as u8;
            if alpha == 0 {
                continue;
            }
            let i = (y as usize * buf_w + x as usize) * 4;
            blend_px(&mut buf[i..i + 4], [color[0], color[1], color[2], alpha]);
        }
    }
}

/// 单像素 source-over（与 blend 模块等价，本地小实现避免循环依赖）
#[inline]
fn blend_px(dst: &mut [u8], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 0 {
        return;
    }
    let inv = 255 - sa;
    for i in 0..3 {
        dst[i] = ((src[i] as u32 * sa + dst[i] as u32 * inv) / 255) as u8;
    }
    let da = dst[3] as u32;
    dst[3] = (sa + da * inv / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_full_coverage() {
        let r = Rect::new(10, 10, 100, 60);
        let c = coverage(60.0, 40.0, &r, 10.0);
        assert!((c - 1.0).abs() < 0.01, "中心覆盖率应为 1，got {c}");
    }

    #[test]
    fn outside_zero_coverage() {
        let r = Rect::new(10, 10, 100, 60);
        let c = coverage(5.0, 5.0, &r, 10.0);
        assert_eq!(c, 0.0);
    }

    #[test]
    fn corner_is_rounded() {
        // 圆角半径 10：角落点（距角 45° 方向 12px）应在外部
        let r = Rect::new(0, 0, 40, 40);
        let c = coverage(2.0, 2.0, &r, 10.0);
        // (2,2) 到圆角圆心 (10,10) 距离 sqrt(128)≈11.3 > 10 → 外部
        assert!(c < 0.5);
        // (8,8) 在圆角内部
        let c2 = coverage(8.0, 8.0, &r, 10.0);
        assert!(c2 > 0.5);
    }

    #[test]
    fn fill_clips_correctly() {
        let mut buf = vec![0u8; 16 * 16 * 4];
        let r = Rect::new(-5, -5, 20, 20); // 部分在画布外
        fill_round_rect(&mut buf, 16, 16, &r, 4.0, [255, 0, 0, 255]);
        // 画布左上角 (0,0) 在矩形内
        assert_eq!(buf[0], 255);
        // 画布右下 (15,15) 在矩形外（矩形 x∈[-5,15) y∈[-5,15)）
        assert_eq!(buf[(15 * 16 + 15) * 4], 0);
    }

    #[test]
    fn stroke_creates_ring() {
        let mut buf = vec![0u8; 64 * 64 * 4];
        let r = Rect::new(8, 8, 48, 48);
        stroke_round_rect(&mut buf, 64, 64, &r, 8.0, 2.0, [255, 255, 255, 255]);
        // 描边环在左上圆角弧上（圆心(16,16)，外半径8，内半径6；取 45° 方向像素 (11,11)）
        assert!(buf[(11 * 64 + 11) * 4] > 0);
        // 内部中心无像素（被挖空）
        assert_eq!(buf[(32 * 64 + 32) * 4], 0);
    }

    #[test]
    fn intersect_math() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        let c = a.intersect(&b).unwrap();
        assert_eq!(c, Rect::new(5, 5, 5, 5));
        let d = Rect::new(20, 20, 2, 2);
        assert!(a.intersect(&d).is_none());
    }
}
