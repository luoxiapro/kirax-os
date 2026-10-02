//! 桌面环境（Glass 设计语言的落地）：壁纸、合成器、任务栏、开始菜单、通知。
//!
//! 渲染管线（每帧）：
//! ```text
//! 壁纸(渐变+星光+行星) ──预计算──▶ 模糊缓存(降采样+3-box)
//! 窗口列表 ──z序──▶ 合成器(玻璃取样模糊缓存 / 纯色降级)
//!                   ──▶ 任务栏(悬浮液态玻璃) + 开始菜单 + 通知卡片
//!                   ──▶ 脏矩形刷新到 framebuffer
//! ```
//! 性能预算（Glass 规范）：同时可见玻璃面板 ≤ 4；模糊半径 8px（Lite 4px，性能 0）；
//! 壁纸模糊只做一次并缓存；窗口拖动/缩放只重绘脏矩形。

use crate::framebuffer;
use crate::klog;
use crate::graphics::{draw_planet_icon, draw_rocket_icon, Surface};
use crate::timer;
use crate::window::{self, WindowManager};
use alloc::vec::Vec;
use space_core::events::{EventKind, InputEvent, RingBuffer};
use space_core::glass::{self, Color, GlassMode};
use space_core::roundrect::Rect;

/// 桌面状态
pub struct Desktop {
    /// 壁纸（含行星与星光）
    wallpaper: Surface,
    /// 壁纸模糊缓存（预计算一次）
    blur_cache: Surface,
    /// 后缓冲（合成目标，等同 framebuffer 背板）
    screen: Surface,
    /// 窗口管理器
    wm: WindowManager,
    /// 输入事件（桌面自消费一份）
    input: RingBuffer<InputEvent, 128>,
    /// 开始菜单是否展开
    start_menu_open: bool,
    /// 通知卡片（最多 3 条）
    notifications: Vec<Notification>,
    /// 玻璃模式
    mode: GlassMode,
}

/// 通知卡片
struct Notification {
    title: &'static str,
    body: &'static str,
    /// 剩余显示帧数
    ttl: u32,
}

impl Desktop {
    pub fn new() -> Self {
        let (w, h) = {
            let info = framebuffer::fb_info();
            (info.width, info.height)
        };
        let mut d = Self {
            wallpaper: Surface::new(w, h),
            blur_cache: Surface::new(w, h),
            screen: Surface::new(w, h),
            wm: WindowManager::new(),
            input: RingBuffer::new(),
            start_menu_open: false,
            notifications: Vec::new(),
            mode: glass::GlassMode::Full,
        };
        d.render_wallpaper();
        d.precompute_blur();
        d
    }

    /// 生成壁纸：深空蓝紫渐变 + 确定性星光 + 行星装饰
    fn render_wallpaper(&mut self) {
        let g = &glass::GLASS;
        self.wallpaper
            .fill_vgradient(g.bg_grad_top, g.bg_grad_mid, g.bg_grad_bottom);

        // 星光（确定性伪随机：根据坐标哈希，避免引入随机数依赖）
        let w = self.wallpaper.w as i32;
        let h = self.wallpaper.h as i32;
        let star_count = match self.mode {
            GlassMode::Performance => g.star_count_perf,
            _ => g.star_count_full,
        };
        for i in 0..star_count {
            // 简单哈希散列
            let mut x = (i as u32).wrapping_mul(2654435761) % w as u32;
            let mut y = (i as u32).wrapping_mul(40503) % h as u32;
            x ^= y.wrapping_mul(97);
            y ^= x.wrapping_mul(31);
            let x = (x % w as u32) as i32;
            let y = (y % h as u32) as i32;
            let bright = 60 + ((i * 37) % 160) as u8;
            let star = Color::rgba(240, 244, 255, bright);
            // 大小 1-2px
            if i % 5 == 0 {
                self.wallpaper.put_pixel(x + 1, y, star);
                self.wallpaper.put_pixel(x, y + 1, star);
            }
            self.wallpaper.put_pixel(x, y, star);
        }

        // 右下角行星（几何风格）
        let cx = w - w / 8;
        let cy = h - h / 5;
        draw_planet_icon(
            &mut self.wallpaper,
            cx,
            cy,
            h / 14,
            glass::palette::PLANET_ORANGE,
            glass::palette::ORBIT_CYAN,
        );
        // 左上角小火箭
        draw_rocket_icon(&mut self.wallpaper, w / 7, h / 6, h / 20, glass::palette::NEBULA);
    }

