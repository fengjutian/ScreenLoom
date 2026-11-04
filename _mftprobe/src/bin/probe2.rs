//! 探针 2：回答「录屏实现」里三个决定架构的问题（逐步定位失败点）。
//!
//! A. MF 能否**直接接受 BGRA 输入**？
//!    若能 → 不需要自己写 BGRA→NV12 转换（省掉最大的 CPU 开销）。
//!    注意：探针 1 用 RGB32 失败（0x80070057），但那次 **stride 填错了**
//!    （RGB32 的 stride 应为 W*4，探针 1 填的是 W）。这里用正确 stride 重测。
//!
//! B. `MF_MT_AVG_BITRATE` 放到**输出媒体类型**上，能不能控制实际码率？
//!
//! C. 颜色属性（YUV matrix / transfer / primaries / range）能不能写进文件？
//!    探针 1 的产物 ffprobe 报 `color_space=unknown`，播放端会猜矩阵 → 可能偏色。
//!
//! 每一步都单独报错误码，这样能精确定位是哪个调用被拒。

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFStartup, MF_VERSION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MF_MT_ALL_SAMPLES_INDEPENDENT, MFTranscodeContainerType_MPEG4,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE,
    MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES,
    MF_MT_YUV_MATRIX, MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoFormat_RGB32,
    MFMediaType_Video, MFVideoPrimaries_BT709, MFVideoTransFunc_709,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const W: u32 = 64;
const H: u32 = 64;
const FRAMES: u32 = 10;

/// `MFVideoColorSpace_BT709` —— windows-rs 0.61 **没有导出**这个枚举
/// （`MFVideoPrimaries` / `MFVideoTransferFunction` 都有，唯独缺 ColorSpace），
/// 这里按 mfapi.h 的原始取值补一个局部常量。
const MF_VIDEO_COLOR_SPACE_BT709: u32 = 1;

/// 一帧缓冲的构成方式。
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// NV12（Y 平面 + UV 交错平面）
    Nv12,
    /// RGB32（内存序 B,G,R,A）
    Bgra,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Nv12 => "NV12",
            Kind::Bgra => "BGRA/RGB32",
        }
    }
    fn stride(self) -> u32 {
        match self {
            Kind::Nv12 => W,
            Kind::Bgra => W * 4,
        }
    }
    fn sample_size(self) -> u32 {
        match self {
            Kind::Nv12 => W * H + (W / 2) * (H / 2) * 2,
            Kind::Bgra => W * H * 4,
        }
    }
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, 0).is_err() {
            eprintln!("MFStartup 失败");
            CoUninitialize();
            return;
        }

        println!("=== 对照组：NV12，无任何额外属性（探针 1 的配置）===");
        run("ctl_nv12", Kind::Nv12, Attrs::none());

        println!("\n=== A. BGRA(RGB32) 直接作为输入类型 ===");
        run("a_bgra", Kind::Bgra, Attrs::none());

        println!("\n=== B. 输出媒体类型带 MF_MT_AVG_BITRATE ===");
        run("b_bitrate", Kind::Nv12, Attrs::bitrate(2_000_000));

        println!("\n=== C. 输出媒体类型带颜色属性 ===");
        run("c_color", Kind::Nv12, Attrs::none().color());

        println!("\n=== D. NV12 + 码率 + 颜色属性（组合）===");
        run("d_all", Kind::Nv12, Attrs::bitrate(2_000_000).color());

        CoUninitialize();
    }
}

/// 输出媒体类型上要附加的属性组合。
#[derive(Clone, Copy)]
struct Attrs {
    bitrate: Option<u32>,
    color: bool,
}

impl Attrs {
    fn none() -> Self {
        Attrs {
            bitrate: None,
            color: false,
        }
    }
    fn bitrate(v: u32) -> Self {
        Attrs {
            bitrate: Some(v),
            color: false,
        }
    }
    fn color(mut self) -> Self {
        self.color = true;
        self
    }
}

