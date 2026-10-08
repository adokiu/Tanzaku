use std::num::NonZeroUsize;

use tz_net::{admission::ConnectionLimit, budget::Budget, pool::BufferPool};

#[test]
fn nested_budget_rolls_back_parent_on_failure() {
    let root = Budget::new(1024);
    let session = root.child(128);
    let lease = session.try_reserve(100).unwrap();
    assert!(session.try_reserve(29).is_err());
    assert_eq!(root.used(), 100);
    assert_eq!(session.used(), 100);
    drop(lease);
    assert_eq!(root.used(), 0);
    assert_eq!(session.used(), 0);
}

#[test]
fn sessions_share_process_limit() {
    let root = Budget::new(100);
    let a = root.child(80);
    let b = root.child(80);
    let first = a.try_reserve(60).unwrap();
    assert!(b.try_reserve(41).is_err());
    let second = b.try_reserve(40).unwrap();
    assert_eq!(root.used(), 100);
    drop((first, second));
    assert_eq!(root.used(), 0);
}

#[test]
fn concurrent_reservations_never_exceed_budget() {
    let root = Budget::new(8);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let root = &root;
            let _ = scope.spawn(move || {
                for _ in 0..1000 {
                    if let Ok(permit) = root.try_reserve(1) {
                        assert!(root.used() <= 8);
                        std::thread::yield_now();
                        drop(permit);
                    }
                }
            });
        }
    });
    assert_eq!(root.used(), 0);
    assert!(root.peak() <= 8);
}

#[tokio::test]
async fn buffer_pool_is_lazy_bounded_and_clears_returned_bytes() {
    let budget = Budget::new(128);
    let pool = BufferPool::new(
        NonZeroUsize::new(64).unwrap(),
        NonZeroUsize::new(2).unwrap(),
        &budget,
    )
    .unwrap();
    assert_eq!(pool.snapshot().allocated_bytes, 0);
    let mut a = pool.acquire().await.unwrap();
    let b = pool.acquire().await.unwrap();
    a.as_mut().fill(42);
    assert_eq!(pool.snapshot().in_use, 2);
    assert!(pool.try_acquire().unwrap().is_none());
    drop(a);
    let c = pool.acquire().await.unwrap();
    assert!(c.as_ref().iter().all(|byte| *byte == 0));
    assert_eq!(pool.snapshot().allocated_bytes, 128);
    drop((b, c));
    assert_eq!(pool.snapshot().in_use, 0);
    drop(pool);
    assert_eq!(budget.used(), 0);
}

#[test]
fn connection_permit_releases_on_drop() {
    let limit = ConnectionLimit::new(1).unwrap();
    let permit = limit.try_acquire().unwrap();
    assert!(limit.try_acquire().is_none());
    assert_eq!(limit.snapshot().active, 1);
    assert_eq!(limit.snapshot().rejected, 1);
    drop(permit);
    assert!(limit.try_acquire().is_some());
}
