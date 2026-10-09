use portable_atomic::AtomicU64;
use std::{
    collections::HashMap,
    num::{NonZeroU64, NonZeroUsize},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone)]
pub struct RateClock(Arc<ClockInner>);

pub struct RateDriver {
    clock: RateClock,
    period: Duration,
}

#[derive(Clone)]
pub struct TokenBucket(Arc<BucketInner>);

struct ClockInner {
    closed: AtomicBool,
    ticks: AtomicU64,
    next_id: AtomicU64,
    work: Notify,
    waiters: Mutex<HashMap<u64, Weak<BucketInner>>>,
}

struct BucketInner {
    id: u64,
    clock: Weak<ClockInner>,
    rate: u64,
    capacity: u64,
    state: Mutex<BucketState>,
    ready: Notify,
}

struct BucketState {
    tokens: u64,
    remainder: u128,
    last: Instant,
}

pub struct TokenGrant {
    bucket: TokenBucket,
    remaining: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RateError {
    #[error("rate clock has stopped")]
    Closed,
    #[error("a token request must be greater than zero")]
    ZeroRequest,
    #[error("token grant cannot be consumed beyond its reservation")]
    Overconsume,
    #[error("rate bucket state is unavailable")]
    Poisoned,
    #[error("invalid rate clock period")]
    InvalidPeriod,
}

#[derive(Debug, Clone, Copy)]
pub struct ClockSnapshot {
    pub registered_waiters: usize,
    pub ticks: u64,
    pub closed: bool,
}

impl RateDriver {
    pub fn new(period: Duration) -> Result<(RateClock, Self), RateError> {
        if period.is_zero() {
            return Err(RateError::InvalidPeriod);
        }
        let clock = RateClock(Arc::new(ClockInner {
            closed: AtomicBool::new(false),
            ticks: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            work: Notify::new(),
            waiters: Mutex::new(HashMap::new()),
        }));
        let driver = Self {
            clock: clock.clone(),
            period,
        };
        Ok((clock, driver))
    }

    pub async fn run(self) {
        loop {
            let notified = self.clock.0.work.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.clock.0.closed.load(Ordering::Acquire) {
                break;
            }
            notified.await;
            loop {
                if self.clock.0.closed.load(Ordering::Acquire) {
                    break;
                }
                let has_waiters = self
                    .clock
                    .0
                    .waiters
                    .lock()
                    .map(|waiters| !waiters.is_empty())
                    .unwrap_or(false);
                if !has_waiters {
                    break;
                }
                tokio::time::sleep(self.period).await;
                self.clock.0.ticks.fetch_add(1, Ordering::Relaxed);
                let ready = self
                    .clock
                    .0
                    .waiters
                    .lock()
                    .map(|mut waiters| {
                        let ready = waiters
                            .drain()
                            .filter_map(|(_, bucket)| bucket.upgrade())
                            .collect::<Vec<_>>();
                        ready
                    })
                    .unwrap_or_default();
                for bucket in ready {
                    bucket.ready.notify_waiters();
                }
            }
        }
        self.clock.close();
    }
}

impl Drop for RateDriver {
    fn drop(&mut self) {
        self.clock.close();
    }
}

impl RateClock {
    fn close(&self) {
        self.0.close();
    }

    pub fn bucket(&self, rate_bytes_per_second: NonZeroU64, capacity: NonZeroUsize) -> TokenBucket {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        TokenBucket(Arc::new(BucketInner {
            id,
            clock: Arc::downgrade(&self.0),
            rate: rate_bytes_per_second.get(),
            capacity: capacity.get().min(u64::MAX as usize) as u64,
            state: Mutex::new(BucketState {
                tokens: capacity.get().min(u64::MAX as usize) as u64,
                remainder: 0,
                last: Instant::now(),
            }),
            ready: Notify::new(),
        }))
    }

    pub fn shutdown(&self) {
        self.0.close();
    }

    pub fn snapshot(&self) -> ClockSnapshot {
        ClockSnapshot {
            registered_waiters: self
                .0
                .waiters
                .lock()
                .map(|waiters| waiters.len())
                .unwrap_or(0),
            ticks: self.0.ticks.load(Ordering::Relaxed),
            closed: self.0.closed.load(Ordering::Acquire),
        }
    }
}

impl ClockInner {
    fn register(&self, bucket: &Arc<BucketInner>) -> Result<(), RateError> {
        let mut waiters = self.waiters.lock().map_err(|_| RateError::Poisoned)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RateError::Closed);
        }
        let _ = waiters.insert(bucket.id, Arc::downgrade(bucket));
        self.work.notify_one();
        Ok(())
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.work.notify_waiters();
        let waiters = self
            .waiters
            .lock()
            .map(|mut waiters| {
                waiters
                    .drain()
                    .filter_map(|(_, bucket)| bucket.upgrade())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for bucket in waiters {
            bucket.ready.notify_waiters();
        }
    }
}

impl TokenBucket {
    pub async fn acquire(&self, requested: usize) -> Result<TokenGrant, RateError> {
        if requested == 0 {
            return Err(RateError::ZeroRequest);
        }
        let clock = self.0.clock.upgrade().ok_or(RateError::Closed)?;
        loop {
            let notified = self.0.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if clock.closed.load(Ordering::Acquire) {
                return Err(RateError::Closed);
            }
            let available = {
                let mut state = self.0.state.lock().map_err(|_| RateError::Poisoned)?;
                refill(&mut state, self.0.rate, self.0.capacity, Instant::now());
                let amount = state.tokens.min(requested.min(u64::MAX as usize) as u64);
                state.tokens -= amount;
                amount
            };
            if available > 0 {
                return Ok(TokenGrant {
                    bucket: self.clone(),
                    remaining: available,
                });
            }
            clock.register(&self.0)?;
            notified.await;
        }
    }
}

fn refill(state: &mut BucketState, rate: u64, capacity: u64, now: Instant) {
    let elapsed = now.saturating_duration_since(state.last).as_nanos();
    state.last = now;
    let produced = elapsed
        .saturating_mul(rate as u128)
        .saturating_add(state.remainder);
    let whole = produced / 1_000_000_000;
    state.remainder = produced % 1_000_000_000;
    state.tokens = state
        .tokens
        .saturating_add(whole.min(u64::MAX as u128) as u64)
        .min(capacity);
    if state.tokens == capacity {
        state.remainder = 0;
    }
}

impl TokenGrant {
    pub fn remaining(&self) -> usize {
        self.remaining.min(usize::MAX as u64) as usize
    }

    pub fn consume(&mut self, amount: usize) -> Result<(), RateError> {
        let amount = amount as u64;
        if amount > self.remaining {
            return Err(RateError::Overconsume);
        }
        self.remaining -= amount;
        Ok(())
    }
}

impl Drop for TokenGrant {
    fn drop(&mut self) {
        if self.remaining == 0 {
            return;
        }
        if let Ok(mut state) = self.bucket.0.state.lock() {
            state.tokens = state
                .tokens
                .saturating_add(self.remaining)
                .min(self.bucket.0.capacity);
        }
        self.bucket.0.ready.notify_one();
    }
}
