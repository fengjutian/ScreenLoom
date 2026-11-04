//! 长截图拼接：纯 Rust 模板匹配 + 拼接。
//!
//! 对齐微信的 `wx::OpenCVWorker`（`splice_init_pixmap` / `handle_splice_pixmap` /
//! `offset` / `splice_id`）与 `wx::SpliceWorker`。
//!
//! 微信用 OpenCV 做模板匹配；这里用纯 Rust 的 SAD（绝对差和）实现同样的语义，
//! 免去 OpenCV 原生依赖。匹配失败时返回 `None`，由上层提示
//! 「截图中断，返回中断位置继续滚动」。

use crate::image::{Rgba, RgbaImage};

/// 两帧之间像素差的代价（忽略 alpha）。
#[inline]
fn pixel_diff(a: Rgba, b: Rgba) -> i64 {
    (a.r as i64 - b.r as i64).abs()
        + (a.g as i64 - b.g as i64).abs()
        + (a.b as i64 - b.b as i64).abs()
}

/// 搜索距离上限（像素）。
///
/// 匹配代价是 `O(search * band * cols)` —— 用户配置里这个值没夹住的话，
/// 填个 999999 会让长截图**卡死**（几十亿次像素比较）。
pub const MAX_SEARCH: i32 = 4000;

/// 求 `next` 相对 `prev` 的纵向偏移 `dy`。
///
/// 语义：`next(y) == prev(y + dy)`。向下滚动 `D` 像素时 `dy = -D`。
/// `search` 为最大搜索距离（像素），**内部会夹到 `[0, MAX_SEARCH]`**。
///
/// 匹配基准取 `prev` 底部若干行，在 `next` 中纵向滑动寻找最相似位置。
pub fn match_offset(prev: &RgbaImage, next: &RgbaImage, search: i32) -> Option<i32> {
    if prev.w == 0 || next.w == 0 || prev.h < 8 || next.h < 8 {
        return None;
    }
    let search = search.clamp(0, MAX_SEARCH);
    let band = (prev.h / 4).clamp(4, 48);
    let cols = prev.w.min(next.w) as usize;
    if cols < 8 {
        return None;
    }
    let step = 2usize.max(cols / 320); // 降采样列，控制耗时
    let y0 = prev.h - band;

    let mut best: Option<(i64, i32)> = None;
    for dy in -search..=search {
        let ny0 = y0 as i64 + dy as i64;
        if ny0 < 0 || ny0 + band as i64 > next.h as i64 {
            continue;
        }
        let ny0 = ny0 as u32;
        let mut sad: i64 = 0;
        let mut n: i64 = 0;
        for y in 0..band {
            for x in (0..cols).step_by(step) {
                let p = prev.pixel(x as u32, y0 + y);
                let q = next.pixel(x as u32, ny0 + y);
                sad += pixel_diff(p, q);
                n += 1;
            }
        }
        if n == 0 {
            continue;
        }
        if best.is_none() || sad < best.unwrap().0 {
            best = Some((sad, dy));
        }
    }

    let (sad, dy) = best?;
    let samples = (band as i64) * (cols.div_ceil(step) as i64);
    let avg = sad / samples.max(1);
    // 平均色差必须足够小，否则认为没匹配上
    if avg > 40 {
        None
    } else {
        Some(dy)
    }
}

/// 按偏移拼接两帧。
///
/// `dy < 0` 表示内容向上移动（向下滚动），新内容在**下方**；
/// `dy > 0` 表示内容向下移动（向上滚动），新内容在**上方**。
pub fn splice(prev: &RgbaImage, next: &RgbaImage, dy: i32) -> Option<RgbaImage> {
    if dy == 0 {
        return Some(next.clone());
    }
    let h = prev.h.min(next.h);
    let w = prev.w.min(next.w);
    if w == 0 || h == 0 {
        return None;
    }
    let d = dy.abs().min(h as i32);
    let out_h = h as i32 + d;

    // (上段来源, 下段来源)
    let (top, bottom) = if dy < 0 { (prev, next) } else { (next, prev) };

    let mut out = RgbaImage::new(w, out_h as u32);
    for y in 0..d {
        copy_row(top, y as u32, &mut out, y as u32);
    }
    for y in 0..h {
        copy_row(bottom, y, &mut out, d as u32 + y);
    }
    Some(out)
}

