//! 示例：抓取 **整块虚拟屏**（多显示器合成）并存成 PNG。
//!
//! 这就是浮窗（`screenloom-ui::Overlay`）期望的背景帧：
//! 图像左上角 = 虚拟屏左上角，图像内坐标 = 虚拟屏物理坐标 − `virtual_origin`。
//!
//! ```text
//! cargo run --manifest-path D:\github\screenloom\Cargo.toml -p screenloom-capture --example capture_virtual
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use screenloom_capture::{
    capture_virtual_screen, default_backend, list_monitors, virtual_screen_origin,
    virtual_screen_size,
};

const DEFAULT_OUT: &str = r"D:\github\screenloom\virtual_test.png";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT));

    let origin = virtual_screen_origin();
    let size = virtual_screen_size();
    println!("== 虚拟屏 ==");
    println!(
        "  origin=({}, {})  size={}x{}",
        origin.x, origin.y, size.w, size.h
    );

    let monitors = list_monitors()?;
    println!("== 显示器列表 ({}) ==", monitors.len());
    for m in &monitors {
        println!(
            "  #{} bounds=({}, {} {}x{}) primary={} scale={:.3}  → 虚拟屏内偏移=({}, {})",
            m.index,
            m.bounds.x,
            m.bounds.y,
            m.bounds.w,
            m.bounds.h,
            m.is_primary,
            m.scale,
            m.bounds.x - origin.x,
            m.bounds.y - origin.y
        );
    }

    let backend = default_backend();
    let frame = capture_virtual_screen(backend.as_ref())?;
    println!("== 捕获结果 ==");
    println!(
        "  bounds=({}, {} {}x{})",
        frame.bounds.x, frame.bounds.y, frame.bounds.w, frame.bounds.h
    );
    println!("  图像: {}x{}", frame.image.w, frame.image.h);
    if frame.image.w as i32 != size.w || frame.image.h as i32 != size.h {
        println!("  !! 图像尺寸与虚拟屏不一致");
    }

    // 每块显示器落点在合成图里的采样，验证「每块屏的内容确实贴对了位置」。
    println!("== 各屏落点采样 ==");
    for m in &monitors {
        let dx = m.bounds.x - origin.x;
        let dy = m.bounds.y - origin.y;
        let px = frame
            .image
            .pixel((dx + m.bounds.w / 2) as u32, (dy + m.bounds.h / 2) as u32);
        println!(
            "  #{} 中心落点=({}, {})  RGBA=({}, {}, {}, {})",
            m.index,
            dx + m.bounds.w / 2,
            dy + m.bounds.h / 2,
            px.r,
            px.g,
            px.b,
            px.a
        );
    }

    let mut hist: HashMap<[u8; 3], usize> = HashMap::new();
    let mut sum = [0u64; 3];
    let mut black = 0usize;
    let total = (frame.image.w as usize) * (frame.image.h as usize);
    for px in frame.image.data.chunks_exact(4) {
        *hist.entry([px[0], px[1], px[2]]).or_insert(0) += 1;
        sum[0] += px[0] as u64;
        sum[1] += px[1] as u64;
        sum[2] += px[2] as u64;
        if px[0] == 0 && px[1] == 0 && px[2] == 0 {
            black += 1;
        }
    }
    let mean = [
        sum[0] as f64 / total as f64,
        sum[1] as f64 / total as f64,
        sum[2] as f64 / total as f64,
    ];
    let top = hist.iter().max_by_key(|(_, c)| **c).unwrap();
    println!(
        "  平均色 R={:.1} G={:.1} B={:.1}；纯黑像素 {}/{} ({:.2}%)",
        mean[0],
        mean[1],
        mean[2],
        black,
        total,
        black as f64 / total as f64 * 100.0
    );
    println!(
        "  主色 RGB=({}, {}, {}) 占比 {:.2}%",
        top.0[0],
        top.0[1],
        top.0[2],
        *top.1 as f64 / total as f64 * 100.0
    );

    let png = frame.image.encode_png()?;
    std::fs::write(&out, &png)?;
    println!("== 输出 ==\n  {} ({} 字节)", out.display(), png.len());
    Ok(())
}
