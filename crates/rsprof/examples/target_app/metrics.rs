//! Metrics module - Contains BOTTLENECK #3 (Hidden CPU hog)
//!
//! This module looks innocent but gets called from Drop impls,
//! making it a hidden performance drain.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

// Global metrics - initialized lazily
static METRICS: OnceLock<Mutex<MetricsCollector>> = OnceLock::new();

struct MetricsCollector {
    counters: HashMap<String, u64>,
    samples: Vec<f64>,
}

impl MetricsCollector {
    fn new() -> Self {
        Self {
            counters: HashMap::new(),
            samples: Vec::new(),
        }
    }
}

// This gets called from trait impls - sneaky!

#[inline(never)]
pub fn record_alloc_size(size: usize) {
    let collector = METRICS.get_or_init(|| Mutex::new(MetricsCollector::new()));
    if let Ok(mut guard) = collector.lock() {
        // BOTTLENECK #3: Called on many allocations, does expensive work
        let key = format!("alloc_bucket_{}", size / 64);
        *guard.counters.entry(key).or_insert(0) += 1;

        // Compute running statistics - expensive!
        guard.samples.push(size as f64);
        if guard.samples.len() > 100 {
            let _mean = compute_mean(&guard.samples);
            let _stddev = compute_stddev(&guard.samples);
            guard.samples.clear();
        }
    }
}

#[inline(never)]
fn compute_mean(samples: &[f64]) -> f64 {
    samples.iter().sum::<f64>() / samples.len() as f64
}

#[inline(never)]
fn compute_stddev(samples: &[f64]) -> f64 {
    let mean = compute_mean(samples);
    let variance: f64 = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    variance.sqrt()
}
