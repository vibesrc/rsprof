# Section 6: Heap Profiling

## 6.1 Overview

Heap profiling tracks memory allocations to determine which source locations hold live memory. Unlike CPU profiling which samples periodically, heap profiling intercepts every allocation and deallocation to maintain accurate counts.

## 6.2 Rust Allocator Interface

### 6.2.1 Global Allocator Functions

Rust's standard library routes all allocations through these symbols:

| Symbol | Signature | Purpose |
|--------|-----------|---------|
| `__rust_alloc` | `(size: usize, align: usize) -> *mut u8` | Allocate memory |
| `__rust_dealloc` | `(ptr: *mut u8, size: usize, align: usize)` | Free memory |
| `__rust_realloc` | `(ptr: *mut u8, old: usize, align: usize, new: usize) -> *mut u8` | Resize allocation |
| `__rust_alloc_zeroed` | `(size: usize, align: usize) -> *mut u8` | Allocate zeroed memory |

These functions exist regardless of the underlying allocator (system, jemalloc, mimalloc, etc.).

### 6.2.2 Why Not malloc/free?

Rust's allocator interface is more reliable than intercepting libc:
1. All Rust allocations go through `__rust_alloc`
2. Size is passed to `__rust_dealloc` (no need to track allocation sizes)
3. Works regardless of underlying allocator

## 6.3 eBPF Uprobe Design

### 6.3.1 Probe Points

rsprof attaches uprobes to three functions:

```
uprobe:__rust_alloc      → record allocation
uprobe:__rust_dealloc    → record deallocation  
uprobe:__rust_realloc    → record resize (dealloc + alloc)
```

For `__rust_alloc`, we also need the return value (pointer), so we use both uprobe and uretprobe.

### 6.3.2 eBPF Program Structure

```c
// Maps
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 65536);
    __type(key, u64);           // return address (callsite)
    __type(value, struct heap_stats);
} heap_stats SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1048576);
    __type(key, u64);           // allocation pointer
    __type(value, struct alloc_info);
} live_allocs SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1048576);
} events SEC(".maps");

// Uprobe on __rust_alloc entry - capture size and return address
SEC("uprobe/__rust_alloc")
int alloc_enter(struct pt_regs *ctx) {
    u64 size = PT_REGS_PARM1(ctx);
    u64 ret_addr = PT_REGS_RET(ctx);  // Caller's return address
    
    // Store in per-CPU scratch for uretprobe
    struct alloc_context ac = { .size = size, .ret_addr = ret_addr };
    bpf_map_update_elem(&alloc_scratch, &tid, &ac, BPF_ANY);
    
    return 0;
}

// Uretprobe on __rust_alloc - capture returned pointer
SEC("uretprobe/__rust_alloc")
int alloc_exit(struct pt_regs *ctx) {
    u64 ptr = PT_REGS_RC(ctx);
    if (!ptr) return 0;  // Allocation failed
    
    // Retrieve context from entry probe
    u32 tid = bpf_get_current_pid_tgid();
    struct alloc_context *ac = bpf_map_lookup_elem(&alloc_scratch, &tid);
    if (!ac) return 0;
    
    // Record live allocation
    struct alloc_info info = { .size = ac->size, .callsite = ac->ret_addr };
    bpf_map_update_elem(&live_allocs, &ptr, &info, BPF_ANY);
    
    // Update per-callsite stats
    struct heap_stats *stats = bpf_map_lookup_elem(&heap_stats, &ac->ret_addr);
    if (stats) {
        __sync_fetch_and_add(&stats->live_bytes, ac->size);
        __sync_fetch_and_add(&stats->total_allocs, 1);
    } else {
        struct heap_stats new_stats = { .live_bytes = ac->size, .total_allocs = 1 };
        bpf_map_update_elem(&heap_stats, &ac->ret_addr, &new_stats, BPF_ANY);
    }
    
    // Send event to userspace
    struct heap_event *e = bpf_ringbuf_reserve(&events, sizeof(*e), 0);
    if (e) {
        e->type = EVENT_ALLOC;
        e->ptr = ptr;
        e->size = ac->size;
        e->callsite = ac->ret_addr;
        bpf_ringbuf_submit(e, 0);
    }
    
    return 0;
}

// Uprobe on __rust_dealloc
SEC("uprobe/__rust_dealloc")
int dealloc_enter(struct pt_regs *ctx) {
    u64 ptr = PT_REGS_PARM1(ctx);
    u64 size = PT_REGS_PARM2(ctx);
    
    // Look up original callsite
    struct alloc_info *info = bpf_map_lookup_elem(&live_allocs, &ptr);
    if (!info) return 0;  // Unknown allocation
    
    u64 callsite = info->callsite;
    
    // Update per-callsite stats
    struct heap_stats *stats = bpf_map_lookup_elem(&heap_stats, &callsite);
    if (stats) {
        __sync_fetch_and_sub(&stats->live_bytes, size);
        __sync_fetch_and_add(&stats->total_frees, 1);
    }
    
    // Remove from live allocations
    bpf_map_delete_elem(&live_allocs, &ptr);
    
    return 0;
}
```

### 6.3.3 CO-RE (Compile Once, Run Everywhere)

rsprof MUST use CO-RE to avoid requiring kernel headers at runtime:

1. Compile BPF program with BTF (BPF Type Format)
2. Embed compiled BPF object in rsprof binary
3. Use libbpf's CO-RE relocations to adapt to running kernel

This allows a single rsprof binary to work across kernel versions.

## 6.4 Callsite Attribution

