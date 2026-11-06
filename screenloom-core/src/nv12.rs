//! BGRA → NV12 转换（纯 Rust，core 层不依赖任何平台 API）。
//!
//! # 为什么必须自己写
//!
//! Media Foundation 的 H.264 编码器**不吃 BGRA**。喂 `MFVideoFormat_RGB32` 时
//! `SetInputMediaType` 会**成功**（类型检查通过，很容易误判为可行），但编码器
//! **一帧都不吐**，`Finalize` 报 `0xC00D4A44`「操作失败，因为接收器未处理任何示例」。
//!
//! 证据与完整踩坑记录见 `doc/录屏方案.md` §4.1。
//!
//! # 色彩规格
//!
//! - **BT.709** 矩阵（HD 通用）
//! - **full range**（Y 0~255，U/V 128 为中性色）—— 这是本机编码器**唯一接受**的输入范围，
//!   填 limited(16~235) 或 unknown 会让整条编码链断掉（见 `doc/录屏方案.md` §4.3）。
//! - 色度按 **2×2 盒式平均**降采样（而不是点采样），避免边缘彩边。

use crate::error::{Error, Result};

/// 一个 NV12 图像（Y 平面 + UV 交错平面）。
///
/// 内存布局：`data[0 .. w*h]` 是 Y，之后 `w*h/2` 字节是 UV（每像素 U、V 交替）。
#[derive(Clone, Debug)]
pub struct Nv12Image {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Nv12Image {
    /// 新建一张 NV12 图像（内容全 0）。
    ///
    /// **宽高必须是正偶数** —— NV12 的色度是 2×2 降采样，奇数尺寸无法表达。
    ///
    /// 录制链路保证传进来的都是偶数（见
    /// `screenloom-app/src/record/source.rs` 的 `even_up` / `fit_frame`）；
    /// 这里再挡一道，防止别处漏了补边直接把整段录制搞崩。
    pub fn new(w: u32, h: u32) -> Result<Nv12Image> {
        if w == 0 || h == 0 {
            return Err(Error::Image(format!("NV12 尺寸非法: {w}x{h}")));
        }
        if w % 2 != 0 || h % 2 != 0 {
            return Err(Error::Image(format!(
                "NV12 宽高必须为偶数（色度 2×2 降采样），收到 {w}x{h}"
            )));
        }
        Ok(Nv12Image {
            w,
            h,
            data: vec![0u8; nv12_bytes_len(w, h)],
        })
    }

    /// Y 平面字节数（= `w * h`）。
    pub fn y_len(&self) -> usize {
        (self.w as usize) * (self.h as usize)
    }

    /// UV 平面字节数（= `w * h / 2`）。
    pub fn uv_len(&self) -> usize {
        self.y_len() / 2
    }

    /// Y 平面。
    pub fn y_plane(&self) -> &[u8] {
        &self.data[..self.y_len()]
    }

