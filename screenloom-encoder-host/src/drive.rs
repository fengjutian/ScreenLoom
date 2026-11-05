//! 自测驱动：自己拉起自己（`--host`），投 90 帧，验证最小闭环。
//!
//! 这不是产品代码，只用于在没有 app 的情况下验证协议 + 共享内存 + 子进程编码。

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use screenloom_encoder_protocol::{nv12_frame_bytes, Message, DEFAULT_SLOTS};

use crate::shm::Shm;

pub fn run(out: &str, frames: u32) -> Result<(), String> {
    let (w, h, fps, bitrate) = (320u32, 240u32, 30u32, 4_000_000u32);
    let frame_bytes = nv12_frame_bytes(w, h);
    let slots = DEFAULT_SLOTS;

    let name = format!(r"Local\ScreenloomEnc-drive-{}", std::process::id());
    let map = Shm::create(&name, slots, frame_bytes as usize)?;

    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let path = cwd.join(out);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&path);

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg("--host")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("拉起子进程失败: {e}"))?;
    let mut cin = child.stdin.take().ok_or("子进程 stdin 不可用")?;
    let mut cout = BufReader::new(child.stdout.take().ok_or("子进程 stdout 不可用")?);

    send(
        &mut cin,
        &Message::Init {
            w,
            h,
            fps,
            bitrate,
            path: path.to_string_lossy().into_owned(),
            shm: name,
            slots,
            frame_bytes,
        },
    )?;

    match read_until(&mut cout, |m| {
        matches!(m, Message::Ready { .. } | Message::Error { .. })
    })? {
        Message::Ready { .. } => {}
        Message::Error { code, msg } => return Err(format!("初始化失败 [{code}]: {msg}")),
        _ => unreachable!(),
    }

    let frame = vec![128u8; frame_bytes as usize];
    for i in 0..frames {
        let slot = i % slots;
        map.write_slot(slot, &frame);
        send(
            &mut cin,
            &Message::Frame {
                seq: i as u64,
                slot,
                pts: i as i64 * 10_000_000 / fps as i64,
                size: frame_bytes,
            },
        )?;
        let m = read_until(&mut cout, |m| match m {
            Message::Ack { seq } => *seq == i as u64,
            Message::Error { .. } => true,
            _ => false,
        })?;
        if let Message::Error { code, msg } = m {
            return Err(format!("写第 {i} 帧失败 [{code}]: {msg}"));
        }
    }

    send(&mut cin, &Message::Stop)?;
    let stopped = read_until(&mut cout, |m| {
        matches!(m, Message::Stopped { .. } | Message::Error { .. })
    })?;
    drop(cin);
    let _ = child.wait();

    match stopped {
        Message::Stopped { frames, bytes } => {
            println!(
                "drive ok: {frames} 帧 / {bytes} 字节 → {}",
                path.display()
            );
            Ok(())
        }
        Message::Error { code, msg } => Err(format!("子进程错误 [{code}]: {msg}")),
        _ => Ok(()),
    }
}

fn send(w: &mut ChildStdin, m: &Message) -> Result<(), String> {
    writeln!(w, "{}", m.encode()).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

fn read_until<F>(r: &mut BufReader<ChildStdout>, pred: F) -> Result<Message, String>
where
    F: Fn(&Message) -> bool,
{
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("子进程提前关闭 stdout".into());
        }
        let m = Message::decode(line.trim_end()).map_err(|e| e.to_string())?;
        if pred(&m) {
            return Ok(m);
        }
    }
}
