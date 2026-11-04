//! 录制帧源：WGC 主路径 + GDI 兜底。
//!
//! WGC 在部分环境会返回 `E_ACCESSDENIED`（锁屏、远程会话、权限策略）；
//! 这时退回 GDI `BitBlt`，保证“至少能录”。GDI 抓不到被遮挡窗口，也拿不到
//! 硬件光标，属于降级路径。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use screenloom_capture::{
    capture_virtual_screen, list_monitors, CaptureBackend, CaptureTarget, GdiBackend, ScreenStream,
};
use screenloom_core::{Error, Result};
use windows::Win32::Foundation::HWND;

use super::RecordTarget;

/// 一帧 BGRA。
#[derive(Clone)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Crop {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
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
    crop: Option<Crop>,
    /// 虚拟屏过大时缩到可编码尺寸（宽, 高）。
    scale_to: Option<(u32, u32)>,
    /// 当前实际使用的后端（日志/错误提示用）。
    pub backend_name: &'static str,
}

impl FrameSource {
    /// 打开帧源：显示器/窗口优先 WGC，失败降级 GDI；虚拟屏直接走 GDI。
    pub fn open(target: RecordTarget, fps: u32, cursor: bool) -> Result<FrameSource> {
        match target {
            RecordTarget::Monitor(i) => open_capture(CaptureTarget::Monitor(i), fps, cursor),
            RecordTarget::Window(hwnd) => {
                let h = HWND(hwnd as *mut core::ffi::c_void);
                match ScreenStream::start_with_cursor(CaptureTarget::Window(h), cursor) {
                    Ok(s) => {
                        let (w, h) = s.size();
                        Ok(FrameSource {
                            inner: Inner::Wgc(s),
                            w,
                            h,
                            crop: None,
                            scale_to: None,
                            backend_name: "WGC(window)",
                        })
                    }
                    Err(e) => {
                        eprintln!("WGC 窗口录制不可用（{e}），降级 GDI/PrintWindow");
                        let gdi = GdiSource::new(GdiTarget::Window(hwnd), fps)?;
                        let (w, h) = gdi.size();
                        Ok(FrameSource {
                            inner: Inner::Gdi(gdi),
                            w,
                            h,
                            crop: None,
                            scale_to: None,
                            backend_name: "GDI(window)",
                        })
                    }
                }
            }
            RecordTarget::VirtualScreen => {
                let gdi = GdiSource::new(GdiTarget::Virtual, fps)?;
                let (w, h) = gdi.size();
                // 4K/多屏合成后尺寸可能远超编码器实时能力；等比缩到 1920 宽以内。
                let scale_to = if w > 1920 || h > 1080 {
                    let f = (1920.0 / w as f32).min(1080.0 / h as f32);
                    Some((
                        ((w as f32 * f) as u32 & !1).max(2),
                        ((h as f32 * f) as u32 & !1).max(2),
                    ))
                } else {
                    None
                };
                let (ow, oh) = scale_to.unwrap_or((w, h));
                Ok(FrameSource {
                    inner: Inner::Gdi(gdi),
                    w: ow,
                    h: oh,
                    crop: None,
                    scale_to,
                    backend_name: "GDI(virtual)",
                })
            }
            RecordTarget::Region(r) => {
                let monitors = list_monitors()?;
                let c = r.center();
                let mon = monitors
                    .iter()
                    .find(|m| m.bounds.contains(c))
                    .or_else(|| monitors.first())
                    .ok_or_else(|| Error::Capture("未找到显示器".into()))?;
                let mut fs = open_capture(CaptureTarget::Monitor(mon.index), fps, cursor)?;
                let x = (r.x - mon.bounds.x).max(0) as u32;
                let y = (r.y - mon.bounds.y).max(0) as u32;
                let w = ((r.w.max(2) as u32) & !1).min(fs.w.saturating_sub(x));
                let h = ((r.h.max(2) as u32) & !1).min(fs.h.saturating_sub(y));
                if w < 2 || h < 2 {
                    return Err(Error::Capture("录制区域无效".into()));
                }
                fs.crop = Some(Crop { x, y, w, h });
                fs.w = w;
                fs.h = h;
                Ok(fs)
            }
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// 取最新帧；没有新帧返回 `None`。
    pub fn take_latest(&mut self) -> Result<Option<Frame>> {
        let f = match &mut self.inner {
            Inner::Wgc(s) => s.take_latest()?.map(|f| Frame {
                w: f.w,
                h: f.h,
                data: f.data,
            }),
            Inner::Gdi(s) => s.take_latest()?,
        };
        Ok(match (f, self.crop, self.scale_to) {
            (Some(f), Some(c), _) => Some(crop_frame(f, c)),
            (Some(f), None, Some((w, h))) => Some(scale_frame(f, w, h)),
            (f, _, _) => f,
        })
    }
}

fn open_capture(target: CaptureTarget, fps: u32, cursor: bool) -> Result<FrameSource> {
    match ScreenStream::start_with_cursor(target, cursor) {
        Ok(s) => {
            let (w, h) = s.size();
            Ok(FrameSource {
                inner: Inner::Wgc(s),
                w,
                h,
                crop: None,
                scale_to: None,
                backend_name: "WGC",
            })
        }
        Err(wgc_err) => {
            eprintln!("WGC 录制不可用（{wgc_err}），降级 GDI");
            let monitor = match target {
                CaptureTarget::Monitor(i) => i,
                CaptureTarget::Window(_) => {
                    return Err(Error::Capture("WGC 窗口录制失败，GDI 无法接管".into()))
                }
            };
            let gdi = GdiSource::new(GdiTarget::Monitor(monitor), fps)?;
            let (w, h) = gdi.size();
            Ok(FrameSource {
                inner: Inner::Gdi(gdi),
                w,
                h,
                crop: None,
                scale_to: None,
                backend_name: "GDI",
            })
        }
    }
}

fn crop_frame(f: Frame, c: Crop) -> Frame {
    let mut data = vec![0u8; (c.w * c.h * 4) as usize];
    for row in 0..c.h {
        let src = (((c.y + row) * f.w + c.x) * 4) as usize;
        let dst = (row * c.w * 4) as usize;
        data[dst..dst + (c.w * 4) as usize]
            .copy_from_slice(&f.data[src..src + (c.w * 4) as usize]);
    }
    Frame {
        w: c.w,
        h: c.h,
        data,
    }
}

/// 最近邻缩放（整块虚拟屏过大时降到 1080p 级别，保证能实时编码）。
fn scale_frame(f: Frame, tw: u32, th: u32) -> Frame {
    let mut data = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        let sy = (y * f.h / th).min(f.h - 1);
        for x in 0..tw {
            let sx = (x * f.w / tw).min(f.w - 1);
            let s = ((sy * f.w + sx) * 4) as usize;
            let d = ((y * tw + x) * 4) as usize;
            data[d..d + 4].copy_from_slice(&f.data[s..s + 4]);
        }
    }
    Frame { w: tw, h: th, data }
}

/// GDI 帧源：后台线程按目标帧率抓帧，主循环只取“最新一帧”。
///
/// GDI 抓 4K 全屏可能单帧就要 100~200 ms；如果同步抓，编码循环会被拖慢，
/// 成片时长会比真实时长短。后台抓帧 + 主循环重复上一帧可以保证成片是实时 CFR。
#[derive(Clone, Copy)]
enum GdiTarget {
    Monitor(usize),
    Window(isize),
    Virtual,
}

struct GdiSource {
    latest: Arc<Mutex<Option<Frame>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    w: u32,
    h: u32,
}

impl GdiSource {
    fn new(target: GdiTarget, fps: u32) -> Result<GdiSource> {
        let backend = GdiBackend::new()?;
        let first = capture_gdi(&backend, target)?;
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
                    if let Ok(frame) = capture_gdi(&backend, target) {
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

fn capture_gdi(backend: &GdiBackend, target: GdiTarget) -> Result<screenloom_capture::CaptureFrame> {
    match target {
        GdiTarget::Monitor(i) => backend.capture(CaptureTarget::Monitor(i)),
        GdiTarget::Window(h) => backend.capture(CaptureTarget::Window(HWND(
            h as *mut core::ffi::c_void,
        ))),
        GdiTarget::Virtual => capture_virtual_screen(backend),
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
