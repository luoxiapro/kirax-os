//! # Glass 设计规范 —— 深空星球 × 液态玻璃（可执行的设计令牌）
//!
//! 这是 Phase 4 图形子系统与 Phase 5 应用共同引用的唯一权威参数来源。
//! 设计目标（针对树莓派 2 的性能约束）：
//! - 玻璃效果“从设计上轻量化”：低分辨率预计算模糊、限制面板数量、默认低模糊半径；
//! - 一键性能模式：关闭模糊、退回纯色半透明面板；
//! - 全部为几何矢量风格，禁止 emoji。

/// RGBA 颜色（组件顺序与 framebuffer 的 BGRA 相反，方便内存直写）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b, 255)
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(r, g, b, a)
    }
    /// 从 `#RRGGBB` 或 `#RRGGBBAA` 解析（供配置文件/应用 manifest 使用）
    pub const fn from_hex(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let n = b.len();
        if n < 7 || b[0] != b'#' {
            return None;
        }
        // 逐 nibble 解析（const fn 不支持 ? 与闭包，用显式 match）
        let mut v = 0u32;
        let mut i = 1usize;
        let mut count = 0usize;
        while i < n && count < 8 {
            let c = b[i];
            let nib = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => return None,
            };
            v = (v << 4) | nib as u32;
            i += 1;
            count += 1;
        }
        match count {
            6 => Some(Self((v >> 16) as u8, (v >> 8) as u8, v as u8, 255)),
            8 => Some(Self((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)),
            _ => None,
        }
    }
    /// 与另一颜色按比例混合（t=0 全自己，t=1 全 other）
    pub const fn mix(self, other: Self, t: u8) -> Self {
        let f = t as u32;
        let inv = 255 - f;
        Self(
            ((self.0 as u32 * inv + other.0 as u32 * f) / 255) as u8,
            ((self.1 as u32 * inv + other.1 as u32 * f) / 255) as u8,
            ((self.2 as u32 * inv + other.2 as u32 * f) / 255) as u8,
            ((self.3 as u32 * inv + other.3 as u32 * f) / 255) as u8,
        )
    }
}

/// 深空星球主题基础色板
pub mod palette {
    use super::Color;
    /// 深空底色（顶部，近黑蓝）
    pub const SPACE_TOP: Color = Color::rgb(7, 9, 20);
    /// 深空底色（中部，蓝紫）
    pub const SPACE_MID: Color = Color::rgb(16, 20, 46);
    /// 深空底色（底部，星云紫）
    pub const SPACE_BOTTOM: Color = Color::rgb(28, 22, 54);
    /// 星云紫（强调）
    pub const NEBULA: Color = Color::rgb(139, 124, 246);
    /// 行星橙（强调）
    pub const PLANET_ORANGE: Color = Color::rgb(255, 159, 69);
    /// 轨道青（强调）
    pub const ORBIT_CYAN: Color = Color::rgb(79, 216, 235);
    /// 火箭红（强调/危险）
    pub const ROCKET_RED: Color = Color::rgb(255, 92, 122);
    /// 星光白
    pub const STAR_WHITE: Color = Color::rgb(240, 244, 255);
    /// 正文主文字
    pub const TEXT_PRIMARY: Color = Color::rgba(238, 241, 255, 240);
    /// 次级文字
    pub const TEXT_SECONDARY: Color = Color::rgba(190, 196, 224, 200);
    /// 图标禁用
    pub const TEXT_DISABLED: Color = Color::rgba(140, 148, 180, 160);
}

/// 性能档位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GlassMode {
    /// 完整玻璃：默认模糊 + 高光 + 噪声
    Full = 0,
    /// 轻玻璃：模糊半径减半，面板数减半
    Lite = 1,
    /// 性能模式：无模糊，纯色半透明面板（圆角与高光保留，去噪声）
    Performance = 2,
}

/// Glass 令牌表 —— 所有图形代码引用这里，禁止散落魔法数字
#[derive(Debug, Clone, Copy)]
pub struct GlassTokens {
    // ---- 壁纸 ----
    /// 渐变起点色
    pub bg_grad_top: Color,
    /// 渐变中间色（屏幕 55% 高度）
    pub bg_grad_mid: Color,
    /// 渐变终点色
    pub bg_grad_bottom: Color,
    /// 星光色
    pub star_color: Color,
    /// 星星数量（性能档位缩放）
    pub star_count_full: u16,
    pub star_count_perf: u16,
    /// 行星色（壁纸装饰元素）
    pub planet_color: Color,
    pub orbit_color: Color,

    // ---- 玻璃面板 ----
    /// 面板填充（半透明白，叠加在模糊壁纸上）
    pub panel_fill: Color,
    /// 性能模式下使用的纯色面板填充（不透明白玻璃模拟）
    pub panel_fill_solid: Color,
    /// 深色玻璃（任务栏/菜单用）
    pub panel_dark: Color,
    /// 上边缘高光（亮）
    pub edge_highlight_top: Color,
    /// 左边缘高光（次亮）
    pub edge_highlight_left: Color,
    /// 下边缘阴影（暗）
    pub edge_shadow_bottom: Color,
    /// 右边缘阴影（次暗）
    pub edge_shadow_right: Color,
    /// 内部顶部 1px 高光线
    pub inner_glow: Color,
    /// 悬停时边缘外发光
    pub hover_glow: Color,