/// 把源图第 `sy` 行拷到目标图第 `dy` 行（宽度取两者较小值）。
fn copy_row(src: &RgbaImage, sy: u32, dst: &mut RgbaImage, dy: u32) {
    if sy >= src.h || dy >= dst.h {
        return;
    }
    let w = src.w.min(dst.w) as usize;
    let soff = (sy as usize) * (src.w as usize) * 4;
    let doff = (dy as usize) * (dst.w as usize) * 4;
    dst.data[doff..doff + w * 4].copy_from_slice(&src.data[soff..soff + w * 4]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张每行颜色不同的图，便于定位。
    fn striped(h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(64, h);
        for y in 0..h {
            for x in 0..64 {
                img.set_pixel(x, y, Rgba::rgb((y % 256) as u8, 0, 0));
            }
        }
        img
    }

    #[test]
    fn match_offset_detects_downward_scroll() {
        // prev 是画面，next 是向下滚动 100 像素后的画面
        let mut prev = striped(400);
        for y in 0..400u32 {
            for x in 0..64u32 {
                prev.set_pixel(x, y, Rgba::rgb((y % 251) as u8, (y / 251) as u8, 0));
            }
        }
        let mut next = RgbaImage::new(64, 400);
        for y in 0..300u32 {
            for x in 0..64u32 {
                let src_y = y + 100;
                next.set_pixel(x, y, prev.pixel(x, src_y));
            }
        }
        let dy = match_offset(&prev, &next, 200).expect("应匹配到偏移");
        assert_eq!(dy, -100);
    }

    #[test]
    fn match_offset_clamps_absurd_search_window() {
        // 搜索距离没夹住的话，填个超大值会直接把长截图卡死。
        // 这里用「超大值」和「夹到上限」两种输入，结果必须一致。
        let mut prev = striped(200);
        for y in 0..200u32 {
            for x in 0..64u32 {
                prev.set_pixel(x, y, Rgba::rgb((y % 251) as u8, (y / 251) as u8, 0));
            }
        }
        let mut next = RgbaImage::new(64, 200);
        for y in 0..100u32 {
            for x in 0..64u32 {
                next.set_pixel(x, y, prev.pixel(x, y + 100));
            }
        }
        let huge = match_offset(&prev, &next, i32::MAX).expect("超大搜索窗口也应能匹配");
        let clamped = match_offset(&prev, &next, MAX_SEARCH).expect("夹住上限也应能匹配");
        assert_eq!(huge, clamped, "搜索窗口应被夹到 MAX_SEARCH");
        assert_eq!(huge, -100);
    }

    #[test]
    fn match_offset_handles_negative_search() {
        let a = striped(100);
        let b = striped(100);
        // 负数搜索距离不是合法输入 —— 必须被夹到 0（= 只比 dy=0），
        // 不能 panic，也不能变成"整屏乱搜"。
        assert_eq!(
            match_offset(&a, &b, -5),
            match_offset(&a, &b, 0),
            "负数搜索应被夹到 0"
        );
    }

    #[test]
    fn match_offset_returns_none_on_unrelated_frames() {
        let a = RgbaImage::new(64, 200);
        let mut b = RgbaImage::new(64, 200);
        for y in 0..200u32 {
            for x in 0..64u32 {
                b.set_pixel(
                    x,
                    y,
                    Rgba::rgb((x * 7 % 256) as u8, (y * 13 % 256) as u8, 0),
                );
            }
        }
        assert!(match_offset(&a, &b, 100).is_none());
    }

    #[test]
    fn splice_grows_by_offset() {
        let prev = striped(100);
        let next = striped(100);
        let out = splice(&prev, &next, -20).unwrap();
        assert_eq!(out.h, 120);
    }
}
