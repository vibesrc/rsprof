# rsprof: Zero-Instrumentation Profiler for Rust

**Version:** 0.1.0-draft  
**Status:** Draft  
**Last Updated:** 2025-01-06

## Abstract

rsprof is a sampling-based profiler that attaches to running Rust processes compiled in debug mode on Linux. It provides real-time, line-level attribution of CPU usage and heap allocations without requiring any code changes, recompilation, or instrumentation in the target process. All profiling data is stored in SQLite for easy querying and analysis.

## Quick Start

```bash
# Record (live TUI + writes to rsprof.my-app.250106143022.db)
rsprof --pid 123456
rsprof --process my-app
rsprof --process my-app -o custom.db

# View recorded data
rsprof top cpu profile.db
rsprof top heap profile.db --top 50
rsprof top cpu profile.db --since 30s --threshold 0.5
```

## Table of Contents

1. [Abstract](./00-abstract.md)
2. [Introduction](./01-introduction.md)
3. [Terminology](./02-terminology.md)
4. [Architecture](./03-architecture.md)
5. [Symbol Resolution](./04-symbol-resolution.md)
6. [CPU Profiling](./05-cpu-profiling.md)
7. [Heap Profiling](./06-heap-profiling.md)
8. [Storage](./07-storage.md)
9. [Command Line Interface](./08-cli.md)
10. [User Interface](./09-user-interface.md)
11. [Security Considerations](./10-security.md)
12. [References](./11-references.md)

## Document Index

| Section | File | Description |
|---------|------|-------------|
| Abstract | [00-abstract.md](./00-abstract.md) | Status and summary |
| Introduction | [01-introduction.md](./01-introduction.md) | Goals, non-goals, prior art |
| Terminology | [02-terminology.md](./02-terminology.md) | Definitions and conventions |
| Architecture | [03-architecture.md](./03-architecture.md) | System design and data flow |
| Symbol Resolution | [04-symbol-resolution.md](./04-symbol-resolution.md) | DWARF parsing and ASLR handling |
| CPU Profiling | [05-cpu-profiling.md](./05-cpu-profiling.md) | perf_event_open sampling |
| Heap Profiling | [06-heap-profiling.md](./06-heap-profiling.md) | eBPF uprobe tracking |
| Storage | [07-storage.md](./07-storage.md) | SQLite schema and queries |
| CLI | [08-cli.md](./08-cli.md) | Command line interface |
| User Interface | [09-user-interface.md](./09-user-interface.md) | TUI design and output format |
| Security | [10-security.md](./10-security.md) | Privilege requirements, risks |
| References | [11-references.md](./11-references.md) | External documentation |

## Quick Navigation

- **Implementers**: Start with [Architecture](./03-architecture.md)
- **Storage format**: See [Storage](./07-storage.md)
- **CLI reference**: See [CLI](./08-cli.md)
- **Output format**: See [User Interface](./09-user-interface.md)
