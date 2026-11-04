//! 独立验证 GDI 兜底抓屏后端。
//!
//! 用途：不依赖 WGC，直接验证「BitBlt/PrintWindow 路径」能否抓到正确画面。
//! 输出 `capture_gdi_test.png` 到工作区根目录。

use screenloom_capture::{CaptureBackend, CaptureTarget, GdiBackend};

fn main() {
    println!("== 显示器列表 ==");
    for m in screenloom_capture::list_monitors().unwrap_or_default() {
        println!(
            "  #{} bounds=({}, {} {}x{}) primary={} scale={}",
            m.index, m.bounds.x, m.bounds.y, m.bounds.w, m.bounds.h, m.is_primary, m.scale
        );
    }

    let backend = GdiBackend::new().expect("创建 GDI 后端失败");
    println!("== 后端 ==\n  GDI (BitBlt / PrintWindow)");

    let frame = match backend.capture(CaptureTarget::Monitor(0)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("GDI 抓屏失败: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "抓取成功: {}x{}  bounds=({}, {} {}x{})",
        frame.image.w,
        frame.image.h,
        frame.bounds.x,
        frame.bounds.y,
        frame.bounds.w,
        frame.bounds.h
    );

    // 抽样几个像素，确认不是全黑/纯色
    let mut nonzero = 0u32;
    let mut total = 0u32;
    for y in (0..frame.image.h).step_by(37) {
        for x in (0..frame.image.w).step_by(37) {
            let p = frame.image.pixel(x, y);
            total += 1;
            if p.r != 0 || p.g != 0 || p.b != 0 {
                nonzero += 1;
            }
        }
    }
    println!("抽样 {total} 点，非黑点 {nonzero} 个");

    let out = std::path::Path::new(r"D:\github\screenloom\capture_gdi_test.png");
    match frame.image.encode_png() {
        Ok(bytes) => {
            std::fs::write(out, &bytes).expect("写 PNG 失败");
            println!("已保存: {}", out.display());
        }
        Err(e) => {
            eprintln!("PNG 编码失败: {e}");
            std::process::exit(1);
        }
    }
}