    /// UV 平面（U/V 交错）。
    pub fn uv_plane(&self) -> &[u8] {
        &self.data[self.y_len()..]
    }
}

/// NV12 一帧的字节数。
pub fn nv12_bytes_len(w: u32, h: u32) -> usize {
    (w as usize) * (h as usize) * 3 / 2
}

/// BGRA → NV12（分配新缓冲）。
///
/// `bgra` 是**内存序 B, G, R, A**（WGC 帧就是这个顺序），每像素 4 字节。
pub fn bgra_to_nv12(bgra: &[u8], w: u32, h: u32) -> Result<Nv12Image> {
    let mut img = Nv12Image::new(w, h)?;
    bgra_to_nv12_into(bgra, w, h, &mut img.data)?;
    Ok(img)
}

/// BGRA → NV12，写进**已分配**的缓冲（录屏时复用同一块缓冲，避免每帧分配）。
///
/// `out` 长度必须 ≥ [`nv12_bytes_len`]。越界或长度不对都返回错误，不会 panic。
pub fn bgra_to_nv12_into(bgra: &[u8], w: u32, h: u32, out: &mut [u8]) -> Result<()> {
    if w == 0 || h == 0 {
        return Err(Error::Image(format!("尺寸非法: {w}x{h}")));
    }
    if w % 2 != 0 || h % 2 != 0 {
        return Err(Error::Image(format!(
            "NV12 宽高必须为偶数（色度 2×2 降采样），收到 {w}x{h}"
        )));
    }
    let px = (w as usize) * (h as usize);
    if bgra.len() < px * 4 {
        return Err(Error::Image(format!(
            "BGRA 缓冲过短: 需要 {} 字节，收到 {}",
            px * 4,
            bgra.len()
        )));
    }
    let need = nv12_bytes_len(w, h);
    if out.len() < need {
        return Err(Error::Image(format!(
            "NV12 输出缓冲过短: 需要 {need} 字节，收到 {}",
            out.len()
        )));
    }

    let wi = w as usize;
    let hi = h as usize;
    // `out` 可能比所需长度长（复用缓冲），所以用 split_at_mut 而不是两个独立切片。
    let (y_plane, uv_plane) = out.split_at_mut(px);

    // 一次遍历 2×2 块：算 4 个 Y + 1 组 Cb/Cr。
    for by in (0..hi).step_by(2) {
        for bx in (0..wi).step_by(2) {
            let p00 = px_at(bgra, bx, by, wi);
            let p10 = px_at(bgra, bx + 1, by, wi);
            let p01 = px_at(bgra, bx, by + 1, wi);
            let p11 = px_at(bgra, bx + 1, by + 1, wi);

            y_plane[by * wi + bx] = luma(p00);
            y_plane[by * wi + bx + 1] = luma(p10);
            y_plane[(by + 1) * wi + bx] = luma(p01);
            y_plane[(by + 1) * wi + bx + 1] = luma(p11);

            // 2×2 盒式平均（整数除法，误差 <1/4，不影响观感）
            let r = (p00.0 as u32 + p10.0 as u32 + p01.0 as u32 + p11.0 as u32) / 4;
            let g = (p00.1 as u32 + p10.1 as u32 + p01.1 as u32 + p11.1 as u32) / 4;
            let b = (p00.2 as u32 + p10.2 as u32 + p01.2 as u32 + p11.2 as u32) / 4;

            let (cb, cr) = chroma(r, g, b);
            let uv_row = (by / 2) * wi;
            let uv_col = (bx / 2) * 2;
            uv_plane[uv_row + uv_col] = cb;
            uv_plane[uv_row + uv_col + 1] = cr;
        }
    }
    Ok(())
}

/// 取一个像素的 (R, G, B)（输入是 BGRA 内存序，故 R 在索引 2）。
#[inline]
fn px_at(bgra: &[u8], x: usize, y: usize, stride: usize) -> (u8, u8, u8) {
    let i = (y * stride + x) * 4;
    (bgra[i + 2], bgra[i + 1], bgra[i])
}

/// BT.709 full-range 亮度：`Y = 0.2126R + 0.7152G + 0.0722B`。
#[inline]
fn luma(rgb: (u8, u8, u8)) -> u8 {
    let (r, g, b) = (rgb.0 as i32, rgb.1 as i32, rgb.2 as i32);
    // 定点数：系数 × 65536
    let y = (13929 * r + 46873 * g + 4732 * b + 32768) >> 16;
    y.clamp(0, 255) as u8
}

/// BT.709 full-range 色度（U=Cb, V=Cr，中性色 128）。
///
/// 定点系数的选取原则：**R/G/B 系数之和必须精确为 0**，
/// 这样纯白 (255,255,255) 的色度才会严格落在 128（中性），不会偏 1 LSB。
#[inline]
fn chroma(r: u32, g: u32, b: u32) -> (u8, u8) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    // Cb = -0.114572R - 0.385428G + 0.5B + 128   （-7508 - 25260 + 32768 = 0）
    let cb = (-7508 * r - 25260 * g + 32768 * b + (128 << 16) + 32768) >> 16;
    // Cr = 0.5R - 0.454153G - 0.045847B + 128     （32768 - 29765 - 3003 = 0）
    let cr = (32768 * r - 29765 * g - 3003 * b + (128 << 16) + 32768) >> 16;
    (cb.clamp(0, 255) as u8, cr.clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张纯色的 BGRA 图。
    fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            v.extend_from_slice(&[b, g, r, 255]); // BGRA 内存序
        }
        v
    }

    /// 取 (0,0) 的 Y / U / V。
    fn yuv0(img: &Nv12Image) -> (u8, u8, u8) {
        let uv = img.uv_plane();
        (img.y_plane()[0], uv[0], uv[1])
    }

    fn near(a: u8, b: i32, tol: i32) -> bool {
        (a as i32 - b).abs() <= tol
    }

    #[test]
    fn gray_maps_to_neutral_chroma() {
        let img = bgra_to_nv12(&solid(4, 4, 128, 128, 128), 4, 4).unwrap();
        let (y, u, v) = yuv0(&img);
        assert!(near(y, 128, 1), "Y={y}");
        assert!(near(u, 128, 1), "U={u}");
        assert!(near(v, 128, 1), "V={v}");
    }

    #[test]
    fn black_and_white_are_extremes() {
        let black = bgra_to_nv12(&solid(2, 2, 0, 0, 0), 2, 2).unwrap();
        assert_eq!(black.y_plane()[0], 0);
        assert_eq!(black.uv_plane()[0], 128);
        assert_eq!(black.uv_plane()[1], 128);

        let white = bgra_to_nv12(&solid(2, 2, 255, 255, 255), 2, 2).unwrap();
        assert_eq!(white.y_plane()[0], 255);
        assert_eq!(white.uv_plane()[0], 128);
        assert_eq!(white.uv_plane()[1], 128);
    }

    #[test]
    fn primary_colors_match_bt709() {
        // 红: Y≈54, Cb≈99, Cr≈255
        let red = bgra_to_nv12(&solid(2, 2, 255, 0, 0), 2, 2).unwrap();
        let (y, u, v) = yuv0(&red);
        assert!(near(y, 54, 1), "红 Y={y}");
        assert!(near(u, 99, 1), "红 U={u}");
        assert!(near(v, 255, 1), "红 V={v}");

        // 绿: Y≈182, Cb≈30, Cr≈12
        let green = bgra_to_nv12(&solid(2, 2, 0, 255, 0), 2, 2).unwrap();
        let (y, u, v) = yuv0(&green);
        assert!(near(y, 182, 1), "绿 Y={y}");
        assert!(near(u, 30, 1), "绿 U={u}");
        assert!(near(v, 12, 1), "绿 V={v}");

        // 蓝: Y≈18, Cb≈255, Cr≈116
        let blue = bgra_to_nv12(&solid(2, 2, 0, 0, 255), 2, 2).unwrap();
        let (y, u, v) = yuv0(&blue);
        assert!(near(y, 18, 1), "蓝 Y={y}");
        assert!(near(u, 255, 1), "蓝 U={u}");
        assert!(near(v, 116, 1), "蓝 V={v}");
    }

    #[test]
    fn layout_is_y_then_interleaved_uv() {
        // 4×2：左半红、右半蓝（**按行构造**：row0 = 红红蓝蓝，row1 = 红红蓝蓝）
        let mut buf = Vec::with_capacity(4 * 2 * 4);
        for _ in 0..2 {
            buf.extend_from_slice(&solid(2, 1, 255, 0, 0));
            buf.extend_from_slice(&solid(2, 1, 0, 0, 255));
        }
        let img = bgra_to_nv12(&buf, 4, 2).unwrap();
        assert_eq!(img.y_len(), 8);
        assert_eq!(img.uv_len(), 4);
        assert_eq!(img.data.len(), nv12_bytes_len(4, 2));

        // 第一行 Y：红红蓝蓝
        assert_eq!(&img.y_plane()[0..4], &[54, 54, 18, 18]);
        // UV 行 0：红块的 U,V 然后蓝块的 U,V
        let uv = img.uv_plane();
        assert!(near(uv[0], 99, 1), "uv[0]={}", uv[0]);
        assert!(near(uv[1], 255, 1), "uv[1]={}", uv[1]);
        assert!(near(uv[2], 255, 1), "uv[2]={}", uv[2]);
        assert!(near(uv[3], 116, 1), "uv[3]={}", uv[3]);
    }

    #[test]
    fn rejects_odd_dimensions() {
        assert!(bgra_to_nv12(&solid(3, 2, 1, 1, 1), 3, 2).is_err());
        assert!(bgra_to_nv12(&solid(2, 3, 1, 1, 1), 2, 3).is_err());
        assert!(bgra_to_nv12(&solid(0, 0, 0, 0, 0), 0, 0).is_err());
    }

    #[test]
    fn rejects_short_buffers() {
        // 输入太短
        assert!(bgra_to_nv12(&[0u8; 10], 4, 4).is_err());
        // 输出太短
        let mut out = vec![0u8; 8];
        assert!(bgra_to_nv12_into(&solid(4, 4, 0, 0, 0), 4, 4, &mut out).is_err());
    }

    #[test]
    fn into_variant_matches_allocating_variant() {
        let src = solid(8, 4, 12, 200, 77);
        let a = bgra_to_nv12(&src, 8, 4).unwrap();
        let mut buf = vec![0u8; nv12_bytes_len(8, 4)];
        bgra_to_nv12_into(&src, 8, 4, &mut buf).unwrap();
        assert_eq!(a.data, buf);
    }
}
