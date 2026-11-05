//! 探针公共工具。
//!
//! 所有探针产物统一落到 `<crate 根>/out/`，由 `.gitignore` 忽略，
//! 不再往仓库根目录写 `_p*.mp4` 这类中间产物。

use std::path::{Path, PathBuf};

/// 探针产物输出目录：默认 `<crate 根>/out/`，不存在则创建。
///
/// 可用环境变量 `MF_PROBE_OUT` 覆盖——沙箱/CI 下仓库目录可能不可写，
/// 这时把产物导到临时目录再校验。
pub fn out_dir() -> PathBuf {
    let dir = match std::env::var_os("MF_PROBE_OUT") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("out"),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 产物完整路径（`name` 相对 `out/`）。
pub fn out_path(name: &str) -> PathBuf {
    out_dir().join(name)
}
