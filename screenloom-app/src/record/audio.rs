//! 系统声音采集（WASAPI loopback）。
//!
//! 输出统一为 **48 kHz / 立体声 / 16-bit PCM**，由 [`AudioCapture::drain`]
//! 交给录制主循环写入 MP4 的 AAC 音轨。
//!
//! 只做系统声（render loopback）；麦克风是后续独立源。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use screenloom_core::{Error, Result};

use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
const MAX_BUFFER_SAMPLES: usize = SAMPLE_RATE as usize * CHANNELS * 5;

const IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// 系统声音采集器。
pub struct AudioCapture {
    buffer: Arc<Mutex<VecDeque<i16>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// 启动系统声音 loopback。
    ///
    /// 初始化失败会返回 `Err`；录制侧可以决定是“放弃音频继续录视频”还是直接报错。
    pub fn start_system() -> Result<AudioCapture> {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);

        let buffer2 = Arc::clone(&buffer);
        let stop2 = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("screenloom-audio".into())
            .spawn(move || capture_loop(buffer2, stop2, init_tx))
            .map_err(|e| Error::Window(format!("启动音频线程失败: {e}")))?;

        match init_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(AudioCapture {
                buffer,
                stop,
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                let _ = join.join();
                Err(Error::Capture("音频初始化超时".into()))
            }
        }
    }

    /// 取走当前已采集的 PCM（48k / 立体声 / i16 交错）。
    pub fn drain(&self) -> Vec<i16> {
        let Ok(mut q) = self.buffer.lock() else {
            return Vec::new();
        };
        q.drain(..).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.lock().map(|q| q.is_empty()).unwrap_or(true)
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

fn capture_loop(
    buffer: Arc<Mutex<VecDeque<i16>>>,
    stop: Arc<AtomicBool>,
    init_tx: mpsc::SyncSender<Result<()>>,
) {
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let result = run_capture(&buffer, &stop, &init_tx);
    if let Err(e) = result {
        eprintln!("系统声音采集结束: {e}");
    }
    if com {
        unsafe { CoUninitialize() };
    }
}

fn run_capture(
    buffer: &Arc<Mutex<VecDeque<i16>>>,
    stop: &Arc<AtomicBool>,
    init_tx: &mpsc::SyncSender<Result<()>>,
) -> Result<()> {
    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| Error::Audio(format!("创建音频设备枚举器失败: {e}")))?
    };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|e| Error::Audio(format!("找不到默认扬声器: {e}")))?;
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| Error::Audio(format!("激活音频客户端失败: {e}")))?;

    let fmt_ptr = unsafe { client.GetMixFormat() }
        .map_err(|e| Error::Audio(format!("读取系统混音格式失败: {e}")))?;
    if fmt_ptr.is_null() {
        return Err(Error::Audio("系统混音格式为空".into()));
    }
    let fmt = unsafe { *fmt_ptr };
    // 必须在 Initialize 前读，因为 Initialize 之后就可以释放系统内存了。
    let is_float = unsafe { is_float_format(fmt_ptr) };
    let src_rate = fmt.nSamplesPerSec;
    let src_channels = fmt.nChannels as usize;
    let src_bits = fmt.wBitsPerSample;

    let init = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            fmt_ptr,
            None,
        )
    };
    unsafe {
        CoTaskMemFree(Some(fmt_ptr as *const core::ffi::c_void));
    }
    init.map_err(|e| Error::Audio(format!("初始化 loopback 失败: {e}")))?;

    let capture: IAudioCaptureClient = unsafe { client.GetService() }
        .map_err(|e| Error::Audio(format!("获取音频采集接口失败: {e}")))?;
    unsafe { client.Start() }.map_err(|e| Error::Audio(format!("启动音频采集失败: {e}")))?;

    // 通知调用方：初始化成功，可以开始录了。
    let _ = init_tx.send(Ok(()));

    let mut pcm: Vec<i16> = Vec::new();
    let mut scratch: Vec<i16> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        let mut got = false;
        loop {
            let Ok(n) = (unsafe { capture.GetNextPacketSize() }) else {
                break;
            };
            if n == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            let ok = unsafe {
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            };
            if ok.is_err() {
                break;
            }
            if frames > 0 {
                if (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    scratch.clear();
                    scratch.resize(frames as usize * CHANNELS, 0);
                } else {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            data,
                            frames as usize * src_channels * (src_bits as usize / 8),
                        )
                    };
                    convert(bytes, frames as usize, src_channels, src_bits, is_float, src_rate, &mut scratch);
                }
                pcm.extend_from_slice(&scratch);
                got = true;
            }
            let _ = unsafe { capture.ReleaseBuffer(frames) };
        }

        if got && !pcm.is_empty() {
            if let Ok(mut q) = buffer.lock() {
                q.extend(pcm.drain(..));
                // 视频循环卡住时不能无限涨；保留最近 5 秒。
                while q.len() > MAX_BUFFER_SAMPLES {
                    q.pop_front();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let _ = unsafe { client.Stop() };
    Ok(())
}

unsafe fn is_float_format(fmt: *const WAVEFORMATEX) -> bool {
    let f = unsafe { *fmt };
    if f.wFormatTag == WAVE_FORMAT_IEEE_FLOAT {
        return true;
    }
    if f.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        let p = fmt as *const WAVEFORMATEXTENSIBLE;
        let sub: GUID = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!((*p).SubFormat)) };
        return sub == IEEE_FLOAT;
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn convert(
    data: &[u8],
    frames: usize,
    channels: usize,
    bits: u16,
    is_float: bool,
    src_rate: u32,
    out: &mut Vec<i16>,
) {
    out.clear();
    if channels == 0 || frames == 0 {
        return;
    }
    let bytes_per_sample = (bits as usize / 8).max(2);
    let frame_bytes = channels * bytes_per_sample;
    if data.len() < frames * frame_bytes {
        return;
    }

    let sample_at = |frame: usize, ch: usize| -> f32 {
        let off = frame * frame_bytes + ch * bytes_per_sample;
        match (bits, is_float) {
            (32, true) => {
                let v = f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                v.clamp(-1.0, 1.0)
            }
            (32, false) => {
                let v = i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                (v as f32 / 2_147_483_648.0).clamp(-1.0, 1.0)
            }
            _ => {
                let v = i16::from_le_bytes([data[off], data[off + 1]]);
                v as f32 / 32_768.0
            }
        }
    };

    let push = |out: &mut Vec<i16>, l: f32, r: f32| {
        out.push((l.clamp(-1.0, 1.0) * 32_767.0) as i16);
        out.push((r.clamp(-1.0, 1.0) * 32_767.0) as i16);
    };

    if src_rate == SAMPLE_RATE {
        for frame in 0..frames {
            let l = sample_at(frame, 0);
            let r = if channels > 1 { sample_at(frame, 1) } else { l };
            push(out, l, r);
        }
        return;
    }

    let ratio = SAMPLE_RATE as f64 / src_rate as f64;
    let out_frames = ((frames as f64) * ratio).round() as usize;
    for i in 0..out_frames {
        let src_pos = i as f64 / ratio;
        let i0 = (src_pos.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let t = (src_pos - i0 as f64) as f32;
        let l0 = sample_at(i0, 0);
        let r0 = if channels > 1 { sample_at(i0, 1) } else { l0 };
        let l1 = sample_at(i1, 0);
        let r1 = if channels > 1 { sample_at(i1, 1) } else { l1 };
        push(out, l0 + (l1 - l0) * t, r0 + (r1 - r0) * t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 有默认扬声器时应能初始化/停止；没有设备的环境直接跳过。
    #[test]
    fn loopback_can_start_and_stop() {
        match AudioCapture::start_system() {
            Ok(a) => {
                std::thread::sleep(Duration::from_millis(100));
                let _ = a.drain();
            }
            Err(e) => eprintln!("跳过音频测试（无设备/被占用）: {e}"),
        }
    }
}
