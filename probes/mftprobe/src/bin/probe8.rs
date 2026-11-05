//! 探针 8：验证 MF 的 **Fragmented MP4** 容器能否作为「崩溃后仍可播」的落盘格式。
//!
//! 要回答的三个问题：
//!
//! 1. `MFTranscodeContainerType_FMPEG4` 能否被 `IMFSinkWriter` 接受？
//! 2. 写入过程中数据是否**增量落盘**（而不是像普通 MP4 那样憋到 `Finalize`）？
//! 3. 进程被杀（不 Flush / 不 Finalize）后，文件能否被解析、被 remux 成普通 MP4？
//!
//! 用法：
//!
//! ```text
//! probe8 write <mp4|fmp4> <frames> <finalize|flush-crash|crash>
//! probe8 boxes <file>
//! probe8 remux <in> <out>
//! ```
//!
//! 产物统一落到 `probes/mftprobe/out/`。

use std::path::{Path, PathBuf};
use std::time::Instant;

use mf_probe::out_path;
use windows::core::{Error as WinError, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL,
    MFMediaType_Video, MFStartup, MFTranscodeContainerType_FMPEG4, MFTranscodeContainerType_MPEG4,
    MFVideoFormat_H264, MFVideoFormat_NV12, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const W: u32 = 320;
const H: u32 = 240;
const FPS: u32 = 30;
const FRAME_BYTES: usize = (W * H + (W / 2) * (H / 2) * 2) as usize;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("write") => write_cmd(&args),
        Some("boxes") => boxes_cmd(&args),
        Some("remux") => remux_cmd(&args),
        _ => {
            usage();
            return;
        }
    };
    if let Err(e) = r {
        eprintln!("失败: {e}");
        std::process::exit(1);
    }
}

