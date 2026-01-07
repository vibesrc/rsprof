use crate::error::{Error, Result};
use libc::{self, c_int, c_ulong, pid_t, syscall, SYS_perf_event_open};
use std::fs;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr;

// perf_event constants (from linux/perf_event.h)
pub const PERF_TYPE_SOFTWARE: u32 = 1;
pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;

pub const PERF_SAMPLE_IP: u64 = 1 << 0;
pub const PERF_SAMPLE_TID: u64 = 1 << 1;
pub const PERF_SAMPLE_TIME: u64 = 1 << 2;

/// perf_event_attr structure
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct PerfEventAttr {
    pub type_: u32,
    pub size: u32,
    pub config: u64,
    pub sample_period_or_freq: u64,
    pub sample_type: u64,
    pub read_format: u64,
    pub flags: u64,
    pub wakeup_events_or_watermark: u32,
    pub bp_type: u32,
    pub config1: u64,
    pub config2: u64,
    pub branch_sample_type: u64,
    pub sample_regs_user: u64,
    pub sample_stack_user: u32,
    pub clockid: i32,
    pub sample_regs_intr: u64,
    pub aux_watermark: u32,
    pub sample_max_stack: u16,
    pub __reserved_2: u16,
    pub aux_sample_size: u32,
    pub __reserved_3: u32,
}

impl PerfEventAttr {
    // Flag bit positions
    const DISABLED_BIT: u64 = 1 << 0;
    #[allow(dead_code)]
    const INHERIT_BIT: u64 = 1 << 1;
    #[allow(dead_code)]
    const EXCLUDE_USER_BIT: u64 = 1 << 4;
    const EXCLUDE_KERNEL_BIT: u64 = 1 << 5;
    const EXCLUDE_HV_BIT: u64 = 1 << 6;
    const FREQ_BIT: u64 = 1 << 10;
    const WATERMARK_BIT: u64 = 1 << 14;

    pub fn new() -> Self {
        PerfEventAttr {
            size: std::mem::size_of::<PerfEventAttr>() as u32,
            ..Default::default()
        }
    }

    pub fn set_disabled(&mut self, val: bool) {
        if val {
            self.flags |= Self::DISABLED_BIT;
        } else {
            self.flags &= !Self::DISABLED_BIT;
        }
    }

    pub fn set_exclude_kernel(&mut self, val: bool) {
        if val {
            self.flags |= Self::EXCLUDE_KERNEL_BIT;
        } else {
            self.flags &= !Self::EXCLUDE_KERNEL_BIT;
        }
    }

    pub fn set_exclude_hv(&mut self, val: bool) {
        if val {
            self.flags |= Self::EXCLUDE_HV_BIT;
        } else {
            self.flags &= !Self::EXCLUDE_HV_BIT;
        }
    }

    pub fn set_freq(&mut self, val: bool) {
        if val {
            self.flags |= Self::FREQ_BIT;
        } else {
            self.flags &= !Self::FREQ_BIT;
        }
    }

    pub fn set_watermark(&mut self, val: bool) {
        if val {
            self.flags |= Self::WATERMARK_BIT;
        } else {
            self.flags &= !Self::WATERMARK_BIT;
        }
    }
}

/// perf_event_mmap_page header structure
#[repr(C)]
pub struct PerfEventMmapPage {
    pub version: u32,
    pub compat_version: u32,
    pub lock: u32,
    pub index: u32,
    pub offset: i64,
    pub time_enabled: u64,
    pub time_running: u64,
    pub capabilities: u64,
    pub pmc_width: u16,
    pub time_shift: u16,
    pub time_mult: u32,
    pub time_offset: u64,
    pub time_zero: u64,
    pub size: u32,
    pub __reserved_1: u32,
    pub time_cycles: u64,
    pub time_mask: u64,
    pub __reserved: [u8; 928],
    pub data_head: u64,
    pub data_tail: u64,
    pub data_offset: u64,
    pub data_size: u64,
    pub aux_head: u64,
    pub aux_tail: u64,
    pub aux_offset: u64,
    pub aux_size: u64,
}

/// perf_event_header for records in the ring buffer
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PerfEventHeader {
    pub type_: u32,
    pub misc: u16,
    pub size: u16,
}

// Record types
pub const PERF_RECORD_SAMPLE: u32 = 9;
#[allow(dead_code)]
pub const PERF_RECORD_LOST: u32 = 2;

/// Wrapper for a perf_event file descriptor
pub struct PerfEvent {
    fd: OwnedFd,
    mmap: *mut u8,
    mmap_size: usize,
    data_size: usize,
}

// SAFETY: The mmap pointer is only used from a single thread
unsafe impl Send for PerfEvent {}

