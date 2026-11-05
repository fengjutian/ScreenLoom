//! 极简 WAV 写入器：48 kHz / 立体声 / 16-bit PCM。
//!
//! 录制时把系统声音先落成 WAV，结束后再由 [`super::mux`] 后置混流成 AAC/MP4。
//! WAV 头在收尾时回填，异常中断时文件头里的长度可能为 0，但音频数据仍在。

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use screenloom_core::{Error, Result};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
pub const BITS: u16 = 16;

/// WAV 写入器。
pub struct WavWriter {
    file: BufWriter<File>,
    path: PathBuf,
    data_bytes: u64,
    finished: bool,
}

impl WavWriter {
    pub fn create(path: &Path) -> Result<WavWriter> {
        if let Some(p) = path.parent() {
            if !p.as_os_str().is_empty() {
                std::fs::create_dir_all(p)?;
            }
        }
        let mut file = BufWriter::new(File::create(path)?);
        // 先写占位头，收尾时回填长度。
        write_header(&mut file, 0)?;
        Ok(WavWriter {
            file,
            path: path.to_path_buf(),
            data_bytes: 0,
            finished: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&mut self, pcm: &[i16]) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.data_bytes += bytes.len() as u64;
        Ok(())
    }

    pub fn has_data(&self) -> bool {
        self.data_bytes > 0
    }

    /// 回填 RIFF/data 长度并落盘。
    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.file.flush()?;
        let data = self.data_bytes.min(u32::MAX as u64) as u32;
        let riff = 36u32.saturating_add(data);
        self.file.seek(SeekFrom::Start(4))?;
        self.file.write_all(&riff.to_le_bytes())?;
        self.file.seek(SeekFrom::Start(40))?;
        self.file.write_all(&data.to_le_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

impl Drop for WavWriter {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn write_header(w: &mut impl Write, data_bytes: u32) -> Result<()> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36u32.saturating_add(data_bytes)).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&CHANNELS.to_le_bytes());
    h.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    h.extend_from_slice(&(SAMPLE_RATE * CHANNELS as u32 * (BITS as u32 / 8)).to_le_bytes());
    h.extend_from_slice(&(CHANNELS * (BITS / 8)).to_le_bytes()); // block align
    h.extend_from_slice(&BITS.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_bytes.to_le_bytes());
    w.write_all(&h).map_err(|e| Error::Io(e))?;
    Ok(())
}

/// 读取本模块写出的 WAV（只支持 48k/立体声/16-bit）。
pub fn read_pcm(path: &Path) -> Result<Vec<i16>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Audio("不是有效的 WAV 文件".into()));
    }
    let mut pos = 12usize;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        if id == b"data" {
            data = Some(&bytes[body..end]);
            break;
        }
        pos = body + len + (len & 1);
    }
    let data = data.ok_or_else(|| Error::Audio("WAV 缺少 data 块".into()))?;
    Ok(data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_readable_pcm() {
        let dir = std::env::temp_dir().join("screenloom-wav-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        let mut w = WavWriter::create(&p).unwrap();
        w.write(&[1, -2, 3, -4]).unwrap();
        w.finish().unwrap();
        let pcm = read_pcm(&p).unwrap();
        assert_eq!(pcm, vec![1, -2, 3, -4]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
