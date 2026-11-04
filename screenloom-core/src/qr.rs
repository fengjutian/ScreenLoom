//! 二维码识别。
//!
//! 基于 [`rqrr`](https://docs.rs/rqrr)（纯 Rust 解码器，无 C 依赖、无 Win32 依赖）。
//!
//! 输入是本 crate 自己的 [`RgbaImage`]（RGBA8 行优先缓冲），**不是** `image` crate 的类型：
//! 先按 ITU-R BT.601 灰度化，再交给 rqrr 做「定位 → 解码」。
//!
//! rqrr 只有解码器没有编码器，所以单元测试里的二维码图样是用 Python `segno`
//! 生成后以 ASCII art（`'  '` = 白、`'##'` = 黑）内嵌在本文件下方的测试模块里。

use crate::image::{Rgba, RgbaImage};

/// 识别前允许的最大边长（像素）。
///
/// 超过这个尺寸的图会先做 box 降采样再识别，避免大图卡顿（截图可能是 4K 屏）。
pub const MAX_DETECT_SIDE: u32 = 4000;

/// 二维码识别结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode {
    /// 解码出的文本内容
    pub text: String,
}

impl QrCode {
    /// 文本是否像 URL（http/https 开头，忽略大小写）。
    pub fn is_url(&self) -> bool {
        let b = self.text.trim().as_bytes();
        let starts = |p: &[u8]| b.len() >= p.len() && b[..p.len()].eq_ignore_ascii_case(p);
        starts(b"http://") || starts(b"https://")
    }
}

/// 在图像中查找并解码二维码；找不到返回 None。
pub fn detect_qr(img: &RgbaImage) -> Option<QrCode> {
    detect_qrs(img).into_iter().next()
}

/// 查找并解码图像中的所有二维码（按检测顺序）。
pub fn detect_qrs(img: &RgbaImage) -> Vec<QrCode> {
    if img.w == 0 || img.h == 0 {
        return Vec::new();
    }
    let (w, h, gray) = to_grayscale(img);
    // rqrr 的 `prepare_from_greyscale` 会自己做自适应二值化，这里只喂灰度值。
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| gray[y * w + x]);
    let grids = prepared.detect_grids();

    let mut out = Vec::with_capacity(grids.len());
    for grid in &grids {
        // 解码可能失败（纠错失败 / 非 UTF-8 字节），失败就跳过这个码。
        if let Ok((_meta, text)) = grid.decode() {
            out.push(QrCode { text });
        }
    }
    out
}

/// ITU-R BT.601 亮度：`(r*299 + g*587 + b*114) / 1000`。
#[inline]
fn luma(r: u8, g: u8, b: u8) -> u8 {
    let v = r as u32 * 299 + g as u32 * 587 + b as u32 * 114;
    (v / 1000) as u8
}

/// RGBA8 → 灰度；边长超过 [`MAX_DETECT_SIDE`] 时先降采样。
fn to_grayscale(img: &RgbaImage) -> (usize, usize, Vec<u8>) {
    let w = img.w as usize;
    let h = img.h as usize;
    let mut gray = Vec::with_capacity(w * h);
    for p in img.data.chunks_exact(4) {
        gray.push(luma(p[0], p[1], p[2]));
    }
    if w.max(h) > MAX_DETECT_SIDE as usize {
        return downsample_gray(w, h, &gray);
    }
    (w, h, gray)
}

