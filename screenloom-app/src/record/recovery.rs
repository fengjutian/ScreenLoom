//! 异常中断录制的恢复：把残留的 `.partial.mp4` 转成可播放文件。
//!
//! 录制用 `FMPEG4` 分片容器，因此即使没来得及 `Finalize`，partial 本身也能解码
//! （见 `doc/录屏方案.md` §4.8）。这里优先把它重封装成普通 MP4（兼容性更好），
//! 重封装失败则退回改名 `.recovered.mp4`，保证素材不丢。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::paths;

static RECOVER_LOCK: Mutex<()> = Mutex::new(());

/// 扫描并恢复目录下所有 `.partial.mp4`，返回恢复后的文件路径。
///
/// 串行执行：并发调用不会重复处理同一个文件。
pub fn recover_dir(dir: &Path) -> Vec<PathBuf> {
    let _guard = RECOVER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !paths::is_partial(&p) {
            continue;
        }
        // 空文件（初始化就失败）没有恢复价值，直接清掉。
        if e.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let stem = name.trim_end_matches(".partial.mp4").to_string();

        // 优先重封装成普通 MP4：文件名就是原本打算交付的正式名。
        let dst = paths::unique_path(&p.with_file_name(format!("{stem}.mp4")));
        match super::mux::remux_video(&p, &dst) {
            Ok(()) => {
                let _ = std::fs::remove_file(&p);
                out.push(dst);
            }
            Err(err) => {
                eprintln!("恢复 {} 失败: {err}（保留分片文件）", p.display());
                let fallback =
                    paths::unique_path(&p.with_file_name(format!("{stem}.recovered.mp4")));
                if std::fs::rename(&p, &fallback).is_ok() {
                    out.push(fallback);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("screenloom-recovery-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_partials_are_removed() {
        let dir = temp("empty");
        let p = dir.join("clip.partial.mp4");
        std::fs::write(&p, b"").unwrap();
        assert!(recover_dir(&dir).is_empty());
        assert!(!p.exists(), "空临时文件应被删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_partials_fall_back_to_renamed_file() {
        let _g = crate::record::test_support::mf_guard();
        let dir = temp("garbage");
        let p = dir.join("clip.partial.mp4");
        std::fs::write(&p, b"not a mp4").unwrap();
        let out = recover_dir(&dir);
        assert_eq!(out.len(), 1, "重封装失败也应退回改名，不能丢文件");
        assert!(out[0].to_string_lossy().ends_with("clip.recovered.mp4"));
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
