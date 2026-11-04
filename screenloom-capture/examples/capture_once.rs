//! 示例：抓一次主显示器并存成 PNG。
//!
//! ```text
//! cargo run --manifest-path D:\github\screenloom\Cargo.toml -p screenloom-capture --example capture_once
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use screenloom_capture::{default_backend, list_monitors, CaptureTarget, GdiBackend, WgcBackend};

const DEFAULT_OUT: &str = r"D:\github\screenloom\capture_test.png";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT));

    let monitors = list_monitors()?;
    println!("== 显示器列表 ({}) ==", monitors.len());
    for m in &monitors {
        println!(
            "  #{} bounds=({}, {} {}x{}) primary={} scale={:.3}",
            m.index, m.bounds.x, m.bounds.y, m.bounds.w, m.bounds.h, m.is_primary, m.scale
        );
    }

    // default_backend() 优先 WGC；这里额外探测一次，好把实际走的路径打印出来。
    let wgc_ready = WgcBackend::new().is_ok();
    println!(
        "== 后端 ==\n  {}",
        if wgc_ready {
            "WGC (Windows.Graphics.Capture)"
        } else {
            "GDI (BitBlt / PrintWindow)"
        }
    );

    // 确认 GDI 后端本身也能构造（降级路径可用）。
    let _ = GdiBackend::new()?;

    let primary = monitors
        .iter()
        .find(|m| m.is_primary)
        .map(|m| m.index)
        .unwrap_or(0);

    let backend = default_backend();
    let frame = backend.capture(CaptureTarget::Monitor(primary))?;

    println!("== 捕获结果 ==");
    println!(
        "  目标: 显示器 #{}  bounds=({}, {} {}x{})",
        primary, frame.bounds.x, frame.bounds.y, frame.bounds.w, frame.bounds.h
    );
    println!("  图像: {}x{}", frame.image.w, frame.image.h);

    // 简单的像素统计，方便肉眼之外再确认一次（不是纯黑 / 不是纯色）。
    let mut hist: HashMap<[u8; 3], usize> = HashMap::new();
    let mut sum = [0u64; 3];
    let mut black = 0usize;
    let total = (frame.image.w as usize) * (frame.image.h as usize);
    for px in frame.image.data.chunks_exact(4) {
        let key = [px[0], px[1], px[2]];
        *hist.entry(key).or_insert(0) += 1;
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
    let c = frame.image.pixel(frame.image.w / 2, frame.image.h / 2);
    println!("  中心像素 RGBA=({}, {}, {}, {})", c.r, c.g, c.b, c.a);

    let png = frame.image.encode_png()?;
    std::fs::write(&out, &png)?;
    println!("== 输出 ==\n  {} ({} 字节)", out.display(), png.len());
    Ok(())
}
