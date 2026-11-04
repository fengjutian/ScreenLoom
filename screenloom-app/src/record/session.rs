//! 录制会话：worker 线程 + 控制面（暂停/停止/放弃）+ 安全收尾。
//!
//! 线程模型：
//!
//! ```text
//! UI 线程                          worker 线程
//! ────────                         ───────────
//! RecordSession::start  ──spawn──► 初始化 WGC/MF（失败回传 Err）
//!         ◄──init result──────────
//! pause()/stop()  ──AtomicBool──► 录制循环
//!         ◄──RecordEvent────────── Finished / Failed / Discarded
//! ```
//!
//! **所有停止路径都必须走到 `flush + Finalize`**，否则 MP4 缺 `moov`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use screenloom_capture::{CaptureTarget, ScreenStream};
use screenloom_core::{bgra_to_nv12_into, nv12_bytes_len, Error, Result};

use super::encoder::VideoEncoder;
use super::pacer::FramePacer;
use super::paths::RecordPaths;
use super::stats::{Phase, SharedStats};
use super::RecordOptions;

/// 收尾补写的帧数（编码器流水线尾部会丢 ~3 帧）。
const TAIL_PADDING_FRAMES: u32 = 5;
/// 取帧轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(4);
/// 暂停时的轮询间隔（省 CPU，仍能及时响应停止）。
const PAUSED_POLL: Duration = Duration::from_millis(20);
/// 低于这个可用空间就自动停止。
pub const MIN_FREE_BYTES: u64 = 200 * 1024 * 1024;

/// 停止原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    User,
    Duration,
    MaxBytes,
    DiskFull,
    TargetLost,
    EncoderError,
    AppExit,
}

impl StopReason {
    pub fn text(self) -> &'static str {
        match self {
            StopReason::User => "用户停止",
            StopReason::Duration => "达到时长上限",
            StopReason::MaxBytes => "达到文件大小上限",
            StopReason::DiskFull => "磁盘空间不足",
            StopReason::TargetLost => "录制目标消失",
            StopReason::EncoderError => "编码失败",
            StopReason::AppExit => "程序退出",
        }
    }
}

/// 录制结束事件（worker 线程 → UI 线程）。
#[derive(Debug)]
pub enum RecordEvent {
    Finished(RecordOutcome),
    Failed(String),
    Discarded(StopReason),
}

/// 录制结果。
#[derive(Clone, Debug)]
pub struct RecordOutcome {
    /// 正式文件路径。
    pub path: PathBuf,
    /// 实际写入帧数。
    pub frames: u64,
    /// 实际录制时长（不含暂停）。
    pub elapsed: Duration,
    /// 文件大小（字节）。
    pub bytes: u64,
}

/// 控制面：所有字段都是原子的，UI 线程可随时写。
struct Control {
    stop: AtomicBool,
    paused: AtomicBool,
    discard: AtomicBool,
    reason: Mutex<StopReason>,
}

impl Control {
    fn new() -> Self {
        Control {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            discard: AtomicBool::new(false),
            reason: Mutex::new(StopReason::User),
        }
    }

    fn request_stop(&self, reason: StopReason) {
        if let Ok(mut r) = self.reason.lock() {
            *r = reason;
        }
        self.stop.store(true, Ordering::Release);
    }

    fn request_discard(&self) {
        self.discard.store(true, Ordering::Release);
        self.request_stop(StopReason::User);
    }

    fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    fn reason(&self) -> StopReason {
        self.reason.lock().map(|r| *r).unwrap_or(StopReason::User)
    }
}

/// 一个正在运行（或刚结束）的录制会话。
pub struct RecordSession {
    control: Arc<Control>,
    stats: Arc<SharedStats>,
    paths: RecordPaths,
    join: Option<JoinHandle<()>>,
}

impl RecordSession {
    /// 启动录制并等待初始化结果。
    ///
    /// 初始化（建 WGC 会话 + 建 MF 编码器）在 worker 里完成，成功后才返回；
    /// 初始化失败会直接返回 `Err`，不会留下空文件。
    pub fn start<F>(opts: &RecordOptions, stats: Arc<SharedStats>, on_event: F) -> Result<Self>
    where
        F: Fn(RecordEvent) + Send + Sync + 'static,
    {
        let paths = RecordPaths::new(opts.out_path.clone())?;
        paths.prepare()?;
        let control = Arc::new(Control::new());
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);

