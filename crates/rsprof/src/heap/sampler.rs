use crate::error::{Error, Result};
use libbpf_rs::skel::{OpenSkel, SkelBuilder};
use libbpf_rs::{MapCore, MapFlags, RingBufferBuilder};
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Include the generated skeleton
mod heap_skel {
    include!(concat!(env!("OUT_DIR"), "/heap.skel.rs"));
}

use heap_skel::*;

/// Stats per callsite (mirrors eBPF struct)
#[derive(Debug, Clone, Default)]
pub struct HeapStats {
    pub live_bytes: i64,
    pub total_allocs: u64,
    pub total_frees: u64,
    pub total_alloc_bytes: u64,
    pub total_free_bytes: u64,
}

/// Event from eBPF ring buffer
#[derive(Debug, Clone)]
pub struct HeapEvent {
    pub user_addr: u64,
    pub ptr: u64,
    pub size: i64,
    pub event_type: HeapEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapEventType {
    Alloc,
    Free,
    Realloc,
}

/// Heap profiler using eBPF uprobes
pub struct HeapSampler {
    skel: HeapSkel<'static>,
    #[allow(dead_code)]
    links: Vec<libbpf_rs::Link>,
    events: Arc<Mutex<Vec<HeapEvent>>>,
}

impl HeapSampler {
    /// Create a new heap sampler for a process
    ///
    /// # Arguments
    /// * `pid` - Target process ID
    /// * `exe_path` - Path to the executable (for finding symbols)
    pub fn new(pid: u32, exe_path: &Path) -> Result<Self> {
        // Check if we have CAP_BPF or are root
        check_bpf_permissions()?;

        // Open and load the BPF skeleton
        let skel_builder = HeapSkelBuilder::default();

        // Create open object storage
        let mut open_object = MaybeUninit::uninit();

        let open_skel = skel_builder.open(&mut open_object).map_err(|e| {
            Error::Bpf(format!(
                "Failed to open BPF skeleton: {}. Make sure clang and libbpf-dev are installed.",
                e
            ))
        })?;

        // Load the BPF program first
        let skel = open_skel
            .load()
            .map_err(|e| Error::Bpf(format!("Failed to load BPF program: {}", e)))?;

        // PID filtering disabled - uprobe attachment to specific binary is sufficient
        let key: u32 = 0;
        let pid_zero: u32 = 0;
        skel.maps
            .target_pid_map
            .update(&key.to_ne_bytes(), &pid_zero.to_ne_bytes(), MapFlags::ANY)
            .map_err(|e| Error::Bpf(format!("Failed to set target PID: {}", e)))?;

        // Find the allocator symbols in the target binary
        let symbols = find_allocator_symbols(exe_path)?;

        eprintln!("[DEBUG] Allocator file offsets:");
        eprintln!(
            "  __rust_alloc:   {:?}",
            symbols.rust_alloc.map(|o| format!("0x{:x}", o))
        );
        eprintln!(
            "  __rust_dealloc: {:?}",
            symbols.rust_dealloc.map(|o| format!("0x{:x}", o))
        );
        eprintln!(
            "  __rust_realloc: {:?}",
            symbols.rust_realloc.map(|o| format!("0x{:x}", o))
        );
        eprintln!("  exe_path: {:?}", exe_path);
        eprintln!("  pid: {}", pid);

        let mut links = Vec::new();

        // Attach uprobes/uretprobes
        // Use pid=-1 to trace all processes using this binary (more reliable on some systems)
        if let Some(alloc_offset) = symbols.rust_alloc {
            eprintln!(
                "[DEBUG] Attaching uprobe: offset=0x{:x}, pid=-1 (all), path={:?}",
                alloc_offset, exe_path
            );
            // Entry probe
            let link = skel
                .progs
                .uprobe_rust_alloc
                .attach_uprobe(
                    false, // not retprobe
                    -1,    // all processes
                    exe_path,
                    alloc_offset as usize,
                )
                .map_err(|e| Error::Bpf(format!("Failed to attach alloc uprobe: {}", e)))?;
            links.push(link);

            // Return probe
            let link = skel
                .progs
                .uretprobe_rust_alloc
                .attach_uprobe(
                    true, // retprobe
                    -1,
                    exe_path,
                    alloc_offset as usize,
                )
                .map_err(|e| Error::Bpf(format!("Failed to attach alloc uretprobe: {}", e)))?;
            links.push(link);
        }

        if let Some(dealloc_offset) = symbols.rust_dealloc {
            let link = skel
                .progs
                .uprobe_rust_dealloc
                .attach_uprobe(false, -1, exe_path, dealloc_offset as usize)
                .map_err(|e| Error::Bpf(format!("Failed to attach dealloc uprobe: {}", e)))?;
            links.push(link);
        }

        if let Some(realloc_offset) = symbols.rust_realloc {
            // Entry
            let link = skel
                .progs
                .uprobe_rust_realloc_v2
                .attach_uprobe(false, -1, exe_path, realloc_offset as usize)
                .map_err(|e| Error::Bpf(format!("Failed to attach realloc uprobe: {}", e)))?;
            links.push(link);

            // Return
            let link = skel
                .progs
                .uretprobe_rust_realloc
                .attach_uprobe(true, -1, exe_path, realloc_offset as usize)
                .map_err(|e| Error::Bpf(format!("Failed to attach realloc uretprobe: {}", e)))?;
            links.push(link);
        }

        let events = Arc::new(Mutex::new(Vec::new()));

        // SAFETY: We need 'static lifetime for the skeleton.
        // The MaybeUninit storage is valid for the lifetime of this struct.
        // We ensure the skeleton doesn't outlive our struct.
        let skel = unsafe { std::mem::transmute::<HeapSkel<'_>, HeapSkel<'static>>(skel) };

        Ok(HeapSampler {
            skel,
            links,
            events,
        })
    }

