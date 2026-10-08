use std::num::NonZeroUsize;
use tz_net::{
    cache::ShardedLru,
    wheel::{TimingWheel, WheelError},
};

#[test]
fn wheel_reschedules_cancels_and_respects_capacity() {
    let mut wheel = TimingWheel::new(8, 2).unwrap();
    assert_eq!(wheel.schedule("a", 3, 10).unwrap(), None);
    assert_eq!(wheel.schedule("a", 5, 20).unwrap(), Some(10));
    assert_eq!(wheel.schedule("b", 4, 30).unwrap(), None);
    assert_eq!(wheel.schedule("c", 6, 40), Err(WheelError::Full));
    assert_eq!(wheel.advance(4), vec![("b", 30)]);
    assert_eq!(wheel.cancel(&"a"), Some(20));
    assert!(wheel.advance(20).is_empty());
    assert!(wheel.is_empty());
}

#[test]
fn wheel_large_time_jump_expires_and_rebuilds_wrapped_entries() {
    let mut wheel = TimingWheel::new(4, 4).unwrap();
    assert_eq!(wheel.schedule(1, 2, "due").unwrap(), None);
    assert_eq!(wheel.schedule(2, 14, "future").unwrap(), None);
    assert_eq!(wheel.advance(10), vec![(1, "due")]);
    assert_eq!(wheel.advance(14), vec![(2, "future")]);
}

#[test]
fn sharded_lru_has_exact_global_capacity() {
    let cache = ShardedLru::new(NonZeroUsize::new(7).unwrap(), NonZeroUsize::new(3).unwrap());
    for key in 0..1000 {
        let _ = cache.put(key, key);
        assert!(cache.snapshot().entries <= 7);
    }
    assert_eq!(cache.snapshot().capacity, 7);
    assert_eq!(cache.get(&999), Some(999));
    assert_eq!(cache.get_or_insert_with(999, || 0), 999);
    assert_eq!(cache.get_or_insert_with(2000, || 2000), 2000);
    assert!(cache.snapshot().entries <= 7);
}
