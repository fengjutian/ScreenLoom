//! 探针 7：定位「节流开启后 WriteSample 阻塞」到底卡在哪一步。
//!
//! 逐帧打点：每次 WriteSample 前后打印相对耗时，Flush / Finalize 前后也打点。
//! 输出目录在 `%TEMP%`，避免被仓库清理脚本删掉。
//!
//! 用法：probe7 <0|1>   0 = 关节流（DISABLE_THROTTLING=1），1 = 开节流（默认）

use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFStartup, MF_VERSION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MF_MT_ALL_SAMPLES_INDEPENDENT, MFTranscodeContainerType_MPEG4,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFMediaType_Video,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const W: u32 = 320;
const H: u32 = 240;

fn main() {
    let throttle_on = std::env::args().nth(1).map(|s| s != "0").unwrap_or(true);
    let n_frames: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    // MF_MT_ALL_SAMPLES_INDEPENDENT 放哪：in / out / none
    let indep_mode = std::env::args().nth(3).unwrap_or_else(|| "in".into());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, 0).is_err() {
            eprintln!("MFStartup 失败");
            CoUninitialize();
            return;
        }

        let dir = std::env::temp_dir().join("screenloom_probe");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "{}_{}f_{}.mp4",
            if throttle_on { "on" } else { "off" },
            n_frames,
            indep_mode
        ));
        let _ = std::fs::remove_file(&path);
        println!("产物: {}", path.display());

        let url: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 0).unwrap();
        let attrs = attrs.unwrap();
        let _ = attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4);
        if !throttle_on {
            let _ = attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1);
        }
        let _ = attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);

        let t0 = Instant::now();
        let sw: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs).unwrap();
        println!("[{:>6}ms] writer 建好", t0.elapsed().as_millis());

        let in_type = MFCreateMediaType().unwrap();
        let _ = in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
        let _ = in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12);
        let _ = in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
        let _ = in_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
        let _ = in_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64));
        let _ = in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, W);
        let _ = in_type.SetUINT32(&MF_MT_SAMPLE_SIZE, W * H + (W / 2) * (H / 2) * 2);
        if indep_mode == "in" {
            let _ = in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
        }

        let out_type = MFCreateMediaType().unwrap();
        let _ = out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
        let _ = out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
        let _ = out_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
        let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
        let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64));
        let _ = out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
        if indep_mode == "in" {
            let _ = out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
        }
        let _ = out_type.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000);

        let si = sw.AddStream(&out_type).unwrap();
        println!("[{:>6}ms] AddStream ok", t0.elapsed().as_millis());
        sw.SetInputMediaType(si, &in_type, None).unwrap();
        println!("[{:>6}ms] SetInputMediaType ok", t0.elapsed().as_millis());
        sw.BeginWriting().unwrap();
        println!("[{:>6}ms] BeginWriting ok", t0.elapsed().as_millis());

        let frame_bytes = (W * H + (W / 2) * (H / 2) * 2) as usize;
        let pixels = vec![128u8; frame_bytes];
        for f in 0..n_frames {
            let sample: IMFSample = MFCreateSample().unwrap();
            let buffer = MFCreateMemoryBuffer(frame_bytes as u32).unwrap();
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len: u32 = 0;
            buffer.Lock(&mut ptr, Some(&mut len), None).unwrap();
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, frame_bytes);
            let _ = buffer.Unlock();
            let _ = buffer.SetCurrentLength(frame_bytes as u32);
            let _ = sample.AddBuffer(&buffer);
            let _ = sample.SetSampleTime(f as i64 * 10_000_000 / 30);
            let _ = sample.SetSampleDuration(10_000_000 / 30);
            sw.WriteSample(si, &sample).unwrap();
            println!("[{:>6}ms] WriteSample #{} ok", t0.elapsed().as_millis(), f);
        }

        println!("[{:>6}ms] 开始 Flush", t0.elapsed().as_millis());
        sw.Flush(si).unwrap();
        println!("[{:>6}ms] Flush ok", t0.elapsed().as_millis());
        sw.Finalize().unwrap();
        println!("[{:>6}ms] Finalize ok", t0.elapsed().as_millis());
        drop(sw);

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("[{:>6}ms] 完成，产物 {} 字节", t0.elapsed().as_millis(), size);

        CoUninitialize();
    }
}
