//! 程序图标：**纯像素绘制**（无平台依赖）。
//!
//! 同一套绘制代码供两处复用，保证 exe 图标与托盘图标**永远一致**：
//!
//! * `screenloom-app/examples/gen_icon.rs` → 打包成 `assets/screenloom.ico` → 注入 exe 资源；
//! * `screenloom-app/src/tray.rs` → 运行时直接画出托盘用的 HICON。
//!
//! 设计：微信绿圆角方块 + 白色**取景框角标**（截图工具的通用符号）。
//!
//! 抗锯齿靠**超采样**（先画 SS 倍大的硬边图，再盒式降采样），
//! 所以小尺寸（16px）不会因为线条太细而糊掉。

use crate::image::{Rgba, RgbaImage};

/// 品牌主色（微信绿）。
pub const BRAND: Rgba = Rgba::rgb(0x07, 0xC1, 0x60);
/// 品牌渐变底部色。
pub const BRAND_DARK: Rgba = Rgba::rgb(0x00, 0x9E, 0x4B);
/// 前景色（白色角标）。
pub const FG: Rgba = Rgba::WHITE;

/// 超采样倍数（抗锯齿）。
const SS: u32 = 4;

/// 画一个程序图标（`size` × `size`，RGBA）。
///
/// `size` 建议是 16/24/32/48/64/128/256；非 2 的幂也可以。
pub fn draw_app_icon(size: u32) -> RgbaImage {
    let size = size.max(8);
    let big = size * SS;
    let mut c = Canvas::new(big, big);
    let s = big as f32;

    // ---- 圆角方块底（竖向渐变）----
    let inset = s * 0.03;
    let radius = s * 0.22;
    let (x0, y0, x1, y1) = (inset, inset, s - inset, s - inset);
    for y in 0..big {
        for x in 0..big {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if !inside_round_rect(px, py, x0, y0, x1, y1, radius) {
                continue;
            }
            let t = ((py - y0) / (y1 - y0).max(1.0)).clamp(0.0, 1.0);
            let col = Rgba {
                r: (BRAND.r as f32 * (1.0 - t) + BRAND_DARK.r as f32 * t) as u8,
                g: (BRAND.g as f32 * (1.0 - t) + BRAND_DARK.g as f32 * t) as u8,
                b: (BRAND.b as f32 * (1.0 - t) + BRAND_DARK.b as f32 * t) as u8,
                a: 255,
            };
            c.blend(x as i32, y as i32, col);
        }
    }

    // ---- 白色取景框角标 ----
    let margin = s * 0.245;
    let fw = s - margin * 2.0;
    // 笔画宽度：小尺寸时保底 1.6px（否则 16px 下会糊）
    let t = (s * 0.062).max(1.6);
    // 角标臂长必须 **小于半框宽**，否则相邻两角会连成整条框线。
    let arm = (fw * 0.42).max(t * 1.2);
    let (lx, ly) = (margin, margin);
    let (rx, ry) = (margin + fw, margin + fw);
    // 四角的取景框角标：左上 / 右上 / 左下 / 右下，每角两段
    let brackets = [
        // 左上
        (lx, ly, lx + arm, ly + t),
        (lx, ly, lx + t, ly + arm),
        // 右上
        (rx - arm, ly, rx, ly + t),
        (rx - t, ly, rx, ly + arm),
        // 左下
        (lx, ry - t, lx + arm, ry),
        (lx, ry - arm, lx + t, ry),
        // 右下
        (rx - arm, ry - t, rx, ry),
        (rx - t, ry - arm, rx, ry),
    ];
    for (bx0, by0, bx1, by1) in brackets {
        for y in 0..big {
            let py = y as f32 + 0.5;
            if py < by0 || py >= by1 {
                continue;
            }
            for x in 0..big {
                let px = x as f32 + 0.5;
                if px < bx0 || px >= bx1 {
                    continue;
                }
                c.blend(x as i32, y as i32, FG);
            }
        }
    }

    c.downsample(SS, size, size)
}

/// 把若干 RGBA 图打包成 `.ico`（classic BMP 条目：32bpp BGRA + 1bpp AND 掩码）。
///
/// 用 classic BMP 而不是 PNG 压缩条目，是为了兼容 `rc.exe`（资源编译器）。
pub fn pack_ico(images: &[RgbaImage]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = 1 (icon)
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    // 先算每个条目的偏移
    let entry_size = 16usize;
    let header = 6 + entry_size * images.len();
    let mut offsets = Vec::new();
    let mut cur = header;
    for img in images {
        offsets.push(cur);
        cur += ico_image_len(img);
    }

    for (i, img) in images.iter().enumerate() {
        let w = img.w.min(256) as u8;
        let h = img.h.min(256) as u8;
        out.push(w); // 0 表示 256
        out.push(h);
        out.push(0); // 调色板数（真彩为 0）
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bitCount
        out.extend_from_slice(&(ico_image_len(img) as u32).to_le_bytes());
        out.extend_from_slice(&(offsets[i] as u32).to_le_bytes());
    }

    for img in images {
        write_ico_image(&mut out, img);
    }
    out
}

/// ICO 里单个图像数据区的字节数（BITMAPINFOHEADER + XOR 位图 + AND 掩码）。
fn ico_image_len(img: &RgbaImage) -> usize {
    let w = img.w as usize;
    let h = img.h as usize;
    let xor = w * h * 4;
    let mask_row = w.div_ceil(32) * 4; // 1bpp，每行补齐到 4 字节
    40 + xor + mask_row * h
}