    /// 预计算壁纸模糊（降采样 → 3-box 近似高斯 → 升采样），只做一次
    fn precompute_blur(&mut self) {
        let g = &glass::GLASS;
        let ds = g.blur_downsample as usize;
        let sigma = g.blur_radius as f32;
        let w = self.wallpaper.w as usize;
        let h = self.wallpaper.h as usize;
        space_core::blur::precompute_wallpaper_blur(
            &self.wallpaper.data,
            w,
            h,
            sigma,
            ds,
            &mut self.blur_cache.data,
        );
    }

    /// 通知（通知卡片）
    pub fn notify(&mut self, title: &'static str, body: &'static str) {
        self.notifications.push(Notification {
            title,
            body,
            ttl: 5 * 60, // 5 秒 @ 60fps 演示值
        });
        if self.notifications.len() > 3 {
            self.notifications.remove(0);
        }
    }

    /// 主循环（由内核任务调用）
    pub fn run(&mut self) -> ! {
        klog!("[desktop] Glass desktop started");
        // 示例窗口（演示合成）
        self.wm.create_window(
            "About Space OS",
            Rect::new(120, 90, 420, 260),
            true,
        );
        self.wm.create_window(
            "Clock",
            Rect::new(620, 200, 280, 160),
            true,
        );

        loop {
            self.handle_events();
            self.render();
            framebuffer::flush_all(); // 演示帧：全刷；优化版走脏矩形
            timer::delay_us(16_000); // ~60fps 演示
        }
    }

    fn handle_events(&mut self) {
        while let Some(ev) = self.input.pop() {
            // 开始菜单开关
            if ev.kind == EventKind::KeyDown && ev.key == 0x3B /* F1 */ {
                self.start_menu_open = !self.start_menu_open;
            }
            self.wm.dispatch_event(&ev);
        }
        // 从全局输入队列补充（USB 驱动写入）
        loop {
            let mut tmp = InputEvent::new(EventKind::MouseMove);
            let n = window::poll_events(&mut tmp, 1);
            if n == 0 {
                break;
            }
            let _ = self.input.push(tmp);
        }
    }

    /// 合成一帧
    fn render(&mut self) {
        // 1. 清屏为壁纸
        self.screen
            .blit(&self.wallpaper, 0, 0, 255);

        // 2. 按 z 序合成窗口
        let glass_on = self.mode != GlassMode::Performance;
        let windows: Vec<&crate::window::Window> = self.wm.windows().iter().collect();
        for win in windows.iter() {
            if !win.visible {
                continue;
            }
            composite_window(&mut self.screen, &self.wallpaper, &self.blur_cache, win, glass_on);
        }

        // 3. 任务栏（悬浮液态玻璃）
        self.render_taskbar();

        // 4. 开始菜单
        if self.start_menu_open {
            self.render_start_menu();
        }

        // 5. 通知卡片
        self.render_notifications();

        // 6. 拷贝到 framebuffer 后缓冲
        let back = framebuffer::back_buffer_mut();
        back[..self.screen.data.len()].copy_from_slice(&self.screen.data);
    }

    // 上面循环需要 &mut self.screen 与 &self.wm 同时存在，
    // 故合成为自由函数，按字段显式传参（Rust 允许不同字段分别借用）
}

