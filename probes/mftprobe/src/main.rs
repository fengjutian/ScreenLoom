//! 录屏可行性探针：Media Foundation 端到端写 H.264 MP4。
//!
//! 判据：**真的写出一个结构完整的 MP4**（ftyp/moov/mdat 齐全），不是"API 调用没报错"。
//!
//! 已验证的关键点（踩坑记录）：
//! 1. `MFCreateSinkWriterFromURL` 要的是**完整文件路径**，不是 `file://` URL。
//! 2. `IMFSinkWriter` 在 windows-rs 里把继承关系展平了，**没有** `AddStreamSink`；
//!    用 `AddStream(目标媒体类型)` 让 SinkWriter 自动挑编码器。
//! 3. MP4 容器必须 `SetOutputMediaType` 显式声明 H.264，否则无法写 moov 头。
//! 4. 输入未压缩帧要用 **NV12**（不是 RGB32），并设 `MF_MT_FRAME_SIZE` /
//!    `MF_MT_DEFAULT_STRIDE` / `MF_MT_SAMPLE_SIZE` / `MF_MT_ALL_SAMPLES_INDEPENDENT`。
//! 5. `MFCreateSample()` 后取 `ConvertToContiguousBuffer()` 会失败（E_UNEXPECTED），
//!    要用 `MFCreateMemoryBuffer` + `sample.AddBuffer`。
//! 6. buffer 写完**必须** `SetCurrentLength`，否则 sample 长度为 0，编码器报参数错误。

use mf_probe::out_path;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFStartup, MFTEnumEx,
    MF_VERSION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MF_MT_ALL_SAMPLES_INDEPENDENT, MFTranscodeContainerType_MPEG4, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SYNCMFT, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFMediaType_Video,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const W: u32 = 320;
const H: u32 = 240;
const FRAMES: u32 = 30;

fn main() {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        println!("CoInitializeEx hr=0x{:08x}", hr.0 as u32);
        let mhr = MFStartup(MF_VERSION, 0);
        println!("MFStartup ok={}", mhr.is_ok());
        if mhr.is_err() {
            println!("MFStartup 失败 —— 系统缺少 Media Foundation");
            CoUninitialize();
            return;
        }

        println!("\n=== 枚举视频编码器 MFT ===");
        for (flags, label) in [
            (MFT_ENUM_FLAG_SYNCMFT, "SYNCMFT(软件)"),
            (MFT_ENUM_FLAG_HARDWARE, "HARDWARE"),
        ] {
            let mut p_activate: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count: u32 = 0;
            match MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                None,
                None,
                &mut p_activate,
                &mut count,
            ) {
                Ok(()) => println!("  {} -> {} 个", label, count),
                Err(e) => println!("  {} -> 失败 {:?}", label, e),
            }
        }

        println!("\n=== 端到端写 H.264 MP4 ===");
        match write_probe_mp4() {
            Ok(msg) => println!("  OK: {}", msg),
            Err(e) => println!("  FAIL: {:?}", e),
        }

        CoUninitialize();
    }
}

/// 真的走一遍：SinkWriter -> H.264 -> MP4 文件。
unsafe fn write_probe_mp4() -> windows::core::Result<String> {
    let path = out_path("_probe_out.mp4");
    let _ = std::fs::remove_file(&path);
    let url: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 0)?;
    let attrs = attrs.ok_or_else(|| {
        windows::core::Error::from(windows::core::HRESULT(0x80004005u32 as i32))
    })?;

    // 显式声明容器类型为 MP4（否则按扩展名推断）
    // MFTranscodeContainerType_MPEG4 = MP4 容器（不是 MFVideoFormat_MP4，那是视频子类型）
    attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
    // 关闭节流：实时写入时不因速率限制而丢帧
    attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
    // 允许硬件变换（走 GPU 编码）
    attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;

    let sink_writer: IMFSinkWriter =
        MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs)?;
    println!("  MFCreateSinkWriterFromURL 成功");

    // ---- 输入媒体类型：未压缩 NV12 ----
    let in_type = MFCreateMediaType()?;
    in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
    in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
    in_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2)?; // progressive
    in_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64))?;
    in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, W)?;
    in_type.SetUINT32(&MF_MT_SAMPLE_SIZE, (W * H + (W / 2) * (H / 2) * 2) as u32)?;
    in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

    // ---- 输出媒体类型：H.264 ----
    let out_type = MFCreateMediaType()?;
    out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    out_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
    out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2)?;
    out_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64))?;
    out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

    // AddStream：给目标类型，让 SinkWriter 自动选编码器
    let stream_index = sink_writer.AddStream(&out_type)?;
    println!("  AddStream(H.264) 成功 -> stream index = {}", stream_index);

    sink_writer.SetInputMediaType(stream_index, &in_type, None)?;
    println!("  SetInputMediaType(NV12) 成功");

    // ---- 准备 NV12 帧数据 ----
    let y_size = (W * H) as usize;
    let uv_size = ((W / 2) * (H / 2)) as usize;
    let frame_bytes = y_size + uv_size * 2;
    let mut buf = vec![0u8; frame_bytes];
    for y in 0..H as usize {
        for x in 0..W as usize {
            let gx = (x * 255 / W as usize).min(255) as u16;
            let gy = (y * 255 / H as usize).min(255) as u16;
            buf[y * W as usize + x] = ((gx + gy) / 2) as u8;
        }
    }
    for i in 0..uv_size {
        buf[y_size + i] = 128;
        buf[y_size + uv_size + i] = 128;
    }

    sink_writer.BeginWriting()?;
    println!("  BeginWriting 成功");

    for f in 0..FRAMES {
        let sample: IMFSample = MFCreateSample()?;
        let buffer = MFCreateMemoryBuffer(frame_bytes as u32)?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        buffer.Lock(&mut ptr, Some(&mut len), None)?;
        if len as usize >= frame_bytes {
            std::ptr::copy_nonoverlapping(buf.as_ptr(), ptr, frame_bytes);
        }
        buffer.Unlock()?;
        // 必须设置有效数据长度，否则 sample 长度为 0
        buffer.SetCurrentLength(frame_bytes as u32)?;

        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(f as i64 * 10_000_000 / 30)?;
        sample.SetSampleDuration(10_000_000 / 30)?;

        if let Err(e) = sink_writer.WriteSample(stream_index, &sample) {
            println!("  WriteSample 第 {} 帧失败: {:?}", f, e);
            return Err(e);
        }
        if f == 0 {
            println!("  第 0 帧写入成功");
        }
    }
    println!("  写入 {} 帧完成", FRAMES);

    sink_writer.Flush(stream_index)?;
    println!("  Flush 成功");
    sink_writer.Finalize()?;
    println!("  Finalize 成功");

    drop(sink_writer);

    // ---- 校验产物 ----
    let meta = std::fs::metadata(&path)?;
    let all = std::fs::read(&path)?;
    let has = |tag: &[u8; 4]| all.windows(4).any(|w| w == tag);
    let is_ftyp = has(b"ftyp");
    let has_moov = has(b"moov");
    let has_mdat = has(b"mdat");
    println!(
        "  产物 {} ({} 字节) ftyp={} moov={} mdat={}",
        path.display(),
        meta.len(),
        is_ftyp,
        has_moov,
        has_mdat
    );

    if !is_ftyp || !has_moov || !has_mdat || meta.len() < 1000 {
        return Err(windows::core::Error::from(windows::core::HRESULT(
            0x80004005u32 as i32,
        )));
    }
    Ok(format!(
        "真实 MP4 已写出：{} 字节，ftyp/moov/mdat 齐全",
        meta.len()
    ))
}