fn usage() {
    eprintln!("用法:");
    eprintln!("  probe8 write <mp4|fmp4> <frames> <finalize|flush-crash|crash>");
    eprintln!("  probe8 boxes <file>");
    eprintln!("  probe8 remux <in> <out>");
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

fn write_cmd(args: &[String]) -> Result<(), String> {
    let container = args.get(2).map(String::as_str).unwrap_or("fmp4");
    let frames: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(90);
    let mode = args.get(4).map(String::as_str).unwrap_or("finalize");
    if !matches!(container, "mp4" | "fmp4") {
        return Err(format!("未知容器 `{container}`"));
    }
    if !matches!(mode, "finalize" | "flush-crash" | "crash") {
        return Err(format!("未知模式 `{mode}`"));
    }

    let _com = ComGuard::init();
    unsafe {
        MFStartup(MF_VERSION, 0).map_err(|e| hr("MFStartup", &e))?;

        let path = out_path(&format!("p8_{container}_{frames}f_{mode}.mp4"));
        let _ = std::fs::remove_file(&path);
        println!("模式: container={container} frames={frames} mode={mode}");
        println!("产物: {}", path.display());

        let t0 = Instant::now();
        let (writer, stream) = create_writer(&path, container == "fmp4")?;
        println!("[{:>6}ms] writer 就绪", t0.elapsed().as_millis());

        let pixels = vec![128u8; FRAME_BYTES];
        let mut next_report = 30;
        for f in 0..frames {
            write_frame(&writer, stream, &pixels, f)?;
            if f + 1 == next_report {
                println!(
                    "[{:>6}ms] 已写 {} 帧，磁盘 {} 字节",
                    t0.elapsed().as_millis(),
                    f + 1,
                    file_len(&path)
                );
                next_report += 30;
            }
        }
        println!(
            "[{:>6}ms] 写帧结束，磁盘 {} 字节",
            t0.elapsed().as_millis(),
            file_len(&path)
        );

        match mode {
            "finalize" => {
                writer.Flush(stream).map_err(|e| hr("Flush", &e))?;
                println!(
                    "[{:>6}ms] Flush ok，磁盘 {} 字节",
                    t0.elapsed().as_millis(),
                    file_len(&path)
                );
                writer.Finalize().map_err(|e| hr("Finalize", &e))?;
                println!(
                    "[{:>6}ms] Finalize ok，磁盘 {} 字节",
                    t0.elapsed().as_millis(),
                    file_len(&path)
                );
                drop(writer);
            }
            "flush-crash" => {
                writer.Flush(stream).map_err(|e| hr("Flush", &e))?;
                println!(
                    "[{:>6}ms] Flush ok，磁盘 {} 字节；现在直接退出（不 Finalize）",
                    t0.elapsed().as_millis(),
                    file_len(&path)
                );
                std::process::exit(0);
            }
            "crash" => {
                println!(
                    "[{:>6}ms] 直接退出（不 Flush / 不 Finalize），模拟进程被杀",
                    t0.elapsed().as_millis()
                );
                std::process::exit(0);
            }
            _ => unreachable!(),
        }

        print_boxes(&path)?;
    }
    Ok(())
}

/// 建 SinkWriter + 视频流。容器通过 `MF_TRANSCODE_CONTAINERTYPE` 选择。
unsafe fn create_writer(path: &Path, fragmented: bool) -> Result<(IMFSinkWriter, u32), String> {
    let url = wide(path);
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 0).map_err(|e| hr("MFCreateAttributes", &e))?;
    let attrs = attrs.ok_or("MFCreateAttributes 返回空")?;

    let container = if fragmented {
        MFTranscodeContainerType_FMPEG4
    } else {
        MFTranscodeContainerType_MPEG4
    };
    attrs
        .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container)
        .map_err(|e| hr("SetGUID(CONTAINERTYPE)", &e))?;
    attrs
        .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
        .map_err(|e| hr("SetUINT32(HW_TRANSFORMS)", &e))?;

    let writer: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs)
        .map_err(|e| hr("MFCreateSinkWriterFromURL", &e))?;

    let out_type = MFCreateMediaType().map_err(|e| hr("MFCreateMediaType(out)", &e))?;
    out_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|e| hr("out.SetGUID(MAJOR)", &e))?;
    out_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
        .map_err(|e| hr("out.SetGUID(SUBTYPE)", &e))?;
    out_type
        .SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)
        .map_err(|e| hr("out.SetUINT64(FPS)", &e))?;
    out_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, 2)
        .map_err(|e| hr("out.SetUINT32(INTERLACE)", &e))?;
    out_type
        .SetUINT64(&MF_MT_FRAME_SIZE, ((W as u64) << 32) | H as u64)
        .map_err(|e| hr("out.SetUINT64(SIZE)", &e))?;
    out_type
        .SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)
        .map_err(|e| hr("out.SetUINT32(BITRATE)", &e))?;
    out_type
        .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
        .map_err(|e| hr("out.SetUINT32(INDEP)", &e))?;

    let stream = writer
        .AddStream(&out_type)
        .map_err(|e| hr("AddStream", &e))?;

    let in_type = MFCreateMediaType().map_err(|e| hr("MFCreateMediaType(in)", &e))?;
    in_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|e| hr("in.SetGUID(MAJOR)", &e))?;
    in_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
        .map_err(|e| hr("in.SetGUID(SUBTYPE)", &e))?;
    in_type
        .SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)
        .map_err(|e| hr("in.SetUINT64(FPS)", &e))?;
    in_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, 2)
        .map_err(|e| hr("in.SetUINT32(INTERLACE)", &e))?;
    in_type
        .SetUINT64(&MF_MT_FRAME_SIZE, ((W as u64) << 32) | H as u64)
        .map_err(|e| hr("in.SetUINT64(SIZE)", &e))?;
    in_type
        .SetUINT32(&MF_MT_DEFAULT_STRIDE, W)
        .map_err(|e| hr("in.SetUINT32(STRIDE)", &e))?;
    in_type
        .SetUINT32(&MF_MT_SAMPLE_SIZE, FRAME_BYTES as u32)
        .map_err(|e| hr("in.SetUINT32(SAMPLE_SIZE)", &e))?;

    writer
        .SetInputMediaType(stream, &in_type, None)
        .map_err(|e| hr("SetInputMediaType", &e))?;
    writer.BeginWriting().map_err(|e| hr("BeginWriting", &e))?;
    Ok((writer, stream))
}

