//! Heap and trace profiling module
//!
//! This module provides profiling through multiple mechanisms:
//!
//! 1. **Shared memory (rsprof-trace)**: Reads CPU and heap events from a ring
//!    buffer populated by the rsprof-trace crate. The target app must use
//!    rsprof-trace with the `profiling` feature enabled. Provides both CPU
//!    sampling and heap tracking without external dependencies.
//!
//! 2. **eBPF uprobes** (requires `heap` feature): Attaches to Rust allocator
//!    functions (__rust_alloc, __rust_dealloc, __rust_realloc). Requires
//!    Linux kernel with eBPF support and root/CAP_BPF capability.

#[cfg(feature = "heap")]
mod sampler;

#[cfg(feature = "heap")]
pub use sampler::{HeapEvent, HeapEventType, HeapSampler, HeapStats};

// Stub types when heap feature is disabled
#[cfg(not(feature = "heap"))]
mod stub;

#[cfg(not(feature = "heap"))]
pub use stub::{HeapEvent, HeapEventType, HeapSampler, HeapStats};

// Shared memory sampler (always available) - reads from rsprof-trace
mod shm_sampler;
pub use shm_sampler::{
    CpuSample, HeapStats as ShmHeapStats, ShmHeapSampler, TraceEvent, TraceEventType,
};

/// Check if eBPF heap profiling is available at compile time
pub const fn heap_compiled() -> bool {
    cfg!(feature = "heap")
}
