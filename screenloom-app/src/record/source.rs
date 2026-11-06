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

/// 把尺寸向上取整到偶数。
///
/// NV12 的色度是 2×2 降采样，宽高必须为偶数。奇数尺寸的窗口很常见
/// （521x425 这种），直接报错等于**完全录不了**，所以这里向上补一像素，
/// 由 [`fit_frame`] 把帧补齐。
fn even_up(v: u32) -> u32 {
    v + (v & 1)
}

/// 把帧调整到目标尺寸：多了就裁，少了就补。
///
/// 补边用**最后一列/行的像素**复制，而不是填黑 —— 一条黑边在画面里很显眼。
fn fit_frame(f: Frame, tw: u32, th: u32) -> Frame {
    if f.w == tw && f.h == th {
        return f;
    }
    let (tw, th) = (tw as usize, th as usize);
    let (fw, fh) = (f.w as usize, f.h as usize);
    let mut data = vec![0u8; tw * th * 4];
    let cw = fw.min(tw);
    let ch = fh.min(th);
    let row_bytes = cw * 4;
    for row in 0..ch {
        let src = row * fw * 4;
        let dst = row * tw * 4;
        data[dst..dst + row_bytes].copy_from_slice(&f.data[src..src + row_bytes]);
    }
    // 右侧不足一列时，用该行最后一列的像素补齐。
    if tw > cw && cw > 0 {
        for row in 0..ch {
            let s = row * fw * 4 + (cw - 1) * 4;
            let px = [f.data[s], f.data[s + 1], f.data[s + 2], f.data[s + 3]];
            for x in cw..tw {
                let d = row * tw * 4 + x * 4;
                data[d..d + 4].copy_from_slice(&px);
            }
        }
    }
    // 底部不足一行时，用最后一行补齐。
    if th > ch && ch > 0 {
        let src_row = (ch - 1) * tw * 4;
        let last: Vec<u8> = data[src_row..src_row + tw * 4].to_vec();
        for y in ch..th {
            let dst = y * tw * 4;
            data[dst..dst + tw * 4].copy_from_slice(&last);
        }
    }
    Frame {
        w: tw as u32,
        h: th as u32,
        data,
    }
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
                        let (w, h) = (even_up(w), even_up(h));
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
                        let (w, h) = (even_up(w), even_up(h));
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
                    w: even_up(ow),
                    h: even_up(oh),
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
                // 右/下边界也要取偶：显示器尺寸取偶后 `fs.w - x` 可能是奇数，
                // 直接 min() 会选出奇数尺寸，区域录制就废了。
                let w = (((r.w.max(2) as u32) & !1).min(fs.w.saturating_sub(x))) & !1;
                let h = (((r.h.max(2) as u32) & !1).min(fs.h.saturating_sub(y))) & !1;
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
        let cropped = match (f, self.crop, self.scale_to) {
            (Some(f), Some(c), _) => Some(crop_frame(f, c)),
            (Some(f), None, Some((w, h))) => Some(scale_frame(f, w, h)),
            (f, _, _) => f,
        };
        // 统一到目标尺寸（偶数）：源尺寸可能是奇数（窗口 521x425 这种），
        // 不归一的话 NV12 转换会直接失败，录制根本跑不起来。
        Ok(cropped.map(|fr| fit_frame(fr, self.w, self.h)))
    }
}

fn open_capture(target: CaptureTarget, fps: u32, cursor: bool) -> Result<FrameSource> {
    match ScreenStream::start_with_cursor(target, cursor) {
        Ok(s) => {
            let (w, h) = s.size();
            let (w, h) = (even_up(w), even_up(h));
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
            let (w, h) = (even_up(w), even_up(h));
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
    // 源帧尺寸可能小于请求的裁剪区（显示器尺寸取偶后多出来的那一列），
    // 必须夹住，否则读越界直接 panic。
    let cw = c.w.min(f.w.saturating_sub(c.x));
    let ch = c.h.min(f.h.saturating_sub(c.y));
    let mut data = vec![0u8; (cw * ch * 4) as usize];
    for row in 0..ch {
        let src = (((c.y + row) * f.w + c.x) * 4) as usize;
        let dst = (row * cw * 4) as usize;
        data[dst..dst + (cw * 4) as usize].copy_from_slice(&f.data[src..src + (cw * 4) as usize]);
    }
    Frame {
        w: cw,
        h: ch,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, fill: u8) -> Frame {
        Frame {
            w,
            h,
            data: vec![fill; (w * h * 4) as usize],
        }
    }

    #[test]
    fn even_up_rounds_to_even() {
        assert_eq!(even_up(520), 520);
        assert_eq!(even_up(521), 522);
        assert_eq!(even_up(1), 2);
        assert_eq!(even_up(0), 0);
    }

    #[test]
    fn fit_frame_pads_odd_width_by_replicating_edge() {
        // 3x2 红帧 → 4x2：第 4 列必须也是红（复制边缘，不能是黑边）。
        let f = frame(3, 2, 200);
        let out = fit_frame(f, 4, 2);
        assert_eq!((out.w, out.h), (4, 2));
        assert_eq!(out.data.len(), 4 * 2 * 4);
        for i in 0..8 {
            assert_eq!(out.data[i * 4], 200, "第 {i} 个像素应是复制的红色");
        }
    }

    #[test]
    fn fit_frame_pads_odd_height_by_replicating_edge() {
        let f = frame(2, 3, 77);
        let out = fit_frame(f, 2, 4);
        assert_eq!((out.w, out.h), (2, 4));
        // 最后一行 = 倒数第二行（复制）
        assert_eq!(&out.data[16..20], &out.data[8..12]);
    }

    #[test]
    fn fit_frame_crops_oversized_frame() {
        let f = frame(5, 5, 9);
        let out = fit_frame(f, 4, 4);
        assert_eq!((out.w, out.h), (4, 4));
        assert_eq!(out.data.len(), 4 * 4 * 4);
    }

    #[test]
    fn fit_frame_is_identity_when_sizes_match() {
        let f = frame(4, 4, 3);
        let out = fit_frame(f, 4, 4);
        assert_eq!((out.w, out.h), (4, 4));
    }

    #[test]
    fn crop_frame_clamps_to_source_bounds() {
        // 源帧只有 3 宽，却要裁 4 宽 → 必须夹住，不能越界 panic。
        let f = frame(3, 2, 5);
        let out = crop_frame(f, Crop { x: 0, y: 0, w: 4, h: 2 });
        assert_eq!((out.w, out.h), (3, 2));
    }
}
