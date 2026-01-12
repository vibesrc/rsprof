//! Heap profiling module using eBPF uprobes
//!
//! This module provides memory profiling by attaching eBPF uprobes to
//! Rust allocator functions (__rust_alloc, __rust_dealloc, __rust_realloc).
//!
//! Requires:
//! - Linux kernel with eBPF support
//! - Root or CAP_BPF capability
//! - For compilation: clang and libbpf-dev

#[cfg(feature = "heap")]
mod sampler;

#[cfg(feature = "heap")]
pub use sampler::{HeapEvent, HeapEventType, HeapSampler, HeapStats};

// Stub types when heap feature is disabled or eBPF unavailable
#[cfg(not(feature = "heap"))]
mod stub;

#[cfg(not(feature = "heap"))]
pub use stub::{HeapEvent, HeapEventType, HeapSampler, HeapStats};

/// Check if heap profiling is available at compile time
pub const fn heap_compiled() -> bool {
    cfg!(feature = "heap")
}
