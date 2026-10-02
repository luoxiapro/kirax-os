//! 轻量 f32 / 整数数学（自研，不依赖任何第三方库）。
//!
//! 背景：thumbv7a-none-eabi 目标的 `core` 未提供 `f32::sqrt / floor / round /
//! ceil / powi / powf` 等浮点数学方法（只提供 abs/max/min/clamp 等位运算级方法）。
//! 为保证 Space OS 全程零依赖、可在任何工具链上编译，这里用基本运算
//! （+ - * / 比较、位转换）自实现所需函数。
//!
//! 精度说明：所有用途都是抗锯齿覆盖率、模糊核尺寸等视觉参数，
//! 误差 < 1% 完全不可见；整数 sqrt 用牛顿迭代精确到 ±1。

/// 向下取整（正负通用）：floor(x) = 整数部分，若为负则 -1
pub fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

/// 四舍五入（正负通用）：round(x) = floor(x + 0.5)（负数对称）
pub fn round(x: f32) -> f32 {
    if x >= 0.0 {
        floor(x + 0.5)
    } else {
        -floor(-x + 0.5)
    }
}

/// 向上取整：ceil(x) = -floor(-x)
pub fn ceil(x: f32) -> f32 {
    -floor(-x)
}

/// 平方根：牛顿迭代（初始猜测 + 5 次迭代，收敛到 <1e-3）
/// 仅用于小正数域（模糊核宽、SDF 距离），超界按 0 处理。
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if !x.is_finite() {
        return x; // 非有限值原样返回（调用方不会传入 NaN/Inf）
    }
    // 初始猜测：指数量级近似（把浮点位模式右移 + 偏移常数）
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FBD_1DF5);
    // 牛顿迭代 y = (y + x/y) / 2，5 次足够
    for _ in 0..5 {
        y = 0.5 * (y + x / y);
    }
    y
}

/// 整数平方根（u32 → u32，牛顿迭代，向下取整）
pub fn isqrt(n: u32) -> u32 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_basic() {
        assert_eq!(floor(3.7), 3.0);
        assert_eq!(floor(-3.7), -4.0);
        assert_eq!(floor(4.0), 4.0);
    }

    #[test]
    fn round_basic() {
        assert_eq!(round(3.4), 3.0);
        assert_eq!(round(3.5), 4.0);
        assert_eq!(round(-3.5), -4.0);
    }

    #[test]
    fn sqrt_accuracy() {
        for v in [1.0f32, 2.0, 4.0, 9.0, 16.0, 100.0, 0.25, 12.0] {
            let s = sqrt(v);
            let err = (s * s - v).abs();
            assert!(err < 0.01, "sqrt({}) err={}", v, err);
        }
    }

    #[test]
    fn isqrt_basic() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(10), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(65535), 255);
    }
}
