//! 把画布标注渲染到图像上。
//!
//! 这是 **app 层** 落像素的入口：裁出选区后调用 [`render_annotations`] 即可得到
//! 带标注的成图（复制到剪贴板 / 存盘 / 贴图都用它）。
//!
//! 坐标约定：标注坐标 = **被截图区域自身的像素坐标**（相对左上角 0,0）。

use screenloom_core::{Annotation, Canvas, Point, Rect, Rgba, RgbaImage, ShapeKind};

use crate::gdiplus::{self, Graphics};

/// 形状填充的透明度（相对 `color.a`）。
///
/// 微信的矩形/椭圆标注是"半透明填充 + 实心描边"，纯不透明填充会把被标注的内容挡住。
const FILL_ALPHA_NUM: u32 = 78; // ~30%
const FILL_ALPHA_DEN: u32 = 255;

/// 把画布上的全部标注渲染到图像上，返回新图（不修改入参）。
///
/// 1. **马赛克**先在像素层处理：对 base 调 `RgbaImage::pixelate(rect, block)`。
/// 2. 其余标注（矩形/椭圆/箭头/画笔/文字）用 GDI+ 画到结果图上。
/// 3. 返回的图尺寸与 `base` 一致。
pub fn render_annotations(base: &RgbaImage, canvas: &Canvas) -> RgbaImage {
    // ---- 1. 马赛克（像素层） ----
    let mut img = base.clone();
    for a in &canvas.items {
        if let Annotation::Mosaic { rect, block } = a {
            img.pixelate(*rect, *block);
        }
    }

    let (w, h) = (img.w as i32, img.h as i32);
    if w <= 0 || h <= 0 {
        return img;
    }
    if gdiplus::startup().is_err() {
        return img;
    }

    // ---- 2. 矢量标注（GDI+） ----
    let bmp = match gdiplus::Bitmap::new(w, h) {
        Ok(b) => b,
        Err(_) => return img,
    };
    {
        let mut d = match bmp.lock() {
            Ok(d) => d,
            Err(_) => return img,
        };
        let px = img.to_bgra_premultiplied();
        let n = px.len().min((d.Stride as usize) * h as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(px.as_ptr(), d.Scan0 as *mut u8, n);
        }
        bmp.unlock(&mut d);
    }
    {
        let mut g = match bmp.graphics() {
            Ok(g) => g,
            Err(_) => return img,
        };
        g.setup_quality();
        paint_annotations(&mut g, canvas, Point::new(0, 0));
    }

    // ---- 3. 读回 ----
    let mut d = match bmp.lock() {
        Ok(d) => d,
        Err(_) => return img,
    };
    let mut out = RgbaImage::new(w as u32, h as u32);
    let stride = d.Stride as usize;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = y * stride + x * 4;
            let px = unsafe {
                (
                    *((d.Scan0 as *const u8).add(i + 2)),
                    *((d.Scan0 as *const u8).add(i + 1)),
                    *((d.Scan0 as *const u8).add(i)),
                    *((d.Scan0 as *const u8).add(i + 3)),
                )
            };
            out.set_pixel(x as u32, y as u32, unpremultiply(px.0, px.1, px.2, px.3));
        }
    }
    bmp.unlock(&mut d);
    out
}

