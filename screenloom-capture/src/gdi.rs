//! GDI 兜底后端：`BitBlt` 抓屏、`PrintWindow` 抓窗。
//!
//! 兼容性最好：只要没有专门的硬件加速路径就用它。缺点是抓不到被遮挡 / 最小化的窗口。

use std::ffi::c_void;
use std::ptr;

use screenloom_core::{Error, Rect, Result, RgbaImage};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::PW_RENDERFULLCONTENT;

use crate::monitors::{list_monitors, window_rect_physical};
use crate::{CaptureBackend, CaptureFrame, CaptureTarget};

/// GDI 抓屏后端。
#[derive(Default)]
pub struct GdiBackend;

impl GdiBackend {
    /// 创建后端。GDI 在 Windows 上总是可用，因此不会失败。
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

impl CaptureBackend for GdiBackend {
    fn capture(&self, target: CaptureTarget) -> Result<CaptureFrame> {
        match target {
            CaptureTarget::Monitor(index) => capture_monitor(index),
            CaptureTarget::Window(hwnd) => capture_window(hwnd),
        }
    }
}

/// 抓取指定显示器。
pub(crate) fn capture_monitor(index: usize) -> Result<CaptureFrame> {
    let monitors = list_monitors()?;
    let info = monitors.get(index).ok_or_else(|| {
        Error::Capture(format!(
            "显示器下标越界: {index}（本机共 {} 个显示器）",
            monitors.len()
        ))
    })?;
    let image = blit_screen_rect(info.bounds)?;
    Ok(CaptureFrame {
        image,
        bounds: info.bounds,
    })
}

/// 抓取窗口：先 `PrintWindow`，失败再退回 `BitBlt`（只能抓到可见部分）。
fn capture_window(hwnd: HWND) -> Result<CaptureFrame> {
    let bounds = window_rect_physical(hwnd)?;

    // SAFETY: 句柄已由 window_rect_physical 校验过存在性。
    unsafe {
        let screen = GetDC(None);
        if screen.0.is_null() {
            return Err(Error::Capture("GetDC(NULL) 失败".into()));
        }
        let _screen = DcGuard::screen(screen);

        let mem = CreateCompatibleDC(Some(screen));
        if mem.0.is_null() {
            return Err(Error::Capture("CreateCompatibleDC 失败".into()));
        }
        let _mem = DcGuard::memory(mem);

        let (bitmap, bits) = create_dib(mem, bounds.w, bounds.h)?;
        let _bitmap = BitmapGuard::new(mem, bitmap);

        // 1) PrintWindow：能拿到被遮挡窗口的内容。
        if PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() {
            if let Ok(image) = read_dib(bits, bounds.w, bounds.h) {
                return Ok(CaptureFrame { image, bounds });
            }
        }

        // 2) 退化到 BitBlt：只能抓到屏幕上可见的部分。
        let (sx, sy) = screen_dc_source(bounds.x, bounds.y);
        if BitBlt(mem, 0, 0, bounds.w, bounds.h, Some(screen), sx, sy, SRCCOPY).is_err() {
            return Err(Error::Capture(format!(
                "PrintWindow 与 BitBlt 均失败（窗口 {:?} 可能受保护或已销毁）",
                hwnd.0
            )));
        }
        let image = read_dib(bits, bounds.w, bounds.h)?;
        Ok(CaptureFrame { image, bounds })
    }
}

/// 用 `BitBlt` 抓取虚拟屏幕坐标系里的某个矩形。
///
/// 源点直接用虚拟屏坐标（可能为负），因为屏幕 DC 的原点就是主显示器左上角，
/// 左/上方的副屏用负坐标直接可达。整块虚拟屏（`virtual_screen_bounds`）也在
/// 这个坐标系里，所以一次 BitBlt 就能拿到跨屏画面。
pub(crate) fn capture_screen_rect(bounds: Rect) -> Result<RgbaImage> {
    blit_screen_rect(bounds)
}

fn blit_screen_rect(bounds: Rect) -> Result<RgbaImage> {
    // SAFETY: DC / DIB 的生命周期都由下面的 guard 保证。
    unsafe {
        let screen = GetDC(None);
        if screen.0.is_null() {
            return Err(Error::Capture("GetDC(NULL) 失败".into()));
        }
        let _screen = DcGuard::screen(screen);

        let mem = CreateCompatibleDC(Some(screen));
        if mem.0.is_null() {
            return Err(Error::Capture("CreateCompatibleDC 失败".into()));
        }
        let _mem = DcGuard::memory(mem);

        let (bitmap, bits) = create_dib(mem, bounds.w, bounds.h)?;
        let _bitmap = BitmapGuard::new(mem, bitmap);

        let (sx, sy) = screen_dc_source(bounds.x, bounds.y);
        if BitBlt(mem, 0, 0, bounds.w, bounds.h, Some(screen), sx, sy, SRCCOPY).is_err() {
            return Err(Error::Capture(format!(
                "BitBlt 失败（目标 {}x{} @ {},{}）",
                bounds.w, bounds.h, sx, sy
            )));
        }
        read_dib(bits, bounds.w, bounds.h)
    }
}

/// 虚拟屏幕坐标 → 屏幕 DC 坐标。
///
/// **屏幕 DC 的坐标与虚拟屏幕坐标 1:1 对应**：DC 原点就是主显示器左上角 (0,0)，
/// 位于主屏左/上方的副屏用 **负坐标** 直接可达。
/// （`GetSystemMetrics(SM_XVIRTUALSCREEN)` 给的是虚拟屏幕的左/上边界，不是 DC 原点，
/// 不能拿它做偏移换算，否则会落到桌面空洞里、截出全黑图。）
fn screen_dc_source(x: i32, y: i32) -> (i32, i32) {
    (x, y)
}

/// 创建 32bpp 自上而下（top-down）的 DIB，返回位图句柄与像素指针。
unsafe fn create_dib(mem: HDC, w: i32, h: i32) -> Result<(HBITMAP, *mut c_void)> {
    if w <= 0 || h <= 0 {
        return Err(Error::Capture(format!("捕获区域尺寸非法: {w}x{h}")));
    }
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // 负值 = 自上而下，与 WGC 的行序一致
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0; // BI_RGB

    let mut bits: *mut c_void = ptr::null_mut();
    let bitmap = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .map_err(|e| Error::Capture(format!("CreateDIBSection 失败: {e}")))?;
    if bits.is_null() {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        return Err(Error::Capture("CreateDIBSection 未返回像素缓冲".into()));
    }
    Ok((bitmap, bits))
}

/// 从 DIB 像素缓冲读出图像（32bpp BGRA）。
unsafe fn read_dib(bits: *mut c_void, w: i32, h: i32) -> Result<RgbaImage> {
    let row = w as usize * 4;
    let len = row * h as usize;
    let mut data = vec![0u8; len];
    ptr::copy_nonoverlapping(bits as *const u8, data.as_mut_ptr(), len);
    Ok(RgbaImage::from_bgra(w as u32, h as u32, &data))
}

/// 自动释放 DC。
struct DcGuard {
    dc: HDC,
    kind: DcKind,
}

/// DC 的来源，决定用哪种释放方式。
enum DcKind {
    /// `GetDC(NULL)` 拿到的屏幕 DC：只能用 `ReleaseDC`，**不能** `DeleteDC`。
    Screen,
    /// `CreateCompatibleDC` 创建的内存 DC：用 `DeleteDC` 释放。
    Memory,
}

impl DcGuard {
    fn screen(dc: HDC) -> Self {
        Self {
            dc,
            kind: DcKind::Screen,
        }
    }

    fn memory(dc: HDC) -> Self {
        Self {
            dc,
            kind: DcKind::Memory,
        }
    }
}

impl Drop for DcGuard {
    fn drop(&mut self) {
        // SAFETY: DC 由本 guard 持有。
        unsafe {
            match self.kind {
                DcKind::Screen => {
                    let _ = ReleaseDC(None, self.dc);
                }
                DcKind::Memory => {
                    let _ = DeleteDC(self.dc);
                }
            }
        }
    }
}

/// 自动把位图从内存 DC 上摘掉并删除。
struct BitmapGuard {
    mem: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
}

impl BitmapGuard {
    fn new(mem: HDC, bitmap: HBITMAP) -> Self {
        // SAFETY: bitmap 是刚创建的有效 HBITMAP。
        let old = unsafe { SelectObject(mem, HGDIOBJ(bitmap.0)) };
        Self { mem, bitmap, old }
    }
}

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        // SAFETY: old 是我们装上去之前的对象，bitmap 由本 guard 拥有。
        unsafe {
            SelectObject(self.mem, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
        }
    }
}
