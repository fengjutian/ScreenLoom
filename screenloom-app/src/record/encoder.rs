//! Media Foundation H.264 编码器（`IMFSinkWriter` 封装）。
//!
//! # 为什么用 MF 而不是 ffmpeg / x264
//!
//! - **零新增依赖**：复用项目已有的 `windows` crate，不引入 C 编译链或外部二进制。
//! - 本机 `MFTEnumEx` 枚举到 **11 个硬件编码器**（NVENC 等）+ 7 个软件编码器。
//!
//! # 已踩过的坑（全部实测过，别改回去）
//!
//! | 坑 | 后果 | 正确做法 |
//! |---|---|---|
//! | 输入用 BGRA/RGB32 | `SetInputMediaType` **成功**但编码器一帧都不吐，`Finalize` 报 `0xC00D4A44` | 输入必须是 **NV12**（由 `screenloom_core::bgra_to_nv12` 转） |
//! | 不设 `MF_MT_AVG_BITRATE` | 小分辨率下 MF 自算的默认码率低于编码器下限 → 编码器完全不出帧 | **必须设**，且要放在**输出**媒体类型上 |
//! | `MF_MT_VIDEO_NOMINAL_RANGE` 填 0(Unknown) 或 1(Limited) | 整条链断（`0xC00D4A44` / "Invalid average bitrate"） | 必须填 **2 (Full 0-255)** |
//! | 颜色属性放**输出**媒体类型 | `SetInputMediaType` 报 "Invalid average bitrate" | 颜色属性放**输入**媒体类型 |
//! | `MFCreateSample()` 后取 `ConvertToContiguousBuffer()` | `0x8000FFFF` | 用 `MFCreateMemoryBuffer` + `AddBuffer` |
//! | buffer 写完不 `SetCurrentLength` | sample 长度为 0，编码器报参数错误 | **必须** `SetCurrentLength(len)` |
//! | `MFCreateSinkWriterFromURL` 传 `file://` URL | `0x8007007B` | 传**完整文件路径** |
//! | 末尾固定丢 ~3 帧 | 尾部 ~100 ms 内容缺失 | 收尾补写几帧填充（见 [`VideoEncoder::finish`]） |
//!
//! 完整证据见 `doc/录屏方案.md` §4。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFShutdown, MFStartup,
    MF_VERSION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX,
    MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_NV12, MFMediaType_Video,
    MFVideoPrimaries_BT709, MFVideoTransFunc_709,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
};

use screenloom_core::{Error, Result};

/// `MFVideoColorSpace_BT709` —— windows-rs 0.61 **没有导出**这个枚举
/// （`MFVideoPrimaries` / `MFVideoTransferFunction` 有，唯独缺 ColorSpace），
/// 按 mfapi.h 的原始取值补常量。
const MF_VIDEO_COLOR_SPACE_BT709: u32 = 1;

/// `MFVideoNormativeRange_Full`（Y 0~255）。本机编码器**只接受** full range。
const MF_VIDEO_NORMATIVE_RANGE_FULL: u32 = 2;

/// H.264 视频编码器。
pub struct VideoEncoder {
    sink: IMFSinkWriter,
    stream_index: u32,
    w: u32,
    h: u32,
    frame_bytes: usize,
    /// 一帧的时长（100 ns 单位）
    duration_100ns: i64,
    written: u64,
    finished: bool,
    /// COM 套间守卫：必须比 `sink` 活得更久（字段顺序 = drop 顺序，放最后）。
    _apartment: ComApartment,
}