/// 合成单个窗口（自由函数：screen/wallpaper/blur 为字段级拆分借用）
fn composite_window(
    screen: &mut Surface,
    wallpaper: &Surface,
    blur_cache: &Surface,
    win: &window::Window,
    glass_on: bool,
) {
        let r = win.rect;
        let g = &glass::GLASS;

        // 投影（简单偏移阴影）
        let shadow = Rect::new(r.x + 4, r.y + 6, r.w, r.h);
        screen
            .fill_round_rect(&shadow, g.corner_window as f32, g.drop_shadow);

        if win.glass && glass_on {
            // 玻璃路径：取样模糊缓存作为窗口底
            let clip = r.intersect(&Rect::new(0, 0, screen.w as i32, screen.h as i32));
            if let Some(c) = clip {
                for y in c.y..c.y + c.h {
                    let sy = (y - r.y).clamp(0, win.surface.h as i32 - 1) as usize;
                    for x in c.x..c.x + c.w {
                        let sx = (x - r.x).clamp(0, win.surface.w as i32 - 1) as usize;
                        // 从模糊缓存取色
                        let bi = (y as u32 * blur_cache.w + x as u32) as usize * 4;
                        let src = [
                            blur_cache.data[bi],
                            blur_cache.data[bi + 1],
                            blur_cache.data[bi + 2],
                            255,
                        ];
                        // 叠加面板填充（白色 10% alpha）
                        let si = (sy as u32 * win.surface.w + sx as u32) as usize * 4;
                        let panel = g.panel_fill;
                        let di = (y as u32 * screen.w + x as u32) as usize * 4;
                        let mut px = src;
                        // 面板填充混合
                        let sa = panel.3 as u32;
                        let inv = 255 - sa;
                        let (pr, pg, pb) = (panel.0 as u32, panel.1 as u32, panel.2 as u32);
                        for c2 in 0..3 {
                            let pc = match c2 {
                                0 => pr,
                                1 => pg,
                                _ => pb,
                            };
                            px[c2] = ((pc * sa + px[c2] as u32 * inv) / 255) as u8;
                        }
                        // 窗口内容混合
                        let wa = win.surface.data[si + 3] as u32;
                        if wa > 0 {
                            let inv2 = 255 - wa;
                            for c2 in 0..3 {
                                px[c2] = ((win.surface.data[si + c2] as u32 * wa
                                    + px[c2] as u32 * inv2)
                                    / 255) as u8;
                            }
                        }
                        screen.data[di] = px[0];
                        screen.data[di + 1] = px[1];
                        screen.data[di + 2] = px[2];
                        screen.data[di + 3] = 255;
                    }
                }
            }
        } else {
            // 性能/纯色路径：纯色面板 + 窗口内容
            screen
                .fill_round_rect(&r, g.corner_window as f32, g.panel_fill_solid);
            let clip = r.intersect(&Rect::new(0, 0, screen.w as i32, screen.h as i32));
            if let Some(c) = clip {
                for y in c.y..c.y + c.h {
                    let sy = (y - r.y).clamp(0, win.surface.h as i32 - 1) as usize;
                    let di = (y as u32 * screen.w + c.x as u32) as usize * 4;
                    let si = (sy as u32 * win.surface.w + (c.x - r.x).clamp(0, win.surface.w as i32 - 1) as u32) as usize * 4;
                    let n = c.w as usize * 4;
                    screen.data[di..di + n].copy_from_slice(&win.surface.data[si..si + n]);
                }
            }
        }

        // 圆角裁剪（把角落恢复为壁纸色，形成圆角）
        cut_round_corners(screen, wallpaper, &r, g.corner_window as f32);

        // 上亮下暗描边
        let (tw, th) = (1, 1);
        // 顶边（亮）
        screen.fill_rect(&Rect::new(r.x + 4, r.y, r.w - 8, tw), g.edge_highlight_top);
        // 左边（次亮）
        screen.fill_rect(&Rect::new(r.x, r.y + 4, tw, r.h - 8), g.edge_highlight_left);
        // 底边（暗）
        screen.fill_rect(&Rect::new(r.x + 4, r.y + r.h - th, r.w - 8, th), g.edge_shadow_bottom);
        // 右边（次暗）
        screen.fill_rect(&Rect::new(r.x + r.w - tw, r.y + 4, tw, r.h - 8), g.edge_shadow_right);

        // 标题栏（24px，内部高光 + 标题文字）
        let title_bar = Rect::new(r.x, r.y, r.w, 24);
        screen.fill_rect(&title_bar, g.inner_glow);
        screen.draw_text(
            r.x + 12,
            r.y + 8,
            win.title,
            glass::palette::TEXT_PRIMARY,
            1,
        );
    }

