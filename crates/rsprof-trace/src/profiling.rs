//! Profiling implementation - aggregated callsite stats for CPU and heap.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Maximum stack depth to capture
const MAX_STACK_DEPTH: usize = 64;

/// Number of callsite stats slots
const CALLSITE_CAPACITY: usize = 8192;

/// Number of allocation tracking slots
const ALLOC_TABLE_CAPACITY: usize = 256 * 1024;

/// Tombstone marker for deleted entries (allows continued probing)
const TOMBSTONE: u64 = u64::MAX;

/// Shared memory path
const SHM_PATH: &[u8] = b"/rsprof-trace\0";

/// Magic number for validation
const MAGIC: u64 = 0x5253_5052_4F46_5333; // "RSPROFS3" (stats v3)

/// Version number
const VERSION: u32 = 3;

/// Aggregated stats per callsite
#[repr(C)]
pub struct CallsiteStats {
    /// Callsite hash (0 = unused slot)
    pub hash: AtomicU64,
    /// Total allocation count
    pub alloc_count: AtomicU64,
    /// Total allocated bytes
    pub alloc_bytes: AtomicU64,
    /// Total free count
    pub free_count: AtomicU64,
    /// Total freed bytes
    pub free_bytes: AtomicU64,
    /// CPU sample count
    pub cpu_samples: AtomicU64,
    /// Stack depth
    pub stack_depth: AtomicU32,
    /// Reserved for alignment
    pub _reserved: u32,
    /// Stack trace (stored once per callsite)
    pub stack: [AtomicU64; MAX_STACK_DEPTH],
}

/// Allocation tracking entry for dealloc attribution
#[repr(C)]
pub struct AllocEntry {
    /// Pointer address (0 = empty slot)
    pub ptr: AtomicU64,
    /// Allocation size
    pub size: AtomicU64,
    /// Callsite hash
    pub callsite_hash: AtomicU64,
}

/// Shared memory header
#[repr(C)]
pub struct StatsHeader {
    /// Magic number for validation
    pub magic: u64,
    /// Version number
    pub version: u32,
    /// Callsite table capacity
    pub callsite_capacity: u32,
    /// Alloc table capacity
    pub alloc_table_capacity: u32,
    /// Process ID
    pub pid: u32,
}

/// Global state
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static IN_SIGNAL_HANDLER: AtomicBool = AtomicBool::new(false);
static mut SHM_BASE: *mut u8 = core::ptr::null_mut();

/// Get pointer to the header
#[inline]
fn get_header() -> *mut StatsHeader {
    unsafe { SHM_BASE as *mut StatsHeader }
}

/// Get pointer to callsite stats array
#[inline]
fn get_callsites() -> *mut CallsiteStats {
    unsafe { SHM_BASE.add(core::mem::size_of::<StatsHeader>()) as *mut CallsiteStats }
}

/// Get pointer to alloc table array
#[inline]
fn get_alloc_table() -> *mut AllocEntry {
    let callsites_size = CALLSITE_CAPACITY * core::mem::size_of::<CallsiteStats>();
    unsafe {
        SHM_BASE
            .add(core::mem::size_of::<StatsHeader>())
            .add(callsites_size) as *mut AllocEntry
    }
}

/// Check if shared memory is initialized
#[inline]
fn shm_ready() -> bool {
    unsafe { !SHM_BASE.is_null() }
}

/// Compute callsite hash from stack for heap events.
/// Skip first 4 frames (allocator internals), hash next 8 frames.
#[inline]
fn stack_key_heap(stack: &[u64], depth: u32) -> u64 {
    let mut key = 0u64;
    let skip = 4.min(depth as usize);
    let take = 8.min((depth as usize).saturating_sub(skip));

    for i in 0..take {
        let addr = stack[skip + i];
        key ^= addr;
        key = key.wrapping_mul(0x100000001b3);
    }

    // Ensure non-zero (0 means empty slot)
    if key == 0 {
        key = 1;
    }
    key
}

