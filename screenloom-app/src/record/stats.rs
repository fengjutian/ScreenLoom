//! 录制状态与统计：worker 写、HUD/托盘读。
//!
//! 全部用原子量，避免为了刷新一次计时器去加锁。

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

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
}

impl Default for SharedStats {
    fn default() -> Self {
        SharedStats {
            phase: AtomicU8::new(Phase::Idle as u8),
            elapsed_ms: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
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
}
