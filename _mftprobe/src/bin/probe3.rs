//! 探针 3：钉死三个结论。
//!
//! 探针 2 的发现：
//! - **不设 `MF_MT_AVG_BITRATE` → `Finalize` 报 `0xC00D4A44`（接收器未处理任何示例）**。
//!   探针 1 之所以成功，是因为 320x240 下 MF 自己算出的默认码率恰好合法；
//!   64x64 下默认码率低于编码器下限 → 编码器一帧都不吐 → 收不到样本。
//!   ⇒ **`MF_MT_AVG_BITRATE` 不是可选项，是必选项。**
//! - `SetInputMediaType(MFVideoFormat_RGB32)` **成功** → MF 能直接吃 BGRA。
//! - 颜色属性放在**输出**媒体类型上会引发 "Invalid average bitrate" / Finalize 失败。
//!   怀疑是颜色属性被 MF 反向传播到输入类型协商，导致编码器认为要做色彩转换却缺参数。
//!
//! 本探针要回答：
//! 1. BGRA 输入 + 码率 → 是否真能产出正确的颜色？（用 ffmpeg 解一帧的实际像素）
//! 2. 颜色属性放 **输入** / **两端都放**，哪个能通过？
//! 3. 不设码率时，320x240 是否仍然成功（验证探针 1 不是巧合）？

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFStartup, MF_VERSION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MF_MT_ALL_SAMPLES_INDEPENDENT, MFTranscodeContainerType_MPEG4,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE,
    MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoFormat_RGB32, MFMediaType_Video,
    MFVideoPrimaries_BT709, MFVideoTransFunc_709,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// `MFVideoColorSpace_BT709` —— windows-rs 0.61 未导出该枚举，按 mfapi.h 取值补常量。
const MF_VIDEO_COLOR_SPACE_BT709: u32 = 1;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Nv12,
    Bgra,
}

/// 颜色属性放在哪儿。
#[derive(Clone, Copy, PartialEq, Debug)]
enum ColorWhere {
    None,
    Input,
    Output,
    Both,
}

struct Case {
    tag: &'static str,
    w: u32,
    h: u32,
    frames: u32,
    kind: Kind,
    bitrate: Option<u32>,
    color: ColorWhere,
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
            // 0. 复现探针 1：320x240 / 30 帧 / 不设码率
            Case { tag: "ctl_320", w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: None, color: ColorWhere::None },
            // 1. BGRA 输入 + 码率 → 验证颜色
            Case { tag: "bgra_bitrate", w: 320, h: 240, frames: 30, kind: Kind::Bgra, bitrate: Some(4_000_000), color: ColorWhere::None },
            // 2. 颜色属性放输入
            Case { tag: "color_in", w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Some(4_000_000), color: ColorWhere::Input },
            // 3. 颜色属性放输出
            Case { tag: "color_out", w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Some(4_000_000), color: ColorWhere::Output },
            // 4. 颜色属性两端都放
            Case { tag: "color_both", w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Some(4_000_000), color: ColorWhere::Both },
        ];

        for c in &cases {
            println!("\n=== {} ({}x{}, {} 帧, {:?}, 码率 {:?}, 颜色 {:?}) ===",
                c.tag, c.w, c.h, c.frames, c.kind, c.bitrate, c.color);
            run(c);
        }

        CoUninitialize();
    }
}

fn report_step<T>(tag: &str, name: &str, r: windows::core::Result<T>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            println!("  [{}] {} 失败: 0x{:08x} {}", tag, name, e.code().0 as u32, e.message());
            None
        }
    }
}

