//! Shared memory trace sampler - reads from rsprof-trace's ring buffer
//!
//! This sampler reads both CPU and heap events from the shared memory ring buffer
//! populated by the rsprof-trace crate.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Maximum stack depth (must match rsprof-trace)
const MAX_STACK_DEPTH: usize = 64;

/// Ring buffer size (must match rsprof-trace)
const RING_BUFFER_SIZE: usize = 64 * 1024;

/// Shared memory path (must match rsprof-trace)
const SHM_PATH: &str = "/rsprof-trace";

/// Magic number for validation (must match rsprof-trace v2)
const MAGIC: u64 = 0x5253_5052_4F46_5452; // "RSPROFTR"

/// Event types (must match rsprof-trace)
const EVENT_TYPE_ALLOC: u8 = 1;
const EVENT_TYPE_DEALLOC: u8 = 2;
const EVENT_TYPE_CPU_SAMPLE: u8 = 3;

/// Get aggregation key from stack.
/// Skip the first 6 frames (allocator/profiler internals), then hash the next 6 frames
/// to differentiate allocation sites while capturing user code context.
///
/// Using more frames in the key helps differentiate allocations that go through
/// the same library code but originate from different user code paths.
#[inline]
fn stack_key(stack: &[u64]) -> u64 {
    // Skip first 6 frames, hash next 6 frames for better differentiation
    let mut key = 0u64;
    for &addr in stack.iter().skip(6).take(6) {
        key ^= addr;
        key = key.wrapping_mul(0x100000001b3);
    }
    key
}

/// Ring buffer header (must match rsprof-trace)
#[repr(C)]
struct RingBufferHeader {
    magic: u64,
    version: u32,
    capacity: u32,
    write_index: AtomicUsize,
    pid: u32,
    _reserved: u32,
}

/// Trace event from shared memory (must match rsprof-trace)
#[repr(C)]
#[derive(Clone, Copy)]
struct ShmTraceEvent {
    event_type: u8,
    _reserved: [u8; 7],
    ptr: u64,
    size: u64,
    timestamp: u64,
    stack_depth: u32,
    _reserved2: u32,
    stack: [u64; MAX_STACK_DEPTH],
}

/// Stats per callsite (for heap profiling)
#[derive(Debug, Clone, Default)]
pub struct HeapStats {
    pub live_bytes: i64,
    pub total_allocs: u64,
    pub total_frees: u64,
    pub total_alloc_bytes: u64,
    pub total_free_bytes: u64,
}

/// CPU sample data
#[derive(Debug, Clone)]
pub struct CpuSample {
    pub timestamp: u64,
    pub stack: Vec<u64>,
}

/// Event from ring buffer
#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub timestamp: u64,
    pub event_type: TraceEventType,
    pub ptr: u64,
    pub size: i64,
    pub stack: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceEventType {
    Alloc,
    Dealloc,
    CpuSample,
}

/// Shared memory trace sampler
pub struct ShmHeapSampler {
    /// Memory-mapped region
    mmap: *mut u8,
    mmap_size: usize,
    /// Last read index
    last_read_index: usize,
    /// Target PID
    #[allow(dead_code)]
    target_pid: u32,
    /// Collected heap stats by first stack frame
    heap_stats: HashMap<u64, HeapStats>,
    /// Live allocations: ptr -> (size, stack)
    live_allocs: HashMap<u64, (u64, Vec<u64>)>,
    /// Collected CPU samples
    cpu_samples: Vec<CpuSample>,
}

// Safety: The mmap pointer is only accessed through &self or &mut self
unsafe impl Send for ShmHeapSampler {}
unsafe impl Sync for ShmHeapSampler {}

