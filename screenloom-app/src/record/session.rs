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

use screenloom_core::{bgra_to_nv12_into, nv12_bytes_len, Error, Result};

use super::audio::AudioCapture;
use super::encoder::VideoEncoder;
use super::wav::WavWriter;
use super::pacer::FramePacer;
use super::paths::RecordPaths;
use super::source::FrameSource;
use super::stats::{Phase, SharedStats, Stage};
use super::RecordOptions;

/// 收尾补写的帧数（编码器流水线尾部会丢 ~3 帧）。
const TAIL_PADDING_FRAMES: u32 = 5;
/// 取帧轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(4);
/// 暂停时的轮询间隔（省 CPU，仍能及时响应停止）。
const PAUSED_POLL: Duration = Duration::from_millis(20);
/// 低于这个可用空间就自动停止。
pub const MIN_FREE_BYTES: u64 = 200 * 1024 * 1024;
/// 看门狗轮询间隔。
const WATCHDOG_POLL: Duration = Duration::from_millis(500);
/// 心跳停滞超过这个时长就判定录制线程卡死（MF 内部阻塞）。
const STALL_TIMEOUT_MS: u64 = 8_000;

/// 看门狗阈值。可用 `SCREENLOOM_STALL_TIMEOUT_MS` 覆盖（排障 / 自测用）。
fn stall_timeout_ms() -> u64 {
    std::env::var("SCREENLOOM_STALL_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(STALL_TIMEOUT_MS)
}

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
    /// 看门狗判定录制线程卡死后置位：用于阻止 worker 再发一次终态事件。
    aborted: AtomicBool,
    reason: Mutex<StopReason>,
}

impl Control {
    fn new() -> Self {
        Control {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            discard: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
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
        if let Some(parent) = paths.partial.parent() {
            for p in super::paths::recover_partials(parent) {
                eprintln!("已恢复上次异常中断的录制: {}", p.display());
            }
        }
        let control = Arc::new(Control::new());
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);

        let opts = opts.clone();
        let control2 = Arc::clone(&control);
        let stats2 = Arc::clone(&stats);
        let paths2 = paths.clone();
        let paths_wd = paths.clone();
        let on_event: Arc<dyn Fn(RecordEvent) + Send + Sync> = Arc::new(on_event);
        let join = std::thread::Builder::new()
            .name("screenloom-record".into())
            .spawn(move || {
                // 内层线程：真正跑录制。它可能**永久阻塞**在 Media Foundation 内部
                // （实测约 9% 的录制会卡在 WriteSample / Flush / Finalize 里出不来），
                // 所以外层必须有看门狗，不能让整个程序跟着挂死。
                let (done_tx, done_rx) = mpsc::channel::<()>();
                let stats_inner = Arc::clone(&stats2);
                let on_event_inner = Arc::clone(&on_event);
                let control_inner = Arc::clone(&control2);
                let _inner = std::thread::Builder::new()
                    .name("screenloom-record-inner".into())
                    .spawn(move || {
                        run_worker(
                            opts,
                            control_inner,
                            stats_inner,
                            paths2,
                            init_tx,
                            on_event_inner,
                        );
                        let _ = done_tx.send(());
                    });

                // 看门狗：录制线程心跳停滞超过阈值 → 判定 MF 内部阻塞 → 中止并报错。
                run_watchdog(
                    &stats2,
                    &control2,
                    &on_event,
                    &done_rx,
                    stall_timeout_ms(),
                    WATCHDOG_POLL,
                    &paths_wd,
                );
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
    // 看门狗判定录制线程卡死后，worker 不能再发终态事件（否则会重复上报）。
    let emit = |ev: RecordEvent| {
        if !control.aborted.load(Ordering::Acquire) {
            on_event(ev);
        }
    };

    // ---- 初始化：磁盘 → 帧源 → 音频 → 视频编码器 ----
    let wav_path = paths.partial.with_extension("wav");
    let init = (|| -> Result<(FrameSource, VideoEncoder, Option<AudioCapture>, Option<WavWriter>)> {
        check_free_space(paths.partial.parent())?;
        let stream = FrameSource::open(opts.target, opts.fps, opts.cursor)?;
        // 音频失败只降级为纯视频，不能因此丢掉整个录制。
        let audio = if opts.audio_system {
            match AudioCapture::start_system() {
                Ok(a) => Some(a),
                Err(e) => {
                    eprintln!("系统声音初始化失败，降级为纯视频: {e}");
                    None
                }
            }
        } else {
            None
        };
        let wav = if audio.is_some() {
            Some(WavWriter::create(&wav_path)?)
        } else {
            None
        };
        let (w, h) = stream.size();
        let enc = VideoEncoder::new(&paths.partial, w, h, opts.fps, opts.bitrate)?;
        Ok((stream, enc, audio, wav))
    })();

    let (mut stream, mut enc, audio, mut wav) = match init {
        Ok(v) => {
            let _ = init_tx.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = init_tx.send(Err(Error::Capture(e.to_string())));
            paths.discard();
            stats.set_phase(Phase::Failed);
            emit(RecordEvent::Failed(e.to_string()));
            return;
        }
    };

    // ---- 倒计时（可被停止取消） ----
    if !countdown(&control, &stats, opts.countdown) {
        paths.discard();
        stats.set_phase(Phase::Idle);
        emit(RecordEvent::Discarded(control.reason()));
        return;
    }

    stats.set_phase(Phase::Recording);

    // 丢掉初始化/倒计时期间积压的音频，避免 WAV 比视频长。
    if let Some(a) = &audio {
        let _ = a.drain();
    }

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
            // 暂停期间也要刷新心跳，否则看门狗会把"暂停很久"误判成卡死。
            stats.touch();
            // 暂停期间的音频直接丢掉，成片不包含暂停时间。
            if let Some(a) = &audio {
                let _ = a.drain();
            }
            std::thread::sleep(PAUSED_POLL);
            continue;
        } else if let Some(t) = paused_at.take() {
            // 暂停时间从成片时间轴里剔除：恢复时把写帧节奏重新对齐到当前时刻。
            paused_total += now.saturating_duration_since(t);
            pacer.rebase(now);
            stats.set_phase(Phase::Recording);
        }

        // 取最新帧；没有新画面就沿用上一帧。
        stats.set_stage(Stage::Capture);
        stats.touch();
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
                stats.set_stage(Stage::Write);
                if let Err(e) = enc.write_frame(&nv12, pts) {
                    reason = StopReason::EncoderError;
                    eprintln!("录屏写帧失败: {e}");
                    break;
                }
                frames += 1;
                stats.set_frames(frames);
                stats.set_stage(Stage::Capture);
            }
        }

