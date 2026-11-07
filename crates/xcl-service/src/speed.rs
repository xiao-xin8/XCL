//! 速度采样：滑动窗口 + 指数平滑。
//!
//! 为什么不能直接用「两次采样的差值 ÷ 间隔」：
//! 界面刷新间隔只有 150ms，而一条 TCP 连接的数据是按块到达的
//! （reqwest 一次给 8~16KB，块之间可能有几十毫秒空档）。
//! 150ms 的窗口经常整段没有新字节，算出来就是 0 ——
//! 于是速度在「800 KB/s → 0 → 1.2 MB/s」之间疯狂跳，ETA 也跟着变成空值。
//! 看起来稳，是因为它算的是一段时间内的平均，不是瞬时值。
//!
//! 这里用两层过滤：
//! 1. 滑动窗口取 4 秒的平均，天然把「块到达的抖动」摊平；
//! 2. 再叠一层 EMA（α = 0.3），去掉窗口边缘进出造成的残余跳动。
//!
//! 窗口是滑动的，所以真的断流时速度会平滑衰减到 0，而不是突然归零。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 默认统计窗口。4 秒足够跨过好几次「块空档」，又不会让速度对真实变化太迟钝。
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(4);

/// 指数平滑系数。越小越稳、越迟钝。0.3 在 150ms 采样下约 1 秒收敛。
const ALPHA: f64 = 0.3;

/// 多久没有新字节就认定「断流」，速度开始衰减。
///
/// 小于这个时长的空拍属于正常的块间空档，不能算断流 ——
/// 这就是速度突然掉 0 的直接原因。
const STALL_GRACE: Duration = Duration::from_millis(1200);

/// 低于这个速度直接归零。1KB/s 以下的下载没有意义，
/// 留着反而让界面在个位数上跳。
const MIN_MEANINGFUL: f64 = 1024.0;

pub struct SpeedMeter {
    window: Duration,
    /// `(采样时刻, 累计字节)`，按时间单调递增
    samples: VecDeque<(Instant, u64)>,
    smoothed: f64,
    last_bytes: u64,
    /// 上次真正收到新字节的时刻，用来判断断流
    last_progress_at: Instant,
    started: bool,
}

impl SpeedMeter {
    pub fn new(window: Duration) -> Self {
        let now = Instant::now();
        Self {
            window,
            samples: VecDeque::new(),
            smoothed: 0.0,
            last_bytes: 0,
            last_progress_at: now,
            started: false,
        }
    }