unsafe fn write_frame(
    writer: &IMFSinkWriter,
    stream: u32,
    pixels: &[u8],
    index: u32,
) -> Result<(), String> {
    let sample: IMFSample = MFCreateSample().map_err(|e| hr("MFCreateSample", &e))?;
    let buffer = MFCreateMemoryBuffer(FRAME_BYTES as u32)
        .map_err(|e| hr("MFCreateMemoryBuffer", &e))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len: u32 = 0;
    buffer
        .Lock(&mut ptr, Some(&mut len), None)
        .map_err(|e| hr("buffer.Lock", &e))?;
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, FRAME_BYTES);
    let _ = buffer.Unlock();
    buffer
        .SetCurrentLength(FRAME_BYTES as u32)
        .map_err(|e| hr("SetCurrentLength", &e))?;
    sample.AddBuffer(&buffer).map_err(|e| hr("AddBuffer", &e))?;
    sample
        .SetSampleTime(index as i64 * 10_000_000 / FPS as i64)
        .map_err(|e| hr("SetSampleTime", &e))?;
    sample
        .SetSampleDuration(10_000_000 / FPS as i64)
        .map_err(|e| hr("SetSampleDuration", &e))?;
    writer
        .WriteSample(stream, &sample)
        .map_err(|e| hr("WriteSample", &e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// boxes
// ---------------------------------------------------------------------------

fn boxes_cmd(args: &[String]) -> Result<(), String> {
    let p = args.get(2).ok_or("缺文件参数")?;
    print_boxes(Path::new(p))
}

/// 打印顶层 box 列表，顺带给出「是否为分片结构」的判定。
fn print_boxes(path: &Path) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("读 {} 失败: {e}", path.display()))?;
    println!(
        "box 列表（{}，共 {} 字节）:",
        path.display(),
        data.len()
    );
    let mut off = 0usize;
    let mut counts: Vec<(String, u32)> = Vec::new();
    let mut i = 0;
    while off + 8 <= data.len() && i < 64 {
        let size32 = u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
        let ty = String::from_utf8_lossy(&data[off + 4..off + 8]).to_string();
        let (size, header) = if size32 == 1 {
            if off + 16 > data.len() {
                break;
            }
            let big = u64::from_be_bytes([
                data[off + 8],
                data[off + 9],
                data[off + 10],
                data[off + 11],
                data[off + 12],
                data[off + 13],
                data[off + 14],
                data[off + 15],
            ]);
            (big as usize, 16)
        } else if size32 == 0 {
            (data.len() - off, 8)
        } else {
            (size32 as usize, 8)
        };
        println!("  @{off:>9}  {ty:<6} {size:>10} 字节");
        if let Some(c) = counts.iter_mut().find(|(k, _)| *k == ty) {
            c.1 += 1;
        } else {
            counts.push((ty.clone(), 1));
        }
        if size < header {
            break;
        }
        off += size;
        i += 1;
    }
    let fragmented = counts
        .iter()
        .any(|(k, n)| (k == "moof" || k == "styp") && *n > 0);
    println!(
        "判定: {} (moov={}, moof={}, mdat={}, styp={})",
        if fragmented {
            "分片结构"
        } else if off >= data.len() {
            "普通结构（或已完整）"
        } else {
            "无法判定/被截断"
        },
        count_of(&counts, "moov"),
        count_of(&counts, "moof"),
        count_of(&counts, "mdat"),
        count_of(&counts, "styp"),
    );
    Ok(())
}

fn count_of(counts: &[(String, u32)], name: &str) -> u32 {
    counts
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, n)| *n)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// remux：分片文件 → 普通 MP4（压缩样本直通，不重编码）
// ---------------------------------------------------------------------------

fn remux_cmd(args: &[String]) -> Result<(), String> {
    let input = args.get(2).ok_or("缺输入文件")?;
    let output = args.get(3).ok_or("缺输出文件")?;
    let _com = ComGuard::init();
    unsafe {
        MFStartup(MF_VERSION, 0).map_err(|e| hr("MFStartup", &e))?;
        let t0 = Instant::now();

        let in_url = wide(Path::new(input));
        let reader = MFCreateSourceReaderFromURL(PCWSTR(in_url.as_ptr()), None)
            .map_err(|e| hr("MFCreateSourceReaderFromURL", &e))?;
        let vstream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(|e| hr("SetStreamSelection(all,false)", &e))?;
        reader
            .SetStreamSelection(vstream, true)
            .map_err(|e| hr("SetStreamSelection(video,true)", &e))?;
        let native = reader
            .GetNativeMediaType(vstream, 0)
            .map_err(|e| hr("GetNativeMediaType", &e))?;
        reader
            .SetCurrentMediaType(vstream, None, &native)
            .map_err(|e| hr("SetCurrentMediaType", &e))?;
        println!("[{:>6}ms] SourceReader 打开输入 ok", t0.elapsed().as_millis());

        let out_url = wide(Path::new(output));
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 0).map_err(|e| hr("MFCreateAttributes", &e))?;
        let attrs = attrs.ok_or("MFCreateAttributes 返回空")?;
        attrs
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .map_err(|e| hr("SetGUID(CONTAINERTYPE)", &e))?;
        let sink: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(out_url.as_ptr()), None, &attrs)
            .map_err(|e| hr("MFCreateSinkWriterFromURL", &e))?;
        let idx = sink
            .AddStream(&native)
            .map_err(|e| hr("AddStream(video)", &e))?;
        sink.SetInputMediaType(idx, &native, None)
            .map_err(|e| hr("SetInputMediaType(video)", &e))?;
        sink.BeginWriting().map_err(|e| hr("BeginWriting", &e))?;

        let mut n = 0u32;
        loop {
            let mut actual = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    vstream,
                    0,
                    Some(&mut actual),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| hr("ReadSample", &e))?;
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            let copy = copy_sample(&sample)?;
            sink.WriteSample(idx, &copy)
                .map_err(|e| hr("WriteSample(remux)", &e))?;
            n += 1;
        }
        sink.Finalize().map_err(|e| hr("Finalize(remux)", &e))?;
        println!(
            "[{:>6}ms] remux 完成：{} 帧 → {}（{} 字节）",
            t0.elapsed().as_millis(),
            n,
            output,
            file_len(Path::new(output))
        );
    }
    Ok(())
}