/// box 滤波降采样到最长边 [`MAX_DETECT_SIDE`]。
fn downsample_gray(w: usize, h: usize, gray: &[u8]) -> (usize, usize, Vec<u8>) {
    let max_side = w.max(h).max(1) as u64;
    let target = MAX_DETECT_SIDE as u64;
    let nw = (((w as u64) * target) / max_side).max(1) as usize;
    let nh = (((h as u64) * target) / max_side).max(1) as usize;
    let mut out = vec![0u8; nw * nh];
    for y in 0..nh {
        let sy0 = (y * h) / nh;
        let sy1 = ((((y + 1) * h) / nh).max(sy0 + 1)).min(h);
        for x in 0..nw {
            let sx0 = (x * w) / nw;
            let sx1 = ((((x + 1) * w) / nw).max(sx0 + 1)).min(w);
            let mut sum = 0u64;
            let mut n = 0u64;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    sum += gray[sy * w + sx] as u64;
                    n += 1;
                }
            }
            out[y * nw + x] = if n > 0 { (sum / n) as u8 } else { 0 };
        }
    }
    (nw, nh, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 ASCII art 矩阵光栅化成图片：每模块 `scale` 像素，外加 `quiet` 模块的静默边距。
    ///
    /// 行尾多余的空白（白色模块）即使被裁掉也不影响结果：缺省按白处理。
    fn rasterize(rows: &[&str], scale: u32, quiet: u32) -> RgbaImage {
        let row_modules = rows.iter().map(|r| r.len() / 2).max().unwrap_or(0);
        let modules = (rows.len() as u32).max(row_modules as u32);
        let side = (modules + quiet * 2) * scale;
        let mut img = RgbaImage::new(side, side);
        for y in 0..side {
            for x in 0..side {
                img.set_pixel(x, y, Rgba::WHITE);
            }
        }
        for (my, row) in rows.iter().enumerate() {
            for (mx, chunk) in row.as_bytes().chunks(2).enumerate() {
                if chunk != b"##" {
                    continue;
                }
                let px = (quiet + mx as u32) * scale;
                let py = (quiet + my as u32) * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        img.set_pixel(px + dx, py + dy, Rgba::BLACK);
                    }
                }
            }
        }
        img
    }

    // --- segno 生成的真实二维码图样（segno 1.6.6，纯 Python） ---

    /// segno.make("https://example.com/screenloom")，version=2 error=L，25x25 模块。
    const QR_URL: &[&str] = &[
        "##############  ##  ##      ##      ##############",
        "##          ##      ##          ##  ##          ##",
        "##  ######  ##    ##  ##  ####  ##  ##  ######  ##",
        "##  ######  ##      ########    ##  ##  ######  ##",
        "##  ######  ##    ##      ####  ##  ##  ######  ##",
        "##          ##      ####    ##  ##  ##          ##",
        "##############  ##  ##  ##  ##  ##  ##############",
        "                ##    ##  ##  ####                ",
        "####  ####  ##    ########      ##  ##          ##",
        "      ##  ##  ##########  ##########  ##########  ",
        "  ##  ##    ##      ########  ######  ######    ##",
        "##  ##  ##    ##########    ##    ######  ########",
        "  ####    ####  ####    ##    ####  ####        ##",
        "####      ##          ##  ##  ####      ##    ##  ",
        "########  ######  ##    ##    ####  ##  ##########",
        "##    ####    ##########          ######  ####  ##",
        "##    ##    ####  ##    ####    ##########  ####  ",
        "                ##              ##      ##  ####  ",
        "##############        ##    ##  ##  ##  ##      ##",
        "##          ##            ##  ####      ##        ",
        "##  ######  ##  ##  ##  ##  ##############    ##  ",
        "##  ######  ##  ####  ##        ######        ####",
        "##  ######  ##      ####  ######  ##    ##########",
        "##          ##  ##  ####      ##      ####  ######",
        "##############  ##    ####      ##  ##    ##    ##",
    ];

    /// segno.make("微信截图 消除杂物")，version=2 error=M，25x25 模块（UTF-8 字节模式）。
    const QR_CN: &[&str] = &[
        "##############    ####    ##    ##  ##############",
        "##          ##  ####    ####    ##  ##          ##",
        "##  ######  ##            ##    ##  ##  ######  ##",
        "##  ######  ##      ##  ##          ##  ######  ##",
        "##  ######  ##  ######      ####    ##  ######  ##",
        "##          ##    ####  ######      ##          ##",
        "##############  ##  ##  ##  ##  ##  ##############",
        "                    ####      ##                  ",
        "##  ##  ##  ##    ####  ####  ####      ##    ##  ",
        "##      ##    ##  ####  ####  ##  ####  ##  ######",
        "  ##    ########          ##  ########            ",
        "##  ####        ##########      ##    ####  ######",
        "      ####  ##  ##    ####  ##                ##  ",
        "    ##          ########    ##        ####      ##",
        "##  ####  ######        ##    ####      ##  ##    ",
        "  ######      ####    ##    ##  ##  ##########    ",
        "##      ##  ##          ##  ##  ############  ####",
        "                ########  ##  ####      ##  ######",
        "##############    ####    ########  ##  ####  ##  ",
        "##          ##    ##  ##        ##      ######    ",
        "##  ######  ##  ####  ######  ############        ",
        "##  ######  ##    ##      ##  ##  ######  ##      ",
        "##  ######  ##  ##    ##  ##    ####      ####  ##",
        "##          ##    ##  ##    ######  ######  ####  ",
        "##############  ##########  ##  ##    ####    ####",
    ];

    const QR_URL_PAYLOAD: &str = "https://example.com/screenloom";
    const QR_CN_PAYLOAD: &str = "微信截图 消除杂物";

    #[test]
    fn detects_url_qr_code() {
        let img = rasterize(QR_URL, 4, 4);
        let qr = detect_qr(&img).expect("应当识别出二维码");
        assert_eq!(qr.text, QR_URL_PAYLOAD);
        assert!(qr.is_url());
    }

    #[test]
    fn detects_all_qr_codes() {
        let img = rasterize(QR_URL, 4, 4);
        let all = detect_qrs(&img);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text, QR_URL_PAYLOAD);
    }

    #[test]
    fn detects_chinese_qr_code() {
        let img = rasterize(QR_CN, 4, 4);
        let qr = detect_qr(&img).expect("应当识别出二维码");
        assert_eq!(qr.text, QR_CN_PAYLOAD);
    }

    #[test]
    fn plain_image_has_no_qr() {
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.set_pixel(x, y, Rgba::rgb(200, 200, 200));
            }
        }
        assert!(detect_qr(&img).is_none());
        assert!(detect_qrs(&img).is_empty());
    }

    #[test]
    fn empty_image_has_no_qr() {
        let img = RgbaImage::new(0, 0);
        assert!(detect_qr(&img).is_none());
    }

    #[test]
    fn is_url_ignores_case_and_surrounding_space() {
        let mk = |t: &str| QrCode { text: t.to_string() };
        assert!(mk("https://example.com").is_url());
        assert!(mk("HTTP://example.com").is_url());
        assert!(mk("HttpS://example.com").is_url());
        assert!(mk("  https://example.com  ").is_url());
        assert!(!mk("ftp://example.com").is_url());
        assert!(!mk("example.com").is_url());
        assert!(!mk("").is_url());
        assert!(!mk("https:/example.com").is_url());
    }

    #[test]
    fn oversized_image_is_downsampled_without_panic() {
        // 边长 4001 > MAX_DETECT_SIDE，应触发降采样路径。
        let img = RgbaImage::new(4001, 12);
        assert!(detect_qr(&img).is_none());
    }
}
