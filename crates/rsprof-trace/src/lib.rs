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
//! For CPU profiling only:
//! ```rust,ignore
//! fn main() {
//!     // Start CPU profiling at 99Hz
//!     rsprof_trace::start_cpu_profiling(99);
//!
//!     // Your application code...
//!
//!     // Stop profiling (optional, stops on process exit)
//!     rsprof_trace::stop_cpu_profiling();
//! }
//! ```
//!
//! For heap profiling, use the global allocator:
//! ```rust,ignore
//! #[global_allocator]
//! static ALLOC: rsprof_trace::ProfilingAllocator = rsprof_trace::ProfilingAllocator;
//! ```
//!
//! Build with frame pointers for accurate stack traces:
//! ```bash
//! RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release --features profiling
//! ```

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
/// When the `heap` feature is enabled, this allocator captures
/// allocation and deallocation events along with stack traces.
/// When disabled, it's a zero-cost passthrough to the system allocator.
pub struct ProfilingAllocator;

#[cfg(not(feature = "heap"))]
mod disabled {
    use super::ProfilingAllocator;
    use core::alloc::{GlobalAlloc, Layout};

    unsafe impl GlobalAlloc for ProfilingAllocator {
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
    use super::{ProfilingAllocator, profiling};
    use core::alloc::{GlobalAlloc, Layout};

    unsafe impl GlobalAlloc for ProfilingAllocator {
        #[inline]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { libc::malloc(layout.size()) as *mut u8 };
            if !ptr.is_null() {
                profiling::record_alloc(ptr, layout.size());
            }
            ptr
        }

        #[inline]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            profiling::record_dealloc(ptr, layout.size());
            unsafe { libc::free(ptr as *mut libc::c_void) }
        }

        #[inline]
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            profiling::record_dealloc(ptr, layout.size());
            let new_ptr = unsafe { libc::realloc(ptr as *mut libc::c_void, new_size) as *mut u8 };
            if !new_ptr.is_null() {
                profiling::record_alloc(new_ptr, new_size);
            }
            new_ptr
        }

        #[inline]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { libc::calloc(1, layout.size()) as *mut u8 };
            if !ptr.is_null() {
                profiling::record_alloc(ptr, layout.size());
            }
            ptr
        }
    }
}