/// 圆角裁剪（自由函数）：把四个圆角外区域恢复为壁纸像素
fn cut_round_corners(screen: &mut Surface, wallpaper: &Surface, r: &Rect, radius: f32) {
        let rad = radius as i32;
        let corners = [
            (r.x, r.y, 1, 1),
            (r.x + r.w - rad, r.y, -1, 1),
            (r.x, r.y + r.h - rad, 1, -1),
            (r.x + r.w - rad, r.y + r.h - rad, -1, -1),
        ];
        for (cx, cy, sx, sy) in corners {
            for dy in 0..rad {
                for dx in 0..rad {
                    let px = cx + dx * sx;
                    let py = cy + dy * sy;
                    // 整数精确判断：(2dx+1)² + (2dy+1)² > (2r)²  ⇔  点(dx+0.5,dy+0.5)在半径 r 外
                    let d2 = (2 * dx + 1) * (2 * dx + 1) + (2 * dy + 1) * (2 * dy + 1);
                    let r2 = (2 * radius as i32) * (2 * radius as i32);
                    if d2 > r2 {
                        // 恢复壁纸色
                        let bi = (py as u32 * wallpaper.w + px as u32) as usize * 4;
                        let di = (py as u32 * screen.w + px as u32) as usize * 4;
                        screen.data[di] = wallpaper.data[bi];
                        screen.data[di + 1] = wallpaper.data[bi + 1];
                        screen.data[di + 2] = wallpaper.data[bi + 2];
                    }
                }
            }
        }
}

impl Desktop {
    /// 任务栏：底部悬浮液态玻璃面板
    fn render_taskbar(&mut self) {
        let g = &glass::GLASS;
        let w = self.screen.w as i32;
        let h = self.screen.h as i32;
        let tb = Rect::new(24, h - 56, w - 48, 40);
        let glass_on = self.mode != GlassMode::Performance;

        if glass_on {
            // 从模糊缓存取样任务栏底
            for y in tb.y..tb.y + tb.h {
                for x in tb.x..tb.x + tb.w {
                    let bi = (y as u32 * self.blur_cache.w + x as u32) as usize * 4;
                    let di = (y as u32 * self.screen.w + x as u32) as usize * 4;
                    self.screen.data[di] = self.blur_cache.data[bi];
                    self.screen.data[di + 1] = self.blur_cache.data[bi + 1];
                    self.screen.data[di + 2] = self.blur_cache.data[bi + 2];
                    self.screen.data[di + 3] = 255;
                }
            }
        } else {
            self.screen.fill_rect(&tb, g.panel_dark);
        }

        // 圆角 + 上亮下暗描边
        cut_round_corners(&mut self.screen, &self.wallpaper, &tb, g.corner_panel as f32);
        self.screen.fill_rect(&Rect::new(tb.x + 4, tb.y, tb.w - 8, 1), g.edge_highlight_top);
        self.screen.fill_rect(&Rect::new(tb.x, tb.y + 4, 1, tb.h - 8), g.edge_highlight_left);
        self.screen.fill_rect(&Rect::new(tb.x + 4, tb.y + tb.h - 1, tb.w - 8, 1), g.edge_shadow_bottom);

        // 开始按钮（火箭图标）
        draw_rocket_icon(&mut self.screen, tb.x + 20, tb.y + 20, 12, glass::palette::ORBIT_CYAN);

        // 时钟（右侧，7 段风格用文本代替）
        let secs = timer::ticks() / 100;
        let hh = (secs / 3600) % 24;
        let mm = (secs / 60) % 60;
        let ss = secs % 60;
        let mut time_buf = [0u8; 16];
        let n = {
            use core::fmt::Write;
            let mut w = TimeWriter(&mut time_buf, 0);
            let _ = write!(w, "{:02}:{:02}:{:02}", hh, mm, ss);
            w.1 // 先取出长度，结束 w 对 time_buf 的借用
        };
        let s = alloc::string::String::from(
            core::str::from_utf8(&time_buf[..n]).unwrap_or("--:--:--"),
        );
        self.screen.draw_text(
            tb.x + tb.w - 80,
            tb.y + 14,
            &s,
            glass::palette::TEXT_PRIMARY,
            1,
        );

        // 玻璃模式指示（性能档显示 [PERF]）
        if self.mode == GlassMode::Performance {
            self.screen.draw_text(tb.x + tb.w - 130, tb.y + 14, "[PERF]", glass::palette::PLANET_ORANGE, 1);
        }
    }

