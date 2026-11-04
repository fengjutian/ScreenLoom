//! 落盘保存。
//!
//! 文件名格式对齐微信：`yyyy-MM-dd_hhmmss_zzz.png`（`wechat_longscreenshot` 用的同款命名）。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::GetLocalTime;

use screenloom_core::{Error, Result, RgbaImage};

/// 取**本地**时间戳字符串 `yyyy-MM-dd_hhmmss_zzz`。
///
/// 注意必须用 `GetLocalTime`：`GetSystemTime` 返回 UTC，会比本地时间少一个时区偏移。
pub fn timestamp() -> String {
    // SAFETY: 无参数调用，纯读取系统本地时间。
    let st: SYSTEMTIME = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}{:02}{:02}_{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// 生成带时间戳的 PNG 文件名。
pub fn timestamp_filename() -> String {
    format!("{}.png", timestamp())
}

/// 默认保存目录：`%USERPROFILE%\Pictures\Screenloom`。
pub fn default_dir() -> PathBuf {
    let pictures = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Pictures");
    pictures.join("Screenloom")
}

/// 保存 PNG，返回实际写入路径。
pub fn save_png(img: &RgbaImage, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(Error::Io)?;
    let path = dir.join(timestamp_filename());
    let bytes = img.encode_png()?;
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_shape() {
        let t = timestamp();
        // yyyy-MM-dd_hhmmss_zzz = 4+1+2+1+2+1+2+2+2+1+3 = 21
        assert_eq!(t.len(), 21, "yyyy-MM-dd_hhmmss_zzz 应为 21 字符: {t}");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], "_");
    }

    #[test]
    fn filename_ends_with_png() {
        assert!(timestamp_filename().ends_with(".png"));
    }

    #[test]
    fn saves_file() {
        let dir = std::env::temp_dir().join("screenloom-save-test");
        let img = RgbaImage::new(4, 4);
        let path = save_png(&img, &dir).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
