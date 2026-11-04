//! 临时探针：打印 inpaint 区域外的差异坐标（审计用）。

use screenloom_core::{inpaint_region, Rect, Rgba, RgbaImage};

fn main() {
    let (w, h) = (120u32, 90u32);
    let mut img = RgbaImage::new(w, h);
    let mut s = 0x1234_5678u32;
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s % 24) as i32 - 12;
            let r = ((x * 255) / w) as i32 + n;
            let g = ((y * 255) / h) as i32 + n;
            let b = (((x + y) * 255) / (w + h)) as i32 + n;
            img.set_pixel(
                x,
                y,
                Rgba::rgb(
                    r.clamp(0, 255) as u8,
                    g.clamp(0, 255) as u8,
                    b.clamp(0, 255) as u8,
                ),
            );
        }
    }
    let region = Rect::new(30, 20, 40, 30);
    let out = inpaint_region(&img, region);
    let mut diffs = 0;
    let mut rows = std::collections::BTreeMap::new();
    for y in 0..h {
        for x in 0..w {
            if region.contains(screenloom_core::Point::new(x as i32, y as i32)) {
                continue;
            }
            if out.pixel(x, y) != img.pixel(x, y) {
                *rows.entry(y).or_insert(0usize) += 1;
                diffs += 1;
            }
        }
    }
    eprintln!("total diffs outside = {diffs}; rows={rows:?}");
}
