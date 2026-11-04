//! 虚拟屏整屏抓取。
//!
//! **为什么需要单独一层**：浮窗（`screenloom-ui::Overlay`）是铺在 **整块虚拟屏** 上的，
//! 它期望的背景帧就是「从虚拟屏左上角开始的一整张图」，即
//!
//! ```text
//! 图像内坐标 = 虚拟屏物理坐标 − virtual_origin
//! ```
//!
//! 所以抓屏层必须提供 **整块虚拟屏** 的图像，而不是「单块显示器」的图像。
//! 否则浮窗会把主屏画面贴到虚拟屏左上角 —— 内容和位置都错，
//! 而且跨屏选区会被算到错误的屏幕空间。
//!
//! 两条路径：
//! 1. **按显示器分别抓再合成**（优先）：走 WGC，逐块抓、贴到虚拟屏画布的对应位置。
//!    尺寸与显示器物理边界不一致时视为失败（防止错位）。
//! 2. **GDI 整块一次 BitBlt**（兜底）：屏幕 DC 坐标与虚拟屏坐标 1:1，
//!    一次 `BitBlt` 即可拿到跨屏画面，且是原子的（不会出现跨屏接缝）。

use screenloom_core::{Error, Rect, Result, RgbaImage};

use crate::monitors::{list_monitors, virtual_screen_origin, virtual_screen_size};
use crate::{CaptureBackend, CaptureFrame, CaptureTarget};

/// 抓取整块虚拟屏。
///
/// 返回的 `frame.bounds` 是虚拟屏在虚拟屏坐标系中的矩形（**物理像素**），
/// `frame.image` 的像素坐标 = 虚拟屏物理坐标 − `bounds` 的左上角。
pub fn capture_virtual_screen(backend: &dyn CaptureBackend) -> Result<CaptureFrame> {
    let origin = virtual_screen_origin();
    let size = virtual_screen_size();
    if size.w <= 0 || size.h <= 0 {
        return Err(Error::Capture(format!(
            "虚拟屏尺寸非法: {}x{}",
            size.w, size.h
        )));
    }
    let bounds = Rect::new(origin.x, origin.y, size.w, size.h);

    // 路径 1：逐显示器抓取后合成。
    if let Some(image) = compose_by_monitor(backend, origin, size) {
        return Ok(CaptureFrame { image, bounds });
    }

    // 路径 2：GDI 整块一次 BitBlt。
    let image = crate::gdi::capture_screen_rect(bounds)?;
    Ok(CaptureFrame { image, bounds })
}

/// 逐显示器抓取并合成到虚拟屏画布。任一块失败或尺寸对不上就返回 `None`（交给兜底路径）。
fn compose_by_monitor(
    backend: &dyn CaptureBackend,
    origin: screenloom_core::Point,
    size: screenloom_core::Size,
) -> Option<RgbaImage> {
    let monitors = list_monitors().ok()?;
    let mut canvas = RgbaImage::new(size.w as u32, size.h as u32);

    for m in &monitors {
        let frame = backend.capture(CaptureTarget::Monitor(m.index)).ok()?;
        // 尺寸必须与显示器物理边界一致，否则贴上去会错位 —— 直接走兜底。
        if frame.image.w != m.bounds.w as u32 || frame.image.h != m.bounds.h as u32 {
            return None;
        }
        canvas.blit(&frame.image, m.bounds.x - origin.x, m.bounds.y - origin.y);
    }
    Some(canvas)
}
