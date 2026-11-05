//! 后置混流：视频-only MP4 + WAV → 音视频 MP4。
//!
//! 录制阶段只写视频，避免实时 SinkWriter 音视频交织导致丢帧；
//! 停止后用 SourceReader 读回压缩 H.264 样本，再和 WAV 的 PCM 一起
//! 写进新的 SinkWriter。视频样本直接透传，不重新解码/编码。

use std::path::Path;

use screenloom_core::{Error, Result};
use windows::core::PCWSTR;

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType,
    MFCreateFile, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFCreateSourceReaderFromByteStream,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video, MFTranscodeContainerType_MPEG4,
    MFAudioFormat_AAC, MFAudioFormat_PCM, MF_ACCESSMODE_READ, MF_FILEFLAGS_NONE,
    MF_OPENMODE_FAIL_IF_NOT_EXIST, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_TRANSCODE_CONTAINERTYPE,
};

use super::wav;

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// 把 `video`（视频-only）+ `wav` 混成 `out`。
pub fn mux(video: &Path, wav_path: &Path, out: &Path) -> Result<()> {
    // 调用方释放 VideoEncoder 后，worker 线程的 COM 套间可能已经反初始化；
    // SourceReader/SinkWriter 都必须在本线程有 MTA。
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let _com = ComGuard(com);

    let pcm = wav::read_pcm(wav_path)?;
    if pcm.is_empty() {
        return Err(Error::Audio("WAV 没有音频数据".into()));
    }
    let w_video = wide(video);
    let w_out = wide(out);

    // SAFETY: 路径/属性都是有效指针；MF 已在编码器里 Startup。
    unsafe {
        let byte_stream = MFCreateFile(
            MF_ACCESSMODE_READ,
            MF_OPENMODE_FAIL_IF_NOT_EXIST,
            MF_FILEFLAGS_NONE,
            PCWSTR(w_video.as_ptr()),
        )
        .map_err(|e| Error::Audio(format!("打开录制视频失败: {e}")))?;
        let reader = MFCreateSourceReaderFromByteStream(&byte_stream, None)
            .map_err(|e| Error::Audio(format!("创建 SourceReader 失败: {e}")))?;
        let vstream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(|e| Error::Audio(format!("关闭源流失败: {e}")))?;
        reader
            .SetStreamSelection(vstream, true)
            .map_err(|e| Error::Audio(format!("选择视频流失败: {e}")))?;
        let vtype = reader
            .GetNativeMediaType(vstream, 0)
            .map_err(|e| Error::Audio(format!("读取视频类型失败: {e}")))?;
        reader
            .SetCurrentMediaType(vstream, None, &vtype)
            .map_err(|e| Error::Audio(format!("设置压缩视频类型失败: {e}")))?;

        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 0).map_err(mf_err)?;
        let attrs = attrs.ok_or_else(|| Error::Audio("MFCreateAttributes 失败".into()))?;
        attrs
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .map_err(mf_err)?;
        // 离线 mux：不按真实时间等待。若出现丢帧可临时注释掉对照。
        attrs
            .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
            .map_err(mf_err)?;
        let sink: IMFSinkWriter =
            MFCreateSinkWriterFromURL(PCWSTR(w_out.as_ptr()), None, &attrs)
                .map_err(|e| Error::Audio(format!("创建输出 MP4 失败: {e}")))?;

        let vidx = sink
            .AddStream(&vtype)
            .map_err(|e| Error::Audio(format!("添加视频流失败: {e}")))?;
        sink.SetInputMediaType(vidx, &vtype, None)
            .map_err(|e| Error::Audio(format!("设置视频输入失败: {e}")))?;

        let a_out = MFCreateMediaType().map_err(mf_err)?;
        a_out
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(mf_err)?;
        a_out
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(mf_err)?;
        a_out
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, wav::CHANNELS as u32)
            .map_err(mf_err)?;
        a_out
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, wav::SAMPLE_RATE)
            .map_err(mf_err)?;
        a_out
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, wav::BITS as u32)
            .map_err(mf_err)?;
        a_out
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)
            .map_err(mf_err)?;
        let aidx = sink
            .AddStream(&a_out)
            .map_err(|e| Error::Audio(format!("添加音频流失败: {e}")))?;

        let a_in = MFCreateMediaType().map_err(mf_err)?;
        a_in
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(mf_err)?;
        a_in
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(mf_err)?;
        a_in
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, wav::CHANNELS as u32)
            .map_err(mf_err)?;
        a_in
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, wav::SAMPLE_RATE)
            .map_err(mf_err)?;
        a_in
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, wav::BITS as u32)
            .map_err(mf_err)?;
        a_in
            .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)
            .map_err(mf_err)?;
        a_in
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000)
            .map_err(mf_err)?;
        sink.SetInputMediaType(aidx, &a_in, None)
            .map_err(|e| Error::Audio(format!("设置音频输入失败: {e}")))?;
        sink.BeginWriting()
            .map_err(|e| Error::Audio(format!("BeginWriting 失败: {e}")))?;

        let mut audio_iter = pcm.chunks(2048).peekable();
        let mut audio_pts = 0i64;
        let mut video_count = 0u32;
        let mut last_vpts = 0i64;

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
                .map_err(|e| Error::Audio(format!("读取视频样本失败: {e}")))?;
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                break;
            }
            let Some(sample) = sample else {
                continue;
            };
            let vpts = sample.GetSampleTime().unwrap_or(0);
            last_vpts = vpts;
            video_count += 1;
            // 先写视频；音频在视频全部写完后补（离线 mux，不怕队列）。
            let _ = vpts;
            sink.WriteSample(vidx, &sample)
                .map_err(|e| Error::Audio(format!("写入视频样本失败: {e}")))?;
        }
        while let Some(chunk) = audio_iter.next() {
            write_audio(&sink, aidx, chunk, audio_pts)?;
            audio_pts += (chunk.len() as i64 / 2) * 10_000_000 / wav::SAMPLE_RATE as i64;
        }

        sink.Flush(vidx).map_err(mf_err)?;
        sink.Flush(aidx).map_err(mf_err)?;
        sink.Finalize().map_err(mf_err)?;
        eprintln!(
            "[mux] video={video_count} last_vpts={last_vpts} audio_pts={audio_pts} pcm_samples={}",
            pcm.len()
        );
    }
    Ok(())
}

