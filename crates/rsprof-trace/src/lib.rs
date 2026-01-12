//! Self-instrumentation library for rsprof.
//!
//! This crate provides CPU and heap profiling through self-instrumentation:
//! - **CPU profiling**: Timer-based sampling using SIGPROF
//! - **Heap profiling**: Custom allocator that tracks allocations
//!
//! # Usage
//!
//! Add to your `Cargo.toml`:
//! ```toml
//! [dependencies]
//! rsprof-trace = { version = "0.1", features = ["profiling"] }
//! ```
//!
//! Enable profiling with the `profiler!` macro:
//! ```rust,ignore
//! rsprof_trace::profiler!();  // CPU at 99Hz + heap profiling
//! ```
//!
//! Or customize the CPU sampling frequency:
//! ```rust,ignore
//! rsprof_trace::profiler!(cpu = 199);  // CPU at 199Hz + heap profiling
//! ```
//!
//! Build with frame pointers for accurate stack traces:
//! ```bash
//! RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release --features profiling
//! ```
//!
//! When the `profiling` feature is disabled, the macro expands to a no-op
//! allocator passthrough with zero overhead.

#![no_std]

extern crate alloc;

// Include profiling module when any profiling feature is enabled
#[cfg(any(feature = "heap", feature = "cpu"))]
mod profiling;

// Re-export CPU profiling functions
#[cfg(feature = "cpu")]
pub use profiling::{start_cpu_profiling, stop_cpu_profiling};

// Stubs when CPU feature is disabled
#[cfg(not(feature = "cpu"))]
#[inline]
pub fn start_cpu_profiling(_freq_hz: u32) {}

#[cfg(not(feature = "cpu"))]
#[inline]
pub fn stop_cpu_profiling() {}

/// A profiling allocator that wraps the system allocator.
///
/// The const generic `CPU_FREQ` specifies the CPU sampling frequency in Hz.
/// Set to 0 to disable CPU profiling.
///
/// When the `heap` feature is enabled, this allocator captures
/// allocation and deallocation events along with stack traces.
/// CPU profiling (if enabled) starts automatically on the first allocation.
///
/// When profiling features are disabled, it's a zero-cost passthrough.
pub struct ProfilingAllocator<const CPU_FREQ: u32 = 99>;

impl<const CPU_FREQ: u32> ProfilingAllocator<CPU_FREQ> {
    pub const fn new() -> Self {
        Self
    }
}

impl<const CPU_FREQ: u32> Default for ProfilingAllocator<CPU_FREQ> {
    fn default() -> Self {
        Self::new()
    }
}

// Legacy alias for backwards compatibility
pub type HeapProfiler = ProfilingAllocator<99>;

#[cfg(not(feature = "heap"))]
mod disabled {
    use super::ProfilingAllocator;
    use core::alloc::{GlobalAlloc, Layout};

    unsafe impl<const CPU_FREQ: u32> GlobalAlloc for ProfilingAllocator<CPU_FREQ> {
        #[inline]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { libc::malloc(layout.size()) as *mut u8 }
        }

        #[inline]
        unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
            unsafe { libc::free(ptr as *mut libc::c_void) }
        }

        #[inline]
        unsafe fn realloc(&self, ptr: *mut u8, _layout: Layout, new_size: usize) -> *mut u8 {
            unsafe { libc::realloc(ptr as *mut libc::c_void, new_size) as *mut u8 }
        }

        #[inline]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            unsafe { libc::calloc(1, layout.size()) as *mut u8 }
        }
    }
}

#[cfg(feature = "heap")]
mod enabled {
    use super::ProfilingAllocator;
    #[cfg(feature = "cpu")]
    use super::profiling::start_cpu_profiling;
    use super::profiling::{record_alloc, record_dealloc};
    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicBool, Ordering};

    static CPU_INITIALIZED: AtomicBool = AtomicBool::new(false);

    #[inline]
    fn maybe_init_cpu<const FREQ: u32>() {
        #[cfg(feature = "cpu")]
        {
            if FREQ > 0 && !CPU_INITIALIZED.swap(true, Ordering::SeqCst) {
                start_cpu_profiling(FREQ);
            }
        }
    }

    unsafe impl<const CPU_FREQ: u32> GlobalAlloc for ProfilingAllocator<CPU_FREQ> {
        #[inline]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            maybe_init_cpu::<CPU_FREQ>();
            let ptr = unsafe { libc::malloc(layout.size()) as *mut u8 };
            if !ptr.is_null() {
                record_alloc(ptr, layout.size());
            }
            ptr
        }

        #[inline]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            record_dealloc(ptr, layout.size());
            unsafe { libc::free(ptr as *mut libc::c_void) }
        }

        #[inline]
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_dealloc(ptr, layout.size());
            let new_ptr = unsafe { libc::realloc(ptr as *mut libc::c_void, new_size) as *mut u8 };
            if !new_ptr.is_null() {
                record_alloc(new_ptr, new_size);
            }
            new_ptr
        }

        #[inline]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            maybe_init_cpu::<CPU_FREQ>();
            let ptr = unsafe { libc::calloc(1, layout.size()) as *mut u8 };
            if !ptr.is_null() {
                record_alloc(ptr, layout.size());
            }
            ptr
        }
    }
}

/// Enable profiling for your application.
///
/// This macro sets up both CPU and heap profiling with sensible defaults.
/// CPU profiling starts automatically on the first allocation.
/// When the `profiling` feature is disabled, it expands to a zero-cost no-op.
///
/// # Examples
///
/// ```rust,ignore
/// // Default: CPU at 99Hz + heap profiling
/// rsprof_trace::profiler!();
///
/// // Custom CPU frequency
/// rsprof_trace::profiler!(cpu = 199);
/// ```
///
/// # Build
///
/// Enable profiling at build time:
/// ```bash
/// RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release --features profiling
/// ```
#[macro_export]
#[cfg(feature = "heap")]
macro_rules! profiler {
    () => {
        $crate::profiler!(cpu = 99);
    };
    (cpu = $freq:expr) => {
        #[global_allocator]
        static __RSPROF_ALLOC: $crate::ProfilingAllocator<$freq> =
            $crate::ProfilingAllocator::<$freq>::new();
    };
}

/// No-op when heap feature is disabled (CPU-only not supported with this macro)
#[macro_export]
#[cfg(not(feature = "heap"))]
macro_rules! profiler {
    () => {};
    (cpu = $freq:expr) => {};
}