unsafe fn copy_sample(src: &IMFSample) -> Result<IMFSample, String> {
    let buffer = src.GetBufferByIndex(0).map_err(|e| hr("GetBufferByIndex", &e))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len: u32 = 0;
    buffer
        .Lock(&mut ptr, Some(&mut len), None)
        .map_err(|e| hr("Lock", &e))?;
    let data = std::slice::from_raw_parts(ptr, len as usize);
    let dst = MFCreateMemoryBuffer(len).map_err(|e| hr("MFCreateMemoryBuffer(copy)", &e))?;
    let mut dst_ptr: *mut u8 = std::ptr::null_mut();
    let mut dst_len: u32 = 0;
    dst.Lock(&mut dst_ptr, Some(&mut dst_len), None)
        .map_err(|e| hr("dst.Lock", &e))?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), dst_ptr, len as usize);
    let _ = buffer.Unlock();
    let _ = dst.SetCurrentLength(len);
    let _ = dst.Unlock();

    let out = MFCreateSample().map_err(|e| hr("MFCreateSample(copy)", &e))?;
    out.AddBuffer(&dst).map_err(|e| hr("AddBuffer(copy)", &e))?;
    if let Ok(t) = src.GetSampleTime() {
        let _ = out.SetSampleTime(t);
    }
    if let Ok(d) = src.GetSampleDuration() {
        let _ = out.SetSampleDuration(d);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn hr(what: &str, e: &WinError) -> String {
    format!("{what} 失败: {e} (HRESULT {:#010X})", e.code().0 as u32)
}

struct ComGuard(#[allow(dead_code)] bool);

impl ComGuard {
    fn init() -> Self {
        let ok = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        ComGuard(ok)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
```

Issues:
- Import `PathBuf` unused → remove.
- `MF_TRANSCODE_CONTAINERTYPE` imported ✓.
- `MF_MT_ALL_SAMPLES_INDEPENDENT` etc ✓.
- `MFMediaType_Video` is a GUID constant; SetGUID takes &GUID; passing &MFMediaType_Video works (probe7 does it).
- `MFCreateSinkWriterFromURL(PCWSTR(...), None, &attrs)`: in 0.61, third param may be `Option<&IMFAttributes>`; probe7 passes &attrs, fine.
- `writer.Flush(stream)` returns Result<()>.
- `SetStreamSelection` in 0.61: `fn SetStreamSelection(&self, dwstreamindex: u32, fenable: BOOL)`. mux.rs passes false — bool accepted via Into<BOOL>. Good.
- `MF_SOURCE_READERF_ENDOFSTREAM.0 as u32` — in mux.rs same.
- `GetNativeMediaType(vstream, 0)` — signature `(dwstreamindex: u32, dwmediatypeindex: u32)`.
- `sample.GetBufferByIndex(0)`.
- unused `MF_MT_DEFAULT_STRIDE` used ✓.
- `count_of` comparing `k == name` where k: &String, name: &str → need `k.as_str() == name` (String == &str works via PartialEq<str> for String; `k == name` with k:&String and name:&str → PartialEq<&str> for &String? Might not compile). Safer: `k.as_str() == name`.
- In counts find: `find(|(k, _)| k == name)` similar issue. Use `k.as_str() == name`.
- `let fragmented = counts.iter().any(|(k, n)| (k == "moof" || k == "styp") && *n > 0);` — same fix.
- `if fragmented {...} else if off >= data.len()` — off variable scope: defined outside loop ✓.
- `ComGuard(#[allow(dead_code)] bool)` — attribute on tuple field is allowed? `struct ComGuard(#[allow(dead_code)] bool);` yes but `bool` used in Drop so not dead. Simplify: `struct ComGuard(bool);` and use self.0.
- process::exit after Flush: COM guard not dropped (exit skips) — fine.
- `MFStartup` return type: in windows-rs 0.61, `MFStartup(version: u32, dwflags: u32) -> Result<()>`. probe7 uses `.is_err()`. Fine.
- With `MFStartup` called but no `MFShutdown` — acceptable.
- Box parse: `size32 == 1` large size; `size32 == 0` to EOF; if size==0 and header 8, off += size = data.len()-off+off = len ✓.
- print_boxes called after drop(writer) in finalize mode ✓. For crash modes we exit before.

Also: for fMP4, the file might be empty until finalize; our per-30-frame print reveals.

One more: `MF_SOURCE_READER_ALL_STREAMS` and `FIRST_VIDEO_STREAM` are `MF_SOURCE_READER_CONSTANTS` maybe named `MF_SOURCE_READER_ALL_STREAMS` with `.0` i32. mux.rs uses same, so fine.

Compile will tell. Let me write and build. Also update probes/README table after success.

First write file, then cargo build.<｜end▁of▁thinking｜>

<｜｜DSML｜｜ calls>
<｜｜DSML｜｜ invoke name="write">
<｜｜DSML｜｜ parameter name="content" string="true">//! 探针 8：验证 MF 的 **Fragmented MP4** 容器能否作为「崩溃后仍可播」的落盘格式。
//!
//! 要回答的三个问题：
//!
//! 1. `MFTranscodeContainerType_FMPEG4` 能否被 `IMFSinkWriter` 接受？
//! 2. 写入过程中数据是否**增量落盘**（而不是像普通 MP4 那样憋到 `Finalize`）？
//! 3. 进程被杀（不 Flush / 不 Finalize）后，文件能否被解析、被 remux 成普通 MP4？
//!
//! 用法：
//!
//! ```text
//! probe8 write <mp4|fmp4> <frames> <finalize|flush-crash|crash>
//! probe8 boxes <file>
//! probe8 remux <in> <out>
//! ```
//!
//! 产物统一落到 `probes/mftprobe/out/`。

use std::path::Path;
use std::time::Instant;

use mf_probe::out_path;
use windows::core::{Error as WinError, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL,
    MFMediaType_Video, MFStartup, MFTranscodeContainerType_FMPEG4, MFTranscodeContainerType_MPEG4,
    MFVideoFormat_H264, MFVideoFormat_NV12, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const W: u32 = 320;
const H: u32 = 240;
const FPS: u32 = 30;
const FRAME_BYTES: usize = (W * H + (W / 2) * (H / 2) * 2) as usize;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("write") => write_cmd(&args),
        Some("boxes") => boxes_cmd(&args),
        Some("remux") => remux_cmd(&args),
        _ => {
            usage();
            return;
        }
    };
    if let Err(e) = r {
        eprintln!("失败: {e}");
        std::process::exit(1);
    }
}

fn usage() {
    eprintln!("用法:");
    eprintln!("  probe8 write <mp4|fmp4> <frames> <finalize|flush-crash|crash>");
    eprintln!("  probe8 boxes <file>");
    eprintln!("  probe8 remux <in> <out>");
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

fn write_cmd(args: &[String]) -> Result<(), String> {
    let container = args.get(2).map(String::as_str).unwrap_or("fmp4");
    let frames: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(90);
    let mode = args.get(4).map(String::as_str).unwrap_or("finalize");
    if !matches!(container, "mp4" | "fmp4") {
        return Err(format!("未知容器 `{container}`"));
    }
    if !matches!(mode, "finalize" | "flush-crash" | "crash") {
        return Err(format!("未知模式 `{mode}`"));
    }

    let _com = ComGuard::init();
    unsafe {
        MFStartup(MF_VERSION, 0).map_err(|e| hr("MFStartup", &e))?;

        let path = out_path(&format!("p8_{container}_{frames}f_{mode}.mp4"));
        let _ = std::fs::remove_file(&path);
        println!("模式: container={container} frames={frames} mode={mode}");
        println!("产物: {}", path.display());

        let t0 = Instant::now();
        let (writer, stream) = create_writer(&path, container == "fmp4")?;
        println!("[{:>6}ms] writer 就绪", t0.elapsed().as_millis());

        let pixels = vec![128u8; FRAME_BYTES];
        let mut next_report = 30;
        for f in 0..frames {
            write_frame(&writer, stream, &pixels, f)?;
            if f + 1 == next_report {
                println!(
                    "[{:>6}ms] 已写 {} 帧，磁盘 {} 字节",
                    t0.elapsed().as_millis(),
                    f + 1,
                    file_len(&path)
                );
                next_report += 30;
            }
        }
        println!(
            "[{:>6}ms] 写帧结束，磁盘 {} 字节",
            t0.elapsed().as_millis(),
            file_len(&path)
        );

        match mode {
            "finalize" => {
                writer.Flush(stream).map_err(|e| hr("Flush", &e))?;
                println!(
                    "[{:>6}ms] Flush ok，磁盘 {} 字节",
                    t0.elapsed().as_millis(),
                    file_len(&path)
                );
                writer.Finalize().map_err(|e| hr("Finalize", &e))?;
                println!(
                    "[{:>6}ms] Finalize ok，磁盘 {} 字节",
                    t0.elapsed().as_millis(),
                    file_len(&path)
                );
                drop(writer);
            }
            "flush-crash" => {
                writer.Flush(stream).map_err(|e| hr("Flush", &e))?;
                println!(
                    "[{:>6}ms] Flush ok，磁盘 {} 字节；现在直接退出（不 Finalize）",
                    t0.elapsed().as_millis(),
                    file_len(&path)
                );
                std::process::exit(0);
            }
            "crash" => {
                println!(
                    "[{:>6}ms] 直接退出（不 Flush / 不 Finalize），模拟进程被杀",
                    t0.elapsed().as_millis()
                );
                std::process::exit(0);
            }
            _ => unreachable!(),
        }

        print_boxes(&path)?;
    }
    Ok(())
}

/// 建 SinkWriter + 视频流。容器通过 `MF_TRANSCODE_CONTAINERTYPE` 选择。
unsafe fn create_writer(path: &Path, fragmented: bool) -> Result<(IMFSinkWriter, u32), String> {
    let url = wide(path);
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 0).map_err(|e| hr("MFCreateAttributes", &e))?;
    let attrs = attrs.ok_or("MFCreateAttributes 返回空")?;

    let container = if fragmented {
        MFTranscodeContainerType_FMPEG4
    } else {
        MFTranscodeContainerType_MPEG4
    };
    attrs
        .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container)
        .map_err(|e| hr("SetGUID(CONTAINERTYPE)", &e))?;
    attrs
        .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
        .map_err(|e| hr("SetUINT32(HW_TRANSFORMS)", &e))?;

    let writer: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs)
        .map_err(|e| hr("MFCreateSinkWriterFromURL", &e))?;

    let out_type = MFCreateMediaType().map_err(|e| hr("MFCreateMediaType(out)", &e))?;
    out_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|e| hr("out.SetGUID(MAJOR)", &e))?;
    out_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
        .map_err(|e| hr("out.SetGUID(SUBTYPE)", &e))?;
    out_type
        .SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)
        .map_err(|e| hr("out.SetUINT64(FPS)", &e))?;
    out_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, 2)
        .map_err(|e| hr("out.SetUINT32(INTERLACE)", &e))?;
    out_type
        .SetUINT64(&MF_MT_FRAME_SIZE, ((W as u64) << 32) | H as u64)
        .map_err(|e| hr("out.SetUINT64(SIZE)", &e))?;
    out_type
        .SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)
        .map_err(|e| hr("out.SetUINT32(BITRATE)", &e))?;
    out_type
        .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
        .map_err(|e| hr("out.SetUINT32(INDEP)", &e))?;

    let stream = writer
        .AddStream(&out_type)
        .map_err(|e| hr("AddStream", &e))?;

    let in_type = MFCreateMediaType().map_err(|e| hr("MFCreateMediaType(in)", &e))?;
    in_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|e| hr("in.SetGUID(MAJOR)", &e))?;
    in_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
        .map_err(|e| hr("in.SetGUID(SUBTYPE)", &e))?;
    in_type
        .SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)
        .map_err(|e| hr("in.SetUINT64(FPS)", &e))?;
    in_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, 2)
        .map_err(|e| hr("in.SetUINT32(INTERLACE)", &e))?;
    in_type
        .SetUINT64(&MF_MT_FRAME_SIZE, ((W as u64) << 32) | H as u64)
        .map_err(|e| hr("in.SetUINT64(SIZE)", &e))?;
    in_type
        .SetUINT32(&MF_MT_DEFAULT_STRIDE, W)
        .map_err(|e| hr("in.SetUINT32(STRIDE)", &e))?;
    in_type
        .SetUINT32(&MF_MT_SAMPLE_SIZE, FRAME_BYTES as u32)
        .map_err(|e| hr("in.SetUINT32(SAMPLE_SIZE)", &e))?;

    writer
        .SetInputMediaType(stream, &in_type, None)
        .map_err(|e| hr("SetInputMediaType", &e))?;
    writer.BeginWriting().map_err(|e| hr("BeginWriting", &e))?;
    Ok((writer, stream))
}

