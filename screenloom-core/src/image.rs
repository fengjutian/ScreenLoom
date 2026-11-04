//! 颜色与像素缓冲。
//!
//! 内部统一 **RGBA8 紧凑排列**（非预乘）。需要给 Win32 用时再由
//! [`RgbaImage::to_bgra_premultiplied`] 转换，避免各处重复处理预乘。

use crate::error::Result;

/// 一个像素的颜色（RGBA，非预乘）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const RED: Rgba = Rgba {
        r: 250,
        g: 81,
        b: 81,
        a: 255,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// 从 Win32 `COLORREF`(0x00BBGGRR) 构造。
    pub fn from_colorref(v: u32) -> Self {
        Rgba {
            r: (v & 0xFF) as u8,
            g: ((v >> 8) & 0xFF) as u8,
            b: ((v >> 16) & 0xFF) as u8,
            a: 255,
        }
    }

    /// 转为 Win32 `COLORREF`(0x00BBGGRR)。
    pub fn to_colorref(self) -> u32 {
        ((self.b as u32) << 16) | ((self.g as u32) << 8) | (self.r as u32)
    }

    /// `#RRGGBB` 解析。
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Rgba::rgb(r, g, b))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// RGBA8 图像，行优先紧密排列。
#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    pub fn new(w: u32, h: u32) -> Self {
        RgbaImage {
            w,
            h,
            data: vec![0u8; (w as usize) * (h as usize) * 4],
        }
    }

    /// 从 BGRA 缓冲构造（WGC 帧是 BGRA，别搞反）。
    pub fn from_bgra(w: u32, h: u32, data: &[u8]) -> Self {
        let mut out = RgbaImage::new(w, h);
        let n = (w as usize) * (h as usize);
        let len = n.min(data.len() / 4) * 4;
        out.data[..len].copy_from_slice(&data[..len]);
        for px in out.data.chunks_exact_mut(4) {
            px.swap(0, 2); // BGR -> RGB
        }
        out
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.w || y >= self.h {
            return Rgba::TRANSPARENT;
        }
        let i = ((y as usize) * (self.w as usize) + x as usize) * 4;
        Rgba {
            r: self.data[i],
            g: self.data[i + 1],
            b: self.data[i + 2],
            a: self.data[i + 3],
        }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, c: Rgba) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = ((y as usize) * (self.w as usize) + x as usize) * 4;
        self.data[i] = c.r;
        self.data[i + 1] = c.g;
        self.data[i + 2] = c.b;
        self.data[i + 3] = c.a;
    }

    /// 裁剪（越界自动裁到边界）。
    pub fn crop(&self, r: crate::geometry::Rect) -> RgbaImage {
        let r = r.normalized();
        let x0 = r.x.max(0) as u32;
        let y0 = r.y.max(0) as u32;
        let x1 = r.right().min(self.w as i32) as u32;
        let y1 = r.bottom().min(self.h as i32) as u32;
        let w = x1.saturating_sub(x0);
        let h = y1.saturating_sub(y0);
        let mut out = RgbaImage::new(w, h);
        for y in 0..h {
            let src = (((y0 + y) as usize) * (self.w as usize) + x0 as usize) * 4;
            let dst = (y as usize) * (w as usize) * 4;
            let n = (w as usize) * 4;
            out.data[dst..dst + n].copy_from_slice(&self.data[src..src + n]);
        }
        out
    }

    /// 把 `src` 粘贴到本图的 `(x, y)` 偏移处（物理像素），越界部分自动裁掉。
    ///
    /// 用于多显示器合成：把每块显示器的图像贴到虚拟屏画布的对应位置。
    pub fn blit(&mut self, src: &RgbaImage, x: i32, y: i32) {
        // 目标与源的交集（源以 (0,0) 为起点，按 (x,y) 平移后与目标求交）。
        let sx0 = (0 - x).max(0);
        let sy0 = (0 - y).max(0);
        let dx0 = x.max(0);
        let dy0 = y.max(0);
        let w = (src.w as i32 - sx0).min(self.w as i32 - dx0);
        let h = (src.h as i32 - sy0).min(self.h as i32 - dy0);
        if w <= 0 || h <= 0 {
            return;
        }
        for row in 0..h {
            let s = (((sy0 + row) as usize) * (src.w as usize) + sx0 as usize) * 4;
            let d = (((dy0 + row) as usize) * (self.w as usize) + dx0 as usize) * 4;
            let n = (w as usize) * 4;
            self.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
        }
    }

    /// 马赛克：区域内按 `block` 边长做块平均。
    pub fn pixelate(&mut self, r: crate::geometry::Rect, block: u32) {
        let b = block.max(1) as i32;
        let r = r.normalized();
        let x0 = r.x.max(0);
        let y0 = r.y.max(0);
        let x1 = r.right().min(self.w as i32);
        let y1 = r.bottom().min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let mut by = y0;
        while by < y1 {
            let bh = b.min(y1 - by);
            let mut bx = x0;
            while bx < x1 {
                let bw = b.min(x1 - bx);
                let (mut sr, mut sg, mut sb, mut sa, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for yy in by..by + bh {
                    for xx in bx..bx + bw {
                        let p = self.pixel(xx as u32, yy as u32);
                        sr += p.r as u32;
                        sg += p.g as u32;
                        sb += p.b as u32;
                        sa += p.a as u32;
                        n += 1;
                    }
                }
                if n > 0 {
                    let div = n.max(1);
                    let avg = Rgba {
                        r: (sr / div) as u8,
                        g: (sg / div) as u8,
                        b: (sb / div) as u8,
                        a: (sa / div) as u8,
                    };
                    for yy in by..by + bh {
                        for xx in bx..bx + bw {
                            self.set_pixel(xx as u32, yy as u32, avg);
                        }
                    }
                }
                bx += bw;
            }
            by += bh;
        }
    }

    /// 转为 **预乘 alpha 的 BGRA**，供 `UpdateLayeredWindow` 使用。
    pub fn to_bgra_premultiplied(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.data.len()];
        for (i, px) in self.data.chunks_exact(4).enumerate() {
            let a = px[3] as u32;
            out[i * 4] = ((px[0] as u32 * a + 127) / 255) as u8; // B
            out[i * 4 + 1] = ((px[1] as u32 * a + 127) / 255) as u8; // G
            out[i * 4 + 2] = ((px[2] as u32 * a + 127) / 255) as u8; // R
            out[i * 4 + 3] = a as u8;
        }
        out
    }

    /// 转为 **非预乘 BGRA**（供 `SoftwareBitmap` 等 WinRT API 使用）。
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.data.len()];
        for (i, px) in self.data.chunks_exact(4).enumerate() {
            out[i * 4] = px[2]; // B
            out[i * 4 + 1] = px[1]; // G
            out[i * 4 + 2] = px[0]; // R
            out[i * 4 + 3] = px[3]; // A
        }
        out
    }

    /// 编码为 PNG。
    pub fn encode_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.w, self.h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header()?;
            writer.write_image_data(&self.data)?;
        }
        Ok(out)
    }

    /// 从 PNG 字节解码。
    pub fn decode_png(data: &[u8]) -> Result<RgbaImage> {
        let decoder = png::Decoder::new(std::io::Cursor::new(data));
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf)?;
        let mut img = RgbaImage::new(info.width, info.height);
        let channels = info.color_type.samples();
        let stride = info.width as usize * channels;
        for y in 0..info.height as usize {
            for x in 0..info.width as usize {
                let s = y * stride + x * channels;
                let d = (y * info.width as usize + x) * 4;
                let (r, g, b, a) = match info.color_type {
                    png::ColorType::Rgba => (buf[s], buf[s + 1], buf[s + 2], buf[s + 3]),
                    png::ColorType::Rgb => (buf[s], buf[s + 1], buf[s + 2], 255),
                    png::ColorType::Grayscale => (buf[s], buf[s], buf[s], 255),
                    png::ColorType::GrayscaleAlpha => (buf[s], buf[s], buf[s], buf[s + 1]),
                    png::ColorType::Indexed => (0, 0, 0, 255),
                };
                img.data[d] = r;
                img.data[d + 1] = g;
                img.data[d + 2] = b;
                img.data[d + 3] = a;
            }
        }
        Ok(img)
    }
}

