//! 录制状态与统计：worker 写、HUD/托盘读。
//!
//! 全部用原子量，避免为了刷新一次计时器去加锁。

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// 录制内部阶段。
///
/// 用途有两个：
/// 1. **诊断**：MF 内部偶发阻塞时，靠它定位卡在哪一步（取帧 / 写帧 / Flush / Finalize）。
/// 2. **看门狗**：配合 [`SharedStats::stalled_ms`] 判断录制线程是否还活着。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    Idle = 0,
    /// 取帧 / BGRA→NV12 转换。
    Capture = 1,
    /// `IMFSinkWriter::WriteSample`。
    Write = 2,
    /// 收尾补帧。
    Tail = 3,
    /// `IMFSinkWriter::Flush`。
    Flush = 4,
    /// `IMFSinkWriter::Finalize`（写 moov）。
    Finalize = 5,
}

impl Stage {
    pub fn from_u8(v: u8) -> Stage {
        match v {
            1 => Stage::Capture,
            2 => Stage::Write,
            3 => Stage::Tail,
            4 => Stage::Flush,
            5 => Stage::Finalize,
            _ => Stage::Idle,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Stage::Idle => "空闲",
            Stage::Capture => "取帧",
            Stage::Write => "写帧",
            Stage::Tail => "收尾补帧",
            Stage::Flush => "刷新编码器",
            Stage::Finalize => "写文件尾",
        }
    }
}

/// 录制阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Idle = 0,
    Countdown = 1,
    Recording = 2,
    Paused = 3,
    Stopping = 4,
    Finished = 5,
    Failed = 6,
}
impl Phase {
    pub fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Countdown,
            2 => Phase::Recording,
            3 => Phase::Paused,
            4 => Phase::Stopping,
            5 => Phase::Finished,
            6 => Phase::Failed,
            _ => Phase::Idle,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Phase::Countdown | Phase::Recording | Phase::Paused)
    }
}

/// HUD / 托盘共享的录制统计。
#[derive(Debug)]
pub struct SharedStats {
    phase: AtomicU8,
    elapsed_ms: AtomicU64,
    frames: AtomicU64,
    dropped: AtomicU64,
    bytes: AtomicU64,
    /// 当前内部阶段。
    stage: AtomicU8,
    /// 最近一次活动的墙钟时刻（毫秒）。
    heartbeat: AtomicU64,
}

impl Default for SharedStats {
    fn default() -> Self {
        SharedStats {
            phase: AtomicU8::new(Phase::Idle as u8),
            elapsed_ms: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            stage: AtomicU8::new(Stage::Idle as u8),
            heartbeat: AtomicU64::new(0),
        }
    }
}

impl SharedStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Relaxed))
    }

    pub fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms.load(Ordering::Relaxed)
    }

    pub fn set_elapsed_ms(&self, ms: u64) {
        self.elapsed_ms.store(ms, Ordering::Relaxed);
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn set_frames(&self, frames: u64) {
        self.frames.store(frames, Ordering::Relaxed);
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn inc_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn set_bytes(&self, bytes: u64) {
        self.bytes.store(bytes, Ordering::Relaxed);
    }

    /// `HH:MM:SS`。
    pub fn elapsed_text(&self) -> String {
        let total = self.elapsed_ms() / 1000;
        format!(
            "{:02}:{:02}:{:02}",
            total / 3600,
            (total % 3600) / 60,
            total % 60
        )
    }

    /// 当前内部阶段。
    pub fn stage(&self) -> Stage {
        Stage::from_u8(self.stage.load(Ordering::Relaxed))
    }

    pub fn set_stage(&self, stage: Stage) {
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    /// 刷新心跳：录制线程每完成一个可推进的步骤就调一次。
    pub fn touch(&self) {
        self.heartbeat
            .store(now_ms(), Ordering::Relaxed);
    }

    /// 心跳停滞了多久（毫秒）。从未活动过则返回 0。
    pub fn stalled_ms(&self) -> u64 {
        let hb = self.heartbeat.load(Ordering::Relaxed);
        if hb == 0 {
            return 0;
        }
        now_ms().saturating_sub(hb)
    }

    /// 心跳停滞是否已超过阈值（看门狗判定卡死）。
    pub fn is_stalled(&self, timeout_ms: u64) -> bool {
        self.stalled_ms() >= timeout_ms
    }

    /// 仅测试用：把心跳强行回拨，模拟"停滞了 `ms` 毫秒"。
    #[cfg(test)]
    pub fn force_stall(&self, ms: u64) {
        self.heartbeat
            .store(now_ms().saturating_sub(ms), Ordering::Relaxed);
    }
}

/// 毫秒级墙钟。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_tracks_activity() {
        let s = SharedStats::new();
        // 从未活动过不算停滞（否则刚启动录制就会被看门狗误杀）。
        assert_eq!(s.stalled_ms(), 0);
        s.touch();
        assert!(s.stalled_ms() < 1_000, "刚刷新过心跳");
    }

    #[test]
    fn stage_roundtrips() {
        let s = SharedStats::new();
        assert_eq!(s.stage(), Stage::Idle);
        for st in [
            Stage::Capture,
            Stage::Write,
            Stage::Tail,
            Stage::Flush,
            Stage::Finalize,
        ] {
            s.set_stage(st);
            assert_eq!(s.stage(), st);
            assert!(!st.text().is_empty());
        }
    }

    #[test]
    fn stall_is_detected_only_after_threshold() {
        let s = SharedStats::new();
        s.touch();
        assert!(!s.is_stalled(1_000), "刚刷新过心跳不应判为停滞");
        // 模拟心跳停滞 5 秒。
        s.force_stall(5_000);
        assert!(s.is_stalled(1_000));
        assert!(!s.is_stalled(10_000), "阈值更大时不应判为停滞");
    }
}