unsafe fn write_frame(
    writer: &IMFSinkWriter,
    stream: u32,
    pixels: &[u8],
    index: u32,
) -> Result<(), String> {
    let sample: IMFSample = MFCreateSample().map_err(|e| hr("MFCreateSample", &e))?;
    let buffer =
        MFCreateMemoryBuffer(FRAME_BYTES as u32).map_err(|e| hr("MFCreateMemoryBuffer", &e))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len: u32 = 0;
    buffer
        .Lock(&mut ptr, Some(&mut len), None)
        .map_err(|e| hr("buffer.Lock", &e))?;
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, FRAME_BYTES);
    let _ = buffer.Unlock();
    buffer
        .SetCurrentLength(FRAME_BYTES as u32)
        .map_err(|e| hr("SetCurrentLength", &e))?;
    sample.AddBuffer(&buffer).map_err(|e| hr("AddBuffer", &e))?;
    sample
        .SetSampleTime(index as i64 * 10_000_000 / FPS as i64)
        .map_err(|e| hr("SetSampleTime", &e))?;
    sample
        .SetSampleDuration(10_000_000 / FPS as i64)
        .map_err(|e| hr("SetSampleDuration", &e))?;
    writer
        .WriteSample(stream, &sample)
        .map_err(|e| hr("WriteSample", &e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// boxes
// ---------------------------------------------------------------------------

fn boxes_cmd(args: &[String]) -> Result<(), String> {
    let p = args.get(2).ok_or("缺文件参数")?;
    print_boxes(Path::new(p))
}

/// 打印顶层 box 列表，顺带给出「是否为分片结构」的判定。
fn print_boxes(path: &Path) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("读 {} 失败: {e}", path.display()))?;
    println!("box 列表（{}，共 {} 字节）:", path.display(), data.len());
    let mut off = 0usize;
    let mut counts: Vec<(String, u32)> = Vec::new();
    let mut i = 0;
    while off + 8 <= data.len() && i < 64 {
        let size32 = u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
        let ty = String::from_utf8_lossy(&data[off + 4..off + 8]).to_string();
        let (size, header) = if size32 == 1 {
            if off + 16 > data.len() {
                break;
            }
            let big = u64::from_be_bytes([
                data[off + 8],
                data[off + 9],
                data[off + 10],
                data[off + 11],
                data[off + 12],
                data[off + 13],
                data[off + 14],
                data[off + 15],
            ]);
            (big as usize, 16)
        } else if size32 == 0 {
            (data.len() - off, 8)
        } else {
            (size32 as usize, 8)
        };
        println!("  @{off:>9}  {ty:<6} {size:>10} 字节");
        if let Some(c) = counts.iter_mut().find(|(k, _)| k.as_str() == ty) {
            c.1 += 1;
        } else {
            counts.push((ty.clone(), 1));
        }
        if size < header {
            break;
        }
        off += size;
        i += 1;
    }
    let fragmented = counts
        .iter()
        .any(|(k, n)| matches!(k.as_str(), "moof" | "styp") && *n > 0);
    println!(
        "判定: {} (moov={}, moof={}, mdat={}, styp={})",
        if fragmented {
            "分片结构"
        } else if off >= data.len() {
            "普通结构（或已完整）"
        } else {
            "无法判定/被截断"
        },
        count_of(&counts, "moov"),
        count_of(&counts, "moof"),
        count_of(&counts, "mdat"),
        count_of(&counts, "styp"),
    );
    Ok(())
}

