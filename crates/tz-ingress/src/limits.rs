use std::num::{NonZeroU64, NonZeroUsize};
use tz_net::{
    meter::TrafficMeter,
    pump::PumpOptions,
    rate::{RateClock, TokenBucket},
};

pub fn pump_options(clock: &RateClock, speed_limit_mbps: i64, meter: TrafficMeter) -> PumpOptions {
    let (left, right) = directional_buckets(clock, speed_limit_mbps);
    PumpOptions {
        left_to_right: left,
        right_to_left: right,
        meter,
    }
}

pub fn new_conn_bucket(clock: &RateClock, max_new_conns_per_sec: i32) -> Option<TokenBucket> {
    if max_new_conns_per_sec <= 0 {
        return None;
    }
    let rate = NonZeroU64::new(max_new_conns_per_sec as u64)?;
    let capacity = NonZeroUsize::new(max_new_conns_per_sec as usize)?;
    Some(clock.bucket(rate, capacity))
}

pub fn directional_buckets(
    clock: &RateClock,
    speed_limit_mbps: i64,
) -> (Option<TokenBucket>, Option<TokenBucket>) {
    if speed_limit_mbps <= 0 {
        return (None, None);
    }
    let bytes_per_second = speed_limit_mbps.saturating_mul(125_000) as u64;
    let Some(rate) = NonZeroU64::new(bytes_per_second.max(1)) else {
        return (None, None);
    };
    let burst = bytes_per_second.max(32 * 1024);
    let capacity = NonZeroUsize::new(burst.min(usize::MAX as u64) as usize)
        .unwrap_or_else(|| NonZeroUsize::new(32 * 1024).expect("burst"));
    let bucket = clock.bucket(rate, capacity);
    (Some(bucket.clone()), Some(bucket))
}
