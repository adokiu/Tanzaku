use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct ConnectionLimit {
    permits: Arc<Semaphore>,
    maximum: usize,
    accepted: Arc<AtomicU64>,
    rejected: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectionSnapshot {
    pub active: usize,
    pub accepted: u64,
    pub rejected: u64,
}

impl ConnectionLimit {
    pub fn new(maximum: usize) -> io::Result<Self> {
        if maximum == 0 || maximum > Semaphore::MAX_PERMITS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid connection limit",
            ));
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(maximum)),
            maximum,
            accepted: Arc::new(AtomicU64::new(0)),
            rejected: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        match self.permits.clone().try_acquire_owned() {
            Ok(permit) => {
                self.accepted.fetch_add(1, Ordering::Relaxed);
                Some(permit)
            }
            Err(_) => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn snapshot(&self) -> ConnectionSnapshot {
        ConnectionSnapshot {
            active: self.maximum - self.permits.available_permits(),
            accepted: self.accepted.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }
}