/// 把矢量标注画到 GDI+ 画布上。
///
/// `offset` 是绘制坐标相对标注坐标的**平移量**：浮窗预览时传"选区在屏幕上的位置"，
/// 让"标注（选区内相对坐标）"落到"屏幕绝对坐标"上。
pub(crate) fn paint_annotations(g: &mut Graphics, canvas: &Canvas, offset: Point) {
    for a in &canvas.items {
        match a {
            Annotation::Shape {
                kind,
                rect,
                color,
                width,
            } => {
                let r = shift_rect(*rect, offset);
                let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
                let c = gdiplus::argb(*color);
                let fill = gdiplus::argb(fill_color(*color));
                match kind {
                    ShapeKind::Rect => {
                        g.fill_rect(x, y, w, h, fill);
                        g.draw_rect(x, y, w, h, c, *width);
                    }
                    ShapeKind::Ellipse => {
                        g.fill_ellipse(x, y, w, h, fill);
                        g.draw_ellipse(x, y, w, h, c, *width);
                    }
                }
            }
            Annotation::Arrow {
                from,
                to,
                color,
                width,
            } => {
                let f = shift_point(*from, offset);
                let t = shift_point(*to, offset);
                let c = gdiplus::argb(*color);
                g.draw_line(f.x as f32, f.y as f32, t.x as f32, t.y as f32, c, *width);
                // 箭头头部
                let dx = (t.x - f.x) as f32;
                let dy = (t.y - f.y) as f32;
                let len = (dx * dx + dy * dy).sqrt();
                if len > 1.0 {
                    let ang = dy.atan2(dx);
                    let hl = (width * 4.0).max(10.0).min(len);
                    for s in [1.0f32, -1.0] {
                        let a = ang + s * 2.62; // 约 ±150°
                        g.draw_line(
                            t.x as f32,
                            t.y as f32,
                            t.x as f32 + a.cos() * hl,
                            t.y as f32 + a.sin() * hl,
                            c,
                            *width,
                        );
                    }
                }
            }
            Annotation::Pen {
                points,
                color,
                width,
            } => {
                let c = gdiplus::argb(*color);
                for w in points.windows(2) {
                    let a = shift_point(w[0], offset);
                    let b = shift_point(w[1], offset);
                    g.draw_line(a.x as f32, a.y as f32, b.x as f32, b.y as f32, c, *width);
                }
                if points.len() == 1 {
                    let a = shift_point(points[0], offset);
                    g.fill_ellipse(
                        a.x as f32 - width / 2.0,
                        a.y as f32 - width / 2.0,
                        *width,
                        *width,
                        c,
                    );
                }
            }
            Annotation::Text {
                at,
                text,
                color,
                size,
            } => {
                // at.y 是文字**顶部**
                let p = shift_point(*at, offset);
                g.draw_text(
                    p.x as f32,
                    p.y as f32,
                    text,
                    *size,
                    gdiplus::argb(*color),
                    false,
                );
            }
            Annotation::Mosaic { .. } => {
                // 已经在像素层处理过了
            }
        }
    }
}

/// 半透明填充色。
fn fill_color(c: Rgba) -> Rgba {
    Rgba {
        a: ((c.a as u32 * FILL_ALPHA_NUM) / FILL_ALPHA_DEN) as u8,
        ..c
    }
}

#[inline]
fn shift_rect(r: Rect, o: Point) -> Rect {
    Rect::new(r.x + o.x, r.y + o.y, r.w, r.h)
}

#[inline]
fn shift_point(p: Point, o: Point) -> Point {
    Point::new(p.x + o.x, p.y + o.y)
}

/// 预乘 → 非预乘。
pub(crate) fn unpremultiply(r: u8, g: u8, b: u8, a: u8) -> Rgba {
    if a == 0 {
        return Rgba::TRANSPARENT;
    }
    if a == 255 {
        return Rgba { r, g, b, a };
    }
    let f = 255.0 / a as f32;
    Rgba {
        r: (r as f32 * f).round().min(255.0) as u8,
        g: (g as f32 * f).round().min(255.0) as u8,
        b: (b as f32 * f).round().min(255.0) as u8,
        a,
    }
}

/// PARGB 字节（预乘 BGRA）→ RgbaImage（非预乘 RGBA）。
pub(crate) fn pargb_to_rgba(bytes: &[u8], w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 4;
            if i + 3 >= bytes.len() {
                break;
            }
            let b = bytes[i];
            let g = bytes[i + 1];
            let r = bytes[i + 2];
            let a = bytes[i + 3];
            out.set_pixel(x as u32, y as u32, unpremultiply(r, g, b, a));
        }
    }
    out
}
