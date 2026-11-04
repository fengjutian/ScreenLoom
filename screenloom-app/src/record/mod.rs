//! 录屏会话编排。
//!
//! # 帧率策略（定速写帧 + 重复上一帧）
//!
//! WGC 的出帧速率由**画面变化**决定 —— 静态桌面可能几秒才产生一帧。
//! 所以不能简单地"来一帧写一帧"，否则：
//!
//! - 静止 10 秒 → 视频里这 10 秒被**压缩掉**（看起来像快进）
//! - 按真实墙钟写 pts → 采集抖动（4~20 ms 的帧间隙）会带进码流，**播放卡顿**
//!
//! 本实现的做法（见 `doc/录屏方案.md` §5.3）：
//!
//! 1. **定速写帧**：不管有没有新画面，都按目标帧率（默认 30 fps）往编码器写。
//! 2. **重复上一帧**：没有新帧时，把上一帧的 NV12 缓存再写一遍（不重新做色彩转换，几乎不花 CPU）。
//! 3. **合成时间戳**：`pts = 已写帧数 × 帧间隔`（100 ns 单位）→ 输出严格 CFR，帧间距均匀。
//!
//! 结果：视频时长 = 真实录制时长，帧率恒定，播放不卡顿，静止段不会丢。
//!
//! # 线程
//!
//! 本模块的 [`record`] 函数**必须在已进入 COM MTA 套间的线程上调用**
//! （内部的 `ScreenStream` / `VideoEncoder` 会各自进入并持有套间守卫）。

pub mod encoder;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use screenloom_capture::{list_monitors, CaptureTarget, ScreenStream};
use screenloom_core::{bgra_to_nv12_into, nv12_bytes_len, Error, Result};

use encoder::VideoEncoder;

/// 收尾补写的帧数。
///
/// 编码器流水线的最后几帧不会被 `Flush` 冲出来（实测固定丢 ~3 帧 ≈ 100 ms），
/// 收尾时把最后一帧再写几遍即可补上。
const TAIL_PADDING_FRAMES: u32 = 5;

/// 轮询帧源的最大间隔（越小越及时，代价是更多空转）。
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// 默认目标帧率。
pub const DEFAULT_FPS: u32 = 30;

/// 默认目标码率（1080p 录屏 8~12 Mbps 是常见区间）。
pub const DEFAULT_BITRATE: u32 = 8_000_000;

/// 录制参数。
#[derive(Clone, Debug)]
pub struct RecordOptions {
    /// 捕获目标（首版只支持单显示器）。
    pub target: CaptureTarget,
    /// 输出文件路径。
    pub out_path: PathBuf,
    /// 目标帧率。
    pub fps: u32,
    /// 目标码率（bps）。
    pub bitrate: u32,
    /// 录制时长；`None` 表示一直录到外部叫停。
    pub duration: Option<Duration>,
}

impl RecordOptions {
    /// 默认参数：主显示器 → `Videos\Screenloom\<时间戳>.mp4`，30 fps，8 Mbps。
    pub fn defaults() -> Result<RecordOptions> {
        let monitors = list_monitors()?;
        let primary = monitors
            .iter()
            .find(|m| m.is_primary)
            .or_else(|| monitors.first())
            .ok_or_else(|| Error::Capture("未找到任何显示器".into()))?;
        Ok(RecordOptions {
            target: CaptureTarget::Monitor(primary.index),
            out_path: default_record_path(),
            fps: DEFAULT_FPS,
            bitrate: DEFAULT_BITRATE,
            duration: None,
        })
    }
}

/// 定速写帧的时间轴。
///
/// 负责两件事：
/// 1. **节流**：不到点就不写帧（保证输出恒定帧率）。
/// 2. **合成时间戳**：`pts = 已写帧数 × 帧间隔`（100 ns 单位）→ 输出严格 CFR。
///
/// 机器卡顿时**不补写风暴**：落后太多就把时间轴重新对齐到当前时刻。
#[derive(Clone, Debug)]
pub struct FramePacer {
    /// 帧间隔（墙钟）
    interval: Duration,
    /// 一帧的时长（100 ns）
    duration_100ns: i64,
    /// 下一次该写帧的墙钟时刻
    next_write: Instant,
    /// 下一个样本的时间戳（100 ns）
    pts: i64,
    /// 已写帧数
    frames: u64,
}

impl FramePacer {
    /// 以 `start` 为起点创建时间轴。
    pub fn new(fps: u32, start: Instant) -> FramePacer {
        let fps = fps.max(1);
        FramePacer {
            interval: Duration::from_nanos(1_000_000_000 / fps as u64),
            duration_100ns: 10_000_000 / fps as i64,
            next_write: start,
            pts: 0,
            frames: 0,
        }
    }

    /// 到点了吗？到点则推进时间轴并返回该帧应使用的 pts（100 ns）。
    pub fn tick(&mut self, now: Instant) -> Option<i64> {
        if now < self.next_write {
            return None;
        }
        let pts = self.pts;
        self.pts += self.duration_100ns;
        self.frames += 1;
        self.next_write += self.interval;
        // 落后太多（机器卡了 / 长时间没帧）→ 重新对齐，不要一口气补写一堆帧
        if self.next_write < now {
            self.next_write = now + self.interval;
        }
        Some(pts)
    }

    /// 已写帧数。
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// 下一个样本的时间戳（100 ns）。
    pub fn pts(&self) -> i64 {
        self.pts
    }

