//! 子进程内的 Media Foundation H.264 编码器（`FMPEG4` 分片容器）。
//!
//! 与 `screenloom-app` 里的编码器保持同样的媒体类型约束（NV12 输入、full range、
//! BT.709、码率放输出类型），但不含音轨——音频仍由父进程后置混流。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video, MFStartup,
    MFTranscodeContainerType_FMPEG4, MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoPrimaries_BT709,
    MFVideoTransFunc_709, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE,
    MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES,
    MF_MT_YUV_MATRIX, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_TRANSCODE_CONTAINERTYPE,
    MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// `MFVideoColorSpace_BT709`（windows-rs 0.61 未导出）。
const COLOR_SPACE_BT709: u32 = 1;
/// `MFVideoNormativeRange_Full`。
const RANGE_FULL: u32 = 2;

pub struct SinkEncoder {
    sink: IMFSinkWriter,
    stream: u32,
    frame_bytes: usize,
    duration_100ns: i64,
    frames: u64,
    finished: bool,
    /// COM 套间守卫必须比 sink 活得久（字段顺序 = drop 顺序）。
    _apartment: Apartment,
}

impl SinkEncoder {
    pub fn new(path: &Path, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<SinkEncoder, String> {
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
            return Err(format!("尺寸非法（需正偶数）: {w}x{h}"));
        }
        if fps == 0 || bitrate == 0 {
            return Err("帧率/码率不能为 0".into());
        }
        let apartment = Apartment::enter();
        static MF_INIT: std::sync::Once = std::sync::Once::new();
        MF_INIT.call_once(|| unsafe {
            let _ = MFStartup(MF_VERSION, 0);
        });

        let url = wide(path);
        let frame_bytes = (w * h * 3 / 2) as usize;

        // SAFETY: 媒体类型/属性指针都在本次调用期间有效。
        let (sink, stream) = unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 0).map_err(|e| format!("MFCreateAttributes: {e}"))?;
            let attrs = attrs.ok_or("MFCreateAttributes 返回空")?;
            attrs
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_FMPEG4)
                .map_err(|e| format!("SetGUID(CONTAINERTYPE): {e}"))?;
            attrs
                .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| format!("SetUINT32(HW_TRANSFORMS): {e}"))?;

            let sink: IMFSinkWriter =
                MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs)
                    .map_err(|e| format!("创建 SinkWriter: {e}"))?;

            let out_type = MFCreateMediaType().map_err(|e| format!("out type: {e}"))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| e.to_string())?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|e| e.to_string())?;
            out_type
                .SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)
                .map_err(|e| e.to_string())?;
            out_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, 2)
                .map_err(|e| e.to_string())?;
            out_type
                .SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)
                .map_err(|e| e.to_string())?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
                .map_err(|e| e.to_string())?;
            out_type
                .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
                .map_err(|e| e.to_string())?;

            let stream = sink
                .AddStream(&out_type)
                .map_err(|e| format!("AddStream: {e}"))?;

            let in_type = MFCreateMediaType().map_err(|e| format!("in type: {e}"))?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| e.to_string())?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, 2)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, w)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_SAMPLE_SIZE, frame_bytes as u32)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, RANGE_FULL)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_YUV_MATRIX, COLOR_SPACE_BT709)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)
                .map_err(|e| e.to_string())?;
            in_type
                .SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)
                .map_err(|e| e.to_string())?;

            sink.SetInputMediaType(stream, &in_type, None)
                .map_err(|e| format!("SetInputMediaType: {e}"))?;
            sink.BeginWriting()
                .map_err(|e| format!("BeginWriting: {e}"))?;
            (sink, stream)
        };

        Ok(SinkEncoder {
            sink,
            stream,
            frame_bytes,
            duration_100ns: 10_000_000 / fps as i64,
            frames: 0,
            finished: false,
            _apartment: apartment,
        })
    }

    pub fn write(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<(), String> {
        if self.finished {
            return Err("编码器已收尾".into());
        }
        if nv12.len() < self.frame_bytes {
            return Err(format!("帧过短: {} < {}", nv12.len(), self.frame_bytes));
        }
        // SAFETY: 有效 COM 对象 + 已校验长度。
        unsafe {
            let sample: IMFSample = MFCreateSample().map_err(|e| e.to_string())?;
            let buffer = MFCreateMemoryBuffer(self.frame_bytes as u32).map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len: u32 = 0;
            buffer
                .Lock(&mut ptr, Some(&mut len), None)
                .map_err(|e| e.to_string())?;
            if (len as usize) < self.frame_bytes {
                let _ = buffer.Unlock();
                return Err("样本缓冲过小".into());
            }
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, self.frame_bytes);
            let _ = buffer.Unlock();
            buffer
                .SetCurrentLength(self.frame_bytes as u32)
                .map_err(|e| e.to_string())?;
            sample.AddBuffer(&buffer).map_err(|e| e.to_string())?;
            sample
                .SetSampleTime(pts_100ns)
                .map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration(self.duration_100ns)
                .map_err(|e| e.to_string())?;
            self.sink
                .WriteSample(self.stream, &sample)
                .map_err(|e| format!("WriteSample #{}: {e}", self.frames))?;
        }
        self.frames += 1;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        // SAFETY: 有效 COM 对象。
        unsafe {
            self.sink
                .Flush(self.stream)
                .map_err(|e| format!("Flush: {e}"))?;
        }
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        // SAFETY: 有效 COM 对象。
        unsafe {
            self.sink
                .Finalize()
                .map_err(|e| format!("Finalize: {e}"))?;
        }
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

struct Apartment(bool);

impl Apartment {
    fn enter() -> Self {
        const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;
        // SAFETY: 与 Drop 配对。
        let com = unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            hr.0 != RPC_E_CHANGED_MODE
        };
        Apartment(com)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: 与 enter() 配对。
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
