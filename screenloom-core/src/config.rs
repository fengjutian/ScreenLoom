//! 配置：极简 `key=value` 文本格式，避免引入序列化依赖。
//!
//! 对齐微信侧的同类开关：`gScreenshotShortcutKey`、`gScreenShotHideWindow`、
//! `clicfg_pc_screenshot_toolbar_collapse`、`longscreenshot_use_grabwindow` 等。

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// 剪贴板写入格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
    /// 只写 PNG（部分应用不支持）
    Png,
    /// 只写 DIB（兼容性好，透明通道丢失）
    Dib,
    /// 两种都写（推荐）
    Both,
}

impl CopyFormat {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "png" => Some(CopyFormat::Png),
            "dib" => Some(CopyFormat::Dib),
            "both" => Some(CopyFormat::Both),
            _ => None,
        }
    }
}

/// 全局配置。
#[derive(Clone, Debug)]
pub struct Config {
    /// 截图热键，默认 Alt+A（对齐微信 `gScreenshotShortcutKey`）
    pub hotkey: String,
    /// 截图时隐藏本工具自身窗口
    pub hide_self: bool,
    /// 工具栏折叠（对齐微信 `clicfg_pc_screenshot_toolbar_collapse`）
    pub toolbar_collapsed: bool,
    /// 复制到剪贴板的格式
    pub copy_format: CopyFormat,
    /// 长截图最大长度（物理像素），0 表示不限
    pub long_screenshot_max: u32,
    /// 长截图搜索窗口（物理像素）
    pub long_screenshot_search: i32,
    /// 默认保存目录，None 表示每次询问
    pub default_dir: Option<PathBuf>,
    /// AI 服务方：`minimax`（默认）/ `deepseek`
    pub ai_provider: String,
    /// AI 模型名，空表示用 provider 默认模型
    pub ai_model: String,
    /// AI 端点，空表示用 provider 默认端点
    pub ai_endpoint: String,
    /// AI API key（通用）
    pub ai_api_key: String,
    /// MiniMax 专用 API key
    pub ai_key_minimax: String,
    /// DeepSeek 专用 API key
    pub ai_key_deepseek: String,
    /// AI 请求超时（秒）
    pub ai_timeout: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: "Alt+A".to_string(),
            hide_self: true,
            toolbar_collapsed: false,
            copy_format: CopyFormat::Both,
            long_screenshot_max: 0,
            long_screenshot_search: 240,
            default_dir: None,
            ai_provider: "minimax".to_string(),
            ai_model: String::new(),
            ai_endpoint: String::new(),
            ai_api_key: String::new(),
            ai_key_minimax: String::new(),
            ai_key_deepseek: String::new(),
            ai_timeout: 120,
        }
    }
}