impl VideoEncoder {
    /// 新建编码器，输出 H.264 / MP4。
    ///
    /// - `path`：输出文件路径（**必须是完整路径**，MF 不接受 `file://` URL）
    /// - `w`, `h`：**偶数**尺寸（NV12 色度 2×2 降采样）
    /// - `fps`：帧率（写入媒体类型；实际写入节奏由调用方按墙钟控制）
    /// - `bitrate`：目标码率（bps）。**必须给**，否则小分辨率下编码器可能一帧都不吐。
    pub fn new(path: &Path, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<VideoEncoder> {
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
            return Err(Error::Image(format!(
                "编码尺寸非法（必须为正偶数）: {w}x{h}"
            )));
        }
        if fps == 0 {
            return Err(Error::Image("帧率不能为 0".into()));
        }
        if bitrate == 0 {
            return Err(Error::Image("码率不能为 0".into()));
        }

        // COM 套间 + MF 生命周期：guard 必须活到 sink 写完。
        let _apartment = ComApartment::enter();
        let w_url = wide(path.as_os_str());

        // 整段 MF 调用都是 unsafe：输入/输出媒体类型与 sink 的初始化必须原子地
        // 成组，任何一步失败都不能留下半初始化的编码器。
        let (sink, stream_index) = unsafe {
            MFStartup(MF_VERSION, 0).map_err(|e| {
                Error::Image(format!("MFStartup 失败（系统缺少 Media Foundation）: {e}"))
            })?;
        let mut attrs: Option<IMFAttributes> = None;
        // SAFETY: 输出参数合法。
        unsafe {
            MFCreateAttributes(&mut attrs, 0).map_err(mf_err)?;
        }
        let attrs = attrs.ok_or_else(|| Error::Image("MFCreateAttributes 没返回属性集".into()))?;
        // 显式声明 MP4 容器（靠扩展名推断会出问题）
        attrs
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .map_err(mf_err)?;
        // 关闭节流：写入节奏由调用方按墙钟控制（否则帧率会随机器负载漂移）
        attrs
            .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
            .map_err(mf_err)?;
        // 允许硬件变换（走 GPU 编码路径）
        attrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(mf_err)?;

        // SAFETY: url 是 NUL 结尾的 UTF-16，attrs 有效。
        let sink: IMFSinkWriter = unsafe { MFCreateSinkWriterFromURL(PCWSTR(w_url.as_ptr()), None, &attrs) }
            .map_err(|e| Error::Image(format!("创建 MP4 写入器失败: {e}")))?;

        // ---- 输入媒体类型：未压缩 NV12 ----
        let in_type = MFCreateMediaType().map_err(mf_err)?;
        in_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(mf_err)?;
        in_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(mf_err)?;
        in_type
            .SetUINT64(&MF_MT_FRAME_RATE, (fps as u64) << 32 | 1)
            .map_err(mf_err)?;
        in_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2).map_err(mf_err)?; // progressive
        in_type
            .SetUINT64(&MF_MT_FRAME_SIZE, (w as u64) << 32 | (h as u64))
            .map_err(mf_err)?;
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, w).map_err(mf_err)?;
        in_type
            .SetUINT32(&MF_MT_SAMPLE_SIZE, (w * h * 3 / 2) as u32)
            .map_err(mf_err)?;
        in_type
            .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
            .map_err(mf_err)?;
        // 颜色：full range + BT.709。**range 必须是 2**，否则整条链断。
        in_type
            .SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MF_VIDEO_NORMATIVE_RANGE_FULL)
            .map_err(mf_err)?;
        in_type
            .SetUINT32(&MF_MT_YUV_MATRIX, MF_VIDEO_COLOR_SPACE_BT709)
            .map_err(mf_err)?;
        in_type
            .SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)
            .map_err(mf_err)?;
        in_type
            .SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)
            .map_err(mf_err)?;

        // ---- 输出媒体类型：H.264 ----
        let out_type = MFCreateMediaType().map_err(mf_err)?;
        out_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(mf_err)?;
        out_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(mf_err)?;
        out_type
            .SetUINT64(&MF_MT_FRAME_RATE, (fps as u64) << 32 | 1)
            .map_err(mf_err)?;
        out_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2).map_err(mf_err)?;
        out_type
            .SetUINT64(&MF_MT_FRAME_SIZE, (w as u64) << 32 | (h as u64))
            .map_err(mf_err)?;
        out_type
            .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
            .map_err(mf_err)?;
        // 码率放**输出**媒体类型上才生效（放输入上会被忽略）
        out_type
            .SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
            .map_err(mf_err)?;

        let stream_index = sink
            .AddStream(&out_type)
            .map_err(|e| Error::Image(format!("AddStream(H.264) 失败: {e}")))?;
        sink.SetInputMediaType(stream_index, &in_type, None)
            .map_err(|e| Error::Image(format!("SetInputMediaType(NV12) 失败: {e}")))?;
            sink.BeginWriting()
                .map_err(|e| Error::Image(format!("BeginWriting 失败: {e}")))?;

            (sink, stream_index)
        };

        Ok(VideoEncoder {
            sink,
            stream_index,
            w,
            h,
            frame_bytes: (w * h * 3 / 2) as usize,
            duration_100ns: 10_000_000 / fps as i64,
            written: 0,
            finished: false,
            _apartment,
        })
    }

    /// 帧尺寸。
    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// 已写入的帧数。
    pub fn frames_written(&self) -> u64 {
        self.written
    }

    /// 写一帧 NV12。
    ///
    /// - `nv12`：长度必须是 `w * h * 3 / 2`
    /// - `pts_100ns`：该帧的显示时间戳（100 ns 单位，**合成时间戳**，见 `doc/录屏方案.md` §5.3）
    pub fn write_frame(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<()> {
        if self.finished {
            return Err(Error::Image("编码器已收尾，不能再写帧".into()));
        }
        if nv12.len() < self.frame_bytes {
            return Err(Error::Image(format!(
                "NV12 帧过短: 需要 {} 字节，收到 {}",
                self.frame_bytes,
                nv12.len()
            )));
        }

        // SAFETY: 以下都是对有效 COM 对象的合法调用。
        unsafe {
            let sample: IMFSample = MFCreateSample().map_err(mf_err)?;
            let buffer = MFCreateMemoryBuffer(self.frame_bytes as u32).map_err(mf_err)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len: u32 = 0;
            buffer
                .Lock(&mut ptr, Some(&mut len), None)
                .map_err(|e| Error::Image(format!("锁定样本缓冲失败: {e}")))?;
            if (len as usize) < self.frame_bytes {
                let _ = buffer.Unlock();
                return Err(Error::Image(format!(
                    "样本缓冲过小: 需要 {} 字节，拿到 {len}",
                    self.frame_bytes
                )));
            }
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, self.frame_bytes);
            let _ = buffer.Unlock();
            // **必须**设置有效数据长度，否则 sample 长度为 0，编码器报参数错误。
            buffer
                .SetCurrentLength(self.frame_bytes as u32)
                .map_err(mf_err)?;

            sample.AddBuffer(&buffer).map_err(mf_err)?;
            sample.SetSampleTime(pts_100ns).map_err(mf_err)?;
            sample
                .SetSampleDuration(self.duration_100ns)
                .map_err(mf_err)?;

            self.sink
                .WriteSample(self.stream_index, &sample)
                .map_err(|e| Error::Image(format!("写入第 {} 帧失败: {e}", self.written)))?;
        }
        self.written += 1;
        Ok(())
    }

    /// 收尾：`Flush` → `Finalize`。
    ///
    /// `Finalize` 才会写 `moov` 头 —— **不调用它，产出的 MP4 不可播**。
    ///
    /// **调用前应先补写尾部填充帧**：编码器流水线的最后几帧不会被 `Flush` 冲出来
    /// （实测固定丢 ~3 帧 ≈ 100 ms）。补帧由上层做（它手里有最后一帧的缓存）。
    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;

        // SAFETY: 同 write_frame。
        unsafe {
            self.sink
                .Flush(self.stream_index)
                .map_err(|e| Error::Image(format!("Flush 失败: {e}")))?;
            self.sink
                .Finalize()
                .map_err(|e| Error::Image(format!("Finalize 失败: {e}")))?;
        }
        Ok(())
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        if !self.finished {
            // 正常路径应该已经 finish() 过；这里兜底，避免文件缺 moov。
            let _ = self.finish();
        }
        // SAFETY: 与 new() 里的 MFStartup 配对。
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// 路径 → NUL 结尾的 UTF-16（MF 要宽字符路径）。
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn mf_err(e: windows::core::Error) -> Error {
    Error::Image(format!("Media Foundation 调用失败: {e}"))
}

