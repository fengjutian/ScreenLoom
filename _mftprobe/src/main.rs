//! 录屏可行性探针：枚举本机视频编码器，并验证 MF SinkWriter 能创建 H.264/MP4。
//!
//! 只读探针，不写工程代码。

use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaType, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateSinkWriterFromURL, MFStartup, MFTEnumEx, MF_VERSION, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SOFTWARE, MFT_ENUM_FLAG_SYNCMFT, MFSTARTUP,
};
use windows::Win32::Media::MediaProperties::{
    MF_MT_FRAME_RATE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_MT_USER_DATA, MFVideoFormat_H264, MFVideoFormat_RGB32, MFMediaType_Video,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

fn main() {
    unsafe {
        // COM + MF 初始化
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        println!("CoInitializeEx hr=0x{:08x}", hr.0 as u32);
        let mhr = MFStartup(MF_VERSION, MFSTARTUP(0));
        println!("MFStartup hr=0x{:08x}", mhr.0 as u32);
        if mhr.is_err() {
            println!("MFStartup 失败 —— 系统缺少 Media Foundation");
            CoUninitialize();
            return;
        }

        println!("\n=== 枚举视频编码器 MFT ===");
        for (flags, label) in [
            (MFT_ENUM_FLAG_SYNCMFT, "SYNCMFT(软件同步)"),
            (MFT_ENUM_FLAG_HARDWARE, "HARDWARE"),
            (MFT_ENUM_FLAG_SOFTWARE, "SOFTWARE"),
        ] {
            match MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                None,
                None,
                Some(&mut Vec::new()),
            ) {
                Ok(activates) => {
                    println!("  {} -> {} 个", label, activates.len());
                    for a in activates {
                        println!("     {}", describe_activate(a));
                    }
                }
                Err(e) => println!("  {} -> 失败 {:?}", label, e),
            }
        }

        println!("\n=== 验证 SinkWriter (H.264 / MP4) ===");
        match probe_sink_writer() {
            Ok(msg) => println!("  OK: {}", msg),
            Err(e) => println!("  FAIL: {:?}", e),
        }

        println!("\n=== 验证 MFVideoFormat_H264 GUID ===");
        println!("  H264  = {:?}", MFVideoFormat_H264);
        println!("  RGB32 = {:?}", MFVideoFormat_RGB32);

        CoUninitialize();
    }
}

/// 读出 MFT 的友好名称。
unsafe fn describe_activate(a: IMFActivate) -> String {
    use windows::Win32::Media::MediaFoundation::IMFActivate;
    match a.GetAttributes() {
        Ok(attrs) => {
            let name = windows::Win32::Media::MediaFoundation::MFGetAttributeString(
                &attrs,
                &windows::Win32::Media::MediaFoundation::MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            );
            match name {
                Ok(s) => format!("{:?}", s),
                Err(_) => "<no name>".to_string(),
            }
        }
        Err(_) => "<no attrs>".to_string(),
    }
}

/// 尝试创建 H.264 + MP4 的 SinkWriter，验证编码器链路可用。
unsafe fn probe_sink_writer() -> windows::core::Result<String> {
    // 输出到临时文件
    let path = std::env::temp_dir().join("mf_probe_out.mp4");
    let _ = std::fs::remove_file(&path);
    let url: Vec<u16> = format!("file:///{}", path.display())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let sink: IMFSinkWriter =
        MFCreateSinkWriterFromURL(&url, None, &MFCreateAttributes(None, 0).unwrap())?;

    // 视频类型：H.264
    let vt = MFCreateMediaType()?;
    vt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    vt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    let _ = vt.SetUINT32(&MF_MT_INTERLACE_MODE, 2); // MF_INTERLACE_MODE_PROGRESSIVE
    // 帧率 30/1
    let _ = vt.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1);

    sink.AddSink(0, &vt, None)?;
    println!("  H.264 sink type 已接受");

    // 也试一下 RGB32 输入类型（录制时喂这个）
    let vt2 = MFCreateMediaType()?;
    vt2.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    vt2.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
    sink.AddSink(0, &vt2, None)?;
    println!("  RGB32 sink type 已接受");

    let _ = std::fs::remove_file(&path);
    Ok("MFCreateSinkWriterFromURL + H.264 媒体类型 均成功".to_string())
}

// 让未使用告警闭嘴
#[allow(dead_code)]
fn _unused() {
    let _ = CoCreateInstance::<_, IMFMediaType, _>;
}