fn count_of(counts: &[(String, u32)], name: &str) -> u32 {
    counts
        .iter()
        .find(|(k, _)| k.as_str() == name)
        .map(|(_, n)| *n)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// remux：分片文件 → 普通 MP4（压缩样本直通，不重编码）
// ---------------------------------------------------------------------------

fn remux_cmd(args: &[String]) -> Result<(), String> {
    let input = args.get(2).ok_or("缺输入文件")?;
    let output = args.get(3).ok_or("缺输出文件")?;
    let _com = ComGuard::init();
    unsafe {
        MFStartup(MF_VERSION, 0).map_err(|e| hr("MFStartup", &e))?;
        let t0 = Instant::now();

        let in_url = wide(Path::new(input));
        let reader = MFCreateSourceReaderFromURL(PCWSTR(in_url.as_ptr()), None)
            .map_err(|e| hr("MFCreateSourceReaderFromURL", &e))?;
        let vstream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(|e| hr("SetStreamSelection(all,false)", &e))?;
        reader
            .SetStreamSelection(vstream, true)
            .map_err(|e| hr("SetStreamSelection(video,true)", &e))?;
        let native = reader
            .GetNativeMediaType(vstream, 0)
            .map_err(|e| hr("GetNativeMediaType", &e))?;
        reader
            .SetCurrentMediaType(vstream, None, &native)
            .map_err(|e| hr("SetCurrentMediaType", &e))?;
        println!(
            "[{:>6}ms] SourceReader 打开输入 ok",
            t0.elapsed().as_millis()
        );

        let out_url = wide(Path::new(output));
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 0).map_err(|e| hr("MFCreateAttributes", &e))?;
        let attrs = attrs.ok_or("MFCreateAttributes 返回空")?;
        attrs
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .map_err(|e| hr("SetGUID(CONTAINERTYPE)", &e))?;
        let sink: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(out_url.as_ptr()), None, &attrs)
            .map_err(|e| hr("MFCreateSinkWriterFromURL", &e))?;
        let idx = sink
            .AddStream(&native)
            .map_err(|e| hr("AddStream(video)", &e))?;
        sink.SetInputMediaType(idx, &native, None)
            .map_err(|e| hr("SetInputMediaType(video)", &e))?;
        sink.BeginWriting().map_err(|e| hr("BeginWriting", &e))?;

        let mut n = 0u32;
        loop {
            let mut actual = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    vstream,
                    0,
                    Some(&mut actual),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| hr("ReadSample", &e))?;
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            let copy = copy_sample(&sample)?;
            sink.WriteSample(idx, &copy)
                .map_err(|e| hr("WriteSample(remux)", &e))?;
            n += 1;
        }
        sink.Finalize().map_err(|e| hr("Finalize(remux)", &e))?;
        println!(
            "[{:>6}ms] remux 完成：{} 帧 → {}（{} 字节）",
            t0.elapsed().as_millis(),
            n,
            output,
            file_len(Path::new(output))
        );
    }
    Ok(())
}

