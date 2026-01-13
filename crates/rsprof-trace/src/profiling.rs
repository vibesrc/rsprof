//! Profiling implementation - captures CPU and heap trace events.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Maximum stack depth to capture
const MAX_STACK_DEPTH: usize = 64;

/// Size of the ring buffer (number of events)
const RING_BUFFER_SIZE: usize = 64 * 1024;

/// Shared memory path for the ring buffer
const SHM_PATH: &[u8] = b"/rsprof-trace\0";

/// Event types
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum EventType {
    Alloc = 1,
    Dealloc = 2,
    CpuSample = 3,
}

/// A trace event recorded in the ring buffer
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceEvent {
    /// Event type (alloc/dealloc/cpu)
    pub event_type: u8,
    /// Reserved for alignment
    pub _reserved: [u8; 7],
    /// Pointer address (for heap events) or 0 (for CPU events)
    pub ptr: u64,
    /// Allocation size (for heap events) or 0 (for CPU events)
    pub size: u64,
    /// Timestamp (nanoseconds since process start)
    pub timestamp: u64,
    /// Number of valid stack frames
    pub stack_depth: u32,
    /// Reserved
    pub _reserved2: u32,
    /// Stack trace (instruction pointers)
    pub stack: [u64; MAX_STACK_DEPTH],
}

/// Ring buffer header stored at the start of shared memory
#[repr(C)]
pub struct RingBufferHeader {
    /// Magic number for validation
    pub magic: u64,
    /// Version number
    pub version: u32,
    /// Buffer capacity (number of events)
    pub capacity: u32,
    /// Write index (wraps around)
    pub write_index: AtomicUsize,
    /// Process ID
    pub pid: u32,
    /// Reserved
    pub _reserved: u32,
}

const MAGIC: u64 = 0x5253_5052_4F46_5452; // "RSPROFTR" (trace)
const VERSION: u32 = 2;

/// Global state for the profiler
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static IN_SIGNAL_HANDLER: AtomicBool = AtomicBool::new(false);
static mut RING_BUFFER: *mut u8 = core::ptr::null_mut();
static mut START_TIME: u64 = 0;

/// Binary code segment range (for filtering stack addresses)
static mut CODE_START: u64 = 0;
static mut CODE_END: u64 = 0;

/// Initialize the profiler - sets up shared memory ring buffer
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    unsafe {
        // Get process start time for relative timestamps
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
        START_TIME = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);

        // Detect binary code segment from /proc/self/maps
        // We look for the r-xp mapping (executable) for our binary
        let fd = libc::open(b"/proc/self/maps\0".as_ptr() as *const libc::c_char, libc::O_RDONLY);
        if fd >= 0 {
            let mut buf = [0u8; 8192];
            let n = libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
            libc::close(fd);

            if n > 0 {
                // Parse maps to find executable segment
                // Format: "addr_start-addr_end perms offset dev inode pathname"
                // We want the r-xp line (executable, not writable)
                let mut i = 0;
                while i < n as usize {
                    // Find the start of the address range
                    let mut addr_start = 0u64;
                    let mut addr_end = 0u64;

                    // Parse start address (hex until '-')
                    while i < n as usize && buf[i] != b'-' {
                        let c = buf[i];
                        addr_start = addr_start * 16 + match c {
                            b'0'..=b'9' => (c - b'0') as u64,
                            b'a'..=b'f' => (c - b'a' + 10) as u64,
                            _ => 0,
                        };
                        i += 1;
                    }
                    if i < n as usize { i += 1; } // skip '-'

                    // Parse end address (hex until ' ')
                    while i < n as usize && buf[i] != b' ' {
                        let c = buf[i];
                        addr_end = addr_end * 16 + match c {
                            b'0'..=b'9' => (c - b'0') as u64,
                            b'a'..=b'f' => (c - b'a' + 10) as u64,
                            _ => 0,
                        };
                        i += 1;
                    }
                    if i < n as usize { i += 1; } // skip ' '

                    // Check permissions - looking for "r-xp" (executable)
                    if i + 4 <= n as usize && buf[i] == b'r' && buf[i+2] == b'x' && buf[i+3] == b'p' {
                        // Found executable segment - store it
                        if CODE_START == 0 {
                            CODE_START = addr_start;
                            CODE_END = addr_end;
                        }
                    }

                    // Skip to next line
                    while i < n as usize && buf[i] != b'\n' {
                        i += 1;
                    }
                    if i < n as usize { i += 1; }

                    // Only parse first few lines (our binary's mappings come first)
                    if i > 2000 { break; }
                }
            }
        }

        // Calculate shared memory size
        let buffer_size = core::mem::size_of::<RingBufferHeader>()
            + RING_BUFFER_SIZE * core::mem::size_of::<TraceEvent>();

        // Open/create shared memory
        let fd = libc::shm_open(
            SHM_PATH.as_ptr() as *const libc::c_char,
            libc::O_CREAT | libc::O_RDWR,
            0o666,
        );
        if fd < 0 {
            INITIALIZED.store(false, Ordering::SeqCst);
            return;
        }

        // Set size
        if libc::ftruncate(fd, buffer_size as libc::off_t) < 0 {
            libc::close(fd);
            INITIALIZED.store(false, Ordering::SeqCst);
            return;
        }

        // Map into memory
        let ptr = libc::mmap(
            core::ptr::null_mut(),
            buffer_size,
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

        RING_BUFFER = ptr as *mut u8;

        // Initialize header
        let header = &mut *(RING_BUFFER as *mut RingBufferHeader);
        header.magic = MAGIC;
        header.version = VERSION;
        header.capacity = RING_BUFFER_SIZE as u32;
        header.write_index = AtomicUsize::new(0);
        header.pid = libc::getpid() as u32;
    }
}

