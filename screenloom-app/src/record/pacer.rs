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
    /// **墙钟基准**：录制真正开始的那一刻。
    ///
    /// burst guard 触发时，用它把合成时间轴拉回真实时间（见 [`FramePacer::tick`]）。
    origin: Instant,
    /// 上一次 tick 的墙钟时刻。暂停恢复时用它算出暂停时长。
    last_now: Instant,
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
            origin: start,
            last_now: start,
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
        self.last_now = now;
        // burst guard：跳过补写（不要一口气 burst），但**必须**把合成时间轴
        // 平移回当前墙钟 —— 否则丢掉的时间会永久缺失，成片比真实短、播放加速。
        //
        // 实测（debug 构建 1080p）：只能跑 13.7fps 时，4.1 秒的真实录制
        // 只产出 1.73 秒的成片（快进 2.4 倍）。平移后成片时长回到真实值。
        if self.next_write < now {
            self.next_write = now + self.interval;
            let wall = wall_100ns(now, self.origin);
            // 容差 2 帧：轻微落后不触发平移，正常跟得上时保持严格 CFR。
            if self.pts + 2 * self.duration_100ns < wall {
                self.pts = wall;
            }
        }
        Some(pts)
    }

    /// 暂停恢复：墙钟节奏重新对齐，但 pts 继续往前走（暂停时间被剔除）。
    pub fn rebase(&mut self, now: Instant) {
        // 暂停期间墙钟在走，但成片时间轴不该走：把墙钟基准同样推后，
        // 否则恢复瞬间会被误判成"落后很久"，把暂停时间算进成片。
        let paused = now.saturating_duration_since(self.last_now);
        self.origin += paused;
        self.next_write = now;
        self.last_now = now;
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

/// 墙钟经过时间（100 ns 单位）。
fn wall_100ns(now: Instant, origin: Instant) -> i64 {
    now.saturating_duration_since(origin).as_nanos() as i64 / 100
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

    /// **关键回归**：性能不足（写帧跟不上目标帧率）时，成片时长必须等于真实时长。
    ///
    /// 失败症状：pts 只按固定步长递增，丢掉的时间永久缺失 → 成片快进。
    #[test]
    fn slow_writer_does_not_lose_elapsed_time() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        // 模拟 13.7fps 的写帧节奏（每 73ms 才写一帧，目标却是 30fps）。
        let step = Duration::from_millis(73);
        let mut wall = Duration::ZERO;
        let mut last_pts = 0i64;
        let mut written = 0u64;
        for _ in 0..50 {
            wall += step;
            if let Some(pts) = p.tick(t0 + wall) {
                last_pts = pts;
                written += 1;
            }
        }
        assert_eq!(written, 50, "每一步都应该写一帧");
        // 真实经过 50 × 73ms = 3.65s，成片时长必须覆盖这段时间。
        let real_100ns = wall.as_nanos() as i64 / 100;
        let clip_100ns = last_pts + p.duration_100ns();
        assert!(
            clip_100ns * 100 >= real_100ns * 95,
            "丢掉的时间没被补回：成片时长={}（{} ms），真实经过={}（{} ms）",
            clip_100ns,
            clip_100ns / 10_000,
            real_100ns,
            real_100ns / 10_000
        );
    }

    /// 跟得上目标帧率时，必须保持严格 CFR（帧间距均匀），不能被平移破坏。
    #[test]
    fn keeps_strict_cfr_when_keeping_up() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, t0);
        let mut prev: Option<i64> = None;
        for i in 0..60 {
            let now = t0 + Duration::from_millis(i * 33);
            if let Some(pts) = p.tick(now) {
                if let Some(prev) = prev {
                    assert_eq!(
                        pts - prev,
                        333_333,
                        "跟得上时帧间距必须恒定（pts 平移被误触发）"
                    );
                }
                prev = Some(pts);
            }
        }
    }
}
