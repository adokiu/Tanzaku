use std::{
    num::{NonZeroU64, NonZeroUsize},
    time::Duration,
};
use tokio::time::{Instant, advance};
use tz_net::rate::RateDriver;

#[tokio::test(start_paused = true)]
async fn fractional_tokens_accumulate_without_rounding_loss() {
    let (clock, driver) = RateDriver::new(Duration::from_millis(10)).unwrap();
    let task = tokio::spawn(driver.run());
    let bucket = clock.bucket(NonZeroU64::new(3).unwrap(), NonZeroUsize::new(1).unwrap());
    bucket.acquire(1).await.unwrap().consume(1).unwrap();
    let start = Instant::now();
    for _ in 0..3 {
        bucket.acquire(1).await.unwrap().consume(1).unwrap();
    }
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert!(start.elapsed() < Duration::from_millis(1050));
    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn unused_tokens_are_refunded() {
    let (clock, driver) = RateDriver::new(Duration::from_millis(10)).unwrap();
    let task = tokio::spawn(driver.run());
    let bucket = clock.bucket(NonZeroU64::new(1).unwrap(), NonZeroUsize::new(100).unwrap());
    let mut grant = bucket.acquire(100).await.unwrap();
    grant.consume(40).unwrap();
    drop(grant);
    let grant = bucket.acquire(100).await.unwrap();
    assert_eq!(grant.remaining(), 60);
    drop(grant);
    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn idle_clock_does_not_tick() {
    let (clock, driver) = RateDriver::new(Duration::from_millis(10)).unwrap();
    let task = tokio::spawn(driver.run());
    tokio::task::yield_now().await;
    advance(Duration::from_secs(60)).await;
    assert_eq!(clock.snapshot().ticks, 0);
    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn driver_shutdown_wakes_waiters() {
    let (clock, driver) = RateDriver::new(Duration::from_millis(10)).unwrap();
    let bucket = clock.bucket(NonZeroU64::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    bucket.acquire(1).await.unwrap().consume(1).unwrap();
    let waiter = tokio::spawn(async move { bucket.acquire(1).await });
    tokio::task::yield_now().await;
    drop(driver);
    assert!(waiter.await.unwrap().is_err());
}
