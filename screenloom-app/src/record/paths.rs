//! 录制文件路径：临时文件、唯一文件名、收尾原子改名。
//!
//! 录制期间写 `<name>.partial.mp4`（`FMPEG4` 分片容器，崩溃后仍可播）；
//! 收尾时由 [`super::mux::remux_video`] 重封装成正式 `<name>.mp4`，
//! 失败才退回改名，避免留下一个“看起来成功、实际不可播”的文件。

use std::path::{Path, PathBuf};

use screenloom_core::{Error, Result};

/// 一次录制的输出路径。
#[derive(Clone, Debug)]
pub struct RecordPaths {
    /// 录制期间写入的临时文件。
    pub partial: PathBuf,
    /// 收尾成功后的正式文件。
    pub final_path: PathBuf,
}

impl RecordPaths {
    /// 根据用户给的路径创建一对路径。
    ///
    /// 正式文件已存在时自动加 `_2`、`_3` 后缀，绝不覆盖。
    pub fn new(out_path: PathBuf) -> Result<RecordPaths> {
        check_out_ext(&out_path)?;
        let final_path = unique_path(&out_path);
        let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
        let stem = final_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "recording".to_string());
        let partial = parent.join(format!("{stem}.partial.mp4"));
        Ok(RecordPaths {
            partial,
            final_path,
        })
    }

    /// 创建父目录。
    pub fn prepare(&self) -> Result<()> {
        if let Some(parent) = self.partial.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(())
    }

    /// 收尾成功：临时文件 → 正式文件。
    pub fn finalize(&self) -> Result<()> {
        std::fs::rename(&self.partial, &self.final_path)?;
        Ok(())
    }

    /// 放弃录制：删掉临时文件。
    pub fn discard(&self) {
        let _ = std::fs::remove_file(&self.partial);
    }
}

/// 校验扩展名。
pub fn check_out_ext(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "mp4" {
        return Err(Error::Image(format!(
            "录制输出应为 .mp4（当前 .{}）",
            if ext.is_empty() { "无扩展名" } else { &ext }
        )));
    }
    Ok(())
}

/// 扫描并恢复上次异常退出留下的 `.partial.mp4`。
///
/// 恢复逻辑见 [`super::recovery::recover_dir`]（优先重封装成普通 MP4）。
/// 这里只保留路径工具。
pub fn is_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.ends_with(".partial.mp4"))
}

/// 同名文件存在时追加 `_2`、`_3`…
pub(crate) fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "recording".to_string());
    for n in 2..10_000u32 {
        let candidate = parent.join(format!("{stem}_{n}.mp4"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端情况下退回时间戳，保证不会覆盖。
    parent.join(format!("{stem}_{}.mp4", crate::save::timestamp()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_mp4() {
        assert!(check_out_ext(Path::new("a.mp4")).is_ok());
        assert!(check_out_ext(Path::new("a.MP4")).is_ok());
        assert!(check_out_ext(Path::new("a.avi")).is_err());
        assert!(check_out_ext(Path::new("a")).is_err());
    }

    #[test]
    fn partial_is_hidden_behind_final_name() {
        let dir = std::env::temp_dir().join("screenloom-record-paths-test");
        let _ = std::fs::create_dir_all(&dir);
        let final_path = dir.join("clip.mp4");
        let _ = std::fs::remove_file(&final_path);
        let p = RecordPaths::new(final_path.clone()).unwrap();
        assert_eq!(p.final_path, final_path);
        assert!(p.partial.to_string_lossy().contains("clip.partial.mp4"));
        std::fs::write(&p.partial, b"x").unwrap();
        p.finalize().unwrap();
        assert!(final_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_file_gets_suffix() {
        let dir = std::env::temp_dir().join("screenloom-record-unique-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("clip.mp4");
        std::fs::write(&path, b"x").unwrap();
        let p = RecordPaths::new(path).unwrap();
        assert!(p.final_path.to_string_lossy().ends_with("clip_2.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_name_is_recognized() {
        assert!(is_partial(Path::new("clip.partial.mp4")));
        assert!(!is_partial(Path::new("clip.mp4")));
    }
}
