# rsprof

Zero-instrumentation profiler for Rust processes.

## Features

- **CPU profiling** via `perf_event_open` - no instrumentation required
- **Heap profiling** via eBPF uprobes on Rust allocator functions
- **Interactive TUI** with real-time charts and tables
- **Symbol resolution** from DWARF debug info
- **SQLite storage** for profiling data

## Installation

### From source (with heap profiling)

Requires clang and development libraries for eBPF compilation:

```bash
# Ubuntu/Debian
sudo apt install clang libelf-dev zlib1g-dev

# Fedora
sudo dnf install clang elfutils-libelf-devel zlib-devel

# Arch
sudo pacman -S clang libelf zlib
```

Then build:

```bash
cargo build --release
```

### From source (CPU-only, no build dependencies)

If you don't need heap profiling:

```bash
cargo build --release --no-default-features
```

This builds without eBPF support and requires no extra dependencies.

## Usage

```bash
# Profile a running process
sudo rsprof attach <PID>

# Run and profile a command
sudo rsprof run -- ./my-rust-program

# TUI keybindings
#   1     - CPU view
#   2     - Memory view
#   3     - Both view
#   m     - Cycle views
#   s     - Toggle sort column (Both mode)
#   j/k   - Navigate table
#   Enter - Select row for chart
#   q     - Quit
```

## Runtime Requirements

| Feature | Requirement |
|---------|-------------|
| CPU profiling | Linux with `perf_event_open` (kernel 2.6.31+) |
| Heap profiling | Root or `CAP_BPF` capability |
| Symbol resolution | Debug symbols in binary (compile with `debug = true`) |

### Running without root

For CPU-only profiling, you can adjust `perf_event_paranoid`:

```bash
sudo sysctl kernel.perf_event_paranoid=1
```

Heap profiling always requires root or CAP_BPF.

## How It Works

### CPU Profiling

Uses `perf_event_open(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK)` to sample the target process at regular intervals, capturing the instruction pointer at each sample.

### Heap Profiling

Attaches eBPF uprobes to Rust's allocator functions:
- `__rust_alloc` - allocation entry/return
- `__rust_dealloc` - deallocation
- `__rust_realloc` - reallocation entry/return

Tracks per-callsite statistics (live bytes, total allocations, etc.) in BPF hash maps.

## Building for Distribution

```bash
cargo build --release
# Binary at target/release/rsprof
```

**Runtime dependencies:**
- CPU-only build (`--no-default-features`): None beyond libc
- With heap profiling: `libbpf` shared library (`apt install libbpf1` or similar)

The eBPF bytecode is embedded in the binary.

## License

MIT
