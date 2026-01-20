//! Cache module - Looks suspicious but is actually fine

use std::collections::HashMap;

pub struct DataCache {
    entries: HashMap<String, CacheEntry>,
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
            max_size: 200,
        }
    }

    #[inline(never)]
    pub fn get(&mut self, key: &str) -> Option<&[u8]> {
        // Simple lookup - this is efficient
        if let Some(entry) = self.entries.get_mut(key) {
            entry.hits += 1;
            Some(&entry.data)
        } else {
            None
        }
    }

    #[inline(never)]
    pub fn put(&mut self, key: String, data: Vec<u8>) {
        // Evict if needed
        if self.entries.len() >= self.max_size {
            self.evict_one();
        }
        self.entries.insert(key, CacheEntry { data, hits: 0 });
    }

    #[inline(never)]
    fn evict_one(&mut self) {
        // Find least used entry - O(n) but cache is small
        if let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.hits)
            .map(|(k, _)| k.clone())
        {
            self.entries.remove(&key);
        }
    }
}