fn write_ico_image(out: &mut Vec<u8>, img: &RgbaImage) {
    let (w, h) = (img.w as i32, img.h as i32);
    // BITMAPINFOHEADER：高度是 2 倍（XOR + AND 掩码）
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&w.to_le_bytes()); // biWidth
    out.extend_from_slice(&(h * 2).to_le_bytes()); // biHeight
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0u32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // XOR 位图：**自下而上**、每像素 BGRA（直通 alpha，非预乘）
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.pixel(x as u32, y as u32);
            out.push(p.b);
            out.push(p.g);
            out.push(p.r);
            out.push(p.a);
        }
    }

    // AND 掩码：1bpp，自下而上；完全透明的像素置 1（老系统按掩码裁剪）
    let mask_row_bytes = (((w + 31) / 32) * 4) as usize;
    for y in (0..h).rev() {
        let mut row = vec![0u8; mask_row_bytes];
        for x in 0..w {
            if img.pixel(x as u32, y as u32).a < 8 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
}

/// 圆角矩形内部判定。
fn inside_round_rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

/// 预乘 alpha 的绘制画布。
struct Canvas {
    w: u32,
    h: u32,
    /// 预乘 RGBA，每通道 f32。
    px: Vec<f32>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Canvas {
            w,
            h,
            px: vec![0.0; (w * h * 4) as usize],
        }
    }

    /// 源 over 混合。
    fn blend(&mut self, x: i32, y: i32, c: Rgba) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let a = c.a as f32 / 255.0;
        if a <= 0.0 {
            return;
        }
        let i = ((y as usize * self.w as usize) + x as usize) * 4;
        let inv = 1.0 - a;
        self.px[i] = (c.r as f32 / 255.0) * a + self.px[i] * inv;
        self.px[i + 1] = (c.g as f32 / 255.0) * a + self.px[i + 1] * inv;
        self.px[i + 2] = (c.b as f32 / 255.0) * a + self.px[i + 2] * inv;
        self.px[i + 3] = a + self.px[i + 3] * inv;
    }

    /// 盒式降采样（预乘空间平均 → 反预乘）。
    fn downsample(&self, factor: u32, out_w: u32, out_h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(out_w, out_h);
        let f = factor as usize;
        let n = (f * f) as f32;
        for oy in 0..out_h as usize {
            for ox in 0..out_w as usize {
                let mut r = 0.0f32;
                let mut g = 0.0f32;
                let mut b = 0.0f32;
                let mut a = 0.0f32;
                for sy in 0..f {
                    for sx in 0..f {
                        let x = ox * f + sx;
                        let y = oy * f + sy;
                        if x >= self.w as usize || y >= self.h as usize {
                            continue;
                        }
                        let i = (y * self.w as usize + x) * 4;
                        r += self.px[i];
                        g += self.px[i + 1];
                        b += self.px[i + 2];
                        a += self.px[i + 3];
                    }
                }
                r /= n;
                g /= n;
                b /= n;
                a /= n;
                let col = if a <= 0.0 {
                    Rgba::TRANSPARENT
                } else {
                    let inv = 1.0 / a;
                    Rgba {
                        r: (r * inv * 255.0).round().min(255.0) as u8,
                        g: (g * inv * 255.0).round().min(255.0) as u8,
                        b: (b * inv * 255.0).round().min(255.0) as u8,
                        a: (a * 255.0).round().min(255.0) as u8,
                    }
                };
                img.set_pixel(ox as u32, oy as u32, col);
            }
        }
        img
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_expected_size() {
        let img = draw_app_icon(32);
        assert_eq!((img.w, img.h), (32, 32));
    }

    #[test]
    fn corners_are_opaque_and_center_is_not() {
        let img = draw_app_icon(64);
        // 四角（取景框角标处）应该是白色
        let corner = img.pixel(64 / 5, 64 / 5);
        assert!(corner.a > 200, "角标应为不透明白色，实际 {corner:?}");
        // 圆角方块外沿（画布角落）应完全透明
        assert_eq!(img.pixel(0, 0).a, 0, "角外应透明");
        // 中心应是品牌绿
        let mid = img.pixel(32, 32);
        assert!(mid.a > 200, "中心应不透明，实际 {mid:?}");
        assert!(mid.g > mid.r, "中心应是绿色调，实际 {mid:?}");
    }

    #[test]
    fn packs_valid_ico_header() {
        let imgs: Vec<RgbaImage> = [16u32, 32, 48].iter().map(|s| draw_app_icon(*s)).collect();
        let ico = pack_ico(&imgs);
        assert_eq!(&ico[0..2], &[0, 0]); // reserved
        assert_eq!(&ico[2..4], &[1, 0]); // type = icon
        assert_eq!(&ico[4..6], &[3, 0]); // count
                                         // 第一个条目：宽 16
        assert_eq!(ico[6], 16);
        assert_eq!(ico[7], 16);
        // 条目布局：w,h,调色板数,reserved(4) | planes(2) | bitCount(2) | 长度(4) | 偏移(4)
        assert_eq!(
            &ico[6 + 8..6 + 12],
            &(ico_image_len(&imgs[0]) as u32).to_le_bytes()
        ); // 图像长度
        assert_eq!(&ico[6 + 12..6 + 16], &54u32.to_le_bytes()); // 图像偏移 = 6 + 16*3
                                                                // 总长度 = 头 + 各图像
        let expect = 6 + 16 * 3 + imgs.iter().map(ico_image_len).sum::<usize>();
        assert_eq!(ico.len(), expect);
    }

    #[test]
    fn ico_256_entry_uses_zero_dimensions() {
        let imgs = vec![draw_app_icon(256)];
        let ico = pack_ico(&imgs);
        assert_eq!(ico[6], 0, "256 在 ICO 头里用 0 表示");
        assert_eq!(ico[7], 0);
    }
}
