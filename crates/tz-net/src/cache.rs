use lru::LruCache;
use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hash, Hasher},
    num::NonZeroUsize,
    sync::Mutex,
};

pub struct ShardedLru<K, V> {
    shards: Vec<Mutex<LruCache<K, V, RandomState>>>,
    hash_builder: RandomState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSnapshot {
    pub entries: usize,
    pub capacity: usize,
    pub shards: usize,
}

impl<K, V> ShardedLru<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    pub fn new(capacity: NonZeroUsize, requested_shards: NonZeroUsize) -> Self {
        let shards = capacity.get().min(requested_shards.get());
        let base = capacity.get() / shards;
        let extra = capacity.get() % shards;
        let caches = (0..shards)
            .map(|index| {
                let shard_capacity = base + usize::from(index < extra);
                Mutex::new(LruCache::with_hasher(
                    NonZeroUsize::new(shard_capacity).expect("non-zero shard capacity"),
                    RandomState::new(),
                ))
            })
            .collect();
        Self {
            shards: caches,
            hash_builder: RandomState::new(),
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.shards[self.index(key)].lock().ok()?.get(key).cloned()
    }

    pub fn get_or_insert_with(&self, key: K, create: impl FnOnce() -> V) -> V {
        let index = self.index(&key);
        let mut shard = self.shards[index]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(value) = shard.get(&key) {
            return value.clone();
        }
        let value = create();
        let _ = shard.put(key, value.clone());
        value
    }

    pub fn put(&self, key: K, value: V) -> Option<(K, V)> {
        let index = self.index(&key);
        self.shards[index].lock().ok()?.push(key, value)
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        self.shards[self.index(key)].lock().ok()?.pop(key)
    }

    pub fn snapshot(&self) -> CacheSnapshot {
        let entries = self
            .shards
            .iter()
            .map(|shard| shard.lock().map(|cache| cache.len()).unwrap_or(0))
            .sum();
        let capacity = self
            .shards
            .iter()
            .map(|shard| shard.lock().map(|cache| cache.cap().get()).unwrap_or(0))
            .sum();
        CacheSnapshot {
            entries,
            capacity,
            shards: self.shards.len(),
        }
    }

    fn index(&self, key: &K) -> usize {
        let mut hasher = self.hash_builder.build_hasher();
        key.hash(&mut hasher);
        hasher.finish() as usize % self.shards.len()
    }
}
