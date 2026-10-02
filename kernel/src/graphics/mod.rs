//! 图形子系统：画布（Surface）与基础绘制原语。
//!
//! 设计：
//! - [`Surface`] 是统一的 RGBA8888 画布（壁纸、窗口背板、framebuffer 双缓冲都用它）；
//! - 全部绘制为纯 CPU 整数/定点运算（Cortex-A7 无硬件 2D 加速）；
//! - 高级效果（高斯模糊/混合/圆角）由 space-core 的纯逻辑模块提供；
//! - 图标一律为矢量几何（行星/轨道/火箭），禁止 emoji。
//!
//! 帧缓冲颜色顺序：内存中为 B,G,R,A（VideoCore 原生 BGRA32）。

use crate::font::{self, FONT8X8};
use alloc::vec;
use alloc::vec::Vec;
use space_core::glass::Color;
use space_core::roundrect::Rect;

/// 画布
pub struct Surface {
    pub w: u32,
    pub h: u32,
    /// 像素数据（BGRA32）
    pub data: Vec<u8>,
}

impl Surface {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            data: vec![0u8; (w * h * 4) as usize],
        }
    }

    pub fn from_buffer(w: u32, h: u32, data: Vec<u8>) -> Self {
        Self { w, h, data }
    }

    /// 清屏（透明黑）
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// 用纯色填充整幅
    pub fn clear_color(&mut self, c: Color) {
        for px in self.data.chunks_mut(4) {
            px.copy_from_slice(&[c.0, c.1, c.2, c.3]);
        }
    }

    /// 画像素（直写，不做混合；透明色用 blend_px）
    #[inline]
    pub fn put_pixel(&mut self, x: i32, y: i32, c: Color) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        self.data[i] = c.0;
        self.data[i + 1] = c.1;
        self.data[i + 2] = c.2;
        self.data[i + 3] = c.3;
    }

    /// 画像素（带 alpha 混合）
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, c: Color) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let px = &mut self.data[i..i + 4];
        let sa = c.3 as u32;
        if sa == 0 {
            return;
        }
        let inv = 255 - sa;
        px[0] = ((c.0 as u32 * sa + px[0] as u32 * inv) / 255) as u8;
        px[1] = ((c.1 as u32 * sa + px[1] as u32 * inv) / 255) as u8;
        px[2] = ((c.2 as u32 * sa + px[2] as u32 * inv) / 255) as u8;
        let da = px[3] as u32;
        px[3] = (sa + da * inv / 255) as u8;
    }

    /// 填充矩形
    pub fn fill_rect(&mut self, r: &Rect, c: Color) {
        let clip = match r.intersect(&Rect::new(0, 0, self.w as i32, self.h as i32)) {
            Some(c) => c,
            None => return,
        };
        if c.3 == 255 {
            for y in clip.y..clip.y + clip.h {
                let base = (y as u32 * self.w + clip.x as u32) * 4;
                for x in 0..clip.w {
                    let i = (base + x as u32 * 4) as usize;
                    self.data[i] = c.0;
                    self.data[i + 1] = c.1;
                    self.data[i + 2] = c.2;
                    self.data[i + 3] = 255;
                }
            }
        } else {
            for y in clip.y..clip.y + clip.h {
                for x in clip.x..clip.x + clip.w {
                    self.blend_pixel(x, y, c);
                }
            }
        }
    }

    /// 填充圆角矩形（带抗锯齿，走 space-core SDF）
    pub fn fill_round_rect(&mut self, r: &Rect, radius: f32, c: Color) {
        space_core::roundrect::fill_round_rect(
            &mut self.data,
            self.w as usize,
            self.h as usize,
            r,
            radius,
            [c.0, c.1, c.2, c.3],
        );
    }

    /// 画矩形描边（玻璃“上亮下暗”高光描边可分段调用）
    pub fn stroke_rect(&mut self, r: &Rect, width: i32, c: Color) {
        // 四条边
        self.fill_rect(&Rect::new(r.x, r.y, r.w, width), c);
        self.fill_rect(&Rect::new(r.x, r.y + r.h - width, r.w, width), c);
        self.fill_rect(&Rect::new(r.x, r.y, width, r.h), c);
        self.fill_rect(&Rect::new(r.x + r.w - width, r.y, width, r.h), c);
    }

    /// 画水平渐变（从上到下，两段插值：top→mid→bottom）
    pub fn fill_vgradient(&mut self, top: Color, mid: Color, bottom: Color) {
        for y in 0..self.h {
            let t = if y < self.h / 2 {
                (y * 255 / (self.h / 2)) as u8
            } else {
                255
            };
            let color = if y < self.h / 2 {
                top.mix(mid, t)
            } else {
                let t2 = ((y - self.h / 2) * 255 / (self.h - self.h / 2)) as u8;
                mid.mix(bottom, t2)
            };
            let base = (y * self.w * 4) as usize;
            for x in 0..self.w as usize {
                let i = base + x * 4;
                self.data[i] = color.0;
                self.data[i + 1] = color.1;
                self.data[i + 2] = color.2;
                self.data[i + 3] = 255;
            }
        }
    }

    /// 画文本（8×8 位图字体）
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color, scale: u32) {
        let mut cx = x;
        for &b in text.as_bytes() {
            match b {
                b'\n' => {
                    cx = x;
                    // 8×scale 行高
                    // （行高由调用方处理，这里简单跳过）
                    continue;
                }
                0x20..=0x7E => {
                    let g = &FONT8X8[(b - 0x20) as usize];
                    self.draw_glyph(cx, y, g, color, scale);
                    cx += (font::GLYPH_W * scale) as i32 + 1;
                }
                _ => {
                    // 不支持的字形画一个方块
                    let g = &[0xFFu8; 8];
                    self.draw_glyph(cx, y, g, color, scale);
                    cx += (font::GLYPH_W * scale) as i32 + 1;
                }
            }
        }
    }

    /// 绘制 8×8 字形（可缩放）
    pub fn draw_glyph(&mut self, x: i32, y: i32, glyph: &[u8; 8], color: Color, scale: u32) {
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << (7 - col)) != 0 {
                    if scale <= 1 {
                        self.blend_pixel(x + col as i32, y + row as i32, color);
                    } else {
                        for dy in 0..scale as i32 {
                            for dx in 0..scale as i32 {
                                self.blend_pixel(
                                    x + col as i32 * scale as i32 + dx,
                                    y + row as i32 * scale as i32 + dy,
                                    color,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// 画直线（Bresenham，1px）
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        let mut x = x0;
        let mut y = y0;
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.blend_pixel(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// 画圆（中点圆算法，填充可选）
    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, filled: bool, c: Color) {
        if filled {
            for y in (cy - r)..=(cy + r) {
                let dy = y - cy;
                let dx = space_core::fmath::isqrt((r * r - dy * dy) as u32) as i32;
                self.fill_rect(&Rect::new(cx - dx, y, 2 * dx + 1, 1), c);
            }
        } else {
            let mut x = r;
            let mut y = 0;
            let mut err = 1 - r;
            while x >= y {
                self.blend_pixel(cx + x, cy + y, c);
                self.blend_pixel(cx + y, cy + x, c);
                self.blend_pixel(cx - y, cy + x, c);
                self.blend_pixel(cx - x, cy + y, c);
                self.blend_pixel(cx - x, cy - y, c);
                self.blend_pixel(cx - y, cy - x, c);
                self.blend_pixel(cx + y, cy - x, c);
                self.blend_pixel(cx + x, cy - y, c);
                y += 1;
                if err < 0 {
                    err += 2 * y + 1;
                } else {
                    x -= 1;
                    err += 2 * (y - x) + 1;
                }
            }
        }
    }

    /// 把另一画布按源覆盖混合到本画布指定位置（带全局透明度）
    pub fn blit(&mut self, src: &Surface, x: i32, y: i32, alpha: u8) {
        space_core::blend::blend_rect(
            &mut self.data,
            self.w as usize,
            self.h as usize,
            &src.data,
            src.w as usize,
            src.h as usize,
            x,
            y,
            alpha,
        );
    }
}

/// 矢量图标绘制：行星（实心圆 + 轨道环）
pub fn draw_planet_icon(s: &mut Surface, cx: i32, cy: i32, r: i32, planet: Color, orbit: Color) {
    s.draw_circle(cx, cy, r, true, planet);
    // 轨道环（椭圆近似：水平线 + 上下弧线简化）
    s.draw_circle(cx, cy, r + 4, false, orbit);
}

/// 矢量图标绘制：火箭（几何三角形组合）
pub fn draw_rocket_icon(s: &mut Surface, cx: i32, cy: i32, size: i32, color: Color) {
    // 机身（三角）
    for dy in 0..size {
        let half = dy * (size / 2) / size;
        s.fill_rect(&Rect::new(cx - half, cy - size / 2 + dy, half * 2 + 1, 1), color);
    }
    // 尾焰（橙）
    let flame = Color::rgb(255, 159, 69);
    s.fill_rect(&Rect::new(cx - size / 6, cy + size / 2, size / 3, size / 3), flame);
    // 舷窗
    s.draw_circle(cx, cy - size / 6, size / 6, true, Color::rgb(79, 216, 235));
}
