//! 编码子进程的控制协议（v1）。
//!
//! 父进程（app）通过 stdin/stdout 给子进程发 JSON lines 控制消息；帧数据放在
//! 命名共享内存里，消息只带槽位与时间戳。设计见 `doc/编码进程隔离方案.md`。
//!
//! 这个 crate **不依赖 windows**，可以在任意平台单测协议编解码。

use std::fmt;

use serde_json::{json, Value};

/// 协议版本。不匹配时子进程应回 `error` 并退出。
pub const VERSION: u32 = 1;

/// 建议的共享内存槽数（父进程可覆盖，但 demo/默认用 3）。
pub const DEFAULT_SLOTS: u32 = 3;

/// 编码阶段（用于心跳上报与卡死定位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Write,
    Flush,
    Finalize,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Write => "write",
            Stage::Flush => "flush",
            Stage::Finalize => "finalize",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        match s {
            "write" => Some(Stage::Write),
            "flush" => Some(Stage::Flush),
            "finalize" => Some(Stage::Finalize),
            _ => None,
        }
    }
}

/// 一条控制消息。
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// 父 → 子：初始化。`path` 必须是绝对路径；`shm` 是共享内存名。
    Init {
        w: u32,
        h: u32,
        fps: u32,
        bitrate: u32,
        path: String,
        shm: String,
        slots: u32,
        frame_bytes: u32,
    },
    /// 子 → 父：初始化完成。
    Ready { frame_bytes: u32 },
    /// 父 → 子：槽位 `slot` 里有一帧，时间戳 `pts`，长度 `size`。
    Frame {
        seq: u64,
        slot: u32,
        pts: i64,
        size: u32,
    },
    /// 子 → 父：槽位可复用。
    Ack { seq: u64 },
    /// 子 → 父：周期性统计。
    Stats {
        frames: u64,
        encode_ms: u32,
        stage: Stage,
    },
    /// 父 → 子：收尾（Flush → Finalize）。
    Stop,
    /// 子 → 父：收尾完成。
    Stopped { frames: u64, bytes: u64 },
    Ping,
    Pong { stage: Stage },
    /// 父 → 子：放弃这次录制（不 Finalize，直接退出）。
    Abort,
    Error { code: String, msg: String },
}

/// 协议错误（消息内容问题，不代表进程失败）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

/// 一帧占用的字节数（NV12）。
pub fn nv12_frame_bytes(w: u32, h: u32) -> u32 {
    w * h * 3 / 2
}

/// 共享内存总大小。
pub fn shm_bytes(slots: u32, frame_bytes: u32) -> u64 {
    slots as u64 * frame_bytes as u64
}

impl Message {
    /// 序列化为一行 JSON（**不含换行**）。
    pub fn encode(&self) -> String {
        let v = match self {
            Message::Init {
                w,
                h,
                fps,
                bitrate,
                path,
                shm,
                slots,
                frame_bytes,
            } => json!({
                "v": VERSION, "type": "init",
                "w": w, "h": h, "fps": fps, "bitrate": bitrate,
                "path": path, "shm": shm, "slots": slots, "frame_bytes": frame_bytes,
            }),
            Message::Ready { frame_bytes } => json!({
                "v": VERSION, "type": "ready", "frame_bytes": frame_bytes,
            }),
            Message::Frame {
                seq,
                slot,
                pts,
                size,
            } => json!({
                "v": VERSION, "type": "frame",
                "seq": seq, "slot": slot, "pts": pts, "size": size,
            }),
            Message::Ack { seq } => json!({
                "v": VERSION, "type": "ack", "seq": seq,
            }),
            Message::Stats {
                frames,
                encode_ms,
                stage,
            } => json!({
                "v": VERSION, "type": "stats",
                "frames": frames, "encode_ms": encode_ms, "stage": stage.as_str(),
            }),
            Message::Stop => json!({ "v": VERSION, "type": "stop" }),
            Message::Stopped { frames, bytes } => json!({
                "v": VERSION, "type": "stopped", "frames": frames, "bytes": bytes,
            }),
            Message::Ping => json!({ "v": VERSION, "type": "ping" }),
            Message::Pong { stage } => json!({
                "v": VERSION, "type": "pong", "stage": stage.as_str(),
            }),
            Message::Abort => json!({ "v": VERSION, "type": "abort" }),
            Message::Error { code, msg } => json!({
                "v": VERSION, "type": "error", "code": code, "msg": msg,
            }),
        };
        v.to_string()
    }

