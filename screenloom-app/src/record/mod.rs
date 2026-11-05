//! 录屏编排层。
//!
//! 模块切分：
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`pacer`] | 定速写帧、CFR 合成时间戳、暂停恢复 |
//! | [`paths`] | 临时文件、唯一文件名、原子改名 |
//! | [`stats`] | 原子统计，HUD/托盘读取 |
//! | [`session`] | worker 线程、暂停/停止/放弃、安全收尾 |
//! | [`encoder`] | Media Foundation H.264 编码器 |
//! | [`hud`] | REC 浮窗 |
//! | [`controller`] | 常驻态单例：热键/托盘/HUD 与 session 的粘合 |
//!
//! 帧率策略与 MF 踩坑见 `doc/录屏方案.md`；需求与验收见 `doc/录屏功能需求.md`。

pub mod audio;
pub mod controller;
pub mod encoder;
pub mod hud;
pub mod mux;
pub mod pacer;
pub mod paths;
pub mod session;
pub mod source;
pub mod stats;
pub mod wav;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use screenloom_core::{Config, Error, Result};

use stats::SharedStats;

pub use session::{RecordEvent, RecordOutcome, RecordSession, StopReason};

/// 默认帧率。
pub const DEFAULT_FPS: u32 = 30;
/// 默认码率（1080p 录屏 8~12 Mbps 是常见区间）。
pub const DEFAULT_BITRATE: u32 = 8_000_000;

/// 录制目标。
///
/// 目前只做单显示器；区域/窗口在 P1。这里刻意不直接用 `CaptureTarget`：
/// 它含 `HWND`，而录制要跨线程，保持 options 可 `Send`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordTarget {
    Monitor(usize),
    /// 单个窗口（HWND 以 isize 保存，保持 options 可 Send）
    Window(isize),
    /// 整块虚拟屏（多显示器合成）
    VirtualScreen,
    /// 屏幕区域（虚拟屏物理像素）
    Region(screenloom_core::Rect),
}

/// 录制参数。
#[derive(Clone, Debug)]
pub struct RecordOptions {
    pub target: RecordTarget,
    pub out_path: PathBuf,
    pub fps: u32,
    pub bitrate: u32,
    /// `None` = 一直录到外部停止。
    pub duration: Option<Duration>,
    /// 开始前倒计时。
    pub countdown: Duration,
    /// 是否把鼠标指针录进画面。
    pub cursor: bool,
    /// 是否录制系统声音（后置混流）。
    pub audio_system: bool,
    /// 最大文件大小；`None` = 不限。
    pub max_bytes: Option<u64>,
}

impl RecordOptions {
    /// 默认参数：主显示器 → `Videos\Screenloom\<时间戳>.mp4`，30 fps，8 Mbps。
    pub fn defaults() -> Result<RecordOptions> {
        Ok(RecordOptions {
            target: RecordTarget::Monitor(primary_monitor_index()?),
            out_path: default_record_path(),
            fps: DEFAULT_FPS,
            bitrate: DEFAULT_BITRATE,
            duration: None,
            countdown: Duration::ZERO,
            cursor: true,
            audio_system: false,
            max_bytes: None,
        })
    }

    /// 从配置构造（常驻模式）。
    pub fn from_config(cfg: &Config) -> Result<RecordOptions> {
        let target = if cfg.record_monitor >= 0 {
            RecordTarget::Monitor(cfg.record_monitor as usize)
        } else {
            RecordTarget::Monitor(primary_monitor_index()?)
        };
        let dir = cfg.record_dir.clone();
        Ok(RecordOptions {
            target,
            out_path: default_record_path_in(dir.as_deref()),
            fps: cfg.record_fps.clamp(1, 120),
            bitrate: cfg.record_bitrate.max(100_000),
            duration: if cfg.record_max_seconds > 0 {
                Some(Duration::from_secs(cfg.record_max_seconds as u64))
            } else {
                None
            },
            countdown: Duration::from_secs(cfg.record_countdown.min(10) as u64),
            cursor: cfg.record_cursor,
            // 音频后置混流仍未稳定（MF SourceReader/视频透传会截断），暂不启用。
            audio_system: false,
            max_bytes: if cfg.record_max_mb > 0 {
                Some(cfg.record_max_mb as u64 * 1024 * 1024)
            } else {
                None
            },
        })
    }
}

/// 一次性阻塞录制（CLI `--record N` 用）。
///
/// UI/常驻模式请用 [`RecordSession`] + [`controller`]。
pub fn record(opts: &RecordOptions) -> Result<RecordOutcome> {
    let stats = std::sync::Arc::new(SharedStats::new());
    let (tx, rx) = mpsc::channel();
    let session = RecordSession::start(opts, stats, move |event| {
        let _ = tx.send(event);
    })?;

    // 事件在 worker 收尾后发送；没有外部停止时由 duration 结束。
    let event = rx
        .recv_timeout(Duration::from_secs(24 * 60 * 60))
        .map_err(|_| Error::Capture("录制等待超时".into()))?;
    session.join()?;

    match event {
        RecordEvent::Finished(out) => Ok(out),
        RecordEvent::Discarded(reason) => Err(Error::Capture(format!(
            "录制已放弃（{}）",
            reason.text()
        ))),
        RecordEvent::Failed(e) => Err(Error::Capture(e)),
    }
}

/// 默认录制输出路径：`%USERPROFILE%\Videos\Screenloom\<时间戳>.mp4`。
pub fn default_record_path() -> PathBuf {
    default_record_path_in(None)
}

/// 指定目录下的默认录制路径。
pub fn default_record_path_in(dir: Option<&Path>) -> PathBuf {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Videos")
            .join("Screenloom"),
    };
    dir.join(format!("{}.mp4", crate::save::timestamp()))
}

/// 主显示器下标。
pub fn primary_monitor_index() -> Result<usize> {
    let monitors = screenloom_capture::list_monitors()?;
    monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .map(|m| m.index)
        .ok_or_else(|| Error::Capture("未找到任何显示器".into()))
}

pub use paths::check_out_ext;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_is_under_videos_screenloom() {
        let p = default_record_path();
        let s = p.to_string_lossy().to_ascii_lowercase();
        assert!(s.contains("videos"), "路径应含 Videos: {s}");
        assert!(s.contains("screenloom"), "路径应含 Screenloom: {s}");
        assert!(s.ends_with(".mp4"), "应是 .mp4: {s}");
    }

    #[test]
    fn custom_dir_is_respected() {
        let p = default_record_path_in(Some(Path::new(r"D:\clips")));
        assert!(p.starts_with(r"D:\clips"));
        assert!(p.to_string_lossy().ends_with(".mp4"));
    }

    #[test]
    fn config_limits_become_record_options() {
        let mut cfg = screenloom_core::Config::default();
        cfg.record_max_seconds = 30;
        cfg.record_max_mb = 10;
        let o = RecordOptions::from_config(&cfg).unwrap();
        assert_eq!(o.duration, Some(Duration::from_secs(30)));
        assert_eq!(o.max_bytes, Some(10 * 1024 * 1024));
    }
}
