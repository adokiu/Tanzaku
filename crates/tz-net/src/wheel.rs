use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

pub struct TimingWheel<K, V> {
    slots: Vec<HashSet<K>>,
    entries: HashMap<K, Entry<V>>,
    cursor: u64,
    capacity: usize,
}

struct Entry<V> {
    deadline: u64,
    value: Option<V>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WheelError {
    #[error("timing wheel capacity reached")]
    Full,
    #[error("timing wheel must have at least one slot and one entry")]
    InvalidCapacity,
}

impl<K, V> TimingWheel<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn new(slots: usize, capacity: usize) -> Result<Self, WheelError> {
        if slots == 0 || capacity == 0 {
            return Err(WheelError::InvalidCapacity);
        }
        Ok(Self {
            slots: (0..slots).map(|_| HashSet::new()).collect(),
            entries: HashMap::new(),
            cursor: 0,
            capacity,
        })
    }

    pub fn schedule(&mut self, key: K, deadline: u64, value: V) -> Result<Option<V>, WheelError> {
        if !self.entries.contains_key(&key) && self.entries.len() == self.capacity {
            return Err(WheelError::Full);
        }
        let slot_count = self.slots.len() as u64;
        let old = self.entries.remove(&key).map(|entry| {
            self.slots[(entry.deadline % slot_count) as usize].remove(&key);
            entry.value.expect("scheduled entries contain a value")
        });
        let deadline = deadline.max(self.cursor.saturating_add(1));
        let index = (deadline % slot_count) as usize;
        self.slots[index].insert(key.clone());
        let _ = self.entries.insert(
            key,
            Entry {
                deadline,
                value: Some(value),
            },
        );
        Ok(old)
    }

    pub fn cancel(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        let slot_count = self.slots.len() as u64;
        self.slots[(entry.deadline % slot_count) as usize].remove(key);
        entry.value
    }

    pub fn advance(&mut self, target: u64) -> Vec<(K, V)> {
        if target <= self.cursor {
            return Vec::new();
        }
        let steps = target - self.cursor;
        self.cursor = target;
        if steps >= self.slots.len() as u64 {
            for slot in &mut self.slots {
                slot.clear();
            }
            let mut expired = Vec::new();
            self.entries.retain(|key, entry| {
                if entry.deadline <= target {
                    expired.push((
                        key.clone(),
                        entry
                            .value
                            .take()
                            .expect("scheduled entries contain a value"),
                    ));
                    false
                } else {
                    true
                }
            });
            let slot_count = self.slots.len() as u64;
            let pending = self
                .entries
                .iter()
                .map(|(key, entry)| (key.clone(), entry.deadline))
                .collect::<Vec<_>>();
            for (key, deadline) in pending {
                self.slots[(deadline % slot_count) as usize].insert(key);
            }
            return expired;
        }
        let slot_count = self.slots.len() as u64;
        let mut expired = Vec::new();
        for tick in (target - steps + 1)..=target {
            let index = tick as usize % self.slots.len();
            let keys = std::mem::take(&mut self.slots[index]);
            for key in keys {
                if self
                    .entries
                    .get(&key)
                    .is_some_and(|entry| entry.deadline <= target)
                {
                    if let Some(entry) = self.entries.remove(&key) {
                        expired
                            .push((key, entry.value.expect("scheduled entries contain a value")));
                    }
                } else if let Some(deadline) = self.entries.get(&key).map(|entry| entry.deadline) {
                    self.slots[(deadline % slot_count) as usize].insert(key);
                }
            }
        }
        expired
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