unsafe fn copy_sample(src: &IMFSample) -> Result<IMFSample, String> {
    let buffer = src
        .GetBufferByIndex(0)
        .map_err(|e| hr("GetBufferByIndex", &e))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len: u32 = 0;
    buffer
        .Lock(&mut ptr, Some(&mut len), None)
        .map_err(|e| hr("Lock", &e))?;
    let data = std::slice::from_raw_parts(ptr, len as usize);
    let dst = MFCreateMemoryBuffer(len).map_err(|e| hr("MFCreateMemoryBuffer(copy)", &e))?;
    let mut dst_ptr: *mut u8 = std::ptr::null_mut();
    let mut dst_len: u32 = 0;
    dst.Lock(&mut dst_ptr, Some(&mut dst_len), None)
        .map_err(|e| hr("dst.Lock", &e))?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), dst_ptr, len as usize);
    let _ = buffer.Unlock();
    let _ = dst.SetCurrentLength(len);
    let _ = dst.Unlock();

    let out = MFCreateSample().map_err(|e| hr("MFCreateSample(copy)", &e))?;
    out.AddBuffer(&dst)
        .map_err(|e| hr("AddBuffer(copy)", &e))?;
    if let Ok(t) = src.GetSampleTime() {
        let _ = out.SetSampleTime(t);
    }
    if let Ok(d) = src.GetSampleDuration() {
        let _ = out.SetSampleDuration(d);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn hr(what: &str, e: &WinError) -> String {
    format!("{what} 失败: {e} (HRESULT {:#010X})", e.code().0 as u32)
}

struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        let ok = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        ComGuard(ok)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
