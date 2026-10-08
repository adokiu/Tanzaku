use crate::budget::{Budget, BudgetExceeded, Reservation};
use std::{
    io,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct BufferPool(Arc<Inner>);

struct Inner {
    chunk_size: usize,
    max_buffers: usize,
    free: Mutex<Vec<Box<[u8]>>>,
    slots: Arc<Semaphore>,
    allocated: AtomicUsize,
    in_use: AtomicUsize,
    _reservation: Reservation,
}

pub struct BufferLease {
    pool: BufferPool,
    buffer: Option<Box<[u8]>>,
    used: usize,
    _slot: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSnapshot {
    pub allocated_bytes: usize,
    pub in_use: usize,
    pub cached: usize,
    pub capacity: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
    #[error("buffer pool allocation failed")]
    Allocation,
    #[error("invalid buffer pool size")]
    InvalidSize,
    #[error("buffer pool mutex poisoned")]
    Poisoned,
}

impl BufferPool {
    pub fn new(
        chunk_size: NonZeroUsize,
        max_buffers: NonZeroUsize,
        budget: &Budget,
    ) -> Result<Self, PoolError> {
        if chunk_size.get() < 64 || max_buffers.get() > Semaphore::MAX_PERMITS {
            return Err(PoolError::InvalidSize);
        }
        let max_bytes = chunk_size
            .get()
            .checked_mul(max_buffers.get())
            .ok_or(PoolError::InvalidSize)?;
        let reservation = budget.try_reserve(max_bytes)?;
        Ok(Self(Arc::new(Inner {
            chunk_size: chunk_size.get(),
            max_buffers: max_buffers.get(),
            free: Mutex::new(Vec::new()),
            slots: Arc::new(Semaphore::new(max_buffers.get())),
            allocated: AtomicUsize::new(0),
            in_use: AtomicUsize::new(0),
            _reservation: reservation,
        })))
    }

    pub async fn acquire(&self) -> io::Result<BufferLease> {
        let slot = self
            .0
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "buffer pool closed"))?;
        self.make_lease(slot)
    }

    pub fn try_acquire(&self) -> io::Result<Option<BufferLease>> {
        let slot = match self.0.slots.clone().try_acquire_owned() {
            Ok(slot) => slot,
            Err(tokio::sync::TryAcquireError::NoPermits) => return Ok(None),
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "buffer pool closed",
                ));
            }
        };
        self.make_lease(slot).map(Some)
    }

    fn make_lease(&self, slot: OwnedSemaphorePermit) -> io::Result<BufferLease> {
        let buffer = self
            .0
            .free
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop();
        let buffer = match buffer {
            Some(buffer) => buffer,
            None => {
                let mut bytes = Vec::new();
                bytes.try_reserve_exact(self.0.chunk_size).map_err(|_| {
                    io::Error::new(io::ErrorKind::OutOfMemory, "buffer pool allocation failed")
                })?;
                bytes.resize(self.0.chunk_size, 0);
                self.0.allocated.fetch_add(1, Ordering::Relaxed);
                bytes.into_boxed_slice()
            }
        };
        self.0.in_use.fetch_add(1, Ordering::Relaxed);
        Ok(BufferLease {
            pool: self.clone(),
            buffer: Some(buffer),
            used: 0,
            _slot: slot,
        })
    }

    pub fn snapshot(&self) -> PoolSnapshot {
        let cached = self
            .0
            .free
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        PoolSnapshot {
            allocated_bytes: self.0.allocated.load(Ordering::Relaxed) * self.0.chunk_size,
            in_use: self.0.in_use.load(Ordering::Relaxed),
            cached,
            capacity: self.0.max_buffers,
        }
    }
}

impl BufferLease {
    pub fn as_ref(&self) -> &[u8] {
        self.buffer.as_deref().unwrap_or_default()
    }

    pub fn as_mut(&mut self) -> &mut [u8] {
        let buffer = self.buffer.as_deref_mut().unwrap_or_default();
        self.used = buffer.len();
        buffer
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut [u8] {
        self.buffer.as_deref_mut().unwrap_or_default()
    }

    pub(crate) fn mark_used(&mut self, used: usize) {
        self.used = self
            .used
            .max(used.min(self.buffer.as_ref().map_or(0, |buffer| buffer.len())));
    }
}

impl Drop for BufferLease {
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer[..self.used].fill(0);
            self.pool
                .0
                .free
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(buffer);
            self.pool.0.in_use.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
