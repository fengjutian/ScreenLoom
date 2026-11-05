//! 录屏编码子进程（库形态）。
//!
//! 这个 crate 有两种用法，**跑的是同一套代码**：
//!
//! - 独立 exe：`screenloom-encoder-host.exe`（调试 / 手工验证用）；
//! - 被 `screenloom.exe --encoder-host` 复用 —— 父进程拉起**自己**当编码子进程。
//!
//! 第二种是产品实际走的路径：`screenloom.exe` 旁边不一定会多放一个 exe，
//! 而"拉起自己"在任何分发形态（绿色包 / 安装包 / 便携版）下都成立，
//! 不会因为漏带 host.exe 就退化成"卡死拿不到文件"。
//!
//! 设计见 `doc/编码进程隔离方案.md`。

pub mod drive;
pub mod encoder;
pub mod host;
pub mod shm;

/// 以「编码子进程」身份运行。
///
/// 从 stdin 读 JSON lines 控制消息，把 NV12 帧写进命名共享内存，
/// 由子进程内的 Media Foundation `SinkWriter` 编码成 `FMPEG4` 分片 MP4。
///
/// 父进程 stdin 关闭（EOF）时直接退出：文件停在最后一个分片，仍然可播。
pub fn run_host() -> Result<(), String> {
    host::run()
}
