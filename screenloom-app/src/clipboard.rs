//! 剪贴板写入。
//!
//! **必须同时写 `CF_PNG` 与 `CF_DIB`**：只写 CF_PNG 时部分应用（Office、部分 IM）
//! 粘贴出空白；只写 CF_DIB 时丢失 alpha。两者都写最稳。

use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};

use screenloom_core::{CopyFormat, Error, Result, RgbaImage};

/// `CF_PNG` 的自定义剪贴板格式 ID。
fn png_format() -> u32 {
    // SAFETY: 常量字符串，注册操作无副作用。
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

/// 把图像写入剪贴板。
pub fn write_image(img: &RgbaImage, format: CopyFormat) -> Result<()> {
    // SAFETY: OpenClipboard 需要独占剪贴板；失败即返回错误，不 panic。
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(Error::Window("打开剪贴板失败（可能被其它程序占用）".into()));
        }
        let _guard = ClipboardGuard;
        let _ = EmptyClipboard();

        if matches!(format, CopyFormat::Png | CopyFormat::Both) {
            let png = img.encode_png()?;
            set_bytes(png_format(), &png)?;
        }
        if matches!(format, CopyFormat::Dib | CopyFormat::Both) {
            let dib = to_dib(img);
            set_bytes(CF_DIB.0 as u32, &dib)?;
        }
    }
    Ok(())
}

/// 关闭剪贴板（Drop 保证异常路径也会关闭）。
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: 与 OpenClipboard 配对。
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// 写入一段字节到剪贴板。
fn set_bytes(format: u32, data: &[u8]) -> Result<()> {
    // SAFETY: 分配全局内存、拷贝、交给系统接管所有权。
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, data.len())
            .map_err(|e| Error::Window(format!("分配剪贴板内存失败: {e}")))?;
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            return Err(Error::Window("锁定剪贴板内存失败".into()));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        let _ = GlobalUnlock(hmem);

        // 成功后所有权转移给系统，不再 GlobalFree
        SetClipboardData(format, Some(HANDLE(hmem.0)))
            .map_err(|e| Error::Window(format!("写入剪贴板失败: {e}")))?;
    }
    Ok(())
}

/// 生成 `CF_DIB` 负载：BITMAPINFOHEADER + 自底向上的 BGRA 像素。
fn to_dib(img: &RgbaImage) -> Vec<u8> {
    const HEADER: usize = 40;
    let w = img.w as i32;
    let h = img.h as i32;
    let stride = w * 4;
    let pixel_bytes = (stride * h) as usize;

    let mut out = vec![0u8; HEADER + pixel_bytes];

    // BITMAPINFOHEADER
    out[0..4].copy_from_slice(&(HEADER as u32).to_le_bytes());
    out[4..8].copy_from_slice(&w.to_le_bytes());
    out[8..12].copy_from_slice(&h.to_le_bytes()); // 正数 = 自底向上
    out[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
    out[14..16].copy_from_slice(&32u16.to_le_bytes()); // bit count
    out[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes());
    // 其余字段（分辨率、颜色数）保持 0

    // 像素：自底向上，每行 4 字节对齐（w*4 天然对齐）
    for row in 0..img.h {
        let src_y = img.h - 1 - row;
        let src_off = (src_y as usize * img.w as usize) * 4;
        let dst_off = HEADER + (row as usize * img.w as usize) * 4;
        for x in 0..img.w as usize {
            let s = src_off + x * 4;
            let d = dst_off + x * 4;
            out[d] = img.data[s + 2]; // B
            out[d + 1] = img.data[s + 1]; // G
            out[d + 2] = img.data[s]; // R
            out[d + 3] = 255; // A 忽略
        }
    }
    out
}

/// 写入纯文本到剪贴板（`CF_UNICODETEXT`）。
pub fn write_text(text: &str) -> Result<()> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0); // NUL 终止
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    // SAFETY: OpenClipboard 成对 CloseClipboard；字节缓冲区在调用期间有效。
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(Error::Window("打开剪贴板失败（可能被其它程序占用）".into()));
        }
        let _guard = ClipboardGuard;
        let _ = EmptyClipboard();
        set_bytes(CF_UNICODETEXT.0 as u32, bytes)?;
    }
    Ok(())
}

/// 读取剪贴板中的 PNG 字节（用于测试与自检）。
pub fn read_png() -> Result<Vec<u8>> {
    // SAFETY: 纯读取，失败返回空。
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(Error::Window("打开剪贴板失败".into()));
        }
        let _guard = ClipboardGuard;
        let fmt = png_format();
        let Ok(handle) = GetClipboardData(fmt) else {
            return Err(Error::Window("剪贴板中没有 PNG 数据".into()));
        };
        let hglobal = HGLOBAL(handle.0);
        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            return Err(Error::Window("锁定剪贴板内存失败".into()));
        }
        let size = GlobalSize(hglobal);
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size as usize).to_vec();
        let _ = GlobalUnlock(hglobal);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenloom_core::Rgba;

    #[test]
    fn dib_header_and_pixels() {
        let mut img = RgbaImage::new(2, 2);
        img.set_pixel(0, 0, Rgba::rgb(10, 20, 30));
        let dib = to_dib(&img);
        assert_eq!(&dib[0..4], &40u32.to_le_bytes());
        assert_eq!(&dib[4..8], &2i32.to_le_bytes());
        assert_eq!(&dib[8..12], &2i32.to_le_bytes());
        assert_eq!(&dib[14..16], &32u16.to_le_bytes());
        // CF_DIB 是自底向上：第 0 行是图像的**最后一行**（这里是黑色）
        assert_eq!(&dib[40..44], &[0u8, 0, 0, 255]);
        // 图像的第 0 行落在 DIB 的最后一行（行距 = w*4 = 8）
        let last = &dib[48..52];
        assert_eq!(last[0], 30); // B
        assert_eq!(last[1], 20); // G
        assert_eq!(last[2], 10); // R
    }
}