    /// 喂一个新采样，返回平滑后的速度（字节/秒）。
    pub fn update(&mut self, done_bytes: u64, now: Instant) -> u64 {
        // 字节被撤回（分片降级重下、失败重试撤进度）：历史样本作废，
        // 否则窗口里会出现「先大后小」，算出来是负数或离谱的低值。
        if self.started && done_bytes < self.last_bytes {
            self.samples.clear();
            self.smoothed = 0.0;
        }
        if done_bytes > self.last_bytes {
            self.last_progress_at = now;
        }
        self.started = true;
        self.last_bytes = done_bytes;
        self.samples.push_back((now, done_bytes));

        // 只保留窗口内的样本，但留一个窗口之前的锚点，
        // 否则样本刚过窗口边界时窗口会瞬间缩短，速度又会跳一下。
        let cutoff = now.checked_sub(self.window).unwrap_or(now);
        while self.samples.len() > 2 && self.samples[1].0 <= cutoff {
            self.samples.pop_front();
        }

        // 断流判定：超过宽限期没有新字节，瞬时速度直接算 0，
        // 让 EMA 平滑地衰减下去，而不是继续拿窗口里的旧数据撑着一个假速度。
        let stalled = now.duration_since(self.last_progress_at) >= STALL_GRACE;

        let Some(&(base_at, base_bytes)) = self.samples.front() else {
            return 0;
        };
        let span = now.duration_since(base_at).as_secs_f64();
        // span 太小时样本还没意义（比如刚启动的第一拍），沿用上次的平滑值
        let instant = if stalled {
            0.0
        } else if span >= 0.15 && done_bytes >= base_bytes {
            (done_bytes - base_bytes) as f64 / span
        } else {
            self.smoothed
        };

        self.smoothed = if self.smoothed <= 0.0 {
            instant
        } else {
            self.smoothed * (1.0 - ALPHA) + instant * ALPHA
        };

        if self.smoothed < MIN_MEANINGFUL {
            0
        } else {
            self.smoothed.round() as u64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(meter: &mut SpeedMeter, t0: Instant, ticks: usize, bytes_per_tick: u64) {
        for index in 1..=ticks {
            let at = t0 + Duration::from_millis(150 * index as u64);
            let done = bytes_per_tick * index as u64;
            meter.update(done, at);
        }
    }

    #[test]
    fn steady_stream_reports_expected_rate() {
        // 每 150ms 收 100KB → 666KB/s
        let mut meter = SpeedMeter::new(DEFAULT_WINDOW);
        let t0 = Instant::now();
        feed(&mut meter, t0, 40, 100 * 1024);

        let at = t0 + Duration::from_millis(150 * 40);
        let done = 100 * 1024 * 40;
        let speed = meter.update(done, at);
        let expected = 100.0 * 1024.0 / 0.15;
        let drift = (speed as f64 - expected).abs() / expected;
        assert!(drift < 0.15, "稳态速度 {speed} 偏离期望 {expected:.0} 超过 15%");
    }

    #[test]
    fn empty_ticks_do_not_crash_speed_to_zero() {
        // 这就是看到的「老是掉到 0」：数据按块到达，中间有空拍。
        // 4 秒窗口下，连续 3 拍没数据不该让速度归零。
        let mut meter = SpeedMeter::new(DEFAULT_WINDOW);
        let t0 = Instant::now();
        feed(&mut meter, t0, 40, 100 * 1024);

        let mut at = t0 + Duration::from_millis(150 * 40);
        let mut done = 100 * 1024 * 40;
        let before = meter.update(done, at);

        for _ in 0..3 {
            at += Duration::from_millis(150);
            let speed = meter.update(done, at);
            assert!(
                speed > before / 2,
                "空拍 3 次后速度从 {before} 掉到 {speed}，太陡了"
            );
        }
        done += 0;
        let _ = done;
    }

    #[test]
    fn stall_decays_gradually_then_reaches_zero() {
        let mut meter = SpeedMeter::new(DEFAULT_WINDOW);
        let t0 = Instant::now();
        feed(&mut meter, t0, 40, 100 * 1024);

        let mut at = t0 + Duration::from_millis(150 * 40);
        let done = 100 * 1024 * 40;
        let mut previous = meter.update(done, at);
        assert!(previous > 0);

        // 断流 6 秒：必须单调下降，最后归零，中途不能突然跳到 0
        for _ in 0..40 {
            at += Duration::from_millis(150);
            let speed = meter.update(done, at);
            assert!(speed <= previous + 1, "速度应当单调下降，{previous} → {speed}");
            previous = speed;
        }
        assert_eq!(previous, 0, "长时间断流后应当归零");
    }

    #[test]
    fn rollback_resets_instead_of_underflowing() {
        // 分片降级时 done_bytes 会被撤回去，不能算出负速度或天文数字
        let mut meter = SpeedMeter::new(DEFAULT_WINDOW);
        let t0 = Instant::now();
        feed(&mut meter, t0, 40, 100 * 1024);

        let speed = meter.update(1024, t0 + Duration::from_secs(7));
        assert!(speed < 10 * 1024 * 1024, "回退后速度异常：{speed}");
        assert_eq!(meter.update(1024, t0 + Duration::from_secs(7)), 0);
    }

    #[test]
    fn first_sample_is_zero() {
        let mut meter = SpeedMeter::new(DEFAULT_WINDOW);
        assert_eq!(meter.update(0, Instant::now()), 0);
    }
}
