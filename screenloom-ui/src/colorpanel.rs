//! 色值面板：吸管取色后显示 HEX / RGB，并支持 Ctrl+C 复制色值。
//!
//! 对齐微信截图工具的 `ColorPanel`（「已复制色值」「按 Ctrl+C 复制色值」）。
//!
//! 画在**物理像素**坐标系里（和浮窗的 GDI+ surface 一致），
//! 字号与内边距按 `scale` 缩放。

use screenloom_core::{Rect, Rgba};

use crate::gdiplus::{self, Graphics};

/// 内边距（逻辑像素）。
const PAD: i32 = 10;
/// 色块边长（逻辑像素）。
const SWATCH: i32 = 28;
/// 面板宽度（逻辑像素）。
const W: i32 = 208;
/// 面板高度（逻辑像素）。
const H: i32 = PAD * 2 + SWATCH + 18;

/// 画一个色值面板。
///
/// `anchor` 是面板左上角（**物理像素**），`anchor.w` 是面板宽度（物理像素）。
pub(crate) fn paint_color_panel(
    g: &mut Graphics,
    anchor: Rect,
    scale: f32,
    color: Rgba,
    copied: bool,
) {
    let pad = (PAD as f32 * scale).round() as i32;
    let sw = (SWATCH as f32 * scale).round() as i32;
    let size = 13.0 * scale;
    let w = anchor.w;
    let h = ((H as f32 * scale).round() as i32).max(1);

    // 面板底
    g.fill_round_rect(
        anchor.x as f32,
        anchor.y as f32,
        w as f32,
        h as f32,
        8.0,
        0xF0_1E_1E_1E,
    );
    g.draw_round_rect(
        anchor.x as f32,
        anchor.y as f32,
        w as f32,
        h as f32,
        8.0,
        0x66_00_00_00,
        1.0,
    );

    // 色块
    let sx = anchor.x + pad;
    let sy = anchor.y + pad;
    g.fill_round_rect(sx as f32, sy as f32, sw as f32, sw as f32, 4.0, gdiplus::argb(color));
    g.draw_round_rect(
        sx as f32,
        sy as f32,
        sw as f32,
        sw as f32,
        4.0,
        0x66_00_00_00,
        1.0,
    );

    // HEX + RGB
    let tx = sx + sw + (8.0 * scale).round() as i32;
    let hex = color.to_hex();
    g.draw_text(
        tx as f32,
        sy as f32,
        &hex,
        size,
        gdiplus::argb(Rgba::rgb(0xEE, 0xEE, 0xEE)),
        true,
    );
    let rgb = format!("R {}  G {}  B {}", color.r, color.g, color.b);
    g.draw_text(
        tx as f32,
        sy as f32 + (18.0 * scale).round() as i32,
        &rgb,
        size,
        gdiplus::argb(Rgba::rgb(0xB0, 0xB0, 0xB0)),
        false,
    );

    // 提示文案
    let hint = if copied { "已复制色值" } else { "按 Ctrl+C 复制色值" };
    let hint_color = if copied {
        gdiplus::argb(Rgba::rgb(0x00, 0xC3, 0x75))
    } else {
        gdiplus::argb(Rgba::rgb(0x99, 0x99, 0x99))
    };
    g.draw_text(
        (anchor.x + pad) as f32,
        (anchor.y + pad + sw + (6.0 * scale).round() as i32) as f32,
        hint,
        size,
        hint_color,
        false,
    );
}
