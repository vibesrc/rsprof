# Section 0: Abstract

## Status of This Document

This document specifies rsprof, a zero-instrumentation profiler for Rust processes on Linux. This is a draft specification.

## Abstract

rsprof provides real-time CPU and heap profiling for Rust applications without requiring any modifications to the target process. It attaches to a running process by PID or process name and uses kernel facilities (perf_event, eBPF) to sample execution state and track allocations. All data is stored in SQLite for easy querying and post-hoc analysis.

Key properties:

- **Zero instrumentation**: No code changes, no recompilation, no custom allocator
- **Line-level attribution**: Maps samples to source file and line number using DWARF debug info
- **Dual metrics**: Reports both CPU time percentage and live heap bytes per source location
- **SQLite storage**: All profiling data persisted to queryable database files
- **Real-time display**: Live TUI during recording, replay via `rsprof top`
- **Minimal overhead**: Sampling-based approach with configurable frequency

## Target Environment

- **Platform**: Linux (kernel 4.18+ for eBPF, 5.8+ recommended)
- **Target binary**: Rust executable compiled with debug info (`debug = true` or `debuginfo = 2`)
- **Architecture**: x86_64 (ARM64 support planned)

## Deliverables

1. `rsprof` - Single statically-linked binary
2. This specification document
