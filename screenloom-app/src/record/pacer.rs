//! 定速写帧时间轴：CFR 合成时间戳 + 卡顿重同步 + 暂停恢复。

use std::time::{Duration, Instant};

/// 定速写帧的时间轴。
#[derive(Clone, Debug)]
pub struct FramePacer {
    interval: Duration,
    duration_100ns: i64,
    next_write: Instant,
    pts: i64,
    frames: u64,
}

impl FramePacer {
    pub fn new(fps: u32, start: Instant) -> FramePacer {
        let fps = fps.max(1);
        FramePacer {
            interval: Duration::from_nanos(1_000_000_000 / fps as u64),
            duration_100ns: 10_000_000 / fps as i64,
            next_write: start,
            pts: 0,
            frames: 0,
        }
    }

    /// 到点则推进时间轴并返回该帧的 pts（100 ns）。
    pub fn tick(&mut self, now: Instant) -> Option<i64> {
        if now < self.next_write {
            return None;
        }
        let pts = self.pts;
        self.pts += self.duration_100ns;
        self.frames += 1;
        self.next_write += self.interval;
        // 落后太多：重新对齐，不要一口气补写。
        if self.next_write < now {
            self.next_write = now + self.interval;
        }
        Some(pts)
    }

    /// 暂停恢复：墙钟节奏重新对齐，但 pts 继续往前走（暂停时间被剔除）。
    pub fn rebase(&mut self, now: Instant) {
        self.next_write = now;
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn pts(&self) -> i64 {
        self.pts
    }

    pub fn duration_100ns(&self) -> i64 {
        self.duration_100ns
    }

    pub fn next_write_after(&self, now: Instant) -> Duration {
        self.next_write.saturating_duration_since(now)
    }

    pub fn advance(&mut self) {
        self.pts += self.duration_100ns;
        self.frames += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_synthetic_cfr_timestamps() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        assert_eq!(p.tick(t0), Some(0));
        assert_eq!(p.tick(t0 + Duration::from_millis(10)), None);
        assert_eq!(p.tick(t0 + Duration::from_millis(34)), Some(333_333));
        assert_eq!(p.tick(t0 + Duration::from_millis(67)), Some(666_666));
        assert_eq!(p.frames(), 3);
    }

    #[test]
    fn does_not_burst_after_a_stall() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        assert_eq!(p.tick(t0), Some(0));
        let now = t0 + Duration::from_secs(5);
        let mut written = 1;
        while p.tick(now).is_some() {
            written += 1;
            assert!(written < 10, "卡顿后不应补写大量帧");
        }
        assert_eq!(written, 2);
    }

    #[test]
    fn rebase_does_not_rewind_pts() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        assert_eq!(p.tick(t0), Some(0));
        p.advance();
        let pts = p.pts();
        p.rebase(t0 + Duration::from_secs(5));
        assert_eq!(p.pts(), pts);
    }
}
