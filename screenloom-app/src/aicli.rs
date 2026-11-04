//! AI 能力的**命令行入口**。
//!
//! 输入图片的来源（按优先级）：
//! 1. `--image <path>` 指定的 PNG 文件；
//! 2. **剪贴板**里的图片——截图工具本身会把截图复制到剪贴板，
//!    所以「先截图、再 `--ask`」是自然流程（与 `--ocr` 的做法一致）。
//!
//! 三件事：
//! - `--ask`：把图片 + 问题发给模型（多模态），回答打印到 stdout 并写回剪贴板；
//! - `--translate`：本地 OCR 取字 → 送给模型翻译（结果同上）；
//! - `--ai-mosaic`：让模型标出敏感区域 → 打码 → 存盘 + 复制到剪贴板。

use std::path::Path;

use screenloom_core::{CopyFormat, Error, Result, RgbaImage};

use crate::ai::{AiClient, AiConfig, ImagePayload};
use crate::{clipboard, ocr, save};

/// 读入待处理的图片（`path` 为 `None` 时取剪贴板）。
pub fn load_image(path: Option<&Path>) -> Result<RgbaImage> {
    match path {
        Some(p) => {
            let bytes = std::fs::read(p)?;
            RgbaImage::decode_png(&bytes)
        }
        None => {
            let bytes = clipboard::read_png()?;
            RgbaImage::decode_png(&bytes)
        }
    }
}

/// 问 AI：图片 + 问题。
pub fn run_ask(cfg: &AiConfig, prompt: &str, image_path: Option<&Path>) -> Result<()> {
    let img = load_image(image_path)?;
    let png = img.encode_png()?;
    let client = AiClient::new(cfg.clone());
    let answer = client.ask(prompt, Some(&ImagePayload::png(png)))?;
    println!("{answer}");
    let _ = clipboard::write_text(&answer);
    Ok(())
}

/// 翻译：本地 OCR 取字 → 模型翻译。
pub fn run_translate(cfg: &AiConfig, target: &str, image_path: Option<&Path>) -> Result<()> {
    let img = load_image(image_path)?;
    let text = ocr::recognize_text(&img)?;
    if text.trim().is_empty() {
        return Err(Error::Unsupported("图片里没有识别到文字，无法翻译".into()));
    }
    let client = AiClient::new(cfg.clone());
    let out = client.translate(&text, target)?;
    println!("{out}");
    let _ = clipboard::write_text(&out);
    Ok(())
}

/// AI 一键打码：模型标出敏感区域 → 打码 → 存盘 + 复制。
pub fn run_ai_mosaic(
    cfg: &AiConfig,
    image_path: Option<&Path>,
    out_dir: Option<&Path>,
    block: u32,
) -> Result<()> {
    let img = load_image(image_path)?;
    let png = img.encode_png()?;
    let client = AiClient::new(cfg.clone());
    let regions = client.detect_mosaic(&ImagePayload::png(png))?;

    let usable: Vec<_> = regions
        .iter()
        .map(|r| r.to_rect(img.w, img.h))
        .filter(|r| !r.is_empty())
        .collect();
    if usable.is_empty() {
        println!("未发现需要打码的区域。");
        return Ok(());
    }

    let mut out = img.clone();
    for r in &usable {
        out.pixelate(*r, block);
    }

    let dir = out_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save::default_dir);
    let path = save::save_png(&out, &dir)?;

    println!("已打码 {} 处，保存到 {}", usable.len(), path.display());
    for (reg, r) in regions.iter().zip(usable.iter()) {
        let label = if reg.label.is_empty() {
            "敏感区域"
        } else {
            &reg.label
        };
        println!("  - {label} @ {r:?}");
    }
    let _ = clipboard::write_image(&out, CopyFormat::Both);
    Ok(())
}
