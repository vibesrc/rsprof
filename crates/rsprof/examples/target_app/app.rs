//! Main application logic

use crate::cache::DataCache;
use crate::metrics;
use crate::processing::RequestProcessor;
use crate::validation::InputValidator;

pub struct Stats {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub errors: u64,
}

pub struct Application {
    tick: u64,
    stats: Stats,
    cache: DataCache,
    processor: RequestProcessor,
    validator: InputValidator,
}

impl Application {
    pub fn new() -> Self {
        Self {
            tick: 0,
            stats: Stats {
                cache_hits: 0,
                cache_misses: 0,
                errors: 0,
            },
            cache: DataCache::new(),
            processor: RequestProcessor::new(),
            validator: InputValidator::new(),
        }
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    #[inline(never)]
    pub fn tick(&mut self) {
        self.tick += 1;

        // Simulate incoming requests
        let request = self.generate_request();

        // Validate input
        if !self.validator.validate(&request) {
            self.stats.errors += 1;
            return;
        }

        // Check cache first
        if let Some(_cached) = self.cache.get(&request.key) {
            self.stats.cache_hits += 1;
            return;
        }
        self.stats.cache_misses += 1;

        // Process the request
        let result = self.processor.process(&request);

        // Store in cache
        self.cache.put(request.key.clone(), result);
    }

    #[inline(never)]
    fn generate_request(&self) -> Request {
        Request {
            key: format!("req_{}", self.tick % 150),
            payload: vec![0u8; 64 + (self.tick % 64) as usize],
            priority: (self.tick % 3) as u8,
        }
    }
}

pub struct Request {
    pub key: String,
    pub payload: Vec<u8>,
    pub priority: u8,
}

// Sneaky: Hook into Drop to call metrics
impl Drop for Request {
    fn drop(&mut self) {
        // This causes metrics::record_alloc_size to be called frequently
        metrics::record_alloc_size(self.payload.len());
    }
}
