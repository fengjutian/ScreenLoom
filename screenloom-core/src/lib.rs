//! # screenloom-core
//!
//! 微信 4.x 截图工具（`screenloom`）的 Rust 复刻 —— **纯逻辑层**。
//!
//! 本 crate **不含任何窗口、IO 或全局状态**，只保留可单元测试的算法与数据模型：
//! 几何、图像缓冲、标注模型、撤销栈、长截图拼接、配置。
//!
//! 平台相关部分在 `screenloom-capture`（抓屏）与 `screenloom-ui`（渲染/交互）。

pub mod annotation;
pub mod canvas;
pub mod color;
pub mod config;
pub mod error;
pub mod geometry;
pub mod icon;
pub mod image;
pub mod inpaint;
pub mod nv12;
pub mod qr;
pub mod stitch;

pub use annotation::{Annotation, ShapeKind, Tool};
pub use canvas::Canvas;
pub use color::{BORDER, MASK_ALPHA, PALETTE, TEXT};
pub use config::{Config, CopyFormat};
pub use error::{Error, Result};
pub use geometry::{Point, Rect, Size};
pub use icon::{draw_app_icon, pack_ico};
pub use image::{sample_color, Rgba, RgbaImage};
pub use inpaint::{inpaint_region, inpaint_region_with, InpaintOptions};
pub use nv12::{bgra_to_nv12, bgra_to_nv12_into, nv12_bytes_len, Nv12Image};
pub use qr::{detect_qr, detect_qrs, QrCode};
pub use stitch::{match_offset, splice};
