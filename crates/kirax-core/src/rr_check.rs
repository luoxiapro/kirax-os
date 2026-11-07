#[test]
fn rr_check() {
    use crate::roundrect::*;
    let mut buf = vec![0u8; 64 * 64 * 4];
    let r = Rect::new(8, 8, 48, 48);
    stroke_round_rect(&mut buf, 64, 64, &r, 8.0, 2.0, [255, 255, 255, 255]);
    let mut hits = vec![];
    for y in 0..64 {
        for x in 0..64 {
            if buf[(y * 64 + x) * 4] > 0 {
                hits.push((x, y, buf[(y * 64 + x) * 4]));
            }
        }
    }
    // 描边 2px、圆角 8px 的 48x48 圆角矩形：覆盖像素数应在合理区间内
    // （太少先剔除边角/描边过细，太多说明圆角外也被填充）
    assert!(hits.len() > 100 && hits.len() < 500, "描边覆盖像素数异常: {}", hits.len());

    // 顶边中点 (32,8) 在描边带上 → 应被绘制
    let top_mid = buf[(8 * 64 + 32) * 4];
    assert_eq!(top_mid, 255, "顶边中点应被实心描边覆盖");

    // 矩形内中心 (32,32) 位于描边内部（距边 > 2px）→ 不应绘制
    let center = buf[(32 * 64 + 32) * 4];
    assert_eq!(center, 0, "矩形内部不应被描边填充");

    // 圆角弧内 (10,10)：像素中心 (10.5,10.5) 距圆心 (16,16) 7.78px < 半径 8px
    // → 描边应覆盖（SDF 圆角的正确语义）
    let in_arc = buf[(10 * 64 + 10) * 4];
    assert!(in_arc > 0, "圆角弧内应被描边覆盖");
    // 圆角外 (7,7)：距圆心 sqrt(8.5²+8.5²)=12px > 半径 → 无像素
    let corner = buf[(7 * 64 + 7) * 4];
    assert_eq!(corner, 0, "圆角外部不应绘制");

    // 矩形外 (8,6)（顶边上方 2px 外）→ 无像素
    let outside = buf[(6 * 64 + 8) * 4];
    assert_eq!(outside, 0, "矩形外不应绘制");
}