        if let (Some(a), Some(w)) = (&audio, wav.as_mut()) {
            let pcm = a.drain();
            if !pcm.is_empty() {
                if let Err(e) = w.write(&pcm) {
                    eprintln!("写音频 WAV 失败: {e}");
                }
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
        stats.set_stage(Stage::Tail);
        for _ in 0..TAIL_PADDING_FRAMES {
            let pts = pacer.pts();
            stats.set_stage(Stage::Write);
            let _ = enc.write_frame(&nv12, pts);
            stats.set_stage(Stage::Tail);
            pacer.advance();
            frames += 1;
        }
    }
    // 音频收尾：把缓冲里最后一段写进 WAV，再回填 WAV 头。
    if let (Some(a), Some(w)) = (&audio, wav.as_mut()) {
        let pcm = a.drain();
        if !pcm.is_empty() {
            let _ = w.write(&pcm);
        }
        let _ = w.finish();
    }
    // 分开标记 Flush / Finalize：MF 内部偶发阻塞时要能定位卡在哪一步。
    stats.set_stage(Stage::Flush);
    let finish_result = enc.flush().and_then(|()| {
        stats.set_stage(Stage::Finalize);
        enc.finalize()
    });

    if control.discard.load(Ordering::Acquire) {
        paths.discard();
        let _ = std::fs::remove_file(&wav_path);
        stats.set_phase(Phase::Idle);
        emit(RecordEvent::Discarded(reason));
        return;
    }

    if let Err(e) = finish_result {
        // 收尾失败：保留 .partial，明确告诉用户文件可能损坏。
        stats.set_phase(Phase::Failed);
        emit(RecordEvent::Failed(format!(
            "{e}（已保留临时文件 {}）",
            paths.partial.display()
        )));
        return;
    }

    // 释放 SinkWriter 对 .partial.mp4 的独占句柄，否则 SourceReader 打不开它。
    drop(enc);
    std::thread::sleep(Duration::from_millis(200));

    // 有音频 → 后置混流；失败则保留纯视频 + WAV，绝不能把视频也弄丢。
    let mut muxed = false;
    if let Some(w) = wav.as_ref() {
        if w.has_data() {
            stats.set_stage(Stage::Mux);
            let mux_path = paths.partial.with_extension("muxed.mp4");
            // 复制一份再读：某些驱动/滤镜会短暂保留写入句柄，直接读会 E_ACCESSDENIED。
            let mux_src = paths.partial.with_extension("muxsrc.mp4");
            let src = match std::fs::copy(&paths.partial, &mux_src) {
                Ok(_) => mux_src.clone(),
                Err(_) => paths.partial.clone(),
            };
            match super::mux::mux(&src, &wav_path, &mux_path) {
                Ok(()) => match std::fs::rename(&mux_path, &paths.final_path) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&paths.partial);
                        let _ = std::fs::remove_file(&wav_path);
                        muxed = true;
                    }
                    Err(e) => {
                        eprintln!("混流文件改名失败: {e}（保留纯视频）");
                        let _ = std::fs::remove_file(&mux_path);
                    }
                },
                Err(e) => {
                    eprintln!("音视频混流失败，保留纯视频: {e}");
                    let _ = std::fs::remove_file(&mux_path);
                }
            }
            if mux_src != paths.partial {
                let _ = std::fs::remove_file(&mux_src);
            }
        }
    }
    if !muxed {
        if let Err(e) = paths.finalize() {
            stats.set_phase(Phase::Failed);
            emit(RecordEvent::Failed(format!(
                "保存失败: {e}（临时文件 {}）",
                paths.partial.display()
            )));
            return;
        }
        // 混流失败时 WAV 也保留下来，至少音频素材还在。
        if wav.is_some() {
            let _ = std::fs::rename(&wav_path, paths.final_path.with_extension("wav"));
        }
    }

    let bytes = std::fs::metadata(&paths.final_path)
        .map(|m| m.len())
        .unwrap_or(0);
    stats.set_bytes(bytes);
    stats.set_elapsed_ms(active.as_millis() as u64);
    stats.set_phase(Phase::Finished);
    emit(RecordEvent::Finished(RecordOutcome {
        path: paths.final_path.clone(),
        frames,
        elapsed: active,
        bytes,
    }));
}

