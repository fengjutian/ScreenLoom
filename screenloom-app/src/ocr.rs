//! 本地 OCR（提取文字）。
//!
//! 微信用的是**腾讯私有网络 OCR**（证据：`kernel::manager::OCRManager::UploadImageCdn`、
//! `clicfg_xwechat_force_netocr`、协议 `micromsg.NewOCRTranslationRequest`），
//! 外部程序无法复刻该协议。
//!
//! 这里改用系统自带的 **`Windows.Media.Ocr`**（完全离线，不联网）。
//!
//! 实现路径：PNG 落临时文件 → `StorageFile` → `BitmapDecoder` → `SoftwareBitmap`
//! → `OcrEngine::RecognizeAsync`。（`SoftwareBitmap::CreateCopyFromBuffer` 需要
//! `IBuffer`，而 windows-rs 0.61 的 `Buffer` 没有写入方法，故走解码器路径。）

use std::path::PathBuf;

use windows::core::HSTRING;
use windows::Graphics::Imaging::{BitmapDecoder, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::{FileAccessMode, StorageFile};

use screenloom_core::{Error, Result, RgbaImage};

/// 识别图片中的文字，返回纯文本。
pub fn recognize_text(img: &RgbaImage) -> Result<String> {
    if img.w == 0 || img.h == 0 {
        return Err(Error::Image("空图片无法识别".into()));
    }

    let (path, png) = (temp_png_path(), img.encode_png()?);
    std::fs::write(&path, png)?;

    let result = recognize_file(&path);
    let _ = std::fs::remove_file(&path);
    result
}

/// 从磁盘上的 PNG 文件识别文字。
pub fn recognize_file(path: &std::path::Path) -> Result<String> {
    // StorageFile 需要绝对路径（反斜杠）
    let abs = absolute_path_for_winrt(path);

    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs))
        .map_err(|e| Error::Image(format!("打开临时文件失败: {e}")))?
        .get()
        .map_err(|e| Error::Image(format!("读取临时文件失败: {e}")))?;

    let stream = file
        .OpenAsync(FileAccessMode::Read)
        .map_err(|e| Error::Image(format!("打开文件流失败: {e}")))?
        .get()
        .map_err(|e| Error::Image(format!("读取文件流失败: {e}")))?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| Error::Image(format!("创建图像解码器失败: {e}")))?
        .get()
        .map_err(|e| Error::Image(format!("解码 PNG 失败: {e}")))?;

    let bitmap: SoftwareBitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| Error::Image(format!("获取软件位图失败: {e}")))?
        .get()
        .map_err(|e| Error::Image(format!("获取软件位图失败: {e}")))?;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
        Error::Unsupported(format!(
            "本机没有可用的 OCR 语言包（设置 → 时间和语言 → 语言和区域 → 语言选项，勾选“光学字符识别”）: {e}"
        ))
    })?;

    let op = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| Error::Image(format!("启动 OCR 失败: {e}")))?;
    let ocr = op
        .get()
        .map_err(|e| Error::Image(format!("OCR 执行失败: {e}")))?;

    let text = ocr
        .Text()
        .map_err(|e| Error::Image(format!("读取 OCR 结果失败: {e}")))?
        .to_string();
    Ok(text)
}

/// 识别剪贴板里的图片文字，并把识别结果作为**纯文本**写回剪贴板。
///
/// 用法：先截图（`--once` 会复制图片到剪贴板），再执行 `screenloom --ocr`。
pub fn from_clipboard() -> Result<String> {
    let bytes = crate::clipboard::read_png()?;
    let img = RgbaImage::decode_png(&bytes)?;
    let text = recognize_text(&img)?;
    crate::clipboard::write_text(&text)?;
    Ok(text)
}

/// OCR 临时文件路径。
fn temp_png_path() -> PathBuf {
    std::env::temp_dir().join(format!("screenloom_ocr_{}.png", std::process::id()))
}

/// 拼出 WinRT `StorageFile` 能吃的**绝对路径**。
///
/// 两个坑：
/// 1. `canonicalize()` 会加上 `\\?\` 扩展长度前缀，
///    而 `StorageFile::GetFileFromPathAsync` **不认**这个前缀，
///    会报 `0x800700A1`「路径过长」—— 路径其实一点都不长（坑：路径 60 字符也报这个）。
/// 2. `\\?\UNC\server\share` 要还原成 `\\server\share`。
fn absolute_path_for_winrt(path: &std::path::Path) -> String {
    let mut abs = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|d| d.join(path))
                    .unwrap_or_else(|_| path.to_path_buf())
            }
        }
    };
    // `canonicalize` 对 UNC 会返回 `\\?\UNC\server\share`；这里还原成常规 UNC 写法。
    let s = abs.to_string_lossy().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        abs = std::path::PathBuf::from(rest);
    }
    abs.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_image_is_rejected() {
        let img = RgbaImage::new(0, 0);
        assert!(recognize_text(&img).is_err());
    }

    #[test]
    fn temp_path_is_absolute() {
        let p = temp_png_path();
        assert!(p.is_absolute());
        assert!(p.extension().map(|e| e == "png").unwrap_or(false));
    }

    /// WinRT 不认 `\\?\` 前缀 —— 必须剥掉，否则报「路径过长」。
    #[test]
    fn winrt_path_has_no_extended_length_prefix() {
        let p = absolute_path_for_winrt(&temp_png_path());
        assert!(!p.starts_with(r"\\?\"), "不能带扩展长度前缀: {p}");
        assert!(!p.starts_with(r"\\?\UNC\"), "UNC 也要还原: {p}");
        assert!(
            std::path::Path::new(&p).is_absolute(),
            "必须是绝对路径: {p}"
        );
    }

    /// 纯逻辑：前缀剥离规则本身。
    #[test]
    fn strips_extended_prefix_rules() {
        assert_eq!(
            absolute_path_for_winrt(std::path::Path::new(r"\\?\C:\a\b.png")),
            r"C:\a\b.png"
        );
    }
}