    /// 一帧的时长（100 ns）。
    pub fn duration_100ns(&self) -> i64 {
        self.duration_100ns
    }

    /// 距离下一次该写帧还有多久（已到点则为 0）。
    pub fn next_write_after(&self, now: Instant) -> Duration {
        self.next_write.saturating_duration_since(now)
    }

    /// 手动推进一帧（补帧用：时间戳继续往后走，但不改变墙钟节奏）。
    pub fn advance(&mut self) {
        self.pts += self.duration_100ns;
        self.frames += 1;
    }
}

/// 录制结果。
#[derive(Clone, Debug)]
pub struct RecordOutcome {
    /// 实际写入的路径。
    pub path: PathBuf,
    /// 写进文件的帧数。
    pub frames: u64,
    /// 实际录制时长。
    pub elapsed: Duration,
}

/// 执行一次录制。
///
/// 必须在已进入 COM MTA 套间的线程上调用。
pub fn record(opts: &RecordOptions) -> Result<RecordOutcome> {
    if let Some(parent) = opts.out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
    }

    let mut stream = ScreenStream::start(opts.target)?;
    let (w, h) = stream.size();
    let mut enc = VideoEncoder::new(&opts.out_path, w, h, opts.fps, opts.bitrate)?;

    // NV12 缓存：既是编码器的输入，也是"重复上一帧"的来源。
    let mut nv12 = vec![0u8; nv12_bytes_len(w, h)];
    let mut have_frame = false;

    let start = Instant::now();
    let mut pacer = FramePacer::new(opts.fps, start);
    let mut frames: u64 = 0;

    loop {
        // 1) 取最新帧（积压的旧帧在流里已被丢弃）
        match stream.take_latest()? {
            Some(frame) => {
                bgra_to_nv12_into(&frame.data, frame.w, frame.h, &mut nv12)?;
                have_frame = true;
            }
            None => {
                // 画面没变 → 沿用上一帧的 NV12（不重复做色彩转换）
            }
        }

        // 2) 定速写帧（到点才写）
        let now = Instant::now();
        if have_frame {
            if let Some(pts) = pacer.tick(now) {
                enc.write_frame(&nv12, pts)?;
                frames += 1;
            }
        }

        // 3) 到时间就停
        if let Some(d) = opts.duration {
            if start.elapsed() >= d {
                break;
            }
        }

        // 4) 睡到下次写帧时刻（但不超过轮询间隔，保证能及时取到新帧）
        let now = Instant::now();
        let until_write = pacer.next_write_after(now);
        let sleep = until_write.min(POLL_INTERVAL);
        if !sleep.is_zero() {
            std::thread::sleep(sleep);
        }
    }

    // 5) 补帧：把最后一帧再写几遍，填上编码器丢掉的尾巴
    if have_frame {
        for _ in 0..TAIL_PADDING_FRAMES {
            let pts = pacer.pts();
            enc.write_frame(&nv12, pts)?;
            pacer.advance();
            frames += 1;
        }
    }
    enc.finish()?;

    Ok(RecordOutcome {
        path: opts.out_path.clone(),
        frames,
        elapsed: start.elapsed(),
    })
}

/// 默认录制输出路径：`%USERPROFILE%\Videos\Screenloom\<时间戳>.mp4`。
pub fn default_record_path() -> PathBuf {
    let videos = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Videos")
        .join("Screenloom");
    videos.join(format!("{}.mp4", crate::save::timestamp()))
}

/// 校验输出路径的扩展名（提醒用户别写错成 `.avi` 之类）。
pub fn check_out_ext(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "mp4" {
        return Err(Error::Image(format!(
            "录制输出应为 .mp4（当前 .{}）",
            if ext.is_empty() { "无扩展名" } else { &ext }
        )));
    }
    Ok(())
}

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
    fn extension_is_checked() {
        assert!(check_out_ext(Path::new("a.mp4")).is_ok());
        assert!(check_out_ext(Path::new("a.MP4")).is_ok());
        assert!(check_out_ext(Path::new("a.avi")).is_err());
        assert!(check_out_ext(Path::new("a")).is_err());
    }

    #[test]
    fn pacer_emits_synthetic_cfr_timestamps() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        // 第 1 帧：立刻
        assert_eq!(p.tick(t0), Some(0));
        // 还没到点（10 ms < 33.3 ms）→ 不写
        assert_eq!(p.tick(t0 + Duration::from_millis(10)), None);
        // 到点 → pts 递增一个帧间隔
        let pts2 = p.tick(t0 + Duration::from_millis(34)).unwrap();
        assert_eq!(pts2, 333_333);
        let pts3 = p.tick(t0 + Duration::from_millis(67)).unwrap();
        assert_eq!(pts3, 666_666);
        assert_eq!(p.frames(), 3);
    }

    #[test]
    fn pacer_does_not_burst_after_a_stall() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        assert_eq!(p.tick(t0), Some(0));
        // 卡了 5 秒 → 只应该补 1 帧，而不是一口气补 150 帧
        let mut written = 1;
        let now = t0 + Duration::from_secs(5);
        while let Some(_pts) = p.tick(now) {
            written += 1;
            assert!(written < 10, "卡顿后不应补写大量帧，实际写了 {written} 帧");
        }
        assert_eq!(written, 2, "卡顿后应只补 1 帧");
    }

    #[test]
    fn pacer_advance_moves_pts_only() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        assert_eq!(p.tick(t0), Some(0));
        p.advance();
        assert_eq!(p.pts(), 333_333);
        assert_eq!(p.frames(), 2);
    }
}