unsafe fn write_audio(
    sink: &IMFSinkWriter,
    stream: u32,
    pcm: &[i16],
    pts: i64,
) -> Result<()> {
    let frames = pcm.len() / 2;
    if frames == 0 {
        return Ok(());
    }
    let bytes = frames * 4;
    let sample: IMFSample = MFCreateSample().map_err(mf_err)?;
    let buffer = MFCreateMemoryBuffer(bytes as u32).map_err(mf_err)?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len: u32 = 0;
    buffer
        .Lock(&mut ptr, Some(&mut len), None)
        .map_err(mf_err)?;
    if (len as usize) < bytes {
        let _ = buffer.Unlock();
        return Err(Error::Audio("音频缓冲过小".into()));
    }
    std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, ptr, bytes);
    let _ = buffer.Unlock();
    buffer.SetCurrentLength(bytes as u32).map_err(mf_err)?;
    sample.AddBuffer(&buffer).map_err(mf_err)?;
    sample.SetSampleTime(pts).map_err(mf_err)?;
    sample
        .SetSampleDuration(frames as i64 * 10_000_000 / wav::SAMPLE_RATE as i64)
        .map_err(mf_err)?;
    sink.WriteSample(stream, &sample)
        .map_err(|e| Error::Audio(format!("写入音频样本失败: {e}")))?;
    Ok(())
}

fn mf_err(e: windows::core::Error) -> Error {
    Error::Audio(format!("Media Foundation 调用失败: {e}"))
}

struct ComGuard(bool);

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