/// 看门狗：监视录制线程的心跳。
///
/// 录制线程每推进一步就刷新 [`SharedStats`] 心跳；这里每 `poll` 检查一次，
/// 心跳停滞超过 `timeout_ms` 就判定录制线程**永久阻塞**在 Media Foundation 内部
/// （实测约 9% 的录制会卡在 `WriteSample` / `Flush` / `Finalize` 里出不来），
/// 于是置 `aborted`、请求停止、发 `Failed` 事件 —— 宁可报错也不能让整个程序挂死。
///
/// 返回 `true` 表示录制线程正常结束，`false` 表示检测到卡死并已处置。
fn run_watchdog(
    stats: &Arc<SharedStats>,
    control: &Arc<Control>,
    on_event: &Arc<dyn Fn(RecordEvent) + Send + Sync>,
    done_rx: &mpsc::Receiver<()>,
    timeout_ms: u64,
    poll: Duration,
    paths: &RecordPaths,
) -> bool {
    loop {
        match done_rx.recv_timeout(poll) {
            Ok(()) => return true,
            Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !stats.is_stalled(timeout_ms) {
                    continue;
                }
                let stalled = stats.stalled_ms();
                let stage = stats.stage();
                control.aborted.store(true, Ordering::Release);
                control.request_stop(StopReason::EncoderError);
                on_event(RecordEvent::Failed(format!(
                    "录制在「{}」阶段停滞超过 {} 秒，已中止。\n\
                     临时文件已保留（可能无法播放）：{}",
                    stage.text(),
                    stalled / 1000,
                    paths.partial.display()
                )));
                return false;
            }
        }
    }
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
        stats.touch();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths() -> RecordPaths {
        RecordPaths::new(PathBuf::from("watchdog-test.mp4")).unwrap()
    }

    /// 看门狗的核心价值：录制线程卡死时，**程序不能跟着挂死**。
    #[test]
    fn watchdog_aborts_when_heartbeat_stalls() {
        let stats = Arc::new(SharedStats::new());
        let control = Arc::new(Control::new());
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (_tx, rx) = mpsc::channel::<()>(); // 发送端留着 → 录制线程"还没结束"

        // 模拟心跳停滞 5 秒。
        stats.force_stall(5_000);
        stats.set_stage(Stage::Write);

        let sink = Arc::clone(&seen);
        let cb: Arc<dyn Fn(RecordEvent) + Send + Sync> = Arc::new(move |ev: RecordEvent| {
            if let RecordEvent::Failed(msg) = ev {
                sink.lock().unwrap().push(msg);
            }
        });
        let aborted = run_watchdog(
            &stats,
            &control,
            &cb,
            &rx,
            1_000,
            Duration::from_millis(20),
            &test_paths(),
        );

        assert!(!aborted, "心跳停滞必须判定为卡死");
        assert!(control.aborted.load(Ordering::Acquire), "应置 aborted 标志");
        assert!(control.is_stopped(), "应请求停止");
        let msgs = seen.lock().unwrap();
        assert_eq!(msgs.len(), 1, "应发出一条失败事件");
        assert!(msgs[0].contains("停滞"), "错误信息应说明停滞: {}", msgs[0]);
        assert!(msgs[0].contains("写帧"), "错误信息应带出卡住的阶段: {}", msgs[0]);
    }

    /// 正常结束时看门狗必须安静退出，且不得误发失败事件。
    #[test]
    fn watchdog_stays_quiet_when_recording_finishes() {
        let stats = Arc::new(SharedStats::new());
        let control = Arc::new(Control::new());
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel::<()>();

        let sink = Arc::clone(&seen);
        let cb: Arc<dyn Fn(RecordEvent) + Send + Sync> = Arc::new(move |ev: RecordEvent| {
            if let RecordEvent::Failed(msg) = ev {
                sink.lock().unwrap().push(msg);
            }
        });
        let h = std::thread::spawn({
            let ctl = Arc::clone(&control);
            move || {
                run_watchdog(
                    &stats,
                    &ctl,
                    &cb,
                    &rx,
                    1_000,
                    Duration::from_millis(20),
                    &test_paths(),
                )
            }
        });
        tx.send(()).unwrap();
        assert!(h.join().unwrap(), "正常结束应返回 true");
        assert!(seen.lock().unwrap().is_empty(), "不应误报失败");
        assert!(!control.aborted.load(Ordering::Acquire));
    }
}