        let opts = opts.clone();
        let control2 = Arc::clone(&control);
        let stats2 = Arc::clone(&stats);
        let paths2 = paths.clone();
        let on_event = Arc::new(on_event);
        let join = std::thread::Builder::new()
            .name("screenloom-record".into())
            .spawn(move || {
                run_worker(opts, control2, stats2, paths2, init_tx, on_event);
            })
            .map_err(|e| Error::Window(format!("启动录制线程失败: {e}")))?;

        match init_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(RecordSession {
                control,
                stats,
                paths,
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => {
                control.request_stop(StopReason::AppExit);
                let _ = join.join();
                Err(Error::Capture("录制初始化超时".into()))
            }
        }
    }

    pub fn stats(&self) -> Arc<SharedStats> {
        Arc::clone(&self.stats)
    }

    pub fn path(&self) -> &PathBuf {
        &self.paths.final_path
    }

    pub fn pause(&self) {
        self.control.paused.store(true, Ordering::Release);
    }

    pub fn resume(&self) {
        self.control.paused.store(false, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }

    pub fn stop(&self, reason: StopReason) {
        self.control.request_stop(reason);
    }

    pub fn discard(&self) {
        self.control.request_discard();
    }

    /// 等待 worker 结束（事件到达后调用，通常很快）。
    pub fn join(mut self) -> Result<()> {
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
        Ok(())
    }
}

fn run_worker(
    opts: RecordOptions,
    control: Arc<Control>,
    stats: Arc<SharedStats>,
    paths: RecordPaths,
    init_tx: mpsc::SyncSender<Result<()>>,
    on_event: Arc<dyn Fn(RecordEvent) + Send + Sync>,
) {
    // ---- 初始化：磁盘 → 帧源 → 编码器 ----
    let init = (|| -> Result<(ScreenStream, VideoEncoder)> {
        check_free_space(paths.partial.parent())?;
        let target = match opts.target {
            super::RecordTarget::Monitor(i) => CaptureTarget::Monitor(i),
        };
        let stream = ScreenStream::start(target)?;
        let (w, h) = stream.size();
        let enc = VideoEncoder::new(&paths.partial, w, h, opts.fps, opts.bitrate)?;
        Ok((stream, enc))
    })();

    let (mut stream, mut enc) = match init {
        Ok(v) => {
            let _ = init_tx.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = init_tx.send(Err(Error::Capture(e.to_string())));
            paths.discard();
            stats.set_phase(Phase::Failed);
            on_event(RecordEvent::Failed(e.to_string()));
            return;
        }
    };

    // ---- 倒计时（可被停止取消） ----
    if !countdown(&control, &stats, opts.countdown) {
        paths.discard();
        stats.set_phase(Phase::Idle);
        on_event(RecordEvent::Discarded(control.reason()));
        return;
    }

    stats.set_phase(Phase::Recording);

    // ---- 录制循环 ----
    let (w, h) = stream.size();
    let mut nv12 = vec![0u8; nv12_bytes_len(w, h)];
    let mut have_frame = false;
    let start = Instant::now();
    let mut paused_total = Duration::ZERO;
    let mut paused_at: Option<Instant> = None;
    let mut pacer = FramePacer::new(opts.fps, start);
    let mut frames = 0u64;
    let mut last_stat = Instant::now();
    let mut reason = StopReason::User;

    loop {
        if control.is_stopped() {
            reason = control.reason();
            break;
        }

        let now = Instant::now();
        if control.is_paused() {
            if paused_at.is_none() {
                paused_at = Some(now);
            }
            stats.set_phase(Phase::Paused);
            stats.set_elapsed_ms(
                (now.saturating_duration_since(start) - paused_total).as_millis() as u64,
            );
            std::thread::sleep(PAUSED_POLL);
            continue;
        } else if let Some(t) = paused_at.take() {
            // 暂停时间从成片时间轴里剔除：恢复时把写帧节奏重新对齐到当前时刻。
            paused_total += now.saturating_duration_since(t);
            pacer.rebase(now);
            stats.set_phase(Phase::Recording);
        }

        // 取最新帧；没有新画面就沿用上一帧。
        match stream.take_latest() {
            Ok(Some(frame)) => {
                if let Err(e) = bgra_to_nv12_into(&frame.data, frame.w, frame.h, &mut nv12) {
                    reason = StopReason::EncoderError;
                    eprintln!("录屏色彩转换失败: {e}");
                    break;
                }
                have_frame = true;
            }
            Ok(None) => {}
            Err(e) => {
                reason = StopReason::TargetLost;
                eprintln!("录屏抓帧失败: {e}");
                break;
            }
        }

        if have_frame {
            if let Some(pts) = pacer.tick(now) {
                if let Err(e) = enc.write_frame(&nv12, pts) {
                    reason = StopReason::EncoderError;
                    eprintln!("录屏写帧失败: {e}");
                    break;
                }
                frames += 1;
                stats.set_frames(frames);
            }
        }

        let active = now.saturating_duration_since(start) - paused_total;
        if let Some(d) = opts.duration {
            if active >= d {
                reason = StopReason::Duration;
                break;
            }
        }

        // 统计 / 磁盘检查：1 Hz 足够，不要每帧都碰文件系统。
        if last_stat.elapsed() >= Duration::from_millis(500) {
            last_stat = Instant::now();
            stats.set_elapsed_ms(active.as_millis() as u64);
            if let Ok(meta) = std::fs::metadata(&paths.partial) {
                stats.set_bytes(meta.len());
                if let Some(max) = opts.max_bytes {
                    if meta.len() >= max {
                        reason = StopReason::MaxBytes;
                        break;
                    }
                }
            }
            if free_bytes(paths.partial.parent()).unwrap_or(u64::MAX) < MIN_FREE_BYTES {
                reason = StopReason::DiskFull;
                break;
            }
        }

        // 睡到下次写帧，但不超过轮询间隔，保证能及时取帧/响应停止。
        let until_write = pacer.next_write_after(Instant::now());
        let sleep = until_write.min(POLL_INTERVAL);
        if !sleep.is_zero() {
            std::thread::sleep(sleep);
        }
    }

    // ---- 收尾：无论成功/失败/放弃，都必须 Finalize ----
    stats.set_phase(Phase::Stopping);
    let active = start.elapsed().saturating_sub(paused_total);
    if !control.discard.load(Ordering::Acquire) && have_frame {
        for _ in 0..TAIL_PADDING_FRAMES {
            let pts = pacer.pts();
            let _ = enc.write_frame(&nv12, pts);
            pacer.advance();
            frames += 1;
        }
    }
    let finish_result = enc.finish();

    if control.discard.load(Ordering::Acquire) {
        paths.discard();
        stats.set_phase(Phase::Idle);
        on_event(RecordEvent::Discarded(reason));
        return;
    }

    if let Err(e) = finish_result {
        // 收尾失败：保留 .partial，明确告诉用户文件可能损坏。
        stats.set_phase(Phase::Failed);
        on_event(RecordEvent::Failed(format!(
            "{e}（已保留临时文件 {}）",
            paths.partial.display()
        )));
        return;
    }

    if let Err(e) = paths.finalize() {
        stats.set_phase(Phase::Failed);
        on_event(RecordEvent::Failed(format!(
            "保存失败: {e}（临时文件 {}）",
            paths.partial.display()
        )));
        return;
    }

    let bytes = std::fs::metadata(&paths.final_path)
        .map(|m| m.len())
        .unwrap_or(0);
    stats.set_bytes(bytes);
    stats.set_elapsed_ms(active.as_millis() as u64);
    stats.set_phase(Phase::Finished);
    on_event(RecordEvent::Finished(RecordOutcome {
        path: paths.final_path.clone(),
        frames,
        elapsed: active,
        bytes,
    }));
}

fn countdown(control: &Control, stats: &SharedStats, total: Duration) -> bool {
    if total.is_zero() {
        return !control.is_stopped();
    }
    stats.set_phase(Phase::Countdown);
    let start = Instant::now();
    while start.elapsed() < total {
        if control.is_stopped() {
            return false;
        }
        stats.set_elapsed_ms(start.elapsed().as_millis() as u64);
        std::thread::sleep(Duration::from_millis(50));
    }
    !control.is_stopped()
}

fn check_free_space(dir: Option<&std::path::Path>) -> Result<()> {
    if free_bytes(dir).unwrap_or(u64::MAX) < MIN_FREE_BYTES {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "磁盘可用空间不足 200 MB",
        )));
    }
    Ok(())
}

fn free_bytes(dir: Option<&std::path::Path>) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let dir = dir?;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut free as *mut u64),
            None,
            None,
        )
        .ok()?;
    }
    Some(free)
}