impl PerfEvent {
    /// Open a perf_event for CPU sampling
    pub fn open(pid: pid_t, freq: u64) -> Result<Self> {
        // Check perf_event_paranoid
        check_perf_paranoid()?;

        let mut attr = PerfEventAttr::new();
        attr.type_ = PERF_TYPE_SOFTWARE;
        attr.config = PERF_COUNT_SW_CPU_CLOCK;
        attr.sample_type = PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_TIME;
        attr.sample_period_or_freq = freq;
        attr.set_freq(true);
        attr.set_disabled(true);
        attr.set_exclude_kernel(true);
        attr.set_exclude_hv(true);
        attr.set_watermark(true);
        attr.wakeup_events_or_watermark = 4096; // Wake when 4KB ready

        let fd = unsafe {
            syscall(
                SYS_perf_event_open,
                &attr as *const PerfEventAttr,
                pid,
                -1 as c_int, // any CPU
                -1 as c_int, // no group
                0 as c_ulong,
            )
        };

        if fd < 0 {
            let err = std::io::Error::last_os_error();
            return Err(match err.raw_os_error() {
                Some(libc::EACCES) | Some(libc::EPERM) => {
                    Error::PermissionDenied(format!(
                        "Cannot attach to PID {}. Try: sudo sysctl kernel.perf_event_paranoid=1",
                        pid
                    ))
                }
                Some(libc::ESRCH) => Error::ProcessNotFound(format!("PID {}", pid)),
                _ => Error::PerfEvent(format!("perf_event_open failed: {}", err)),
            });
        }

        let fd = unsafe { OwnedFd::from_raw_fd(fd as c_int) };

        // Memory map the ring buffer
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let data_pages = 64; // 64 pages = 256KB
        let mmap_size = (1 + data_pages) * page_size; // 1 metadata page + data pages
        let data_size = data_pages * page_size;

        let mmap = unsafe {
            libc::mmap(
                ptr::null_mut(),
                mmap_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };

        if mmap == libc::MAP_FAILED {
            return Err(Error::PerfEvent(format!(
                "Failed to mmap perf buffer: {}",
                std::io::Error::last_os_error()
            )));
        }

        // Enable the event
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), 0x2400, 0) }; // PERF_EVENT_IOC_ENABLE
        if ret < 0 {
            unsafe { libc::munmap(mmap, mmap_size) };
            return Err(Error::PerfEvent(format!(
                "Failed to enable perf event: {}",
                std::io::Error::last_os_error()
            )));
        }

        Ok(PerfEvent {
            fd,
            mmap: mmap as *mut u8,
            mmap_size,
            data_size,
        })
    }

    /// Read samples from the ring buffer
    pub fn read_samples(&mut self) -> Vec<u64> {
        let mut samples = Vec::new();

        let header = unsafe { &*(self.mmap as *const PerfEventMmapPage) };
        let data_ptr = unsafe { self.mmap.add(header.data_offset as usize) };

        // Read barrier
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

        let mut tail = header.data_tail;
        let head = header.data_head;

        while tail < head {
            let offset = (tail % self.data_size as u64) as usize;
            let event_header = unsafe { &*(data_ptr.add(offset) as *const PerfEventHeader) };

            if event_header.type_ == PERF_RECORD_SAMPLE {
                // Sample record: header followed by IP (and optionally TID, TIME)
                // We configured PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_TIME
                // Layout: ip, pid, tid, time
                let ip_offset = offset + std::mem::size_of::<PerfEventHeader>();
                let ip_ptr = data_ptr.wrapping_add(ip_offset % self.data_size);
                let ip = unsafe { *(ip_ptr as *const u64) };
                samples.push(ip);
            }

            tail += event_header.size as u64;
        }

        // Update tail pointer
        // Write barrier
        std::sync::atomic::fence(std::sync::atomic::Ordering::Release);

        unsafe {
            let header_mut = &mut *(self.mmap as *mut PerfEventMmapPage);
            header_mut.data_tail = tail;
        }

        samples
    }
}

impl Drop for PerfEvent {
    fn drop(&mut self) {
        unsafe {
            // Disable the event
            libc::ioctl(self.fd.as_raw_fd(), 0x2401, 0); // PERF_EVENT_IOC_DISABLE
            // Unmap
            libc::munmap(self.mmap as *mut libc::c_void, self.mmap_size);
        }
    }
}

/// Check /proc/sys/kernel/perf_event_paranoid
fn check_perf_paranoid() -> Result<()> {
    let path = "/proc/sys/kernel/perf_event_paranoid";
    match fs::read_to_string(path) {
        Ok(content) => {
            let level: i32 = content.trim().parse().unwrap_or(2);
            if level > 1 {
                eprintln!(
                    "Warning: perf_event_paranoid={}, profiling may be restricted.",
                    level
                );
                eprintln!("Consider: sudo sysctl kernel.perf_event_paranoid=1");
            }
            Ok(())
        }
        Err(_) => Ok(()), // File might not exist on some systems
    }
}