### 6.4.1 Return Address Capture

The return address on the stack identifies the callsite:

```
main()
  └─> process_data()
        └─> Vec::push()           ← we want this location
              └─> __rust_alloc()  ← uprobe fires here
                    return addr points to Vec::push()
```

`PT_REGS_RET(ctx)` gives the return address, which after ASLR adjustment and DWARF resolution yields the source location.

### 6.4.2 Inlining Considerations

If `Vec::push` is inlined into `process_data`:

```
process_data()  ← return addr points here
  [inlined Vec::push]
    └─> __rust_alloc()
```

The return address points to `process_data`, but DWARF inline information (see [Section 4.2.4](./04-symbol-resolution.md#424-inlined-functions)) can recover the original `Vec::push` location.

### 6.4.3 Multiple Levels

For deeper attribution (e.g., "which function called Vec::push?"), stack unwinding is needed. This is more expensive and is NOT REQUIRED for the base implementation.

## 6.5 Statistics Tracking

### 6.5.1 Per-Callsite Metrics

```rust
struct HeapStats {
    live_bytes: i64,      // Current allocated (can be negative during accounting)
    peak_bytes: u64,      // Maximum live at any point
    total_allocs: u64,    // Cumulative allocation count
    total_frees: u64,     // Cumulative free count
    total_bytes: u64,     // Cumulative bytes allocated
}
```

### 6.5.2 Global Metrics

```rust
struct GlobalHeapStats {
    live_bytes: u64,
    peak_bytes: u64,
    total_allocs: u64,
    total_frees: u64,
    active_callsites: usize,
}
```

### 6.5.3 Map Reading

Userspace periodically reads the `heap_stats` BPF map:

```rust
fn read_heap_stats(map: &Map) -> HashMap<u64, HeapStats> {
    let mut stats = HashMap::new();
    for key in map.keys() {
        if let Ok(value) = map.lookup(&key, MapFlags::ANY) {
            let callsite = u64::from_ne_bytes(key.try_into().unwrap());
            let hs: HeapStats = unsafe { std::ptr::read(value.as_ptr() as *const _) };
            stats.insert(callsite, hs);
        }
    }
    stats
}
```

## 6.6 Handling Edge Cases

### 6.6.1 Realloc

`__rust_realloc` can:
1. Return the same pointer (in-place resize)
2. Return a new pointer (moved)
3. Return NULL (failure)

rsprof MUST handle all cases:

```c
SEC("uprobe/__rust_realloc")
int realloc_enter(struct pt_regs *ctx) {
    u64 old_ptr = PT_REGS_PARM1(ctx);
    u64 old_size = PT_REGS_PARM2(ctx);
    u64 new_size = PT_REGS_PARM4(ctx);
    
    // Treat as dealloc of old + alloc of new
    // The uretprobe will record the new allocation
    handle_dealloc(old_ptr, old_size);
    
    // Store new_size for uretprobe
    ...
}
```

### 6.6.2 Allocation Failure

If `__rust_alloc` returns NULL, no allocation occurred. The uretprobe MUST check for this.

### 6.6.3 Pre-existing Allocations

When rsprof attaches to a running process, existing allocations are unknown. Options:

1. **Accept inaccuracy**: Live bytes will be negative until old allocations are freed
2. **Scan heap**: Use `/proc/[pid]/maps` to estimate current heap size
3. **Reset baseline**: After warmup period, reset stats to zero

rsprof SHOULD implement option 1 with a warning, and MAY implement option 3.

### 6.6.4 Custom Allocators

If the target uses a custom global allocator that doesn't route through `__rust_alloc`, heap tracking will miss allocations. rsprof SHOULD:

1. Detect this condition (allocations from jemalloc/mimalloc symbols)
2. Warn the user
3. Offer alternative probe points if known

## 6.7 Performance Considerations

### 6.7.1 Overhead

eBPF uprobes add overhead to every allocation:
- ~100-500ns per allocation (depends on kernel version)
- For 1M allocs/sec, this is 100-500ms of overhead (10-50% for allocation-heavy code)

rsprof SHOULD provide a sampling mode for high-allocation workloads:

```c
// Sample 1 in N allocations
if (bpf_get_prandom_u32() % sample_rate != 0)
    return 0;
```

### 6.7.2 Map Size Limits

The `live_allocs` map tracks every outstanding allocation. For long-running processes with many allocations:

| Max Entries | Memory | Typical Coverage |
|-------------|--------|------------------|
| 65,536 | ~4 MB | Light workloads |
| 262,144 | ~16 MB | Medium workloads |
| 1,048,576 | ~64 MB | Heavy workloads |

rsprof SHOULD default to 1M entries and warn if the map fills up.

### 6.7.3 Ring Buffer Sizing

The events ring buffer SHOULD be sized to handle burst allocations:

```
buffer_size = expected_allocs_per_second * drain_interval * event_size * 2
```

For 100K allocs/sec, 100ms drain interval, 32-byte events:
```
100,000 * 0.1 * 32 * 2 = 640 KB
```

rsprof SHOULD use 1 MB ring buffer by default.

## 6.8 Fallback: smaps-based Tracking

If eBPF is unavailable (kernel too old, CAP_BPF denied), rsprof MAY fall back to `/proc/[pid]/smaps` polling:

```
55a4b2c00000-55a4b2c50000 rw-p 00000000 00:00 0    [heap]
Size:               320 kB
Rss:                256 kB
...
```

This provides only total heap size, not per-callsite attribution. rsprof MUST clearly indicate this limitation to the user.