/// 颜色取样：读取某点像素（取色器用）。
pub fn sample_color(img: &RgbaImage, x: i32, y: i32) -> Option<Rgba> {
    if x < 0 || y < 0 || x as u32 >= img.w || y as u32 >= img.h {
        return None;
    }
    Some(img.pixel(x as u32, y as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    #[test]
    fn bgra_conversion_swaps_channels() {
        let buf = vec![10u8, 20, 30, 255]; // B=10 G=20 R=30
        let img = RgbaImage::from_bgra(1, 1, &buf);
        let p = img.pixel(0, 0);
        assert_eq!((p.r, p.g, p.b), (30, 20, 10));
    }

    #[test]
    fn crop_clips_to_bounds() {
        let mut img = RgbaImage::new(10, 10);
        for y in 0..10 {
            for x in 0..10 {
                img.set_pixel(x, y, Rgba::rgb(x as u8, y as u8, 0));
            }
        }
        let c = img.crop(Rect::new(5, 5, 100, 100));
        assert_eq!((c.w, c.h), (5, 5));
        assert_eq!(c.pixel(0, 0).r, 5);
    }

    #[test]
    fn pixelate_averages_block() {
        let mut img = RgbaImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, Rgba::rgb(100, 100, 100));
            }
        }
        img.pixelate(Rect::new(0, 0, 4, 4), 2);
        assert_eq!(img.pixel(0, 0).r, 100);
        assert_eq!(img.pixel(3, 3).r, 100);
    }

    #[test]
    fn blit_pastes_at_offset_and_clips() {
        let mut src = RgbaImage::new(2, 2);
        src.set_pixel(0, 0, Rgba::rgb(1, 1, 1));
        src.set_pixel(1, 0, Rgba::rgb(2, 2, 2));
        src.set_pixel(0, 1, Rgba::rgb(3, 3, 3));
        src.set_pixel(1, 1, Rgba::rgb(4, 4, 4));

        let mut dst = RgbaImage::new(4, 4);
        dst.blit(&src, 2, 1);
        assert_eq!(dst.pixel(2, 1), Rgba::rgb(1, 1, 1));
        assert_eq!(dst.pixel(3, 1), Rgba::rgb(2, 2, 2));
        assert_eq!(dst.pixel(2, 2), Rgba::rgb(3, 3, 3));
        assert_eq!(dst.pixel(3, 2), Rgba::rgb(4, 4, 4));
        // 目标之外不受影响（新图的空白像素 alpha 为 0）
        assert_eq!(dst.pixel(1, 0), Rgba::TRANSPARENT);

        // 负偏移：源的左侧一列落在目标外被裁掉
        // 源(1,0)=2 → 目标(0,0)；源(1,1)=4 → 目标(0,1)
        let mut dst2 = RgbaImage::new(4, 4);
        dst2.blit(&src, -1, 0);
        assert_eq!(dst2.pixel(0, 0), Rgba::rgb(2, 2, 2));
        assert_eq!(dst2.pixel(0, 1), Rgba::rgb(4, 4, 4));
        assert_eq!(dst2.pixel(1, 0), Rgba::TRANSPARENT);
    }

    #[test]
    fn png_roundtrip_preserves_pixels() {
        let mut img = RgbaImage::new(3, 2);
        img.set_pixel(1, 1, Rgba::rgb(10, 20, 30));
        let bytes = img.encode_png().unwrap();
        let back = RgbaImage::decode_png(&bytes).unwrap();
        assert_eq!((back.w, back.h), (3, 2));
        assert_eq!(back.pixel(1, 1), Rgba::rgb(10, 20, 30));
    }

    #[test]
    fn premultiply_darkens_color() {
        let img = RgbaImage::from_bgra(1, 1, &[10, 20, 30, 128]);
        let out = img.to_bgra_premultiplied();
        assert_eq!(out[3], 128);
        assert!(out[2] < 30); // R 被预乘压暗
    }
}
