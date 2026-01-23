//! Shared memory stats reader - reads aggregated callsite stats from rsprof-trace.
//!
//! This reader reads pre-aggregated CPU and heap stats from shared memory
//! populated by the rsprof-trace crate. No event processing needed.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Maximum stack depth (must match rsprof-trace)
const MAX_STACK_DEPTH: usize = 64;

/// Callsite table capacity (must match rsprof-trace)
const CALLSITE_CAPACITY: usize = 8192;

/// Shared memory path (must match rsprof-trace)
const SHM_PATH: &str = "/rsprof-trace";

/// Magic number for validation (must match rsprof-trace v3)
const MAGIC: u64 = 0x5253_5052_4F46_5333; // "RSPROFS3"

/// Shared memory header (must match rsprof-trace)
#[repr(C)]
struct StatsHeader {
    magic: u64,
    version: u32,
    callsite_capacity: u32,
    alloc_table_capacity: u32,
    pid: u32,
}

/// Callsite stats (must match rsprof-trace)
#[repr(C)]
struct ShmCallsiteStats {
    hash: AtomicU64,
    alloc_count: AtomicU64,
    alloc_bytes: AtomicU64,
    free_count: AtomicU64,
    free_bytes: AtomicU64,
    cpu_samples: AtomicU64,
    stack_depth: AtomicU32,
    _reserved: u32,
    stack: [AtomicU64; MAX_STACK_DEPTH],
}

/// Stats per callsite (public API)
#[derive(Debug, Clone, Default)]
pub struct HeapStats {
    pub live_bytes: i64,
    pub total_allocs: u64,
    pub total_frees: u64,
    pub total_alloc_bytes: u64,
    pub total_free_bytes: u64,
}

/// CPU sample data (for compatibility)
#[derive(Debug, Clone)]
pub struct CpuSample {
    pub timestamp: u64,
    pub stack: Vec<u64>,
}

/// Snapshot of a callsite's stats
#[derive(Debug, Clone)]
pub struct CallsiteSnapshot {
    pub hash: u64,
    pub alloc_count: u64,
    pub alloc_bytes: u64,
    pub free_count: u64,
    pub free_bytes: u64,
    pub cpu_samples: u64,
    pub stack: Vec<u64>,
}

/// Event types for compatibility with existing code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceEventType {
    Alloc,
    Dealloc,
    CpuSample,
}

/// TraceEvent for compatibility (not used in new implementation)
#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub timestamp: u64,
    pub event_type: TraceEventType,
    pub ptr: u64,
    pub size: i64,
    pub stack: Vec<u64>,
}

/// Shared memory stats reader
pub struct ShmHeapSampler {
    /// Memory-mapped region
    mmap: *mut u8,
    mmap_size: usize,
    /// Target PID
    #[allow(dead_code)]
    target_pid: u32,
    /// Previous CPU sample counts per callsite (for computing deltas)
    prev_cpu_counts: HashMap<u64, u64>,
}

// Safety: The mmap pointer is only accessed through &self or &mut self
unsafe impl Send for ShmHeapSampler {}
unsafe impl Sync for ShmHeapSampler {}