/// Get current timestamp relative to process start
#[inline]
fn get_timestamp() -> u64 {
    unsafe {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
        let now = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
        now.saturating_sub(START_TIME)
    }
}

/// Capture stack trace using frame pointers
#[inline]
#[allow(dead_code)]
fn capture_stack(stack: &mut [u64; MAX_STACK_DEPTH]) -> u32 {
    capture_stack_from_fp(stack, core::ptr::null())
}

/// Capture stack trace by walking frame pointers
/// Requires binary to be built with -C force-frame-pointers=yes
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

        // Debug: print first few frames once
        static DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
        let count = DEBUG_COUNT.fetch_add(1, Ordering::Relaxed);
        let do_debug = count == 5000;

        // Print initial RBP and this function's address
        if do_debug {
            // Print RBP
            let mut buf = [0u8; 32];
            buf[0] = b'R';
            buf[1] = b'B';
            buf[2] = b'P';
            buf[3] = b':';
            let rbp_val = fp as u64;
            for i in 0..16 {
                let nibble = ((rbp_val >> ((15 - i) * 4)) & 0xf) as u8;
                buf[4 + i] = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            }
            buf[20] = b'\n';
            let _ = libc::write(2, buf.as_ptr() as _, 21);

            // Print this function's address for reference
            let fn_addr = capture_stack_from_fp as *const () as u64;
            buf[0] = b'F';
            buf[1] = b'N';
            buf[2] = b'A';
            buf[3] = b':';
            for i in 0..16 {
                let nibble = ((fn_addr >> ((15 - i) * 4)) & 0xf) as u8;
                buf[4 + i] = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            }
            buf[20] = b'\n';
            let _ = libc::write(2, buf.as_ptr() as _, 21);
        }

        // Walk the stack using frame pointers
        while !fp.is_null() && depth < MAX_STACK_DEPTH as u32 {
            // Validate frame pointer alignment
            if (fp as usize) & 0x7 != 0 {
                if do_debug { let _ = libc::write(2, b"ALIGN\n".as_ptr() as _, 6); }
                break;
            }

            // Bounds check
            let fp_val = fp as usize;
            if !(0x1000..=0x7fff_ffff_ffff).contains(&fp_val) {
                if do_debug { let _ = libc::write(2, b"BOUNDS\n".as_ptr() as _, 7); }
                break;
            }

            // Read return address at [fp + 8]
            let ret_addr = *fp.add(1);
            if ret_addr == 0 {
                if do_debug { let _ = libc::write(2, b"ZERO\n".as_ptr() as _, 5); }
                break;
            }

            // Debug print frame
            if do_debug && depth < 10 {
                let mut buf = [0u8; 32];
                buf[0] = b'F';
                buf[1] = b'0' + (depth as u8);
                buf[2] = b':';
                for i in 0..16 {
                    let nibble = ((ret_addr >> ((15 - i) * 4)) & 0xf) as u8;
                    buf[3 + i] = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
                }
                buf[19] = b'\n';
                let _ = libc::write(2, buf.as_ptr() as _, 20);
            }

            stack[depth as usize] = ret_addr as u64;
            depth += 1;

            // Move to next frame (saved RBP is at [fp])
            let next_fp = *fp as *const usize;
            if next_fp <= fp {
                if do_debug { let _ = libc::write(2, b"BACKWARDS\n".as_ptr() as _, 10); }
                break;
            }
            fp = next_fp;
        }

        if do_debug {
            let mut buf = [0u8; 16];
            let msg = b"DEPTH:";
            buf[..6].copy_from_slice(msg);
            buf[6] = b'0' + ((depth / 10) as u8);
            buf[7] = b'0' + ((depth % 10) as u8);
            buf[8] = b'\n';
            let _ = libc::write(2, buf.as_ptr() as _, 9);
        }
    }

    depth
}

/// Record an event to the ring buffer
#[inline(never)]
fn record_event_internal(event_type: EventType, ptr: u64, size: u64) {
    record_event_with_fp(event_type, ptr, size, core::ptr::null())
}

