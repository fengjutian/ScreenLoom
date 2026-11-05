//! 正常模式：读父进程的控制消息，写帧、应答心跳、收尾。

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Instant;

use screenloom_encoder_protocol::{Message, Stage};

use crate::encoder::SinkEncoder;
use crate::shm::Shm;

pub fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();

    let first = lines
        .next()
        .ok_or("缺少 init 消息")?
        .map_err(|e| e.to_string())?;
    let init = Message::decode(&first).map_err(|e| e.to_string())?;
    let Message::Init {
        w,
        h,
        fps,
        bitrate,
        path,
        shm,
        slots,
        frame_bytes,
    } = init
    else {
        return Err("首条消息必须是 init".into());
    };

    let map = Shm::open(&shm, slots, frame_bytes as usize)?;
    let mut enc = SinkEncoder::new(Path::new(&path), w, h, fps, bitrate)?;
    send(&Message::Ready { frame_bytes })?;

    let mut stage = Stage::Write;
    for line in lines {
        let line = line.map_err(|e| e.to_string())?;
        let msg = match Message::decode(&line) {
            Ok(m) => m,
            Err(e) => {
                let _ = send(&Message::Error {
                    code: "protocol".into(),
                    msg: e.to_string(),
                });
                return Ok(());
            }
        };

        match msg {
            Message::Frame {
                seq,
                slot,
                pts,
                size,
            } => {
                hang_if("write", seq);
                let data = map.slot(slot, size as usize)?;
                let t0 = Instant::now();
                if let Err(e) = enc.write(data, pts) {
                    let _ = send(&Message::Error {
                        code: "write".into(),
                        msg: e,
                    });
                    return Ok(());
                }
                let encode_ms = t0.elapsed().as_millis() as u32;
                if enc.frames() % 30 == 0 {
                    let _ = send(&Message::Stats {
                        frames: enc.frames(),
                        encode_ms,
                        stage,
                    });
                }
                let _ = send(&Message::Ack { seq });
            }
            Message::Ping => {
                let _ = send(&Message::Pong { stage });
            }
            Message::Stop => {
                stage = Stage::Flush;
                if let Err(e) = enc.flush() {
                    let _ = send(&Message::Error {
                        code: "flush".into(),
                        msg: e,
                    });
                    return Ok(());
                }
                stage = Stage::Finalize;
                hang_if("finalize", 0);
                if let Err(e) = enc.finalize() {
                    let _ = send(&Message::Error {
                        code: "finalize".into(),
                        msg: e,
                    });
                    return Ok(());
                }
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let _ = send(&Message::Stopped {
                    frames: enc.frames(),
                    bytes,
                });
                return Ok(());
            }
            Message::Abort => return Ok(()),
            _ => {}
        }
    }

    // stdin EOF：父进程已退出，子进程跟着走（文件停在最后一个分片，仍可播）。
    Ok(())
}

fn send(m: &Message) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", m.encode()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

/// 测试用：`SCREENLOOM_HOST_HANG=write|finalize` 模拟卡死。
///
/// `SCREENLOOM_HOST_HANG_AFTER=N` 让它先写 N 帧再卡死 —— 真实故障通常是
/// "录到一半才卡"，那种情况下文件里已经有内容，才是真正要测的救回场景。
fn hang_if(where_: &str, seq: u64) {
    if std::env::var("SCREENLOOM_HOST_HANG").ok().as_deref() != Some(where_) {
        return;
    }
    let after = std::env::var("SCREENLOOM_HOST_HANG_AFTER")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if seq < after {
        return;
    }
    eprintln!("encoder-host: 模拟卡死在 {where_}（第 {seq} 帧）");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