impl ShmHeapSampler {
    /// Create a new shared memory trace sampler
    ///
    /// # Arguments
    /// * `pid` - Target process ID (used to validate the shared memory)
    /// * `_exe_path` - Unused, kept for API compatibility
    pub fn new(pid: u32, _exe_path: &Path) -> Result<Self> {
        let shm_path = std::ffi::CString::new(SHM_PATH).unwrap();

        // Calculate size
        let buffer_size = std::mem::size_of::<RingBufferHeader>()
            + RING_BUFFER_SIZE * std::mem::size_of::<ShmTraceEvent>();

        unsafe {
            // Open shared memory
            let fd = libc::shm_open(shm_path.as_ptr(), libc::O_RDONLY, 0);

            if fd < 0 {
                return Err(Error::Bpf(format!(
                    "Failed to open shared memory '{}'. Is the target app using rsprof-trace with profiling feature?",
                    SHM_PATH
                )));
            }

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
                return Err(Error::Bpf("Failed to map shared memory".to_string()));
            }

            let mmap = ptr as *mut u8;

            // Validate header
            let header = &*(mmap as *const RingBufferHeader);

            if header.magic != MAGIC {
                libc::munmap(ptr, buffer_size);
                return Err(Error::Bpf(format!(
                    "Invalid shared memory magic: expected 0x{:x}, got 0x{:x}",
                    MAGIC, header.magic
                )));
            }

            if header.pid != pid {
                eprintln!(
                    "[WARN] Shared memory PID ({}) doesn't match target PID ({})",
                    header.pid, pid
                );
            }

            let write_index = header.write_index.load(Ordering::Acquire);

            Ok(ShmHeapSampler {
                mmap,
                mmap_size: buffer_size,
                last_read_index: write_index, // Start from current position
                target_pid: pid,
                heap_stats: HashMap::new(),
                live_allocs: HashMap::new(),
                cpu_samples: Vec::new(),
            })
        }
    }

    /// Read current heap stats
    pub fn read_stats(&self) -> HashMap<u64, HeapStats> {
        self.heap_stats.clone()
    }

    /// Read inline stacks from live allocations
    /// Uses a hash of the first few stack frames as the key to distinguish
    /// different call sites that share the same immediate return address.
    pub fn read_inline_stacks(&self) -> HashMap<u64, Vec<u64>> {
        let mut result = HashMap::new();
        for (_, (_, stack)) in &self.live_allocs {
            if !stack.is_empty() {
                let key = stack_key(stack);
                result.entry(key).or_insert_with(|| stack.clone());
            }
        }
        result
    }

    /// Read collected CPU samples and clear the buffer
    pub fn read_cpu_samples(&mut self) -> Vec<CpuSample> {
        std::mem::take(&mut self.cpu_samples)
    }

    /// Poll events from the ring buffer
    pub fn poll_events(&mut self, _timeout: Duration) -> Vec<TraceEvent> {
        let mut events = Vec::new();

        unsafe {
            let header = &*(self.mmap as *const RingBufferHeader);
            let events_start = self.mmap.add(std::mem::size_of::<RingBufferHeader>());

            let current_write = header.write_index.load(Ordering::Acquire);

            // Handle wraparound - only process up to RING_BUFFER_SIZE events
            let events_to_read = if current_write >= self.last_read_index {
                current_write - self.last_read_index
            } else {
                // Wrapped around
                current_write + RING_BUFFER_SIZE - self.last_read_index
            };

            // Limit to avoid reading stale data after wraparound
            let events_to_read = events_to_read.min(RING_BUFFER_SIZE);

            for i in 0..events_to_read {
                let index = (self.last_read_index + i) % RING_BUFFER_SIZE;
                let event_ptr = events_start.add(index * std::mem::size_of::<ShmTraceEvent>())
                    as *const ShmTraceEvent;

                let shm_event = &*event_ptr;

                // Convert stack to Vec, filtering zeros
                let stack: Vec<u64> = shm_event.stack[..shm_event.stack_depth as usize]
                    .iter()
                    .copied()
                    .filter(|&addr| addr != 0)
                    .collect();

                let event_type = match shm_event.event_type {
                    EVENT_TYPE_ALLOC => TraceEventType::Alloc,
                    EVENT_TYPE_DEALLOC => TraceEventType::Dealloc,
                    EVENT_TYPE_CPU_SAMPLE => TraceEventType::CpuSample,
                    _ => continue,
                };

                // Process event based on type
                match event_type {
                    TraceEventType::Alloc => {
                        if !stack.is_empty() {
                            let key = stack_key(&stack);
                            let stats = self.heap_stats.entry(key).or_default();
                            stats.live_bytes += shm_event.size as i64;
                            stats.total_allocs += 1;
                            stats.total_alloc_bytes += shm_event.size;
                            self.live_allocs
                                .insert(shm_event.ptr, (shm_event.size, stack.clone()));
                        }
                    }
                    TraceEventType::Dealloc => {
                        if let Some((size, old_stack)) = self.live_allocs.remove(&shm_event.ptr)
                            && !old_stack.is_empty()
                        {
                            let key = stack_key(&old_stack);
                            if let Some(stats) = self.heap_stats.get_mut(&key) {
                                stats.live_bytes -= size as i64;
                                stats.total_frees += 1;
                                stats.total_free_bytes += size;
                            }
                        }
                    }
                    TraceEventType::CpuSample => {
                        // Store CPU sample
                        self.cpu_samples.push(CpuSample {
                            timestamp: shm_event.timestamp,
                            stack: stack.clone(),
                        });
                    }
                }

                events.push(TraceEvent {
                    timestamp: shm_event.timestamp,
                    ptr: shm_event.ptr,
                    size: shm_event.size as i64,
                    event_type,
                    stack,
                });
            }

            self.last_read_index = current_write;
        }

        events
    }

    /// Get the target PID from shared memory
    pub fn shm_pid(&self) -> u32 {
        unsafe {
            let header = &*(self.mmap as *const RingBufferHeader);
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
