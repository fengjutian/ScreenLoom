//! 抓**屏幕物理区域**并存 PNG —— 视觉留证专用。
//!
//! 用法：
//! ```text
//! cargo run --manifest-path ...\Cargo.toml -p screenloom-app --example grab_region -- x y w h out.png
//! ```
//!
//! 坐标是**虚拟屏物理像素**（与 `screenloom_core::Rect::to_physical` 同一坐标系）。
//! 用 WGC 抓整块虚拟屏再裁剪，所以拿到的是真实合成画面（含 DWM 分层窗口）。

use std::path::PathBuf;

use screenloom_capture::{capture_virtual_screen, default_backend, virtual_screen_origin};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 5 {
        eprintln!("用法: grab_region <x> <y> <w> <h> <out.png>");
        std::process::exit(2);
    }
    let x: i32 = a[0].parse().expect("x");
    let y: i32 = a[1].parse().expect("y");
    let w: u32 = a[2].parse().expect("w");
    let h: u32 = a[3].parse().expect("h");
    let out = PathBuf::from(&a[4]);

    let backend = default_backend();
    let frame = capture_virtual_screen(backend.as_ref()).expect("抓屏失败");
    let origin = virtual_screen_origin();
    println!(
        "虚拟屏: origin=({}, {}) bounds=({}, {} {}x{})",
        origin.x, origin.y, frame.bounds.x, frame.bounds.y, frame.bounds.w, frame.bounds.h
    );

    // 虚拟屏物理坐标 → 图像内坐标
    let rx = (x - frame.bounds.x).max(0);
    let ry = (y - frame.bounds.y).max(0);
    let crop = screenloom_core::Rect::new(rx, ry, w as i32, h as i32);
    let sub = frame.image.crop(crop);
    println!("裁剪: {}x{} → {}", w, h, sub.w * sub.h);

    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let png = sub.encode_png().expect("PNG 编码失败");
    std::fs::write(&out, &png).expect("写 PNG 失败");
    println!("已保存: {} ({} 字节)", out.display(), png.len());
}
