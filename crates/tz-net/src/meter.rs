use portable_atomic::AtomicU64;
use std::sync::{
    atomic::Ordering,
    Arc,
};

#[derive(Debug)]
pub struct ByteCounter(Arc<AtomicU64>);

impl Default for ByteCounter {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }
}

impl Clone for ByteCounter {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl ByteCounter {
    pub fn add(&self, bytes: usize) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(total.saturating_add(bytes as u64))
            });
    }

    pub fn total(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn take(&self) -> u64 {
        self.0.swap(0, Ordering::Relaxed)
    }
}

#[derive(Clone, Default, Debug)]
pub struct TrafficMeter {
    pub left_to_right: ByteCounter,
    pub right_to_left: ByteCounter,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficSnapshot {
    pub left_to_right: u64,
    pub right_to_left: u64,
}

impl TrafficMeter {
    pub fn snapshot(&self) -> TrafficSnapshot {
        TrafficSnapshot {
            left_to_right: self.left_to_right.total(),
            right_to_left: self.right_to_left.total(),
        }
    }

    pub fn take_delta(&self) -> TrafficSnapshot {
        TrafficSnapshot {
            left_to_right: self.left_to_right.take(),
            right_to_left: self.right_to_left.take(),
        }
    }
}