unsafe fn run(tag: &str, kind: Kind, attrs: Attrs) {
    let path = format!(r"D:\github\screenloom\_probe_{tag}.mp4");
    let p = std::path::PathBuf::from(&path);
    let _ = std::fs::remove_file(&p);
    let url: Vec<u16> = p
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut attr_store: Option<IMFAttributes> = None;
    if MFCreateAttributes(&mut attr_store, 0).is_err() || attr_store.is_none() {
        println!("  [{}] MFCreateAttributes 失败", tag);
        return;
    }
    let attr_store = attr_store.unwrap();


    if let Err(e) = attr_store.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
    {
        println!("  [{}] 设容器类型失败: {:?}", tag, e);
        return;
    }
    let _ = attr_store.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1);
    let _ = attr_store.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);

    let sw: IMFSinkWriter = match report_step(
        tag,
        "MFCreateSinkWriterFromURL",
        MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attr_store),
    ) {
        Some(v) => v,
        None => return,
    };

    // ---- 输入媒体类型 ----
    let in_type = match report_step(tag, "MFCreateMediaType(input)", MFCreateMediaType()) {
        Some(v) => v,
        None => return,
    };
    let _ = in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = in_type.SetGUID(
        &MF_MT_SUBTYPE,
        match kind {
            Kind::Nv12 => &MFVideoFormat_NV12,
            Kind::Bgra => &MFVideoFormat_RGB32,
        },
    );
    let _ = in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = in_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = in_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64));
    let _ = in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, kind.stride());
    let _ = in_type.SetUINT32(&MF_MT_SAMPLE_SIZE, kind.sample_size());
    let _ = in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);

    // ---- 输出媒体类型 ----
    let out_type = match report_step(tag, "MFCreateMediaType(output)", MFCreateMediaType()) {
        Some(v) => v,
        None => return,
    };
    let _ = out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);
    let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, (W as u64) << 32 | (H as u64));
    let _ = out_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
    if let Some(b) = attrs.bitrate {
        if let Err(e) = out_type.SetUINT32(&MF_MT_AVG_BITRATE, b) {
            println!("  [{}] 设 MF_MT_AVG_BITRATE 失败: {:?}", tag, e);
            return;
        }
        println!("  [{}] MF_MT_AVG_BITRATE = {}", tag, b);
    }
    if attrs.color {
        let _ = out_type.SetUINT32(&MF_MT_YUV_MATRIX, MF_VIDEO_COLOR_SPACE_BT709);
        let _ = out_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
        let _ = out_type.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        let _ = out_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, 0); // limited 16-235
        println!("  [{}] 颜色属性已设（BT.709 / limited）", tag);
    }

    let stream_index = match report_step(tag, "AddStream(H.264)", sw.AddStream(&out_type)) {
        Some(v) => v,
        None => return,
    };
    if let None = report_step(tag, "SetInputMediaType", sw.SetInputMediaType(stream_index, &in_type, None)) {
        return;
    }
    if let None = report_step(tag, "BeginWriting", sw.BeginWriting()) {
        return;
    }

    // ---- 写帧 ----
    let frame_bytes = kind.sample_size() as usize;
    let pixels = make_pixels(kind);
    for f in 0..FRAMES {
        let sample: IMFSample = match MFCreateSample() {
            Ok(s) => s,
            Err(e) => {
                println!("  [{}] MFCreateSample 失败: {:?}", tag, e);
                return;
            }
        };
        let buffer = match MFCreateMemoryBuffer(frame_bytes as u32) {
            Ok(b) => b,
            Err(e) => {
                println!("  [{}] MFCreateMemoryBuffer 失败: {:?}", tag, e);
                return;
            }
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        if let Err(e) = buffer.Lock(&mut ptr, Some(&mut len), None) {
            println!("  [{}] Lock 失败: {:?}", tag, e);
            return;
        }
        if len as usize >= frame_bytes {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, frame_bytes);
        }
        let _ = buffer.Unlock();
        if let Err(e) = buffer.SetCurrentLength(frame_bytes as u32) {
            println!("  [{}] SetCurrentLength 失败: {:?}", tag, e);
            return;
        }
        if let Err(e) = sample.AddBuffer(&buffer) {
            println!("  [{}] AddBuffer 失败: {:?}", tag, e);
            return;
        }
        let _ = sample.SetSampleTime(f as i64 * 10_000_000 / 30);
        let _ = sample.SetSampleDuration(10_000_000 / 30);
        if let Err(e) = sw.WriteSample(stream_index, &sample) {
            println!(
                "  [{}] WriteSample 第 {} 帧失败: 0x{:08x} {}",
                tag,
                f,
                e.code().0 as u32,
                e.message()
            );
            return;
        }
    }
    println!("  [{}] 写入 {} 帧完成", tag, FRAMES);

    if let Err(e) = sw.Flush(stream_index) {
        println!("  [{}] Flush 失败: 0x{:08x} {}", tag, e.code().0 as u32, e.message());
        return;
    }
    println!("  [{}] Flush OK", tag);
    if let Err(e) = sw.Finalize() {
        println!(
            "  [{}] Finalize 失败: 0x{:08x} {}",
            tag,
            e.code().0 as u32,
            e.message()
        );
        return;
    }
    println!("  [{}] Finalize OK", tag);
    drop(sw);

    match std::fs::metadata(&p) {
        Ok(m) => println!("  [{}] 产物 {} ({} 字节)", tag, path, m.len()),
        Err(e) => println!("  [{}] 产物读不到: {:?}", tag, e),
    }
}

/// 报告一步的结果；成功打印 OK，失败打印 HRESULT 并返回 None。
fn report_step<T>(tag: &str, name: &str, r: windows::core::Result<T>) -> Option<T> {
    match r {
        Ok(v) => {
            println!("  [{}] {} OK", tag, name);
            Some(v)
        }
        Err(e) => {
            println!(
                "  [{}] {} 失败: 0x{:08x} {}",
                tag,
                name,
                e.code().0 as u32,
                e.message()
            );
            None
        }
    }
}

/// 造一帧测试内容。
///
/// - NV12：Y=128 中灰，U=V=128。
/// - BGRA：纯红（内存序 B=0,G=0,R=255,A=255）。
///   若 MF 按 BGRA 解释 → 解码是红；若按 RGBA 解释 → 解码是蓝。
unsafe fn make_pixels(kind: Kind) -> Vec<u8> {
    match kind {
        Kind::Nv12 => {
            let y_size = (W * H) as usize;
            let uv_size = ((W / 2) * (H / 2)) as usize;
            let mut v = vec![128u8; y_size + uv_size * 2];
            v[..y_size].iter_mut().for_each(|b| *b = 128);
            v
        }
        Kind::Bgra => vec![0u8, 0, 255, 255].repeat((W * H) as usize),
    }
}
