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
    /// 隐私模式：本工具的**所有**窗口（浮窗 / 设置窗 / 贴图窗）对屏幕捕获不可见。
    ///
    /// 对齐微信的 `clicfg_screen_privacy` / `gScreenPrivacyEnabled`。
    /// 与 [`Config::hide_self`] 的区别：`hide_self` 是「截图前把自己藏起来」，
    /// 隐私模式是「**别人**截图时看不到我们」（`WDA_EXCLUDEFROMCAPTURE`）。
    pub privacy_mode: bool,
    /// 按窗口排除抓屏：截图时把这些窗口从结果里去掉。
    ///
    /// 条目格式 `标题|类名`，多个条目用 `;` 分隔。
    /// 存标题+类名而不是 HWND —— HWND 每次启动都会变。
    pub exclude_windows: Vec<String>,
    /// 工具栏折叠（对齐微信 `clicfg_pc_screenshot_toolbar_collapse`）
    pub toolbar_collapsed: bool,
    /// 复制到剪贴板的格式
    pub copy_format: CopyFormat,
    /// 长截图最大长度（物理像素），0 表示不限
    pub long_screenshot_max: u32,
    /// 长截图搜索窗口（物理像素）
    pub long_screenshot_search: i32,
    /// 录屏热键，默认 Alt+R
    pub record_hotkey: String,
    /// 录制输出目录；None 表示 `Videos\Screenloom`
    pub record_dir: Option<PathBuf>,
    /// 录制帧率（1–120）
    pub record_fps: u32,
    /// 录制码率（bps）
    pub record_bitrate: u32,
    /// 是否录制鼠标指针
    pub record_cursor: bool,
    /// 是否录制系统声音（WASAPI loopback，结束时后置混流）
    pub record_audio_system: bool,
    /// 是否录制默认麦克风（与系统声混合）
    pub record_audio_mic: bool,
    /// 开始录制前的倒计时（秒，0 = 不等待）
    pub record_countdown: u32,
    /// 是否显示 REC HUD
    pub record_show_hud: bool,
    /// 录制结束后打开所在文件夹
    pub record_open_folder: bool,
    /// 上次录制的显示器下标；-1 = 主显示器
    pub record_monitor: i32,
    /// 最长录制时长（秒，0 = 不限）
    pub record_max_seconds: u32,
    /// 最大录制文件（MB，0 = 不限）
    pub record_max_mb: u32,
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
            privacy_mode: false,
            exclude_windows: Vec::new(),
            toolbar_collapsed: false,
            copy_format: CopyFormat::Both,
            long_screenshot_max: 0,
            long_screenshot_search: 240,
            record_hotkey: "Alt+R".to_string(),
            record_dir: None,
            record_fps: 30,
            record_bitrate: 8_000_000,
            record_cursor: true,
            record_audio_system: false,
            record_audio_mic: false,
            record_countdown: 3,
            record_show_hud: true,
            record_open_folder: true,
            record_monitor: -1,
            record_max_seconds: 0,
            record_max_mb: 0,
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
                "privacy_mode" => cfg.privacy_mode = parse_bool(v).unwrap_or(cfg.privacy_mode),
                // 空值 = 不排除任何窗口；条目之间用 `;` 分隔。
                "exclude_windows" => {
                    cfg.exclude_windows = v
                        .split(';')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                }
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
                // ---- 录屏 ----
                "record_hotkey" => cfg.record_hotkey = v.to_string(),
                "record_dir" => {
                    cfg.record_dir = if v.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(v))
                    }
                }
                "record_fps" => cfg.record_fps = v.parse().unwrap_or(cfg.record_fps),
                "record_bitrate" => {
                    cfg.record_bitrate = v.parse().unwrap_or(cfg.record_bitrate)
                }
                "record_cursor" => {
                    cfg.record_cursor = parse_bool(v).unwrap_or(cfg.record_cursor)
                }
                "record_audio_system" => {
                    cfg.record_audio_system = parse_bool(v).unwrap_or(cfg.record_audio_system)
                }
                "record_audio_mic" => {
                    cfg.record_audio_mic = parse_bool(v).unwrap_or(cfg.record_audio_mic)
                }
                "record_countdown" => {
                    cfg.record_countdown = v.parse().unwrap_or(cfg.record_countdown)
                }
                "record_show_hud" => {
                    cfg.record_show_hud = parse_bool(v).unwrap_or(cfg.record_show_hud)
                }
                "record_open_folder" => {
                    cfg.record_open_folder = parse_bool(v).unwrap_or(cfg.record_open_folder)
                }
                "record_monitor" => {
                    cfg.record_monitor = v.parse().unwrap_or(cfg.record_monitor)
                }
                "record_max_seconds" => {
                    cfg.record_max_seconds = v.parse().unwrap_or(cfg.record_max_seconds)
                }
                "record_max_mb" => {
                    cfg.record_max_mb = v.parse().unwrap_or(cfg.record_max_mb)
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
             privacy_mode = {}\n\
             exclude_windows = {}\n\
             toolbar_collapsed = {}\n\
             copy_format = {}\n\
             long_screenshot_max = {}\n\
             long_screenshot_search = {}\n\
             record_hotkey = {}\n\
             record_dir = {}\n\
             record_fps = {}\n\
             record_bitrate = {}\n\
             record_cursor = {}\n\
             record_audio_system = {}\n\
             record_audio_mic = {}\n\
             record_countdown = {}\n\
             record_show_hud = {}\n\
             record_open_folder = {}\n\
             record_monitor = {}\n\
             record_max_seconds = {}\n\
             record_max_mb = {}\n\
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
            self.privacy_mode,
            self.exclude_windows.join(";"),
            self.toolbar_collapsed,
            match self.copy_format {
                CopyFormat::Png => "png",
                CopyFormat::Dib => "dib",
                CopyFormat::Both => "both",
            },
            self.long_screenshot_max,
            self.long_screenshot_search,
            self.record_hotkey,
            self.record_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            self.record_fps,
            self.record_bitrate,
            self.record_cursor,
            self.record_audio_system,
            self.record_audio_mic,
            self.record_countdown,
            self.record_show_hud,
            self.record_open_folder,
            self.record_monitor,
            self.record_max_seconds,
            self.record_max_mb,
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

    /// 旧版配置目录名（项目改名前的旧名）。
    ///
    /// 仅用于**一次性迁移**：这里必须是旧目录的真实名字，才能找到旧配置文件，
    /// 所以这里是**全仓唯一保留旧名字面量的地方**（其余所有代码/文档/脚本均已改名）。
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

/// 生成一个「排除窗口」条目（`标题|类名`）。
///
/// 标题 / 类名里的 `|` 和 `;` 会被替换掉 —— 它们是本格式的分隔符。
pub fn exclude_entry(title: &str, class: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c == '|' || c == ';' { ' ' } else { c })
            .collect::<String>()
    };
    format!("{}|{}", clean(title), clean(class))
}