impl ShmHeapSampler {
    /// Create a new shared memory stats reader
    pub fn new(pid: u32, _exe_path: &Path) -> Result<Self> {
        let shm_path = std::ffi::CString::new(SHM_PATH).unwrap();

        unsafe {
            // Open shared memory
            let fd = libc::shm_open(shm_path.as_ptr(), libc::O_RDONLY, 0);

            if fd < 0 {
                return Err(Error::Sampler(format!(
                    "Failed to open shared memory '{}'. Is the target app using rsprof-trace with profiling feature?",
                    SHM_PATH
                )));
            }

            // Get the size from fstat
            let mut stat: libc::stat = std::mem::zeroed();
            if libc::fstat(fd, &mut stat) < 0 {
                libc::close(fd);
                return Err(Error::Sampler("Failed to stat shared memory".to_string()));
            }
            let buffer_size = stat.st_size as usize;

            // Map into memory
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                buffer_size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );

            libc::close(fd);

            if ptr == libc::MAP_FAILED {
                return Err(Error::Sampler("Failed to map shared memory".to_string()));
            }

            let mmap = ptr as *mut u8;

            // Validate header
            let header = &*(mmap as *const StatsHeader);

            if header.magic != MAGIC {
                libc::munmap(ptr, buffer_size);
                return Err(Error::Sampler(format!(
                    "Invalid shared memory magic: expected 0x{:x}, got 0x{:x}. Make sure rsprof-trace is v3.",
                    MAGIC, header.magic
                )));
            }

            if header.pid != pid {
                eprintln!(
                    "[WARN] Shared memory PID ({}) doesn't match target PID ({})",
                    header.pid, pid
                );
            }

            Ok(ShmHeapSampler {
                mmap,
                mmap_size: buffer_size,
                target_pid: pid,
                prev_cpu_counts: HashMap::new(),
            })
        }
    }

    /// Get pointer to callsites array
    unsafe fn get_callsites(&self) -> *const ShmCallsiteStats {
        unsafe { self.mmap.add(std::mem::size_of::<StatsHeader>()) as *const ShmCallsiteStats }
    }

    /// Read current snapshot of all callsites
    pub fn read_snapshot(&self) -> Vec<CallsiteSnapshot> {
        let mut result = Vec::new();

        unsafe {
            let callsites = self.get_callsites();

            for i in 0..CALLSITE_CAPACITY {
                let entry = &*callsites.add(i);
                let hash = entry.hash.load(Ordering::Acquire);

                if hash == 0 {
                    continue; // Empty slot
                }

                let stack_depth = entry.stack_depth.load(Ordering::Relaxed) as usize;
                let stack: Vec<u64> = entry.stack[..stack_depth.min(MAX_STACK_DEPTH)]
                    .iter()
                    .map(|a| a.load(Ordering::Relaxed))
                    .filter(|&addr| addr != 0)
                    .collect();

                result.push(CallsiteSnapshot {
                    hash,
                    alloc_count: entry.alloc_count.load(Ordering::Relaxed),
                    alloc_bytes: entry.alloc_bytes.load(Ordering::Relaxed),
                    free_count: entry.free_count.load(Ordering::Relaxed),
                    free_bytes: entry.free_bytes.load(Ordering::Relaxed),
                    cpu_samples: entry.cpu_samples.load(Ordering::Relaxed),
                    stack,
                });
            }
        }

        result
    }

    /// Read current heap stats (compatible with old API)
    pub fn read_stats(&self) -> HashMap<u64, HeapStats> {
        let snapshot = self.read_snapshot();
        let mut result = HashMap::new();

        for cs in snapshot {
            if cs.alloc_count > 0 || cs.free_count > 0 {
                result.insert(
                    cs.hash,
                    HeapStats {
                        live_bytes: cs.alloc_bytes as i64 - cs.free_bytes as i64,
                        total_allocs: cs.alloc_count,
                        total_frees: cs.free_count,
                        total_alloc_bytes: cs.alloc_bytes,
                        total_free_bytes: cs.free_bytes,
                    },
                );
            }
        }

        result
    }

    /// Read inline stacks from callsites
    pub fn read_inline_stacks(&self) -> HashMap<u64, Vec<u64>> {
        let snapshot = self.read_snapshot();
        let mut result = HashMap::new();

        for cs in snapshot {
            if !cs.stack.is_empty() {
                result.insert(cs.hash, cs.stack);
            }
        }

        result
    }

    /// Read CPU samples - returns snapshots with cpu_samples > 0
    /// Note: In the new model, we don't have individual samples with timestamps,
    /// just aggregated counts per callsite.
    pub fn read_cpu_samples(&mut self) -> Vec<CpuSample> {
        // For compatibility, return empty - the new model uses read_cpu_stats() instead
        Vec::new()
    }

    /// Read CPU stats per callsite (returns deltas since last read)
    pub fn read_cpu_stats(&mut self) -> HashMap<u64, (u64, Vec<u64>)> {
        let snapshot = self.read_snapshot();
        let mut result = HashMap::new();

        for cs in snapshot {
            if cs.cpu_samples > 0 {
                let prev = self.prev_cpu_counts.get(&cs.hash).copied().unwrap_or(0);
                let delta = cs.cpu_samples.saturating_sub(prev);
                if delta > 0 {
                    result.insert(cs.hash, (delta, cs.stack));
                }
                self.prev_cpu_counts.insert(cs.hash, cs.cpu_samples);
            }
        }

        result
    }

    /// Poll events - for compatibility, computes deltas from snapshots
    pub fn poll_events(&mut self, _timeout: std::time::Duration) -> Vec<TraceEvent> {
        // The new model doesn't have individual events
        // Return empty for compatibility
        Vec::new()
    }

    /// Get the target PID from shared memory
    pub fn shm_pid(&self) -> u32 {
        unsafe {
            let header = &*(self.mmap as *const StatsHeader);
            header.pid
        }
    }
}

impl Drop for ShmHeapSampler {
    fn drop(&mut self) {
        unsafe {
            if !self.mmap.is_null() {
                libc::munmap(self.mmap as *mut libc::c_void, self.mmap_size);
            }
        }
    }
}
