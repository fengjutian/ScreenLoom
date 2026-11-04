//! 探针 5：颜色属性到底哪个在捣乱（一次只改一个变量）。
//!
//! 探针 4 的结果：
//! - 颜色属性放**输出**媒体类型 → `SetInputMediaType` 失败 "Invalid average bitrate"
//! - 颜色属性放**输入**媒体类型 → `Finalize` 失败 "接收器未处理任何示例"
//!
//! 注意 `MF_MT_VIDEO_NOMINAL_RANGE` 我之前填的是 **0**，
//! 而 MFVideoNormativeRange 里 0 = Unknown、1 = Limited(16-235)、2 = Full(0-255) ——
//! 很可能真正的问题是这个值填错（声明成 Unknown）。
//!
//! 本探针逐个属性 + 不同取值组合来定位。

use mf_probe::out_path;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
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
const FRAMES: u32 = 30;

/// 要往媒体类型上打哪些颜色属性。
#[derive(Clone, Copy)]
struct Color {
    yuv_matrix: Option<u32>,
    transfer: Option<u32>,
    primaries: Option<u32>,
    nominal_range: Option<u32>,
    label: &'static str,
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
            Color { yuv_matrix: None, transfer: None, primaries: None, nominal_range: None, label: "无颜色属性" },
            Color { yuv_matrix: Some(MF_VIDEO_COLOR_SPACE_BT709), transfer: None, primaries: None, nominal_range: None, label: "只 YUV_MATRIX(输入)" },
            Color { yuv_matrix: None, transfer: Some(MFVideoTransFunc_709.0 as u32), primaries: None, nominal_range: None, label: "只 TRANSFER(输入)" },
            Color { yuv_matrix: None, transfer: None, primaries: Some(MFVideoPrimaries_BT709.0 as u32), nominal_range: None, label: "只 PRIMARIES(输入)" },
            Color { yuv_matrix: None, transfer: None, primaries: None, nominal_range: Some(0), label: "只 RANGE=0 Unknown(输入)" },
            Color { yuv_matrix: None, transfer: None, primaries: None, nominal_range: Some(1), label: "只 RANGE=1 Limited(输入)" },
            Color { yuv_matrix: None, transfer: None, primaries: None, nominal_range: Some(2), label: "只 RANGE=2 Full(输入)" },
            Color { yuv_matrix: Some(MF_VIDEO_COLOR_SPACE_BT709), transfer: None, primaries: None, nominal_range: Some(1), label: "MATRIX+RANGE=1(输入)" },
            Color { yuv_matrix: Some(MF_VIDEO_COLOR_SPACE_BT709), transfer: Some(MFVideoTransFunc_709.0 as u32), primaries: Some(MFVideoPrimaries_BT709.0 as u32), nominal_range: Some(1), label: "全套 RANGE=1(输入)" },
        ];

        for c in &cases {
            let r = run(c);
            println!("{:<24} => {}", c.label, r);
        }

        CoUninitialize();
    }
}

unsafe fn run(c: &Color) -> String {
    let path = out_path(&format!(
        "_p5_{}.mp4",
        c.label.replace([' ', '(', ')'], "_")
    ))
    .display()
    .to_string();
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
    let _ = attr_store.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1);
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
    let _ = out_type.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000);

    // 颜色属性只打**输入**媒体类型
    if let Some(v) = c.yuv_matrix {
        let _ = in_type.SetUINT32(&MF_MT_YUV_MATRIX, v);
    }
    if let Some(v) = c.transfer {
        let _ = in_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, v);
    }
    if let Some(v) = c.primaries {
        let _ = in_type.SetUINT32(&MF_MT_VIDEO_PRIMARIES, v);
    }
    if let Some(v) = c.nominal_range {
        let _ = in_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, v);
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
    for f in 0..FRAMES {
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
    drop(sw);
    match std::fs::metadata(&p) {
        Ok(m) => format!("OK {} 字节", m.len()),
        Err(e) => format!("产物读不到 {:?}", e),
    }
}
