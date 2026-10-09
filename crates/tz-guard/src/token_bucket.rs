//! 滑动窗口令牌桶：`window_secs` 秒内最多 `burst` 个事件。
//! 以微令牌（1 token = 1e6 micro）连续补充，避免 32/600s 这类小速率在毫秒粒度下被截断为 0。

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

const MICRO_PER_TOKEN: u64 = 1_000_000;

#[derive(Debug)]
struct State {
    burst: u32,
    window_secs: u32,
    last_ms: u64,
    tokens_micro: u64,
}

#[derive(Debug)]
pub struct TokenBucket {
    state: Mutex<State>,
    started: Instant,
}

impl TokenBucket {
    /// `burst` 为 0 表示不限。
    pub fn new(burst: u32, window_secs: u32) -> Self {
        let started = Instant::now();
        let tokens_micro = u64::from(burst) * MICRO_PER_TOKEN;
        Self {
            state: Mutex::new(State {
                burst,
                window_secs: window_secs.max(1),
                last_ms: 0,
                tokens_micro,
            }),
            started,
        }
    }

    /// 热更新窗口/容量：剩余令牌按比例换算，避免瞬间清零或爆满。
    pub fn reconfigure(&self, burst: u32, window_secs: u32) {
        let mut state = self.state.lock().expect("token bucket");
        let old_burst = u64::from(state.burst.max(1));
        let new_burst = u64::from(burst.max(1));
        // 剩余比例换算到新容量，向上取整避免极小速率下被清零。
        let scaled =
            (state.tokens_micro / old_burst) * new_burst + (state.tokens_micro % old_burst) * new_burst / old_burst;
        state.burst = burst;
        state.window_secs = window_secs.max(1);
        state.tokens_micro = scaled.min(new_burst * MICRO_PER_TOKEN);
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// 消耗 1 个令牌；`burst=0` 表示不限，恒为 true。
    pub fn allow(&self) -> bool {
        let mut state = self.state.lock().expect("token bucket");
        let now_ms = self.now_ms();
        let elapsed_ms = now_ms.saturating_sub(state.last_ms);
        if state.burst > 0 {
            let burst = u64::from(state.burst);
            let window = u64::from(state.window_secs).max(1);
            // micro/ms 补充速率 = burst * 1e6 / (window * 1000) = burst * 1000 / window
            let refill = elapsed_ms.saturating_mul(burst) * 1000 / window;
            state.tokens_micro = (state.tokens_micro + refill).min(burst * MICRO_PER_TOKEN);
        } else {
            state.tokens_micro = MICRO_PER_TOKEN;
        }
        state.last_ms = now_ms;
        if state.tokens_micro >= MICRO_PER_TOKEN {
            state.tokens_micro -= MICRO_PER_TOKEN;
            true
        } else {
            false
        }
    }

    /// 距最近一次消耗的时长；用于清理长期不活跃条目。
    pub fn idle(&self) -> Duration {
        let state = self.state.lock().expect("token bucket");
        Duration::from_millis(self.now_ms().saturating_sub(state.last_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_window_allows_burst_then_rejects() {
        let bucket = TokenBucket::new(3, 60);
        assert!(bucket.allow());
        assert!(bucket.allow());
        assert!(bucket.allow());
        assert!(!bucket.allow());
        assert!(!bucket.allow());
    }

    #[test]
    fn zero_burst_is_unlimited() {
        let bucket = TokenBucket::new(0, 60);
        for _ in 0..1000 {
            assert!(bucket.allow());
        }
    }

    #[test]
    fn tokens_refill_over_time() {
        // 2 个 / 1 秒：用尽后等待窗口过半应恢复 1 个以上。
        let bucket = TokenBucket::new(2, 1);
        assert!(bucket.allow());
        assert!(bucket.allow());
        assert!(!bucket.allow());
        std::thread::sleep(Duration::from_millis(600));
        assert!(bucket.allow());
    }

    #[test]
    fn reconfigure_rescales_remaining_tokens() {
        let bucket = TokenBucket::new(10, 60);
        assert!(bucket.allow());
        bucket.reconfigure(100, 60);
        // 剩余 9/10 → 换算后 90，仍可连续消耗。
        for _ in 0..20 {
            assert!(bucket.allow());
        }
        bucket.reconfigure(2, 60);
        // 剩余 70/100 → 换算后 1.4，第一次成功、随后拒绝。
        assert!(bucket.allow());
        assert!(!bucket.allow());
        assert!(!bucket.allow());
    }
}