/// 解析一个「排除窗口」条目（`标题|类名`）。
///
/// 格式不对时返回 `None`。
pub fn parse_exclude_entry(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let (title, class) = s.split_once('|')?;
    Some((title.trim().to_string(), class.trim().to_string()))
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

    #[test]
    fn privacy_mode_and_exclude_windows_roundtrip() {
        let dir = std::env::temp_dir().join("screenloom-cfg-privacy");
        let path = dir.join("config.txt");
        let cfg = Config {
            privacy_mode: true,
            exclude_windows: vec![
                exclude_entry("记事本", "Notepad"),
                exclude_entry("微信", "WeChatMainWndForPC"),
            ],
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert!(back.privacy_mode);
        assert_eq!(back.exclude_windows.len(), 2);
        assert_eq!(
            parse_exclude_entry(&back.exclude_windows[0]),
            Some(("记事本".to_string(), "Notepad".to_string()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exclude_entry_sanitizes_separators() {
        // `|` 和 `;` 是条目分隔符，标题里出现必须被替换掉
        let e = exclude_entry("a|b;c", "d|e");
        assert_eq!(e, "a b c|d e");
        assert_eq!(parse_exclude_entry(&e).unwrap().0, "a b c");
    }

    #[test]
    fn parse_exclude_entry_rejects_malformed() {
        assert_eq!(parse_exclude_entry("没有分隔符"), None);
    }
}
