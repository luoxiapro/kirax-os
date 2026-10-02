//! 自研窗口系统：窗口管理 + 输入事件分发 + 合成对接。
//!
//! 架构（与 Phase 4 文档对应）：
//! - 每个窗口拥有独立 [`Surface`] 背板，应用（或内核服务）直接绘制；
//! - 合成器按 z 序把窗口合成到后缓冲，玻璃面板从“壁纸模糊缓存”取样；
//! - 输入事件进入全局环形缓冲；命中测试按 z 序分发到窗口；
//! - 用户态通过 `sys_events_poll` / `sys_window_*` 使用。

use crate::graphics::Surface;
use crate::klog;
use alloc::vec::Vec;
use space_core::events::{EventKind, InputEvent, RingBuffer};
use space_core::glass::GlassMode;
use space_core::roundrect::Rect;

/// 全局输入事件缓冲（驱动写入 / 应用读取）
static INPUT_QUEUE: RingBuffer<InputEvent, 512> = RingBuffer::new();

/// 全局玻璃模式（性能降级开关）
static GLASS_MODE: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(GlassMode::Full as u8);

/// 驱动/中断上下文写入输入事件
pub fn post_event(ev: InputEvent) {
    let _ = INPUT_QUEUE.push(ev);
}

/// 读取输入事件（系统调用；返回拷贝数量）
pub fn poll_events(buf: *mut InputEvent, max: usize) -> usize {
    if buf.is_null() {
        return 0;
    }
    let mut n = 0usize;
    while n < max {
        match INPUT_QUEUE.pop() {
            Some(ev) => unsafe {
                core::ptr::write_volatile(buf.add(n), ev);
                n += 1;
            },
            None => break,
        }
    }
    n
}

/// 设置玻璃模式（0=Full 1=Lite 2=Performance）
pub fn set_glass_mode(mode: GlassMode) {
    GLASS_MODE.store(mode as u8, core::sync::atomic::Ordering::Release);
    klog!("[wm] 玻璃模式切换 -> {:?}", mode);
}

/// 当前玻璃模式
pub fn glass_mode() -> GlassMode {
    match GLASS_MODE.load(core::sync::atomic::Ordering::Acquire) {
        0 => GlassMode::Full,
        1 => GlassMode::Lite,
        _ => GlassMode::Performance,
    }
}

/// 窗口
pub struct Window {
    pub id: u32,
    pub title: &'static str,
    pub rect: Rect,
    pub visible: bool,
    /// 是否玻璃面板（模糊背景）
    pub glass: bool,
    /// 是否可拖拽标题栏
    pub movable: bool,
    /// 窗口背板（合成与绘制目标）
    pub surface: Surface,
    /// 脏矩形（自上次合成以来）
    pub dirty: bool,
}

impl Window {
    fn new(id: u32, title: &'static str, rect: Rect, glass: bool) -> Self {
        let surface = Surface::new(rect.w as u32, rect.h as u32);
        Self {
            id,
            title,
            rect,
            visible: true,
            glass,
            movable: true,
            surface,
            dirty: true,
        }
    }
}

/// 窗口管理器
pub struct WindowManager {
    windows: Vec<Window>,
    next_id: u32,
    focus: Option<u32>,
    /// 拖拽状态
    drag: Option<(u32, i32, i32)>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            focus: None,
            drag: None,
        }
    }

    pub fn create_window(&mut self, title: &'static str, rect: Rect, glass: bool) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push(Window::new(id, title, rect, glass));
        self.focus = Some(id);
        id
    }

    pub fn destroy_window(&mut self, id: u32) {
        self.windows.retain(|w| w.id != id);
        if self.focus == Some(id) {
            self.focus = self.windows.last().map(|w| w.id);
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn get(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn focus(&self) -> Option<u32> {
        self.focus
    }

    /// 命中测试：返回 z 序最上（列表末尾）的可见窗口
    fn hit_test(&self, x: i32, y: i32) -> Option<u32> {
        self.windows
            .iter()
            .rev()
            .find(|w| w.visible && w.rect.contains(x, y))
            .map(|w| w.id)
    }

    /// 输入分发（桌面循环调用）
    pub fn dispatch_event(&mut self, ev: &InputEvent) {
        // 拖拽处理
        if let Some((win_id, offx, offy)) = self.drag {
            match ev.kind {
                EventKind::MouseMove => {
                    if let Some(w) = self.get_mut(win_id) {
                        w.rect.x = ev.x as i32 - offx;
                        w.rect.y = ev.y as i32 - offy;
                        w.dirty = true;
                    }
                    return;
                }
                EventKind::MouseUp => {
                    self.drag = None;
                    return;
                }
                _ => {}
            }
        }

        match ev.kind {
            EventKind::MouseDown => {
                if let Some(id) = self.hit_test(ev.x as i32, ev.y as i32) {
                    self.focus = Some(id);
                    // 点击标题栏（顶部 24px）开始拖拽
                    if let Some(w) = self.get(id) {
                        if w.movable && ev.y as i32 <= w.rect.y + 24 {
                            self.drag = Some((id, ev.x as i32 - w.rect.x, ev.y as i32 - w.rect.y));
                        }
                    }
                }
            }
            EventKind::KeyDown => {
                // 焦点窗口键盘事件（应用通过 sys_events_poll 自行获取，这里仅更新焦点）
            }
            _ => {}
        }
    }

    /// 标记全部窗口需要重绘
    pub fn mark_all_dirty(&mut self) {
        for w in self.windows.iter_mut() {
            w.dirty = true;
        }
    }
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}
