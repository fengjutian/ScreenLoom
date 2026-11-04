//! # screenloom-capture
//!
//! 微信 4.x 截图工具（`screenloom`）的 Rust 复刻 —— **抓屏层**。
//!
//! 提供两套后端：
//! - [`WgcBackend`]：`Windows.Graphics.Capture`（WGC）。能捕获被遮挡 / 最小化的窗口，
//!   画面质量高；失败时由 [`default_backend`] 自动降级。
//! - [`GdiBackend`]：`BitBlt` / `PrintWindow`。兼容性最好的兜底实现。
//!
//! **坐标约定**：本 crate 暴露的所有 `Rect` / `Point` 一律是 **物理像素**，
//! 且位于 **虚拟屏幕坐标系**（原点 = 主显示器左上角，负坐标表示主屏左侧/上方的显示器）。
//! 逻辑坐标换算请交给 `screenloom_core::Rect::to_physical`。

use screenloom_core::{Rect, Result, RgbaImage};
use windows::Win32::Foundation::HWND;

mod errors;
mod gdi;
mod monitors;
mod stream;
mod virtual_screen;
mod wgc;
// 模块名用 `wins` 而不是 `windows`：crate 内 `mod windows` 会让
// `use windows::...` 变成二义路径（crate 名 vs 模块名）。
#[path = "windows.rs"]
mod wins;

pub use errors::{is_capture_protected_err, CAPTURE_PROTECTED_MESSAGE};
pub use gdi::GdiBackend;
pub use monitors::{
    ensure_dpi_awareness, list_monitors, virtual_screen_origin, virtual_screen_size,
};
pub use stream::{BgraFrame, ScreenStream};
pub use virtual_screen::capture_virtual_screen;
pub use wgc::WgcBackend;
pub use wins::{
    is_window_capture_excluded, list_capture_windows, list_own_windows,
    set_window_capture_excluded, WindowInfo, WDA_EXCLUDEFROMCAPTURE,
};

/// 一次捕获的结果。
#[derive(Clone, Debug)]
pub struct CaptureFrame {
    /// 捕获到的像素（RGBA8，非预乘）。
    pub image: RgbaImage,
    /// 捕获区域在虚拟屏幕坐标系中的 **物理像素** 矩形。
    pub bounds: Rect,
}

/// 单个显示器的信息。
#[derive(Clone, Debug)]
pub struct MonitorInfo {
    /// 在 [`list_monitors`] 返回列表中的下标。
    pub index: usize,
    /// 显示器在虚拟屏幕坐标系中的 **物理像素** 矩形。
    pub bounds: Rect,
    /// 是否为主显示器。
    pub is_primary: bool,
    /// 物理像素 / 逻辑像素 的缩放比（96 DPI → 1.0，144 DPI → 1.5）。
    pub scale: f32,
}

/// 捕获目标。
#[derive(Clone, Copy, Debug)]
pub enum CaptureTarget {
    /// 按 [`list_monitors`] 的下标捕获显示器。
    Monitor(usize),
    /// 按窗口句柄捕获窗口。
    Window(HWND),
}

/// 抓屏后端。
pub trait CaptureBackend {
    /// 捕获指定目标。
    fn capture(&self, target: CaptureTarget) -> Result<CaptureFrame>;
}

/// 构造默认后端：优先 WGC，初始化失败时降级到 GDI。
///
/// 该函数不会返回错误 —— GDI 后端在 Windows 上总是可用。
pub fn default_backend() -> Box<dyn CaptureBackend> {
    match WgcBackend::new() {
        Ok(b) => Box::new(b),
        Err(_) => Box::new(GdiBackend::new().unwrap_or_default()),
    }
}
