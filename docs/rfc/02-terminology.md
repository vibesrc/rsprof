# Section 2: Terminology

## 2.1 RFC 2119 Keywords

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in RFC 2119.

## 2.2 Definitions

**Target process**
: The Rust process being profiled, identified by PID.

**Profiler process**
: The rsprof process that attaches to and monitors the target.

**Sample**
: A single observation of the target's execution state at a point in time.

**Source location**
: A tuple of (file path, line number) identifying a position in source code.

**Symbol resolution**
: The process of mapping a memory address to a source location.

**DWARF**
: Debugging With Attributed Record Formats - the debug information format used by Rust/LLVM.

**ASLR**
: Address Space Layout Randomization - kernel feature that randomizes process memory layout.

**perf_event**
: Linux kernel subsystem for performance monitoring and sampling.

**eBPF**
: Extended Berkeley Packet Filter - in-kernel virtual machine for safe, efficient tracing.

**uprobe**
: User-space probe - eBPF mechanism to intercept function calls in user programs.

**Live bytes**
: Memory currently allocated (allocated minus freed) attributed to a source location.

**CPU percentage**
: Fraction of samples where the instruction pointer was in code from a source location.

## 2.3 Notation

Source locations are written as `file:line`, for example:
```
src/parser.rs:142
```

Memory sizes use SI prefixes: KB (10³), MB (10⁶), GB (10⁹). Binary prefixes (KiB, MiB) are used when referring to actual memory allocation sizes.

Addresses are written in hexadecimal with `0x` prefix: `0x7f4a3b2c1000`.
