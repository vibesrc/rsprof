# Section 1: Introduction

## 1.1 Problem Statement

Profiling Rust applications typically requires one of:

1. **Instrumentation** - Adding profiling code, custom allocators, or compile-time macros
2. **Sampling tools** - Using `perf` or `flamegraph` which provide function-level granularity
3. **Heap profilers** - Tools like `heaptrack` or `valgrind` that require process launch under the profiler

None of these provide line-level attribution of both CPU and heap simultaneously for an already-running process.

## 1.2 Goals

1. **Attach to running process** - Profile any Rust process by PID without restart
2. **Line-level CPU attribution** - Show which source lines consume CPU time
3. **Line-level heap attribution** - Show which source lines hold live allocations
4. **Combined view** - Single display showing both CPU% and heap bytes per line
5. **Real-time updates** - Continuously refresh metrics, not just post-mortem analysis
6. **Minimal target impact** - Sampling approach with <5% overhead at default settings

## 1.3 Non-Goals

1. **Instruction-level profiling** - Line granularity is sufficient
2. **Distributed tracing** - Single-process focus
3. **Historical analysis** - Real-time display, not trace recording (export may be added later)
4. **Non-Linux platforms** - Linux-only due to perf_event and eBPF requirements
5. **Release builds without debug info** - DWARF symbols are required
6. **Memory leak detection** - Shows live allocations, not leak analysis

## 1.4 Prior Art

| Tool | CPU | Heap | Line-level | Attach | Notes |
|------|-----|------|------------|--------|-------|
| perf | ✓ | ✗ | ✗ | ✓ | Function-level only |
| flamegraph | ✓ | ✗ | ✗ | ✓ | Post-mortem visualization |
| heaptrack | ✗ | ✓ | ✓ | ✗ | Must launch under profiler |
| valgrind | ✓ | ✓ | ✓ | ✗ | 10-50x slowdown |
| bytehound | ✗ | ✓ | ✓ | ✗ | Requires LD_PRELOAD |
| samply | ✓ | ✗ | ✗ | ✓ | macOS focus, function-level |
| pprof-rs | ✓ | ✓ | ✗ | ✗ | Requires code instrumentation |

rsprof fills the gap: attach to running process, line-level, both CPU and heap.

## 1.5 Design Principles

1. **Use kernel facilities** - perf_event_open and eBPF are battle-tested
2. **Minimize target perturbation** - No ptrace stop/continue cycles during normal operation
3. **Leverage debug info** - DWARF provides accurate source mapping
4. **Fail gracefully** - Degrade to function-level if line info unavailable
5. **Single binary** - No runtime dependencies, no BPF compiler needed (CO-RE)