impl Config {
    /// 读取配置；文件不存在时返回默认配置。
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        // 去掉 UTF-8 BOM（写的时候带，见 save()）
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut cfg = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "hotkey" => cfg.hotkey = v.to_string(),
                "hide_self" => cfg.hide_self = parse_bool(v).unwrap_or(cfg.hide_self),
                "toolbar_collapsed" => {
                    cfg.toolbar_collapsed = parse_bool(v).unwrap_or(cfg.toolbar_collapsed)
                }
                "copy_format" => cfg.copy_format = CopyFormat::parse(v).unwrap_or(cfg.copy_format),
                "long_screenshot_max" => {
                    cfg.long_screenshot_max = v.parse().unwrap_or(cfg.long_screenshot_max)
                }
                "long_screenshot_search" => {
                    cfg.long_screenshot_search = v.parse().unwrap_or(cfg.long_screenshot_search)
                }
                // 空值表示「不指定目录」，不能当成路径 ""（否则会存到当前工作目录）。
                "default_dir" => {
                    cfg.default_dir = if v.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(v))
                    }
                }
                // ---- AI 接入 ----
                "ai_provider" => cfg.ai_provider = v.to_string(),
                "ai_model" => cfg.ai_model = v.to_string(),
                "ai_endpoint" => cfg.ai_endpoint = v.to_string(),
                "ai_api_key" => cfg.ai_api_key = v.to_string(),
                "ai_key_minimax" => cfg.ai_key_minimax = v.to_string(),
                "ai_key_deepseek" => cfg.ai_key_deepseek = v.to_string(),
                "ai_timeout" => cfg.ai_timeout = v.parse().unwrap_or(cfg.ai_timeout),
                _ => {}
            }
        }
        Ok(cfg)
    }

    /// 写回配置文件（自动创建父目录）。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Config(e.to_string()))?;
        }
        let body = format!(
            "# screenloom 配置\n\
             hotkey = {}\n\
             hide_self = {}\n\
             toolbar_collapsed = {}\n\
             copy_format = {}\n\
             long_screenshot_max = {}\n\
             long_screenshot_search = {}\n\
             default_dir = {}\n\
             # ---- AI 接入（key 也可走环境变量 MINIMAX_API_KEY / DEEPSEEK_API_KEY）----\n\
             ai_provider = {}\n\
             ai_model = {}\n\
             ai_endpoint = {}\n\
             ai_api_key = {}\n\
             ai_key_minimax = {}\n\
             ai_key_deepseek = {}\n\
             ai_timeout = {}\n",
            self.hotkey,
            self.hide_self,
            self.toolbar_collapsed,
            match self.copy_format {
                CopyFormat::Png => "png",
                CopyFormat::Dib => "dib",
                CopyFormat::Both => "both",
            },
            self.long_screenshot_max,
            self.long_screenshot_search,
            self.default_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            self.ai_provider,
            self.ai_model,
            self.ai_endpoint,
            self.ai_api_key,
            self.ai_key_minimax,
            self.ai_key_deepseek,
            self.ai_timeout,
        );
        // 带 UTF-8 BOM：记事本 / PowerShell 才能正确显示中文注释
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(body.as_bytes());
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// 默认配置路径：`%APPDATA%\screenloom\config.txt`
    pub fn default_path() -> PathBuf {
        appdata_dir().join("screenloom").join("config.txt")
    }

    /// 旧版配置目录名（项目改名前）。仅用于一次性迁移。
    const LEGACY_DIR: &'static str = "weshot";

    /// 若存在旧版配置而新版缺失，则复制过来（**保留旧文件作备份**）。
    ///
    /// 返回迁移后的配置路径；没发生迁移则返回 `None`。
    pub fn migrate_legacy_config() -> Option<PathBuf> {
        let new_path = Self::default_path();
        if new_path.exists() {
            return None;
        }
        let legacy = appdata_dir().join(Self::LEGACY_DIR).join("config.txt");
        if !legacy.is_file() {
            return None;
        }
        if let Some(parent) = new_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return None;
            }
        }
        // 复制（不移动）：旧文件保留作备份，迁移失败也不丢原数据。
        if std::fs::copy(&legacy, &new_path).is_err() {
            return None;
        }
        Some(new_path)
    }
}

/// `%APPDATA%` 根目录；取不到时退回当前目录。
fn appdata_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrip() {
        let dir = std::env::temp_dir().join("screenloom-cfg-test");
        let path = dir.join("config.txt");
        let cfg = Config {
            hotkey: "Ctrl+Shift+S".into(),
            hide_self: false,
            copy_format: CopyFormat::Png,
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(back.hotkey, "Ctrl+Shift+S");
        assert!(!back.hide_self);
        assert_eq!(back.copy_format, CopyFormat::Png);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let cfg = Config::load(Path::new(r"Z:\__no_such__\config.txt")).unwrap();
        assert_eq!(cfg.hotkey, "Alt+A");
    }

    #[test]
    fn empty_default_dir_means_none() {
        // 空值不能被当成路径 ""，否则截图会存到当前工作目录。
        let dir = std::env::temp_dir().join("screenloom-cfg-empty-dir");
        let path = dir.join("config.txt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "default_dir =\n").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert!(cfg.default_dir.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
