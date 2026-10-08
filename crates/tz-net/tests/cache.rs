use std::{num::NonZeroUsize, sync::Arc, thread};
use tz_net::cache::ShardedLru;

#[test]
fn shard_capacity_never_exceeds_configured_limit_under_concurrency() {
    let cache = Arc::new(ShardedLru::new(
        NonZeroUsize::new(257).unwrap(),
        NonZeroUsize::new(16).unwrap(),
    ));
    thread::scope(|scope| {
        for worker in 0..16 {
            let cache = cache.clone();
            let _ = scope.spawn(move || {
                for offset in 0..10_000 {
                    let key = worker * 10_000 + offset;
                    let _ = cache.put(key, key);
                }
            });
        }
    });
    let snapshot = cache.snapshot();
    assert_eq!(snapshot.capacity, 257);
    assert_eq!(snapshot.entries, 257);
}