    /// Read current heap stats from BPF maps
    /// Returns map of key_addr -> HeapStats
    pub fn read_stats(&self) -> HashMap<u64, HeapStats> {
        let mut result = HashMap::new();
        let map = &self.skel.maps.heap_stats;

        // Iterate over all entries in the map (key is first stack frame)
        for key in map.keys() {
            if let Ok(Some(value)) = map.lookup(&key, MapFlags::ANY)
                && key.len() >= 8
                && value.len() >= 40
            {
                let key_addr = u64::from_ne_bytes(key[0..8].try_into().unwrap());
                let stats = HeapStats {
                    live_bytes: i64::from_ne_bytes(value[0..8].try_into().unwrap()),
                    total_allocs: u64::from_ne_bytes(value[8..16].try_into().unwrap()),
                    total_frees: u64::from_ne_bytes(value[16..24].try_into().unwrap()),
                    total_alloc_bytes: u64::from_ne_bytes(value[24..32].try_into().unwrap()),
                    total_free_bytes: u64::from_ne_bytes(value[32..40].try_into().unwrap()),
                };
                result.insert(key_addr, stats);
            }
        }

        result
    }

    /// Read debug counters from BPF (for debugging probe hits)
    pub fn read_debug_counters(&self) -> [u64; 6] {
        let mut result = [0u64; 6];
        let map = &self.skel.maps.debug_counters;
        for i in 0u32..6 {
            let key = i.to_ne_bytes();
            if let Ok(Some(value)) = map.lookup(&key, MapFlags::ANY)
                && value.len() >= 8
            {
                result[i as usize] = u64::from_ne_bytes(value[0..8].try_into().unwrap());
            }
        }
        result
    }

    /// Read inline stacks from live allocations
    /// Returns map of key_addr (first frame) -> full stack trace
    /// Used for better symbol resolution
    pub fn read_inline_stacks(&self) -> HashMap<u64, Vec<u64>> {
        let mut result: HashMap<u64, Vec<u64>> = HashMap::new();
        let map = &self.skel.maps.live_allocs;

        // alloc_info layout: size(8) + stack[32](256) + stack_len(1) = 265 bytes (padded)
        for key in map.keys() {
            if let Ok(Some(value)) = map.lookup(&key, MapFlags::ANY)
                && value.len() >= 265
            {
                let stack_len = value[264] as usize;
                if stack_len > 0 && stack_len <= 32 {
                    let mut stack = Vec::with_capacity(stack_len);
                    for i in 0..stack_len {
                        let offset = 8 + i * 8;
                        let addr =
                            u64::from_ne_bytes(value[offset..offset + 8].try_into().unwrap());
                        if addr != 0 {
                            stack.push(addr);
                        }
                    }
                    if !stack.is_empty() {
                        let key_addr = stack[0];
                        // Only insert if we don't have this key yet (first wins)
                        result.entry(key_addr).or_insert(stack);
                    }
                }
            }
        }

        result
    }

