//! Stub implementations when heap profiling is not compiled

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct HeapStats {
    pub live_bytes: i64,
    pub total_allocs: u64,
    pub total_frees: u64,
    pub total_alloc_bytes: u64,
    pub total_free_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct HeapEvent {
    pub callsite: u64,
    pub ptr: u64,
    pub size: i64,
    pub event_type: HeapEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapEventType {
    Alloc,
    Free,
    Realloc,
}

pub struct HeapSampler;

impl HeapSampler {
    pub fn new(_pid: u32, _exe_path: &Path) -> Result<Self> {
        Err(Error::Bpf(
            "Heap profiling not available. Rebuild with: cargo build --features heap\n\
             Requires: clang and libbpf-dev (sudo apt install clang libbpf-dev)".to_string()
        ))
    }

    pub fn read_stats(&self) -> HashMap<u64, HeapStats> {
        HashMap::new()
    }

    pub fn poll_events(&mut self, _timeout: Duration) -> Vec<HeapEvent> {
        Vec::new()
    }
}