/// Record an event with a specific starting frame pointer
#[inline(never)]
fn record_event_with_fp(event_type: EventType, ptr: u64, size: u64, start_fp: *const usize) {
    unsafe {
        if RING_BUFFER.is_null() {
            return;
        }

        let header = &*(RING_BUFFER as *const RingBufferHeader);

        // Get next write slot
        let index = header.write_index.fetch_add(1, Ordering::Relaxed) % RING_BUFFER_SIZE;

        // Calculate event location
        let events_start = RING_BUFFER.add(core::mem::size_of::<RingBufferHeader>());
        let event =
            &mut *(events_start.add(index * core::mem::size_of::<TraceEvent>()) as *mut TraceEvent);

        // Fill in event
        event.event_type = event_type as u8;
        event.ptr = ptr;
        event.size = size;
        event.timestamp = get_timestamp();

        // Capture stack trace from specified frame pointer
        event.stack_depth = capture_stack_from_fp(&mut event.stack, start_fp);
    }
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

    record_event_internal(EventType::Alloc, ptr as u64, size as u64);
}

/// Record a deallocation event
#[cfg(feature = "heap")]
#[inline(never)]
pub fn record_dealloc(ptr: *mut u8, size: usize) {
    // Don't record deallocations from within signal handler
    if IN_SIGNAL_HANDLER.load(Ordering::Relaxed) {
        return;
    }

    // Ensure initialized
    if !INITIALIZED.load(Ordering::Relaxed) {
        init();
    }

    record_event_internal(EventType::Dealloc, ptr as u64, size as u64);
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

    /// Signal handler for CPU sampling with siginfo context
    /// This receives the ucontext which contains the interrupted thread's registers
    extern "C" fn cpu_sample_handler(
        _sig: libc::c_int,
        _info: *mut libc::siginfo_t,
        ucontext: *mut libc::c_void,
    ) {
        // Prevent reentrant calls and heap allocations during signal handling
        if IN_SIGNAL_HANDLER.swap(true, Ordering::SeqCst) {
            return;
        }

        // Extract the interrupted registers from the ucontext
        let (rip, start_fp) = if !ucontext.is_null() {
            unsafe {
                let uc = ucontext as *const libc::ucontext_t;
                // mcontext.gregs indices on x86_64 Linux:
                // REG_RIP = 16, REG_RBP = 10
                const REG_RIP: usize = 16;
                const REG_RBP: usize = 10;
                let rip = (*uc).uc_mcontext.gregs[REG_RIP] as u64;
                let rbp = (*uc).uc_mcontext.gregs[REG_RBP] as usize;
                (rip, rbp as *const usize)
            }
        } else {
            (0, core::ptr::null())
        };

        // Record CPU sample with the interrupted PC as the first frame
        record_cpu_sample_with_context(rip, start_fp);

        IN_SIGNAL_HANDLER.store(false, Ordering::SeqCst);
    }

    /// Record a CPU sample with the interrupted PC and frame pointer
    fn record_cpu_sample_with_context(rip: u64, start_fp: *const usize) {
        unsafe {
            if RING_BUFFER.is_null() {
                return;
            }

            let header = &*(RING_BUFFER as *const RingBufferHeader);

            // Get next write slot
            let index = header.write_index.fetch_add(1, Ordering::Relaxed) % RING_BUFFER_SIZE;

            // Calculate event location
            let events_start = RING_BUFFER.add(core::mem::size_of::<RingBufferHeader>());
            let event = &mut *(events_start.add(index * core::mem::size_of::<TraceEvent>())
                as *mut TraceEvent);

            // Fill in event
            event.event_type = EventType::CpuSample as u8;
            event.ptr = 0;
            event.size = 0;
            event.timestamp = get_timestamp();

            // First frame is the interrupted PC (RIP)
            let mut depth = 0u32;
            if rip != 0 {
                event.stack[0] = rip;
                depth = 1;
            }

            // Then walk the stack from the interrupted frame pointer
            if !start_fp.is_null() {
                let mut fp = start_fp;

                while !fp.is_null() && (depth as usize) < MAX_STACK_DEPTH {
                    // Validate frame pointer
                    if (fp as usize) & 0x7 != 0 {
                        break;
                    }
                    let fp_val = fp as usize;
                    if !(0x1000..=0x7fff_ffff_ffff).contains(&fp_val) {
                        break;
                    }

                    // Read return address (at fp + 8)
                    let ret_addr = *fp.add(1);
                    if ret_addr == 0 {
                        break;
                    }

                    event.stack[depth as usize] = ret_addr as u64;
                    depth += 1;

                    // Get next frame pointer
                    let next_fp = *fp as *const usize;
                    if next_fp <= fp {
                        break;
                    }
                    fp = next_fp;
                }
            }

            event.stack_depth = depth;
        }
    }

    /// Start CPU profiling with timer-based sampling
    pub fn start_cpu_profiling(freq_hz: u32) {
        // Ensure ring buffer is initialized
        if !INITIALIZED.load(Ordering::Relaxed) {
            init();
        }

        unsafe {
            // Set up signal handler for SIGPROF with SA_SIGINFO to get ucontext
            let mut sa: libc::sigaction = core::mem::zeroed();
            sa.sa_sigaction = cpu_sample_handler as usize;
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
