//! 录制帧源：WGC 主路径 + GDI 兜底。
//!
//! WGC 在部分环境会返回 `E_ACCESSDENIED`（锁屏、远程会话、权限策略）；
//! 这时退回 GDI `BitBlt`，保证“至少能录”。GDI 抓不到被遮挡窗口，也拿不到
//! 硬件光标，属于降级路径。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use screenloom_capture::{CaptureBackend, CaptureTarget, GdiBackend, ScreenStream};
use screenloom_core::{Error, Result};

use super::RecordTarget;

/// 一帧 BGRA。
#[derive(Clone)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

enum Inner {
    Wgc(ScreenStream),
    Gdi(GdiSource),
}

/// 录制帧源。
pub struct FrameSource {
    inner: Inner,
    w: u32,
    h: u32,
    /// 当前实际使用的后端（日志/错误提示用）。
    pub backend_name: &'static str,
}

impl FrameSource {
    /// 打开帧源：先 WGC，失败自动降级 GDI。
    pub fn open(target: RecordTarget, fps: u32, cursor: bool) -> Result<FrameSource> {
        let capture_target = match target {
            RecordTarget::Monitor(i) => CaptureTarget::Monitor(i),
        };
        match ScreenStream::start_with_cursor(capture_target, cursor) {
            Ok(s) => {
                let (w, h) = s.size();
                Ok(FrameSource {
                    inner: Inner::Wgc(s),
                    w,
                    h,
                    backend_name: "WGC",
                })
            }
            Err(wgc_err) => {
                eprintln!("WGC 录制不可用（{wgc_err}），降级 GDI");
                let gdi = GdiSource::new(capture_target, fps)?;
                let (w, h) = gdi.size();
                Ok(FrameSource {
                    inner: Inner::Gdi(gdi),
                    w,
                    h,
                    backend_name: "GDI",
                })
            }
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// 取最新帧；没有新帧返回 `None`。
    pub fn take_latest(&mut self) -> Result<Option<Frame>> {
        match &mut self.inner {
            Inner::Wgc(s) => Ok(s.take_latest()?.map(|f| Frame {
                w: f.w,
                h: f.h,
                data: f.data,
            })),
            Inner::Gdi(s) => s.take_latest(),
        }
    }
}

/// GDI 帧源：后台线程按目标帧率抓帧，主循环只取“最新一帧”。
///
/// GDI 抓 4K 全屏可能单帧就要 100~200 ms；如果同步抓，编码循环会被拖慢，
/// 成片时长会比真实时长短。后台抓帧 + 主循环重复上一帧可以保证成片是实时 CFR。
struct GdiSource {
    latest: Arc<Mutex<Option<Frame>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    w: u32,
    h: u32,
}

impl GdiSource {
    fn new(target: CaptureTarget, fps: u32) -> Result<GdiSource> {
        let monitor = match target {
            CaptureTarget::Monitor(i) => i,
            CaptureTarget::Window(_) => {
                return Err(Error::Capture("GDI 录制首版只支持显示器".into()))
            }
        };
        let backend = GdiBackend::new()?;
        let first = backend.capture(CaptureTarget::Monitor(monitor))?;
        if first.image.w == 0 || first.image.h == 0 {
            return Err(Error::Capture("GDI 抓到空图像".into()));
        }
        let (w, h) = (first.image.w, first.image.h);
        let latest = Arc::new(Mutex::new(Some(Frame {
            w,
            h,
            data: first.image.to_bgra(),
        })));
        let stop = Arc::new(AtomicBool::new(false));

        let latest2 = Arc::clone(&latest);
        let stop2 = Arc::clone(&stop);
        let interval = Duration::from_nanos(1_000_000_000 / fps.max(1) as u64);
        let join = std::thread::Builder::new()
            .name("screenloom-gdi-capture".into())
            .spawn(move || {
                let Ok(backend) = GdiBackend::new() else {
                    return;
                };
                let mut next = Instant::now();
                while !stop2.load(Ordering::Acquire) {
                    if let Ok(frame) = backend.capture(CaptureTarget::Monitor(monitor)) {
                        if frame.image.w > 0 && frame.image.h > 0 {
                            let f = Frame {
                                w: frame.image.w,
                                h: frame.image.h,
                                data: frame.image.to_bgra(),
                            };
                            if let Ok(mut slot) = latest2.lock() {
                                *slot = Some(f);
                            }
                        }
                    }
                    next += interval;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
            })
            .map_err(|e| Error::Window(format!("启动 GDI 抓帧线程失败: {e}")))?;

        Ok(GdiSource {
            latest,
            stop,
            join: Some(join),
            w,
            h,
        })
    }

    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn take_latest(&mut self) -> Result<Option<Frame>> {
        Ok(self.latest.lock().ok().and_then(|mut slot| slot.take()))
    }
}

impl Drop for GdiSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}
