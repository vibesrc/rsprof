//! Heap and trace profiling module
//!
//! This module provides profiling through shared memory (rsprof-trace).
//!
//! Reads CPU and heap events from a ring buffer populated by the rsprof-trace
//! crate. The target app must use rsprof-trace with the `profiling` feature
//! enabled. Provides both CPU sampling and heap tracking without external
//! dependencies.

// Shared memory sampler (always available) - reads from rsprof-trace
mod shm_sampler;
pub use shm_sampler::{
    CpuSample, HeapStats as ShmHeapStats, ShmHeapSampler, TraceEvent, TraceEventType,
};
