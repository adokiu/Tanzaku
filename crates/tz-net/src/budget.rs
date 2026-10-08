use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Debug)]
pub struct Budget(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    limit: usize,
    used: AtomicUsize,
    peak: AtomicUsize,
    parent: Option<Budget>,
}

#[derive(Debug, thiserror::Error)]
#[error("memory budget exceeded: requested {requested}, available {available}")]
pub struct BudgetExceeded {
    pub requested: usize,
    pub available: usize,
}

#[derive(Debug)]
pub struct Reservation {
    budget: Budget,
    bytes: usize,
    _parent: Option<Box<Reservation>>,
}

impl Budget {
    pub fn new(limit: usize) -> Self {
        Self::with_parent(limit, None)
    }

    pub fn child(&self, limit: usize) -> Self {
        Self::with_parent(limit.min(self.0.limit), Some(self.clone()))
    }

    fn with_parent(limit: usize, parent: Option<Self>) -> Self {
        Self(Arc::new(Inner {
            limit,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            parent,
        }))
    }

    pub fn limit(&self) -> usize {
        self.0.limit
    }

    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Relaxed)
    }

    pub fn peak(&self) -> usize {
        self.0.peak.load(Ordering::Relaxed)
    }

    pub fn try_reserve(&self, bytes: usize) -> Result<Reservation, BudgetExceeded> {
        let parent = self
            .0
            .parent
            .as_ref()
            .map(|parent| parent.try_reserve(bytes).map(Box::new))
            .transpose()?;
        let previous = self
            .0
            .used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.0.limit)
            })
            .map_err(|used| BudgetExceeded {
                requested: bytes,
                available: self.0.limit.saturating_sub(used),
            })?;
        self.0.peak.fetch_max(previous + bytes, Ordering::Relaxed);
        Ok(Reservation {
            budget: self.clone(),
            bytes,
            _parent: parent,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
