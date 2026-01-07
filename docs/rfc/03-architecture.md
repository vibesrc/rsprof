# Section 3: Architecture

## 3.1 Overview

rsprof consists of five major components:

```
┌─────────────────────────────────────────────────────────────────┐
│                        rsprof process                           │
├─────────────┬─────────────┬──────────────┬──────────┬──────────┤
│   Symbol    │    CPU      │     Heap     │  SQLite  │   TUI    │
│  Resolver   │  Sampler    │   Tracker    │  Storage │ Renderer │
│             │             │              │          │          │
│  - DWARF    │  - perf     │  - eBPF      │ - WAL    │ - ratatui│
│  - gimli    │    event    │  - uprobes   │ - batch  │ - live   │
│  - ASLR     │  - ring     │  - alloc map │   writes │   queries│
│    offset   │    buffer   │              │          │          │
└──────┬──────┴──────┬──────┴───────┬──────┴────┬─────┴────┬─────┘
       │             │              │           │          │
       │             │              │           │          │
       ▼             ▼              ▼           ▼          ▼
┌──────────────────────────────────────────────────────────────────┐
│                           Linux Kernel                           │
├──────────────┬───────────────────┬───────────────────────────────┤
│ /proc/[pid]  │   perf_event      │           eBPF                │
│   - exe      │   subsystem       │         subsystem             │
│   - maps     │                   │                               │
│   - comm     │   PERF_SAMPLE_IP  │   uprobe: __rust_alloc        │
│              │   PERF_SAMPLE_    │   uprobe: __rust_dealloc      │
│              │   STACK_USER      │   uprobe: __rust_realloc      │
└──────────────┴───────────────────┴───────────────────────────────┘
                              │
                              ▼
                 ┌─────────────────────────┐
                 │     Target Process      │
                 │                         │
                 │  [Rust binary with      │
                 │   debug symbols]        │
                 └─────────────────────────┘
```

## 3.2 Data Flow

### 3.2.1 Initialization

1. Parse command-line arguments (PID or process name)
2. Resolve process name to PID if needed (via `/proc/*/comm`)
3. Verify target process exists via `/proc/[pid]/exe`
4. Load DWARF debug info from target executable
5. Calculate ASLR offset from `/proc/[pid]/maps`
6. Open SQLite database (WAL mode)
7. Set up perf_event for CPU sampling
8. Load eBPF program and attach uprobes for heap tracking
9. Initialize TUI (unless `--quiet`)

### 3.2.2 Steady State

```
┌────────────────────────────────────────────────────────────────┐
│                    Main Event Loop                              │
│                                                                 │
│  every 10ms:                                                    │
│    ├─► drain perf_event ring buffer                            │
│    │     └─► for each sample:                                  │
│    │           addr = sample.ip - aslr_offset                  │
│    │           pending_cpu[addr] += 1                          │
│    │                                                           │
│    └─► drain eBPF ring buffer                                  │
│          └─► for each event:                                   │
│                match event.type:                               │
│                  Alloc  → pending_heap[addr].alloc += size     │
│                  Dealloc→ pending_heap[addr].free += size      │
│                                                                 │
│  every [checkpoint_interval] (default 1s):                      │
│    ├─► resolve symbols for new addresses                       │
│    ├─► INSERT checkpoint to SQLite                             │
│    ├─► batch INSERT cpu_samples                                │
│    ├─► batch INSERT heap_events                                │
│    └─► clear pending maps                                      │
│                                                                 │
│  every [render_interval] (default 100ms):                       │
│    └─► TUI queries SQLite, renders top N                       │
└────────────────────────────────────────────────────────────────┘
```

### 3.2.3 Shutdown

1. Detach eBPF uprobes
2. Close perf_event file descriptors
3. Restore terminal state
4. Optionally export final statistics

## 3.3 Component Responsibilities

### 3.3.1 Symbol Resolver

- Parse DWARF `.debug_info` and `.debug_line` sections
- Build address-to-location index
- Handle inlined functions (report innermost frame)
- Cache lookups for performance

See [Section 4: Symbol Resolution](./04-symbol-resolution.md) for details.

### 3.3.2 CPU Sampler

- Configure perf_event_open with PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK
- Set sample frequency (default 99 Hz to avoid lockstep with timers)
- Read samples from ring buffer
- Extract instruction pointer and optional stack trace

See [Section 5: CPU Profiling](./05-cpu-profiling.md) for details.

### 3.3.3 Heap Tracker

- Load pre-compiled eBPF program (CO-RE format)
- Attach uprobes to `__rust_alloc`, `__rust_dealloc`, `__rust_realloc`
- Maintain per-callsite allocation map in eBPF map
- Periodically read map from userspace

See [Section 6: Heap Profiling](./06-heap-profiling.md) for details.

### 3.3.4 SQLite Storage

- Open database in WAL mode for concurrent read/write
- Batch insert samples at checkpoint intervals
- Resolve and store symbols lazily
- Provide query interface for TUI and `rsprof top`

See [Section 7: Storage](./07-storage.md) for details.

### 3.3.5 TUI Renderer

- Query SQLite for top CPU and heap consumers
- Display sorted table of source locations
- Show CPU%, heap bytes, function name for each
- Support keyboard navigation and filtering
- Handle terminal resize

See [Section 9: User Interface](./09-user-interface.md) for details.

## 3.4 Threading Model

```
┌─────────────────────────────────────────────────────┐
│                   rsprof threads                    │
├─────────────────────────────────────────────────────┤
│                                                     │
│  [main thread]                                      │
│    - TUI event loop                                 │
│    - keyboard input                                 │
│    - SQLite read queries                           │
│    - rendering                                      │
│                                                     │
│  [sampler thread]                                   │
│    - perf_event polling                            │
│    - eBPF map reading                              │
│    - SQLite writes (exclusive)                     │
│    - checkpoint flush                              │
│                                                     │
└─────────────────────────────────────────────────────┘
```

The sampler thread owns the write connection to SQLite. The main thread uses a separate read connection. WAL mode ensures they don't block each other.

## 3.5 Memory Budget

rsprof SHOULD use less than 100 MB of memory for typical workloads:

| Component | Estimated Size |
|-----------|----------------|
| DWARF index | 10-50 MB (depends on binary size) |
| Pending CPU map | ~1 MB (10K addresses × 100 bytes) |
| Pending heap map | ~1 MB (10K addresses × 100 bytes) |
| eBPF maps | ~4 MB (kernel side) |
| Ring buffers | ~16 MB (perf + eBPF) |
| SQLite cache | ~10 MB |
| TUI buffers | ~1 MB |

SQLite database size on disk grows over time (see [Section 7.7.4](./07-storage.md#774-expected-sizes)).

For binaries with exceptionally large debug info (>500 MB), rsprof MAY use proportionally more memory for the DWARF index.
