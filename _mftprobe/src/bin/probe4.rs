//! 探针 4：干净的对照矩阵，把混淆的变量拆开。
//!
//! 已知（探针 1~3）：
//! - (320x240, 30帧, NV12, 无码率无颜色) → OK
//! - (64x64,  10帧, NV12, 无码率)        → Finalize 失败
//! - (64x64,  10帧, NV12, 码率2M@输出)    → OK
//! - (320x240,30帧, NV12, 码率4M@输出)    → Finalize 失败
//! - (320x240,30帧, NV12, 码率4M + 颜色)  → Finalize 失败
//! - (320x240,30帧, BGRA, 码率4M@输出)    → Finalize 失败
//!
//! 混淆点：探针 3 里每个用例都带码率，分不清是「码率」还是「颜色/BGRA」导致的。
//!
//! 本探针把三个因子拆开：
//!   因子 A：码率放哪 —— 无 / 输出媒体类型 / 输入媒体类型
//!   因子 B：颜色属性放哪 —— 无 / 输入 / 输出 / 两端
//!   因子 C：输入格式 —— NV12 / BGRA
//! 另外加一维：帧数（10 / 30），验证「编码器需要预读若干帧才吐样本」这个猜想。

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
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

const MF_VIDEO_COLOR_SPACE_BT709: u32 = 1;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Nv12,
    Bgra,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Where {
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
    /// 码率放在哪个媒体类型上
    bitrate: Where,
    bitrate_value: u32,
    color: Where,
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, 0).is_err() {
            eprintln!("MFStartup 失败");
            CoUninitialize();
            return;
        }

        let br = 4_000_000u32;
        let cases = vec![
            // 基线
            Case { tag: "a_ctl",        w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Where::None,   bitrate_value: br, color: Where::None },
            // 因子 A：码率放输出
            Case { tag: "b_br_out",     w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Where::Output, bitrate_value: br, color: Where::None },
            // 因子 A：码率放输入
            Case { tag: "c_br_in",      w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Where::Input,  bitrate_value: br, color: Where::None },
            // 因子 B：颜色（都不带码率）
            Case { tag: "d_col_in",     w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Where::None,   bitrate_value: br, color: Where::Input },
            Case { tag: "e_col_out",    w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Where::None,   bitrate_value: br, color: Where::Output },
            Case { tag: "f_col_both",   w: 320, h: 240, frames: 30, kind: Kind::Nv12, bitrate: Where::None,   bitrate_value: br, color: Where::Both },
            // 因子 C：BGRA（不带码率）
            Case { tag: "g_bgra",       w: 320, h: 240, frames: 30, kind: Kind::Bgra, bitrate: Where::None,   bitrate_value: br, color: Where::None },
            // 因子 C + A：BGRA + 码率放输入
            Case { tag: "h_bgra_brin",  w: 320, h: 240, frames: 30, kind: Kind::Bgra, bitrate: Where::Input,  bitrate_value: br, color: Where::None },
            // 帧数：10 帧
            Case { tag: "i_10f",        w: 320, h: 240, frames: 10, kind: Kind::Nv12, bitrate: Where::None,   bitrate_value: br, color: Where::None },
            Case { tag: "j_10f_bgra",   w: 320, h: 240, frames: 10, kind: Kind::Bgra, bitrate: Where::None,   bitrate_value: br, color: Where::None },
            // 小尺寸
            Case { tag: "k_64",         w: 64,  h: 64,  frames: 30, kind: Kind::Nv12, bitrate: Where::None,   bitrate_value: br, color: Where::None },
        ];

        for c in &cases {
            let r = run(c);
            println!(
                "{:<14} {:>5}x{:<4} {:>2}帧 {:<5?} 码率@{:?} 颜色@{:?}  => {}",
                c.tag, c.w, c.h, c.frames, c.kind, c.bitrate, c.color, r
            );
        }

        CoUninitialize();
    }
}

unsafe fn run(c: &Case) -> String {
    let path = format!(r"D:\github\screenloom\_p4_{}.mp4", c.tag);
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

    let sw: IMFSinkWriter =
        match MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attr_store) {
            Ok(v) => v,
            Err(e) => return format!("建 writer 失败 0x{:08x}", e.code().0 as u32),
        };

    let (stride, sample_size) = match c.kind {
        Kind::Nv12 => (c.w, c.w * c.h + (c.w / 2) * (c.h / 2) * 2),
        Kind::Bgra => (c.w * 4, c.w * c.h * 4),
    };

    let in_type = match MFCreateMediaType() {
        Ok(v) => v,
        Err(e) => return format!("建输入类型失败 0x{:08x}", e.code().0 as u32),
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

    let out_type = match MFCreateMediaType() {
        Ok(v) => v,
        Err(e) => return format!("建输出类型失败 0x{:08x}", e.code().0 as u32),
    };
    let _ = out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, (c.w as u64) << 32 | (c.h as u64));
    let _ = out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);

    if matches!(c.bitrate, Where::Input | Where::Both) {
        let _ = in_type.SetUINT32(&MF_MT_AVG_BITRATE, c.bitrate_value);
    }
    if matches!(c.bitrate, Where::Output | Where::Both) {
        let _ = out_type.SetUINT32(&MF_MT_AVG_BITRATE, c.bitrate_value);
    }
    if matches!(c.color, Where::Input | Where::Both) {
        set_color(&in_type);
    }
    if matches!(c.color, Where::Output | Where::Both) {
        set_color(&out_type);
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

    let frame_bytes = sample_size as usize;
    let pixels = make_pixels(c.kind, c.w, c.h);
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

unsafe fn set_color(t: &IMFMediaType) {
    let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MF_VIDEO_COLOR_SPACE_BT709);
    let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, 0);
}

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