    /// 开始菜单（磨砂玻璃面板）
    fn render_start_menu(&mut self) {
        let g = &glass::GLASS;
        let _w = self.screen.w as i32;
        let h = self.screen.h as i32;
        let menu = Rect::new(24, h - 56 - 300, 260, 296);
        let glass_on = self.mode != GlassMode::Performance;

        if glass_on {
            for y in menu.y..menu.y + menu.h {
                for x in menu.x..menu.x + menu.w {
                    let bi = (y as u32 * self.blur_cache.w + x as u32) as usize * 4;
                    let di = (y as u32 * self.screen.w + x as u32) as usize * 4;
                    self.screen.data[di] = self.blur_cache.data[bi];
                    self.screen.data[di + 1] = self.blur_cache.data[bi + 1];
                    self.screen.data[di + 2] = self.blur_cache.data[bi + 2];
                    self.screen.data[di + 3] = 255;
                }
            }
        } else {
            self.screen.fill_rect(&menu, g.panel_dark);
        }

        cut_round_corners(&mut self.screen, &self.wallpaper, &menu, g.corner_panel as f32);
        self.screen.fill_rect(&Rect::new(menu.x + 4, menu.y, menu.w - 8, 1), g.edge_highlight_top);
        self.screen.fill_rect(&Rect::new(menu.x + 4, menu.y + menu.h - 1, menu.w - 8, 1), g.edge_shadow_bottom);

        self.screen.draw_text(menu.x + 16, menu.y + 12, "SPACE OS", glass::palette::TEXT_PRIMARY, 1);
        let items = ["About", "Calculator", "Clock", "Terminal"];
        for (i, item) in items.iter().enumerate() {
            let y = menu.y + 44 + i as i32 * 40;
            // 菜单项背景（悬停高亮省略）
            self.screen.draw_text(menu.x + 20, y, item, glass::palette::TEXT_SECONDARY, 1);
            // 几何图标点
            self.screen.draw_circle(menu.x + 12, y + 4, 3, true, glass::palette::ORBIT_CYAN);
        }
    }

    /// 通知卡片（右上角堆叠）
    fn render_notifications(&mut self) {
        let g = &glass::GLASS;
        let w = self.screen.w as i32;
        let mut y = 20i32;
        self.notifications.retain(|n| {
            n.ttl > 0
        });
        for n in self.notifications.iter_mut() {
            let card = Rect::new(w - 340, y, 320, 64);
            self.screen
                .fill_round_rect(&card, g.corner_notification as f32, g.panel_fill_solid);
            cut_round_corners(&mut self.screen, &self.wallpaper, &card, g.corner_notification as f32);
            self.screen
                .fill_rect(&Rect::new(card.x + 4, card.y, card.w - 8, 1), g.edge_highlight_top);
            self.screen.draw_text(card.x + 14, card.y + 10, n.title, glass::palette::TEXT_PRIMARY, 1);
            self.screen.draw_text(card.x + 14, card.y + 28, n.body, glass::palette::TEXT_SECONDARY, 1);
            n.ttl = n.ttl.saturating_sub(1);
            y += 72;
        }
    }
}

/// 临时写入缓冲的辅助 writer
struct TimeWriter<'a>(&'a mut [u8], usize);
impl<'a> core::fmt::Write for TimeWriter<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.0.len());
        self.0[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.1 += n;
        self.0 = &mut core::mem::take(&mut self.0)[n..];
        Ok(())
    }
}

/// 桌面任务入口（内核线程）
pub extern "C" fn desktop_task() {
    // 等待 framebuffer 就绪（Phase 3 初始化后由主流程调用 run）
    let mut d = Desktop::new();
    d.notify("Space OS", "Glass 桌面已启动");
    d.run();
}
