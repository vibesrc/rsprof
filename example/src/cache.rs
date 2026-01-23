//! Cache with a small bookkeeping leak that recovers.

use std::collections::HashMap;

pub struct DataCache {
    entries: HashMap<String, CacheEntry>,
    ghost_keys: Vec<String>,
    max_size: usize,
}

struct CacheEntry {
    data: Vec<u8>,
    hits: u32,
}

impl DataCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ghost_keys: Vec::new(),
            max_size: 512,
        }
    }

    #[inline(never)]
    pub fn get(&mut self, key: &str) -> Option<&[u8]> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.hits += 1;
            Some(&entry.data)
        } else {
            None
        }
    }

    #[inline(never)]
    pub fn put(&mut self, key: String, data: Vec<u8>) {
        if self.entries.len() >= self.max_size {
            self.evict_one();
        }
        self.ghost_keys.push(key.clone());
        self.entries.insert(key, CacheEntry { data, hits: 0 });

        if self.ghost_keys.len() > 4000 {
            self.ghost_keys.drain(0..2000);
        }
    }

    #[inline(never)]
    fn evict_one(&mut self) {
        if let Some((evict_key, _)) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.hits)
            .map(|(k, e)| (k.clone(), e.hits))
        {
            self.entries.remove(&evict_key);
        }
    }
}
