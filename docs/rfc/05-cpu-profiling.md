# Section 5: CPU Profiling

## 5.1 Overview

CPU profiling uses the Linux `perf_event` subsystem to sample the target process at regular intervals. Each sample captures the instruction pointer (IP), which is then resolved to a source location.

## 5.2 perf_event Configuration

### 5.2.1 System Call

rsprof uses `perf_event_open(2)` to create a sampling event:

```c
struct perf_event_attr attr = {
    .type = PERF_TYPE_SOFTWARE,
    .config = PERF_COUNT_SW_CPU_CLOCK,
    .sample_type = PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_TIME,
    .sample_freq = 99,           // Hz, not period
    .freq = 1,                   // Use frequency, not period
    .inherit = 0,                // Don't inherit to children
    .exclude_kernel = 1,         // Skip kernel samples
    .exclude_hv = 1,             // Skip hypervisor
    .disabled = 1,               // Start disabled
    .enable_on_exec = 0,
    .task = 0,
    .watermark = 1,
    .wakeup_watermark = 4096,    // Wake when 4KB ready
};

int fd = perf_event_open(&attr, pid, -1, -1, 0);
//                       ^pid  ^cpu ^group ^flags
//                       target, any CPU, no group
```

### 5.2.2 Sample Types

| Flag | Purpose | Required |
|------|---------|----------|
| `PERF_SAMPLE_IP` | Instruction pointer | MUST |
| `PERF_SAMPLE_TID` | Thread ID | SHOULD |
| `PERF_SAMPLE_TIME` | Timestamp | SHOULD |
| `PERF_SAMPLE_CALLCHAIN` | Stack trace | MAY |
| `PERF_SAMPLE_STACK_USER` | User stack bytes | MAY |

For line-level attribution, only `PERF_SAMPLE_IP` is strictly required. Stack traces enable call graph analysis but increase overhead.

### 5.2.3 Sampling Frequency

Default: 99 Hz (samples per second)

The value 99 is chosen because:
1. High enough for statistical significance (6000 samples/minute)
2. Low enough for minimal overhead (<1% CPU typically)
3. Prime number avoids lockstep with system timers (100 Hz is common)

rsprof MUST support configurable frequency via command-line argument.

Recommended ranges:
- 49 Hz: Low overhead, coarse granularity
- 99 Hz: Default, good balance
- 999 Hz: High resolution, ~5% overhead
- 4999 Hz: Maximum useful, ~15% overhead

Frequencies above 5000 Hz are NOT RECOMMENDED due to diminishing returns and overhead.

## 5.3 Ring Buffer

### 5.3.1 Memory Mapping

The perf_event file descriptor is memory-mapped to create a ring buffer:

```c
size_t page_size = sysconf(_SC_PAGESIZE);
size_t mmap_size = (1 + 64) * page_size;  // 1 metadata + 64 data pages

void *base = mmap(NULL, mmap_size, PROT_READ | PROT_WRITE, 
                  MAP_SHARED, fd, 0);

struct perf_event_mmap_page *header = base;
char *data = base + page_size;
```

### 5.3.2 Reading Samples

The ring buffer uses `data_head` and `data_tail` pointers:

```c
uint64_t head = header->data_head;
uint64_t tail = header->data_tail;
rmb();  // Read barrier

while (tail < head) {
    struct perf_event_header *event = data + (tail % data_size);
    
    if (event->type == PERF_RECORD_SAMPLE) {
        uint64_t ip = *(uint64_t *)(event + 1);
        process_sample(ip);
    }
    
    tail += event->size;
}

header->data_tail = tail;
wmb();  // Write barrier
```

### 5.3.3 Buffer Sizing

| Pages | Size | Samples at 99Hz | Duration |
|-------|------|-----------------|----------|
| 16 | 64 KB | ~2000 | ~20 sec |
| 64 | 256 KB | ~8000 | ~80 sec |
| 256 | 1 MB | ~32000 | ~5 min |

rsprof SHOULD use 64 pages (256 KB) by default. This allows several seconds of buffering if the profiler thread is delayed.

## 5.4 Statistics Aggregation

### 5.4.1 Data Structure

```rust
struct CpuStats {
    // Key: (file_id, line_number)
    // Value: sample count
    samples: HashMap<(u32, u32), u64>,
    total_samples: u64,
}
```

### 5.4.2 Percentage Calculation

For each location:
```
cpu_percent = (location_samples / total_samples) * 100
```

### 5.4.3 Time Window

rsprof SHOULD support two modes:

1. **Cumulative** (default): All samples since attachment
2. **Windowed**: Only samples in last N seconds

Windowed mode is useful for observing behavior changes over time.

## 5.5 Stack Unwinding

### 5.5.1 Optional Feature

Stack unwinding provides call graph information but increases complexity and overhead. rsprof MAY implement stack unwinding as an optional feature.

### 5.5.2 Frame Pointer Unwinding

If the target is compiled with frame pointers (`-C force-frame-pointers=yes`), unwinding is simple:

```c
uint64_t *fp = (uint64_t *)regs.rbp;
while (fp && is_valid_address(fp)) {
    uint64_t return_addr = fp[1];
    record_frame(return_addr);
    fp = (uint64_t *)fp[0];
}
```

### 5.5.3 DWARF Unwinding

Without frame pointers, DWARF `.eh_frame` or `.debug_frame` sections contain unwinding instructions. This is more complex and slower.

rsprof SHOULD prefer frame pointer unwinding when available.

## 5.6 Multi-threading

### 5.6.1 Per-thread vs Process-wide

With `pid` parameter to `perf_event_open`:
- Specific PID: Samples from that thread only
- PID -1: System-wide (requires CAP_PERFMON)

rsprof MUST sample all threads of the target process. Options:

1. **Inherit flag**: Set `attr.inherit = 1` (doesn't work for attach)
2. **Per-thread FDs**: Open perf_event for each thread
3. **Process-wide**: Use `PERF_FLAG_PID_CGROUP` (requires cgroup)

Recommended approach: enumerate threads via `/proc/[pid]/task/` and open a perf_event FD for each.

### 5.6.2 Thread Discovery

```bash
$ ls /proc/12345/task/
12345  12346  12347  12348
```

rsprof MUST:
1. Enumerate threads at startup
2. Periodically check for new threads
3. Handle thread exit gracefully

## 5.7 Error Handling

### 5.7.1 Permission Denied

`perf_event_open` may fail with `EACCES` if:
- `perf_event_paranoid` is too restrictive
- The process is not owned by the current user
- CAP_SYS_PTRACE is required

rsprof MUST check `/proc/sys/kernel/perf_event_paranoid`:

| Value | Meaning |
|-------|---------|
| -1 | No restrictions |
| 0 | Allow process-wide for all users |
| 1 | Allow process-wide for user's processes |
| 2 | Disallow all process-wide (default on some distros) |
| 3+ | Disallow all perf events |

If value is 2+, rsprof SHOULD suggest:
```
sudo sysctl kernel.perf_event_paranoid=1
```

### 5.7.2 Ring Buffer Overflow

If samples arrive faster than processing, the ring buffer overflows. The kernel sets `PERF_RECORD_LOST` records indicating lost samples.

rsprof SHOULD:
1. Track lost sample count
2. Display warning if loss rate > 1%
3. Suggest reducing frequency or increasing buffer size