/// COM 套间守卫（进入 MTA，离开配对反初始化）。
struct ComApartment {
    com: bool,
}

impl ComApartment {
    fn enter() -> Self {
        const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;
        let mut com = false;
        // SAFETY: 与 Drop 配对。
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.0 != RPC_E_CHANGED_MODE {
                // S_OK 与 S_FALSE 都要配对一次 CoUninitialize。
                com = true;
            }
        }
        Self { com }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: 与 enter() 配对。
        unsafe {
            if self.com {
                CoUninitialize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一帧 NV12（中灰 + 中性色度）。
    fn gray_nv12(w: u32, h: u32) -> Vec<u8> {
        vec![128u8; (w * h * 3 / 2) as usize]
    }

    #[test]
    fn writes_a_playable_mp4() {
        let dir = std::env::temp_dir().join("screenloom-encoder-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("out.mp4");
        let _ = std::fs::remove_file(&path);

        let (w, h) = (64u32, 64u32);
        let mut enc = VideoEncoder::new(&path, w, h, 30, 2_000_000).unwrap();
        let frame = gray_nv12(w, h);
        for i in 0..30i64 {
            enc.write_frame(&frame, i * 333_333).unwrap();
        }
        assert_eq!(enc.frames_written(), 30);
        enc.finish().unwrap();

        // 产物必须包含 ftyp / moov / mdat —— 缺 moov 的 MP4 是不可播的。
        let bytes = std::fs::read(&path).unwrap();
        let has = |tag: &[u8; 4]| bytes.windows(4).any(|w| w == tag);
        assert!(has(b"ftyp"), "缺 ftyp");
        assert!(has(b"moov"), "缺 moov（说明没 Finalize）");
        assert!(has(b"mdat"), "缺 mdat");
        assert!(bytes.len() > 1000, "产物过小: {} 字节", bytes.len());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_invalid_parameters() {
        let dir = std::env::temp_dir().join("screenloom-encoder-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.mp4");
        // 奇数宽
        assert!(VideoEncoder::new(&path, 63, 64, 30, 1_000_000).is_err());
        // 零帧率
        assert!(VideoEncoder::new(&path, 64, 64, 0, 1_000_000).is_err());
        // 零码率
        assert!(VideoEncoder::new(&path, 64, 64, 30, 0).is_err());
    }

    #[test]
    fn rejects_short_frames() {
        let dir = std::env::temp_dir().join("screenloom-encoder-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("short.mp4");
        let _ = std::fs::remove_file(&path);
        let mut enc = VideoEncoder::new(&path, 64, 64, 30, 1_000_000).unwrap();
        assert!(enc.write_frame(&[0u8; 10], 0).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