    // ---- 圆角 ----
    pub corner_window: u16,
    pub corner_panel: u16,
    pub corner_button: u16,
    pub corner_notification: u16,

    // ---- 模糊参数 ----
    /// 默认模糊半径（像素）
    pub blur_radius: u8,
    /// 轻玻璃档模糊半径
    pub blur_radius_lite: u8,
    /// 性能档模糊半径（0 = 关闭模糊）
    pub blur_radius_perf: u8,
    /// 模糊降采样倍率（先在 1/scale 分辨率上模糊再放大）
    pub blur_downsample: u8,
    /// 高斯近似盒式模糊次数
    pub blur_box_passes: u8,
    /// 同时可见玻璃面板数量上限
    pub max_glass_panels: u8,
    /// 轻玻璃档面板上限
    pub max_glass_panels_lite: u8,
    /// 模糊总面积占屏比上限（超过则自动降级）
    pub max_blur_area_ratio: u8,

    // ---- 噪声纹理 ----
    pub noise_strength: u8,
    pub noise_tile: u16,

    // ---- 阴影/深度 ----
    /// 窗口投影（非玻璃区域）
    pub drop_shadow: Color,
    pub shadow_radius: u8,
}

/// 默认玻璃令牌（Full 档）
pub const GLASS: GlassTokens = GlassTokens {
    bg_grad_top: palette::SPACE_TOP,
    bg_grad_mid: palette::SPACE_MID,
    bg_grad_bottom: palette::SPACE_BOTTOM,
    star_color: Color::rgba(240, 244, 255, 180),
    star_count_full: 240,
    star_count_perf: 80,
    planet_color: palette::PLANET_ORANGE,
    orbit_color: palette::ORBIT_CYAN,

    // 玻璃面板：白色 10% alpha（叠加在模糊背景上形成“液态玻璃”）
    panel_fill: Color::rgba(255, 255, 255, 26),
    panel_fill_solid: Color::rgba(28, 32, 56, 235),
    panel_dark: Color::rgba(16, 18, 40, 150),

    // 上亮下暗描边：顶/左亮，底/右暗
    edge_highlight_top: Color::rgba(255, 255, 255, 64),
    edge_highlight_left: Color::rgba(255, 255, 255, 36),
    edge_shadow_bottom: Color::rgba(0, 0, 0, 90),
    edge_shadow_right: Color::rgba(0, 0, 0, 60),
    inner_glow: Color::rgba(255, 255, 255, 38),
    hover_glow: Color::rgba(120, 160, 255, 60),

    corner_window: 22,
    corner_panel: 14,
    corner_button: 9,
    corner_notification: 16,

    blur_radius: 8,
    blur_radius_lite: 4,
    blur_radius_perf: 0,
    blur_downsample: 2,
    blur_box_passes: 3,
    max_glass_panels: 4,
    max_glass_panels_lite: 2,
    max_blur_area_ratio: 20, // 20%

    noise_strength: 6,
    noise_tile: 64,

    drop_shadow: Color::rgba(0, 0, 0, 120),
    shadow_radius: 12,
};

/// 依据档位返回实际模糊半径
pub fn effective_blur_radius(mode: GlassMode) -> u8 {
    match mode {
        GlassMode::Full => GLASS.blur_radius,
        GlassMode::Lite => GLASS.blur_radius_lite,
        GlassMode::Performance => GLASS.blur_radius_perf,
    }
}

/// 依据档位返回玻璃面板上限
pub fn effective_max_panels(mode: GlassMode) -> u8 {
    match mode {
        GlassMode::Full => GLASS.max_glass_panels,
        GlassMode::Lite => GLASS.max_glass_panels_lite,
        GlassMode::Performance => 0, // 性能模式不允许玻璃面板
    }
}

/// 字体排版令牌
pub const FONT: FontTokens = FontTokens {
    size_small: 12,
    size_body: 16,
    size_title: 20,
    size_display: 28,
    line_height: 22,
    letter_spacing: 0,
};

pub struct FontTokens {
    pub size_small: u16,
    pub size_body: u16,
    pub size_title: u16,
    pub size_display: u16,
    pub line_height: u16,
    pub letter_spacing: i8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parse() {
        assert_eq!(Color::from_hex("#FFFFFF"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(Color::from_hex("#0B0E1A"), Some(Color::rgb(11, 14, 26)));
        assert_eq!(Color::from_hex("#FF9F4580"), Some(Color::rgba(255, 159, 69, 128)));
        assert_eq!(Color::from_hex("FF0000"), None);
        assert_eq!(Color::from_hex("#GG0000"), None);
    }

    #[test]
    fn mix_math() {
        let a = Color::rgb(0, 0, 0);
        let b = Color::rgb(255, 255, 255);
        // t=255 → 全 b
        assert_eq!(a.mix(b, 255), b);
        // t=0 → 全 a
        assert_eq!(a.mix(b, 0), a);
        // t≈128 → 中间灰（整数除法误差 ±1）
        let m = a.mix(b, 128);
        assert!(m.0 == 127 || m.0 == 128, "got {}", m.0);
    }

    #[test]
    fn mode_switch() {
        assert_eq!(effective_blur_radius(GlassMode::Full), 8);
        assert_eq!(effective_blur_radius(GlassMode::Lite), 4);
        assert_eq!(effective_blur_radius(GlassMode::Performance), 0);
        assert_eq!(effective_max_panels(GlassMode::Full), 4);
        assert_eq!(effective_max_panels(GlassMode::Performance), 0);
    }
}
