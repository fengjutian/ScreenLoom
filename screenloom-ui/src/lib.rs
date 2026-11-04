//! # screenloom-ui
//!
//! 微信 4.x 截图工具（`screenloom`）的 Rust 复刻 —— **渲染与交互层**。
//!
//! * [`Overlay`]：全屏分层浮窗，负责选区 → 编辑 → 工具栏。
//! * [`PinWindow`]：置顶贴图窗。
//! * [`render_annotations`]：把标注渲染到图像上（app 层落像素用）。
//!
//! 平台：Windows 10 1903+ / 11，x64。

pub mod gdiplus;
pub mod render;

mod aipanel;
mod overlay;
mod pin;
mod toolbar;
mod win;

pub use overlay::{
    post_ai_reply, AiReply, AiRequest, MosaicRect, Overlay, OverlayAction, OverlayConfig,
    OverlayOutcome, WM_AI_REPLY,
};
pub use pin::PinWindow;
pub use render::render_annotations;
