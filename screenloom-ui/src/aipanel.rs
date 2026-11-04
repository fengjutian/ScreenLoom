//! AI 结果面板：半透明深色底 + 自动换行文本。
//!
//! 画在**物理像素**坐标系里（和浮窗的 GDI+ surface 一致），
//! 字号与行高按 `scale` 缩放。

use screenloom_core::{Rect, Rgba};

use crate::gdiplus::{self, Graphics};

/// 内边距（逻辑像素）。
const PAD: i32 = 10;
/// 标题高度（逻辑像素）。
const HEAD_H: i32 = 20;
/// 正文字号（逻辑像素）。
const BODY_SIZE: f32 = 13.0;
/// 行高（逻辑像素）。
const LINE_H: i32 = 18;
/// 面板最大宽度（逻辑像素）。
const MAX_W: i32 = 440;
/// 面板最大高度（逻辑像素）。
const MAX_H: i32 = 280;
/// 最多显示多少行正文。
const MAX_LINES: usize = 12;

/// 画一个 AI 结果面板。
///
/// `anchor` 是面板左上角与**期望宽度**（物理像素），高度按内容算。
pub(crate) fn paint_panel(
    g: &mut Graphics,
    anchor: Rect,
    scale: f32,
    title: &str,
    body: &str,
    is_error: bool,
) {
    let pad = (PAD as f32 * scale).round() as i32;
    let head_h = (HEAD_H as f32 * scale).round() as i32;
    let line_h = ((LINE_H as f32 * scale).round() as i32).max(1);
    let size = BODY_SIZE * scale;
    let max_w = (anchor.w - pad * 2).max(40);
    let max_h = ((MAX_H as f32 * scale).round() as i32) - pad * 2 - head_h;

    let mut lines = wrap(g, body, size, max_w as f32);
    let mut truncated = false;
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        truncated = true;
    }
    if truncated {
        let last = lines.len().saturating_sub(1);
        lines[last] = format!("{}…（完整内容已复制到剪贴板）", lines[last]);
    }

    let content_h = lines.len() as i32 * line_h;
    let panel_h = (head_h + content_h + pad * 2).min(max_h.max(head_h + line_h + pad * 2));
    let panel_w = anchor.w;

    // 面板底
    let bg = if is_error {
        0xF0_3A_1E_1E
    } else {
        0xF0_1E_1E_1E
    };
    g.fill_round_rect(
        anchor.x as f32,
        anchor.y as f32,
        panel_w as f32,
        panel_h as f32,
        8.0,
        bg,
    );
    g.draw_round_rect(
        anchor.x as f32,
        anchor.y as f32,
        panel_w as f32,
        panel_h as f32,
        8.0,
        0x66_00_00_00,
        1.0,
    );

    // 标题
    let head_color = if is_error {
        gdiplus::argb(Rgba::rgb(0xFF, 0x8A, 0x8A))
    } else {
        gdiplus::argb(Rgba::rgb(0x00, 0xC3, 0x75))
    };
    g.draw_text(
        (anchor.x + pad) as f32,
        (anchor.y + pad) as f32,
        title,
        13.0 * scale,
        head_color,
        true,
    );

    // 正文
    let body_color = gdiplus::argb(Rgba::rgb(0xEE, 0xEE, 0xEE));
    let mut y = anchor.y + pad + head_h;
    for l in &lines {
        if y + line_h > anchor.y + panel_h {
            break;
        }
        g.draw_text(
            (anchor.x + pad) as f32,
            y as f32,
            l,
            size,
            body_color,
            false,
        );
        y += line_h;
    }
}

/// 按宽度折行（逐字符测宽，兼容中英文混排）。
fn wrap(g: &mut Graphics, text: &str, size: f32, max_w: f32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        for ch in raw.chars() {
            let s = ch.to_string();
            let cw = g.measure_text(&s, size, false).0;
            if !cur.is_empty() && cur_w + cw > max_w {
                out.push(std::mem::take(&mut cur));
                cur_w = 0.0;
            }
            cur.push(ch);
            cur_w += cw;
        }
        out.push(cur);
    }
    out
}

/// 面板的逻辑宽度上限。
pub(crate) const MAX_WIDTH_LOGICAL: i32 = MAX_W;