    /// Poll events from the ring buffer
    pub fn poll_events(&mut self, timeout: Duration) -> Vec<HeapEvent> {
        let events_clone = Arc::clone(&self.events);

        // Create ring buffer with callback
        let mut builder = RingBufferBuilder::new();

        let callback = move |data: &[u8]| {
            if data.len() >= 26 {
                let event = HeapEvent {
                    user_addr: u64::from_ne_bytes(data[0..8].try_into().unwrap()),
                    ptr: u64::from_ne_bytes(data[8..16].try_into().unwrap()),
                    size: i64::from_ne_bytes(data[16..24].try_into().unwrap()),
                    event_type: match data[24] {
                        0 => HeapEventType::Alloc,
                        1 => HeapEventType::Free,
                        _ => HeapEventType::Realloc,
                    },
                };
                if let Ok(mut events) = events_clone.lock() {
                    events.push(event);
                }
            }
            0 // Continue processing
        };

        if builder.add(&self.skel.maps.events, callback).is_ok()
            && let Ok(ring_buffer) = builder.build()
        {
            let _ = ring_buffer.poll(timeout);
        }

        // Drain and return events
        let mut events = self.events.lock().unwrap();
        std::mem::take(&mut *events)
    }
}

/// Allocator symbol offsets
struct AllocatorSymbols {
    rust_alloc: Option<u64>,
    rust_dealloc: Option<u64>,
    rust_realloc: Option<u64>,
}

/// Convert a virtual address to file offset using segment information
fn vaddr_to_file_offset(file: &object::File, vaddr: u64) -> Option<u64> {
    use object::{Object, ObjectSegment};

    for segment in file.segments() {
        let seg_vaddr = segment.address();
        let seg_size = segment.size();

        // Get file offset from segment's file range (returns (offset, size) tuple)
        let (seg_file_offset, _) = segment.file_range();

        // Check if vaddr falls within this segment
        if vaddr >= seg_vaddr && vaddr < seg_vaddr + seg_size {
            // Calculate file offset
            let offset_in_segment = vaddr - seg_vaddr;
            return Some(seg_file_offset + offset_in_segment);
        }
    }

    // Fallback: assume vaddr == file offset (non-PIE or simple case)
    Some(vaddr)
}

/// Find allocator symbols in the target binary
fn find_allocator_symbols(exe_path: &Path) -> Result<AllocatorSymbols> {
    use object::{Object, ObjectSymbol};

    let data = std::fs::read(exe_path)
        .map_err(|e| Error::Bpf(format!("Failed to read executable: {}", e)))?;

    let file = object::File::parse(&*data)
        .map_err(|e| Error::Bpf(format!("Failed to parse executable: {}", e)))?;

    let mut symbols = AllocatorSymbols {
        rust_alloc: None,
        rust_dealloc: None,
        rust_realloc: None,
    };

    // Look for allocator symbols - they may be mangled (e.g., _RNv...___rust_alloc)
    // or plain (e.g., __rust_alloc)
    for sym in file.symbols() {
        if let Ok(name) = sym.name() {
            let vaddr = sym.address();
            if name == "__rust_alloc" || name.contains("___rust_alloc") {
                if symbols.rust_alloc.is_none() {
                    symbols.rust_alloc = vaddr_to_file_offset(&file, vaddr);
                }
            } else if name == "__rust_dealloc" || name.contains("___rust_dealloc") {
                if symbols.rust_dealloc.is_none() {
                    symbols.rust_dealloc = vaddr_to_file_offset(&file, vaddr);
                }
            } else if (name == "__rust_realloc" || name.contains("___rust_realloc"))
                && symbols.rust_realloc.is_none()
            {
                symbols.rust_realloc = vaddr_to_file_offset(&file, vaddr);
            }
        }
    }

    // Also try dynamic symbols
    for sym in file.dynamic_symbols() {
        if let Ok(name) = sym.name() {
            let vaddr = sym.address();
            if (name == "__rust_alloc" || name.contains("___rust_alloc"))
                && symbols.rust_alloc.is_none()
            {
                symbols.rust_alloc = vaddr_to_file_offset(&file, vaddr);
            } else if (name == "__rust_dealloc" || name.contains("___rust_dealloc"))
                && symbols.rust_dealloc.is_none()
            {
                symbols.rust_dealloc = vaddr_to_file_offset(&file, vaddr);
            } else if (name == "__rust_realloc" || name.contains("___rust_realloc"))
                && symbols.rust_realloc.is_none()
            {
                symbols.rust_realloc = vaddr_to_file_offset(&file, vaddr);
            }
        }
    }

    if symbols.rust_alloc.is_none() && symbols.rust_dealloc.is_none() {
        return Err(Error::Bpf(
            "No Rust allocator symbols found. Target may not be a Rust binary or may be stripped."
                .to_string(),
        ));
    }

    Ok(symbols)
}

/// Check if we have permissions to use eBPF
fn check_bpf_permissions() -> Result<()> {
    // Try to check if we're root or have CAP_BPF
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }

    // Check /proc/sys/kernel/unprivileged_bpf_disabled
    if let Ok(content) = std::fs::read_to_string("/proc/sys/kernel/unprivileged_bpf_disabled") {
        let disabled: i32 = content.trim().parse().unwrap_or(1);
        if disabled != 0 {
            return Err(Error::PermissionDenied(
                "eBPF requires root or CAP_BPF. Try running with sudo.".to_string(),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heap_event_type() {
        assert_eq!(HeapEventType::Alloc as u8, 0);
    }
}