unsafe fn run(c: &Case) {
    let path = format!(r"D:\github\screenloom\_p3_{}.mp4", c.tag);
    let p = std::path::PathBuf::from(&path);
    let _ = std::fs::remove_file(&p);
    let url: Vec<u16> = p
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut attr_store: Option<IMFAttributes> = None;
    if MFCreateAttributes(&mut attr_store, 0).is_err() {
        println!("  MFCreateAttributes 失败");
        return;
    }
    let attr_store = attr_store.unwrap();
    let _ = attr_store.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4);
    let _ = attr_store.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1);
    let _ = attr_store.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);

    let sw: IMFSinkWriter = match report_step(
        c.tag,
        "MFCreateSinkWriterFromURL",
        MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attr_store),
    ) {
        Some(v) => v,
        None => return,
    };

    let (stride, sample_size) = match c.kind {
        Kind::Nv12 => (c.w, c.w * c.h + (c.w / 2) * (c.h / 2) * 2),
        Kind::Bgra => (c.w * 4, c.w * c.h * 4),
    };

    let in_type = match report_step(c.tag, "MFCreateMediaType(in)", MFCreateMediaType()) {
        Some(v) => v,
        None => return,
    };
    let _ = in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = in_type.SetGUID(
        &MF_MT_SUBTYPE,
        match c.kind {
            Kind::Nv12 => &MFVideoFormat_NV12,
            Kind::Bgra => &MFVideoFormat_RGB32,
        },
    );
    let _ = in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = in_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = in_type.SetUINT64(&MF_MT_FRAME_SIZE, (c.w as u64) << 32 | (c.h as u64));
    let _ = in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride);
    let _ = in_type.SetUINT32(&MF_MT_SAMPLE_SIZE, sample_size);
    let _ = in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);

    let out_type = match report_step(c.tag, "MFCreateMediaType(out)", MFCreateMediaType()) {
        Some(v) => v,
        None => return,
    };
    let _ = out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, (c.w as u64) << 32 | (c.h as u64));
    let _ = out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
    if let Some(b) = c.bitrate {
        let _ = out_type.SetUINT32(&MF_MT_AVG_BITRATE, b);
    }

    // 颜色属性
    if matches!(c.color, ColorWhere::Input | ColorWhere::Both) {
        set_color(&in_type);
    }
    if matches!(c.color, ColorWhere::Output | ColorWhere::Both) {
        set_color(&out_type);
    }

    let stream_index = match report_step(c.tag, "AddStream", sw.AddStream(&out_type)) {
        Some(v) => v,
        None => return,
    };
    if let None = report_step(
        c.tag,
        "SetInputMediaType",
        sw.SetInputMediaType(stream_index, &in_type, None),
    ) {
        return;
    }
    if let None = report_step(c.tag, "BeginWriting", sw.BeginWriting()) {
        return;
    }

    let frame_bytes = sample_size as usize;
    let pixels = make_pixels(c.kind, c.w, c.h);
    for f in 0..c.frames {
        let sample: IMFSample = match MFCreateSample() {
            Ok(s) => s,
            Err(e) => {
                println!("  [{}] MFCreateSample 失败: {:?}", c.tag, e);
                return;
            }
        };
        let buffer = match MFCreateMemoryBuffer(frame_bytes as u32) {
            Ok(b) => b,
            Err(e) => {
                println!("  [{}] MFCreateMemoryBuffer 失败: {:?}", c.tag, e);
                return;
            }
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        if buffer.Lock(&mut ptr, Some(&mut len), None).is_err() {
            println!("  [{}] Lock 失败", c.tag);
            return;
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
            println!(
                "  [{}] WriteSample 第 {} 帧失败: 0x{:08x} {}",
                c.tag,
                f,
                e.code().0 as u32,
                e.message()
            );
            return;
        }
    }

    if let Err(e) = sw.Flush(stream_index) {
        println!("  [{}] Flush 失败: 0x{:08x} {}", c.tag, e.code().0 as u32, e.message());
        return;
    }
    if let Err(e) = sw.Finalize() {
        println!(
            "  [{}] Finalize 失败: 0x{:08x} {}",
            c.tag,
            e.code().0 as u32,
            e.message()
        );
        return;
    }
    drop(sw);
    match std::fs::metadata(&p) {
        Ok(m) => println!("  [{}] OK -> {} ({} 字节)", c.tag, path, m.len()),
        Err(e) => println!("  [{}] 产物读不到: {:?}", c.tag, e),
    }
}

/// BT.709 / limited range。
unsafe fn set_color(t: &windows::Win32::Media::MediaFoundation::IMFMediaType) {
    let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MF_VIDEO_COLOR_SPACE_BT709);
    let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, 0); // limited 16-235
}

/// BGRA 帧填「纯红」（内存序 B=0,G=0,R=255,A=255）。
/// 若 MF 按 BGRA 解释 → 解码是红；若按 RGBA 解释 → 解码是蓝。
unsafe fn make_pixels(kind: Kind, w: u32, h: u32) -> Vec<u8> {
    match kind {
        Kind::Bgra => vec![0u8, 0, 255, 255].repeat((w * h) as usize),
        Kind::Nv12 => {
            let y_size = (w * h) as usize;
            let uv = ((w / 2) * (h / 2)) as usize;
            vec![128u8; y_size + uv * 2]
        }
    }
}