/// Compute callsite hash from stack for CPU samples.
/// No skip needed - CPU stacks start with the interrupted PC.
#[inline]
fn stack_key_cpu(stack: &[u64], depth: u32) -> u64 {
    let mut key = 0u64;
    let take = 6.min(depth as usize);

    for i in 0..take {
        let addr = stack[i];
        key ^= addr;
        key = key.wrapping_mul(0x100000001b3);
    }

    // Ensure non-zero (0 means empty slot)
    if key == 0 {
        key = 1;
    }
    key
}

/// Find or create a callsite entry. Returns pointer to the CallsiteStats.
#[inline]
fn find_or_create_callsite(
    hash: u64,
    stack: &[u64; MAX_STACK_DEPTH],
    depth: u32,
) -> *mut CallsiteStats {
    let callsites = get_callsites();
    let mut idx = (hash as usize) % CALLSITE_CAPACITY;

    for _ in 0..CALLSITE_CAPACITY {
        let entry = unsafe { callsites.add(idx) };
        let stored_hash = unsafe { (*entry).hash.load(Ordering::Acquire) };

        if stored_hash == hash {
            // Found existing entry
            return entry;
        }

        if stored_hash == 0 {
            // Empty slot - try to claim it
            if unsafe {
                (*entry)
                    .hash
                    .compare_exchange(0, hash, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            } {
                // Successfully claimed - store the stack
                unsafe {
                    (*entry).stack_depth.store(depth, Ordering::Relaxed);
                    for i in 0..(depth as usize).min(MAX_STACK_DEPTH) {
                        (*entry).stack[i].store(stack[i], Ordering::Relaxed);
                    }
                }
                return entry;
            }

            // Another thread claimed it, re-check
            let new_hash = unsafe { (*entry).hash.load(Ordering::Acquire) };
            if new_hash == hash {
                return entry;
            }
        }

        // Linear probe
        idx = (idx + 1) % CALLSITE_CAPACITY;
    }

    // Table full - return first slot as fallback (will aggregate there)
    callsites
}

/// Find a callsite by hash only (for dealloc attribution)
#[inline]
fn find_callsite(hash: u64) -> *mut CallsiteStats {
    let callsites = get_callsites();
    let mut idx = (hash as usize) % CALLSITE_CAPACITY;

    for _ in 0..CALLSITE_CAPACITY {
        let entry = unsafe { callsites.add(idx) };
        let stored_hash = unsafe { (*entry).hash.load(Ordering::Acquire) };

        if stored_hash == hash {
            return entry;
        }

        if stored_hash == 0 {
            // Not found
            return core::ptr::null_mut();
        }

        idx = (idx + 1) % CALLSITE_CAPACITY;
    }

    core::ptr::null_mut()
}

/// Track an allocation in the alloc table
#[inline]
fn track_alloc(ptr: u64, size: u64, callsite_hash: u64) {
    let alloc_table = get_alloc_table();
    // Use pointer bits for better distribution (skip low bits which are often 0)
    let mut idx = ((ptr >> 4) as usize) % ALLOC_TABLE_CAPACITY;

    for _ in 0..1024 {
        // Limited probing to avoid long searches
        let entry = unsafe { alloc_table.add(idx) };
        let stored_ptr = unsafe { (*entry).ptr.load(Ordering::Acquire) };

        // Can claim empty slot (0) or tombstone (deleted)
        if stored_ptr == 0 || stored_ptr == TOMBSTONE {
            if unsafe {
                (*entry)
                    .ptr
                    .compare_exchange(stored_ptr, ptr, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            } {
                unsafe {
                    (*entry).size.store(size, Ordering::Relaxed);
                    (*entry)
                        .callsite_hash
                        .store(callsite_hash, Ordering::Release);
                }
                return;
            }
            // CAS failed, another thread took this slot - continue probing
        }

        idx = (idx + 1) % ALLOC_TABLE_CAPACITY;
    }

    // Table full or too much probing - drop this allocation's tracking
}

/// Untrack an allocation, returning (size, callsite_hash) if found
#[inline]
fn untrack_alloc(ptr: u64) -> Option<(u64, u64)> {
    let alloc_table = get_alloc_table();
    let mut idx = ((ptr >> 4) as usize) % ALLOC_TABLE_CAPACITY;

    for _ in 0..1024 {
        let entry = unsafe { alloc_table.add(idx) };
        let stored_ptr = unsafe { (*entry).ptr.load(Ordering::Acquire) };

        if stored_ptr == ptr {
            let size = unsafe { (*entry).size.load(Ordering::Relaxed) };
            let callsite_hash = unsafe { (*entry).callsite_hash.load(Ordering::Acquire) };
            // Mark as tombstone (not 0!) to allow continued probing
            unsafe { (*entry).ptr.store(TOMBSTONE, Ordering::Release) };
            return Some((size, callsite_hash));
        }

        if stored_ptr == 0 {
            // Empty slot means not found (allocation wasn't tracked or before profiling started)
            return None;
        }

        // Tombstone - continue probing
        idx = (idx + 1) % ALLOC_TABLE_CAPACITY;
    }

    None
}

/// Initialize the profiler - sets up shared memory
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    unsafe {
        // Calculate shared memory size
        let header_size = core::mem::size_of::<StatsHeader>();
        let callsites_size = CALLSITE_CAPACITY * core::mem::size_of::<CallsiteStats>();
        let alloc_table_size = ALLOC_TABLE_CAPACITY * core::mem::size_of::<AllocEntry>();
        let total_size = header_size + callsites_size + alloc_table_size;

        // Remove any existing shared memory to ensure fresh start
        libc::shm_unlink(SHM_PATH.as_ptr() as *const libc::c_char);

        // Create new shared memory
        let fd = libc::shm_open(
            SHM_PATH.as_ptr() as *const libc::c_char,
            libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
            0o666,
        );
        if fd < 0 {
            INITIALIZED.store(false, Ordering::SeqCst);
            return;
        }

        // Set size (new file will be zero-filled)
        if libc::ftruncate(fd, total_size as libc::off_t) < 0 {
            libc::close(fd);
            INITIALIZED.store(false, Ordering::SeqCst);
            return;
        }

        // Map into memory
        let ptr = libc::mmap(
            core::ptr::null_mut(),
            total_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        libc::close(fd);

        if ptr == libc::MAP_FAILED {
            INITIALIZED.store(false, Ordering::SeqCst);
            return;
        }

        SHM_BASE = ptr as *mut u8;

        // Explicitly zero the entire region to be safe
        core::ptr::write_bytes(SHM_BASE, 0, total_size);

        // Initialize header
        let header = get_header();
        (*header).magic = MAGIC;
        (*header).version = VERSION;
        (*header).callsite_capacity = CALLSITE_CAPACITY as u32;
        (*header).alloc_table_capacity = ALLOC_TABLE_CAPACITY as u32;
        (*header).pid = libc::getpid() as u32;

        // Zero-initialize tables (mmap may already be zeroed, but be explicit)
        // Callsites and alloc table use 0 as "empty" marker
    }
}

/// Capture stack trace using frame pointers
#[inline(never)]
fn capture_stack(stack: &mut [u64; MAX_STACK_DEPTH]) -> u32 {
    capture_stack_from_fp(stack, core::ptr::null())
}

/// Capture stack trace by walking frame pointers
#[inline(never)]
fn capture_stack_from_fp(stack: &mut [u64; MAX_STACK_DEPTH], start_fp: *const usize) -> u32 {
    let mut depth = 0u32;

    unsafe {
        // Get starting frame pointer
        let mut fp: *const usize = if start_fp.is_null() {
            let current_fp: *const usize;
            core::arch::asm!(
                "mov {}, rbp",
                out(reg) current_fp,
                options(nomem, nostack, preserves_flags)
            );
            current_fp
        } else {
            start_fp
        };

        // Walk the stack using frame pointers
        while !fp.is_null() && depth < MAX_STACK_DEPTH as u32 {
            // Validate frame pointer alignment
            if (fp as usize) & 0x7 != 0 {
                break;
            }

            // Bounds check
            let fp_val = fp as usize;
            if !(0x1000..=0x7fff_ffff_ffff).contains(&fp_val) {
                break;
            }

            // Read return address at [fp + 8]
            let ret_addr = *fp.add(1);
            if ret_addr == 0 {
                break;
            }

            stack[depth as usize] = ret_addr as u64;
            depth += 1;

            // Move to next frame (saved RBP is at [fp])
            let next_fp = *fp as *const usize;
            if next_fp <= fp {
                break;
            }
            fp = next_fp;
        }
    }

    depth
}

// =============================================================================
// Heap profiling (conditional on "heap" feature)
// =============================================================================

/// Record an allocation event
#[cfg(feature = "heap")]
#[inline(never)]
pub fn record_alloc(ptr: *mut u8, size: usize) {
    // Don't record allocations from within signal handler
    if IN_SIGNAL_HANDLER.load(Ordering::Relaxed) {
        return;
    }

    // Ensure initialized
    if !INITIALIZED.load(Ordering::Relaxed) {
        init();
    }

    if !shm_ready() {
        return;
    }

    // Capture stack and compute hash
    let mut stack = [0u64; MAX_STACK_DEPTH];
    let depth = capture_stack(&mut stack);
    let hash = stack_key_heap(&stack, depth);

    // Find or create callsite, update stats
    let callsite = find_or_create_callsite(hash, &stack, depth);
    unsafe {
        (*callsite).alloc_count.fetch_add(1, Ordering::Relaxed);
        (*callsite)
            .alloc_bytes
            .fetch_add(size as u64, Ordering::Relaxed);
    }

    // Track allocation for later dealloc attribution
    track_alloc(ptr as u64, size as u64, hash);
}

/// Record a deallocation event
#[cfg(feature = "heap")]
#[inline(never)]
pub fn record_dealloc(ptr: *mut u8, _size: usize) {
    // Don't record deallocations from within signal handler
    if IN_SIGNAL_HANDLER.load(Ordering::Relaxed) {
        return;
    }

    // Can't dealloc if never initialized
    if !INITIALIZED.load(Ordering::Relaxed) || !shm_ready() {
        return;
    }

    // Look up the allocation to get size and callsite
    if let Some((size, callsite_hash)) = untrack_alloc(ptr as u64) {
        // Find the callsite and update free stats
        let callsite = find_callsite(callsite_hash);
        if !callsite.is_null() {
            unsafe {
                (*callsite).free_count.fetch_add(1, Ordering::Relaxed);
                (*callsite).free_bytes.fetch_add(size, Ordering::Relaxed);
            }
        }
    }
}

// Stubs when heap feature is disabled
#[cfg(not(feature = "heap"))]
#[inline]
pub fn record_alloc(_ptr: *mut u8, _size: usize) {}

#[cfg(not(feature = "heap"))]
#[inline]
pub fn record_dealloc(_ptr: *mut u8, _size: usize) {}

// =============================================================================
// CPU profiling (conditional on "cpu" feature)
// =============================================================================

#[cfg(feature = "cpu")]
mod cpu_profiling {
    use super::*;

    /// Default sampling frequency in Hz
    const DEFAULT_FREQ_HZ: u32 = 99;

    /// Signal handler for CPU sampling
    extern "C" fn cpu_sample_handler(
        _sig: libc::c_int,
        _info: *mut libc::siginfo_t,
        ucontext: *mut libc::c_void,
    ) {
        // Prevent reentrant calls
        if IN_SIGNAL_HANDLER.swap(true, Ordering::SeqCst) {
            return;
        }

        if !shm_ready() {
            IN_SIGNAL_HANDLER.store(false, Ordering::SeqCst);
            return;
        }

        // Extract the interrupted registers from the ucontext
        let (rip, start_fp) = if !ucontext.is_null() {
            unsafe {
                let uc = ucontext as *const libc::ucontext_t;
                const REG_RIP: usize = 16;
                const REG_RBP: usize = 10;
                let rip = (*uc).uc_mcontext.gregs[REG_RIP] as u64;
                let rbp = (*uc).uc_mcontext.gregs[REG_RBP] as usize;
                (rip, rbp as *const usize)
            }
        } else {
            (0, core::ptr::null())
        };

        // Build stack with RIP as first frame
        let mut stack = [0u64; MAX_STACK_DEPTH];
        let mut depth = 0u32;

        if rip != 0 {
            stack[0] = rip;
            depth = 1;
        }

        // Walk the rest of the stack
        if !start_fp.is_null() {
            let mut fp = start_fp;

            while !fp.is_null() && (depth as usize) < MAX_STACK_DEPTH {
                if (fp as usize) & 0x7 != 0 {
                    break;
                }
                let fp_val = fp as usize;
                if !(0x1000..=0x7fff_ffff_ffff).contains(&fp_val) {
                    break;
                }

                let ret_addr = unsafe { *fp.add(1) };
                if ret_addr == 0 {
                    break;
                }

                stack[depth as usize] = ret_addr as u64;
                depth += 1;

                let next_fp = unsafe { *fp as *const usize };
                if next_fp <= fp {
                    break;
                }
                fp = next_fp;
            }
        }

        // Compute callsite hash and update stats
        let hash = stack_key_cpu(&stack, depth);
        let callsite = find_or_create_callsite(hash, &stack, depth);
        unsafe { (*callsite).cpu_samples.fetch_add(1, Ordering::Relaxed) };

        IN_SIGNAL_HANDLER.store(false, Ordering::SeqCst);
    }

    /// Start CPU profiling with timer-based sampling
    pub fn start_cpu_profiling(freq_hz: u32) {
        // Ensure initialized
        if !INITIALIZED.load(Ordering::Relaxed) {
            init();
        }

        unsafe {
            // Set up signal handler for SIGPROF with SA_SIGINFO
            let mut sa: libc::sigaction = core::mem::zeroed();
            sa.sa_sigaction = cpu_sample_handler as *const () as usize;
            sa.sa_flags = libc::SA_RESTART | libc::SA_SIGINFO;
            libc::sigemptyset(&mut sa.sa_mask);

            if libc::sigaction(libc::SIGPROF, &sa, core::ptr::null_mut()) < 0 {
                return;
            }

            // Set up interval timer
            let freq = if freq_hz == 0 {
                DEFAULT_FREQ_HZ
            } else {
                freq_hz
            };
            let interval_usec = 1_000_000 / freq as i64;

            let timer = libc::itimerval {
                it_interval: libc::timeval {
                    tv_sec: 0,
                    tv_usec: interval_usec,
                },
                it_value: libc::timeval {
                    tv_sec: 0,
                    tv_usec: interval_usec,
                },
            };

            libc::setitimer(libc::ITIMER_PROF, &timer, core::ptr::null_mut());
        }
    }

    /// Stop CPU profiling
    pub fn stop_cpu_profiling() {
        unsafe {
            // Disable timer
            let timer = libc::itimerval {
                it_interval: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                it_value: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
            };
            libc::setitimer(libc::ITIMER_PROF, &timer, core::ptr::null_mut());

            // Reset signal handler
            let mut sa: libc::sigaction = core::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigaction(libc::SIGPROF, &sa, core::ptr::null_mut());
        }
    }
}

#[cfg(feature = "cpu")]
pub use cpu_profiling::{start_cpu_profiling, stop_cpu_profiling};

// Stubs when cpu feature is disabled
#[cfg(not(feature = "cpu"))]
pub fn start_cpu_profiling(_freq_hz: u32) {}

#[cfg(not(feature = "cpu"))]
pub fn stop_cpu_profiling() {}
