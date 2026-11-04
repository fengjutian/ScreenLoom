//! 探针 6：帧数丢失 + 颜色属性最终形态。
//!
//! 探针 5 的意外发现：**写 30 帧，文件里只有 18~23 帧**，而且时长自洽
//! （18 帧 → 0.6s @30fps）。也就是硬件编码器**吞掉了一批输入帧**。
//!
//! 最可能的原因：探针里设了 `MF_SINK_WRITER_DISABLE_THROTTLING = 1`，
//! 于是 30 帧在一瞬间全部灌进去，编码器输入队列溢出 → 丢帧。
//! 真实录屏是实时投递的，不该关节流。
//!
//! 本探针：
//! - t1/t2：节流开 / 关，看帧数与耗时
//! - t5：写 4 秒（120 帧）验证节流是否按时间戳真实等待
//! - t3/t4：颜色属性放**输出**（配 range=2）—— 探针 5 里输出侧失败是因为
//!   我把 NOMINAL_RANGE 填成了 0(Unknown)；换成 2(Full) 可能就通了，
//!   而且只有放输出侧才可能写进 MP4 的 `colr` box。

use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFStartup, MF_VERSION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MF_MT_ALL_SAMPLES_INDEPENDENT, MFTranscodeContainerType_MPEG4,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE,
    MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFMediaType_Video, MFVideoPrimaries_BT709,
    MFVideoTransFunc_709,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const MF_VIDEO_COLOR_SPACE_BT709: u32 = 1;
const W: u32 = 320;
const H: u32 = 240;

struct Case {
    tag: &'static str,
    frames: u32,
    throttle: bool,
    color: &'static str, // none / in / out
    bitrate: Option<u32>,
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, 0).is_err() {
            eprintln!("MFStartup 失败");
            CoUninitialize();
            return;
        }

        let cases = vec![
            Case { tag: "t1_throttle_on",  frames: 30,  throttle: true,  color: "none", bitrate: Some(4_000_000) },
            Case { tag: "t2_throttle_off", frames: 30,  throttle: false, color: "none", bitrate: Some(4_000_000) },
            Case { tag: "t3_col_out",      frames: 30,  throttle: true,  color: "out",  bitrate: Some(4_000_000) },
            Case { tag: "t4_col_in",       frames: 30,  throttle: true,  color: "in",   bitrate: Some(4_000_000) },
            Case { tag: "t5_4sec",         frames: 120, throttle: true,  color: "none", bitrate: Some(4_000_000) },
            Case { tag: "t6_4sec_off",     frames: 120, throttle: false, color: "none", bitrate: Some(4_000_000) },
        ];

        for c in &cases {
            let r = run(c);
            println!("{:<16} {:>3}帧 节流={:<5} 颜色@{:<4} => {}", c.tag, c.frames, c.throttle, c.color, r);
        }

        CoUninitialize();
    }
}

unsafe fn run(c: &Case) -> String {
    let path = format!(r"D:\github\screenloom\_p6_{}.mp4", c.tag);
    let p = std::path::PathBuf::from(&path);
    let _ = std::fs::remove_file(&p);
    let url: Vec<u16> = p
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut attr_store: Option<IMFAttributes> = None;
    if MFCreateAttributes(&mut attr_store, 0).is_err() {
        return "MFCreateAttributes 失败".into();
    }
    let attr_store = attr_store.unwrap();
    let _ = attr_store.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4);
    if !c.throttle {
        let _ = attr_store.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1);
    }
    let _ = attr_store.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);

    let sw: IMFSinkWriter = match MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attr_store)
    {
        Ok(v) => v,
        Err(e) => return format!("建 writer 失败 0x{:08x}", e.code().0 as u32),
    };

    let in_type = match MFCreateMediaType() {
        Ok(v) => v,
        Err(e) => return format!("建输入类型失败 0x{:08x}", e.code().0 as u32),
    };
    let _ = in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12);
    let _ = in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = in_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = in_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64));
    let _ = in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, W);
    let _ = in_type.SetUINT32(&MF_MT_SAMPLE_SIZE, W * H + (W / 2) * (H / 2) * 2);
    let _ = in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);

    let out_type = match MFCreateMediaType() {
        Ok(v) => v,
        Err(e) => return format!("建输出类型失败 0x{:08x}", e.code().0 as u32),
    };
    let _ = out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64));
    let _ = out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
    if let Some(b) = c.bitrate {
        let _ = out_type.SetUINT32(&MF_MT_AVG_BITRATE, b);
    }

    // 颜色属性：range 一律用 2 (Full)，matrix=BT.709，transfer=709，primaries=BT.709
    if c.color != "none" {
        let t: &IMFMediaType = if c.color == "in" { &in_type } else { &out_type };
        let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MF_VIDEO_COLOR_SPACE_BT709);
        let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, 2); // Full 0-255
    }

    let stream_index = match sw.AddStream(&out_type) {
        Ok(v) => v,
        Err(e) => return format!("AddStream 失败 0x{:08x} {}", e.code().0 as u32, e.message()),
    };
    if let Err(e) = sw.SetInputMediaType(stream_index, &in_type, None) {
        return format!(
            "SetInputMediaType 失败 0x{:08x} {}",
            e.code().0 as u32,
            e.message()
        );
    }
    if let Err(e) = sw.BeginWriting() {
        return format!("BeginWriting 失败 0x{:08x}", e.code().0 as u32);
    }

    let frame_bytes = (W * H + (W / 2) * (H / 2) * 2) as usize;
    let pixels = vec![128u8; frame_bytes];
    let t0 = Instant::now();
    for f in 0..c.frames {
        let sample: IMFSample = match MFCreateSample() {
            Ok(s) => s,
            Err(e) => return format!("MFCreateSample 失败 0x{:08x}", e.code().0 as u32),
        };
        let buffer = match MFCreateMemoryBuffer(frame_bytes as u32) {
            Ok(b) => b,
            Err(e) => return format!("MFCreateMemoryBuffer 失败 0x{:08x}", e.code().0 as u32),
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        if buffer.Lock(&mut ptr, Some(&mut len), None).is_err() {
            return "Lock 失败".into();
        }
        if len as usize >= frame_bytes {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, frame_bytes);
        }
        let _ = buffer.Unlock();
        let _ = buffer.SetCurrentLength(frame_bytes as u32);
        let _ = sample.AddBuffer(&buffer);
        let _ = sample.SetSampleTime(f as i64 * 10_000_000 / 30);
        let _ = sample.SetSampleDuration(10_000_000 / 30);
        if let Err(e) = sw.WriteSample(stream_index, &sample) {
            return format!(
                "WriteSample#{} 失败 0x{:08x} {}",
                f,
                e.code().0 as u32,
                e.message()
            );
        }
    }
    let write_elapsed = t0.elapsed();

    if let Err(e) = sw.Flush(stream_index) {
        return format!("Flush 失败 0x{:08x} {}", e.code().0 as u32, e.message());
    }
    if let Err(e) = sw.Finalize() {
        return format!(
            "Finalize 失败 0x{:08x} {}",
            e.code().0 as u32,
            e.message()
        );
    }
    let total_elapsed = t0.elapsed();
    drop(sw);
    match std::fs::metadata(&p) {
        Ok(m) => format!(
            "OK {} 字节, 写样本耗时 {:.0}ms, 全程 {:.0}ms",
            m.len(),
            write_elapsed.as_millis(),
            total_elapsed.as_millis()
        ),
        Err(e) => format!("产物读不到 {:?}", e),
    }
}
