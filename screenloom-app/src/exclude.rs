//! 按窗口排除抓屏：把配置里的窗口从截图/录制画面中抹掉。
//!
//! 这里只做**像素层**处理，不依赖任何 Win32 窗口对象：
//! 配置存的是 `标题|类名`（HWND 每次启动都会变），每次抓屏时重新枚举窗口、
//! 找到匹配项、按窗口矩形把内容替换成该区域的平均色。
//!
//! 这套逻辑同时服务截图与录屏：录屏侧对每一帧调用 [`apply_excluded_windows`]。

use screenloom_capture::{list_capture_windows, WindowInfo};
use screenloom_core::{exclude_entry, parse_exclude_entry, Config, Rect, Result, Rgba, RgbaImage};

use crate::windowpicker::{self, PickerMode};

/// 把配置中排除的窗口从 `img` 里抹掉。
///
/// - `img`：整块虚拟屏图像（`frame.bounds` 为虚拟屏物理坐标）
/// - `frame_bounds`：图像左上角对应的虚拟屏物理坐标
///
/// 找不到匹配窗口、窗口在屏幕外、区域为空都直接跳过，不算错误。
pub fn apply_excluded_windows(img: &mut RgbaImage, cfg: &Config, frame_bounds: Rect) {
    if cfg.exclude_windows.is_empty() {
        return;
    }
    let windows = list_capture_windows();
    for entry in &cfg.exclude_windows {
        let Some((title, class)) = parse_exclude_entry(entry) else {
            continue;
        };
        for w in windows.iter().filter(|w| matches(w, &title, &class)) {
            let r = window_rect_in_image(w, frame_bounds);
            if r.w <= 0 || r.h <= 0 {
                continue;
            }
            fill_with_average(img, r);
        }
    }
}

fn matches(w: &WindowInfo, title: &str, class: &str) -> bool {
    // 标题/类名都允许为空：空串表示“不限制这一项”。
    (title.is_empty() || w.title == title) && (class.is_empty() || w.class_name == class)
}

fn window_rect_in_image(w: &WindowInfo, frame_bounds: Rect) -> Rect {
    let screen = w.bounds;
    Rect::new(
        screen.x - frame_bounds.x,
        screen.y - frame_bounds.y,
        screen.w,
        screen.h,
    )
}

/// 用区域平均色填充矩形（只改 RGB，alpha 保持 255）。
fn fill_with_average(img: &mut RgbaImage, r: Rect) {
    let x0 = r.x.max(0) as u32;
    let y0 = r.y.max(0) as u32;
    let x1 = (r.right() as u32).min(img.w);
    let y1 = (r.bottom() as u32).min(img.h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    // 大区域按步长采样，避免每次截图都扫一遍 4K 全图。
    let step = ((x1 - x0).max(y1 - y0) / 256).max(1);
    let mut sum = (0u64, 0u64, 0u64, 0u64);
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let c = img.pixel(x, y);
            sum.0 += c.r as u64;
            sum.1 += c.g as u64;
            sum.2 += c.b as u64;
            sum.3 += 1;
            x += step;
        }
        y += step;
    }
    if sum.3 == 0 {
        return;
    }
    let fill = Rgba::rgb(
        (sum.0 / sum.3) as u8,
        (sum.1 / sum.3) as u8,
        (sum.2 / sum.3) as u8,
    );

    for y in y0..y1 {
        for x in x0..x1 {
            img.set_pixel(x, y, fill);
        }
    }
}

/// 弹出多选窗口选择器，把选中的窗口写回配置。
pub fn edit_exclude_windows(cfg_path: &std::path::Path) -> Result<()> {
    let Some(picked) = windowpicker::run(PickerMode::PickExclusions) else {
        return Ok(());
    };

    let mut cfg = Config::load(cfg_path)?;
    cfg.exclude_windows = picked
        .iter()
        .map(|w| exclude_entry(&w.title, &w.class_name))
        .collect();
    cfg.save(cfg_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_region_with_average_color() {
        let mut img = RgbaImage::new(10, 10);
        // 左半黑、右半白
        for y in 0..10 {
            for x in 0..10 {
                let v = if x < 5 { 0 } else { 255 };
                img.set_pixel(x, y, Rgba::rgb(v, v, v));
            }
        }
        fill_with_average(&mut img, Rect::new(0, 0, 5, 5));
        assert_eq!(img.pixel(0, 0), Rgba::rgb(0, 0, 0));
        assert_eq!(img.pixel(9, 9), Rgba::rgb(255, 255, 255));
    }

    #[test]
    fn window_rect_is_translated_into_image_space() {
        let w = WindowInfo {
            hwnd: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
            pid: 0,
            title: "t".into(),
            class_name: "c".into(),
            bounds: Rect::new(100, 50, 20, 10),
            is_foreground: false,
        };
        let r = window_rect_in_image(&w, Rect::new(80, 40, 100, 100));
        assert_eq!(r, Rect::new(20, 10, 20, 10));
    }
}