    /// 从一行 JSON 解析。版本不匹配会报错。
    pub fn decode(line: &str) -> Result<Message, ProtocolError> {
        let v: Value = serde_json::from_str(line)
            .map_err(|e| ProtocolError(format!("JSON 解析失败: {e}")))?;
        let ver = get_u64(&v, "v")? as u32;
        if ver != VERSION {
            return Err(ProtocolError(format!(
                "协议版本不匹配: 期望 {VERSION}，收到 {ver}"
            )));
        }
        let kind = get_str(&v, "type")?;
        Ok(match kind {
            "init" => Message::Init {
                w: get_u32(&v, "w")?,
                h: get_u32(&v, "h")?,
                fps: get_u32(&v, "fps")?,
                bitrate: get_u32(&v, "bitrate")?,
                path: get_str(&v, "path")?.to_string(),
                shm: get_str(&v, "shm")?.to_string(),
                slots: get_u32(&v, "slots")?,
                frame_bytes: get_u32(&v, "frame_bytes")?,
            },
            "ready" => Message::Ready {
                frame_bytes: get_u32(&v, "frame_bytes")?,
            },
            "frame" => Message::Frame {
                seq: get_u64(&v, "seq")?,
                slot: get_u32(&v, "slot")?,
                pts: get_i64(&v, "pts")?,
                size: get_u32(&v, "size")?,
            },
            "ack" => Message::Ack {
                seq: get_u64(&v, "seq")?,
            },
            "stats" => Message::Stats {
                frames: get_u64(&v, "frames")?,
                encode_ms: get_u32(&v, "encode_ms")?,
                stage: Stage::parse(get_str(&v, "stage")?)
                    .ok_or_else(|| ProtocolError("未知 stage".into()))?,
            },
            "stop" => Message::Stop,
            "stopped" => Message::Stopped {
                frames: get_u64(&v, "frames")?,
                bytes: get_u64(&v, "bytes")?,
            },
            "ping" => Message::Ping,
            "pong" => Message::Pong {
                stage: Stage::parse(get_str(&v, "stage")?)
                    .ok_or_else(|| ProtocolError("未知 stage".into()))?,
            },
            "abort" => Message::Abort,
            "error" => Message::Error {
                code: get_str(&v, "code")?.to_string(),
                msg: get_str(&v, "msg")?.to_string(),
            },
            other => return Err(ProtocolError(format!("未知消息类型: {other}"))),
        })
    }
}

fn get_u64(v: &Value, key: &str) -> Result<u64, ProtocolError> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtocolError(format!("缺字段或类型错误: {key}")))
}

fn get_u32(v: &Value, key: &str) -> Result<u32, ProtocolError> {
    let n = get_u64(v, key)?;
    u32::try_from(n).map_err(|_| ProtocolError(format!("字段越界: {key}={n}")))
}

fn get_i64(v: &Value, key: &str) -> Result<i64, ProtocolError> {
    v.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| ProtocolError(format!("缺字段或类型错误: {key}")))
}

fn get_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, ProtocolError> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError(format!("缺字段或类型错误: {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: Message) {
        let line = m.encode();
        assert!(!line.contains('\n'), "必须是一行");
        let back = Message::decode(&line).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn roundtrips_all_messages() {
        roundtrip(Message::Init {
            w: 1920,
            h: 1080,
            fps: 30,
            bitrate: 8_000_000,
            path: r"C:\videos\a.partial.mp4".into(),
            shm: r"Local\ScreenloomEnc-1-ab".into(),
            slots: DEFAULT_SLOTS,
            frame_bytes: nv12_frame_bytes(1920, 1080),
        });
        roundtrip(Message::Ready { frame_bytes: 123 });
        roundtrip(Message::Frame {
            seq: 7,
            slot: 1,
            pts: 233_333,
            size: 123,
        });
        roundtrip(Message::Ack { seq: 7 });
        roundtrip(Message::Stats {
            frames: 90,
            encode_ms: 6,
            stage: Stage::Write,
        });
        roundtrip(Message::Stop);
        roundtrip(Message::Stopped {
            frames: 90,
            bytes: 1024,
        });
        roundtrip(Message::Ping);
        roundtrip(Message::Pong {
            stage: Stage::Finalize,
        });
        roundtrip(Message::Error {
            code: "init".into(),
            msg: "失败".into(),
        });
    }

    #[test]
    fn rejects_version_mismatch() {
        let err = Message::decode(r#"{"v":2,"type":"stop"}"#).unwrap_err();
        assert!(err.0.contains("版本"));
    }

    #[test]
    fn rejects_unknown_type() {
        assert!(Message::decode(r#"{"v":1,"type":"nope"}"#).is_err());
    }

    #[test]
    fn rejects_missing_field() {
        let err = Message::decode(r#"{"v":1,"type":"frame","seq":1}"#).unwrap_err();
        assert!(err.0.contains("slot") || err.0.contains("字段"));
    }

    #[test]
    fn stage_roundtrip() {
        for s in [Stage::Write, Stage::Flush, Stage::Finalize] {
            assert_eq!(Stage::parse(s.as_str()), Some(s));
        }
        assert_eq!(Stage::parse("bogus"), None);
    }

    #[test]
    fn shm_size_is_slots_times_frame() {
        assert_eq!(shm_bytes(3, 100), 300);
        assert_eq!(nv12_frame_bytes(320, 240), 115_200);
    }
}
