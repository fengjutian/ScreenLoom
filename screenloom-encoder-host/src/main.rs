//! 编码子进程：把 Media Foundation 的 `SinkWriter` 关在独立进程里。
//!
//! - 正常模式（父进程拉起）：从 stdin 读控制消息，写 `FMPEG4` 分片 MP4；
//! - `--drive <out> <frames>`：自测驱动，自己拉起自己，跑完 90 帧的最小闭环。
//!
//! 设计见 `doc/编码进程隔离方案.md`。

mod drive;
mod encoder;
mod host;
mod shm;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--drive") => {
            let out = args.get(1).map(String::as_str).unwrap_or("out.mp4");
            let frames = args
                .get(2)
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(90);
            drive::run(out, frames)
        }
        _ => host::run(),
    };
    if let Err(e) = result {
        eprintln!("encoder-host: {e}");
        std::process::exit(1);
    }
}
