//! 探针：把 OCR 打开临时文件这一步单独拎出来，定位「路径过长」到底是谁的问题。
//!
//! 运行：
//! `cargo run --manifest-path D:\github\screenloom\Cargo.toml -p screenloom-app --example ocr_probe`

use windows::core::HSTRING;
use windows::Storage::StorageFile;

fn main() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("screenloom_ocr_{}.png", std::process::id()));
    println!("temp_dir   = {dir:?}");
    println!("path       = {path:?}");
    println!("abs        = {:?}", path.to_string_lossy());

    // 造一个最小 PNG
    let img = screenloom_core::RgbaImage::new(8, 8);
    match img.encode_png() {
        Ok(png) => {
            std::fs::write(&path, png).expect("写临时文件失败");
            println!(
                "已写入 {} 字节",
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            );
        }
        Err(e) => {
            println!("编码 PNG 失败: {e}");
            return;
        }
    }

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    println!("canonical  = {canonical:?}");

    for (label, p) in [
        ("canonical", canonical.to_string_lossy().to_string()),
        ("plain-abs", path.to_string_lossy().to_string()),
    ] {
        match StorageFile::GetFileFromPathAsync(&HSTRING::from(p.as_str())) {
            Ok(op) => match op.get() {
                Ok(f) => println!(
                    "[OK]   {label}: {p} -> {:?}",
                    f.Path().map(|x| x.to_string())
                ),
                Err(e) => println!(
                    "[FAIL] {label}: {p} -> get() 失败: {e} / code={:?}",
                    e.code()
                ),
            },
            Err(e) => println!("[FAIL] {label}: {p} -> 调用失败: {e} / code={:?}", e.code()),
        }
    }

    let _ = std::fs::remove_file(&path);
}
