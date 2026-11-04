//! 生成程序图标资源：`assets/screenloom.ico`（+ 一张 PNG 预览）。
//!
//! 运行：
//! ```powershell
//! cargo run --manifest-path D:\github\screenloom\Cargo.toml -p screenloom-app --example gen_icon
//! ```
//!
//! 产物被 `screenloom-app/build.rs` 用来注入 exe 的 PE 资源节。

use std::path::PathBuf;

use screenloom_core::{draw_app_icon, pack_ico};

/// 打进 .ico 的尺寸集合。
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 放在 screenloom-app/assets/ 下：build.rs 从同一个相对位置读取并注入 PE 资源
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    std::fs::create_dir_all(&assets)?;

    let imgs: Vec<_> = SIZES.iter().map(|s| draw_app_icon(*s)).collect();
    let ico = pack_ico(&imgs);
    let ico_path = assets.join("screenloom.ico");
    std::fs::write(&ico_path, &ico)?;
    println!("[1] {} ({} bytes)", ico_path.display(), ico.len());

    // PNG 预览（给人看的）
    let big = draw_app_icon(256);
    let png_path = assets.join("screenloom.png");
    std::fs::write(&png_path, big.encode_png()?)?;
    println!("[2] {} (256x256)", png_path.display());

    // 单尺寸预览，方便肉眼检查小图标是否糊
    let small = draw_app_icon(32);
    let small_path = assets.join("screenloom_32.png");
    std::fs::write(&small_path, small.encode_png()?)?;
    println!("[3] {} (32x32)", small_path.display());

    Ok(())
}
