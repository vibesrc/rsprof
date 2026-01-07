# Section 4: Symbol Resolution

## 4.1 Overview

Symbol resolution maps runtime instruction addresses to source locations. This requires:

1. Reading DWARF debug information from the target binary
2. Handling ASLR (Address Space Layout Randomization)
3. Resolving inlined functions to their original source
4. Demangling Rust symbol names

## 4.2 DWARF Processing

### 4.2.1 Required Sections

rsprof MUST read these ELF sections from the target executable:

| Section | Purpose |
|---------|---------|
| `.debug_info` | Compilation unit and type information |
| `.debug_line` | Line number program (address → file:line mapping) |
| `.debug_str` | String table for debug info |
| `.debug_abbrev` | Abbreviation tables |
| `.symtab` / `.dynsym` | Symbol tables for function names |

### 4.2.2 Line Number Resolution

The `.debug_line` section contains a state machine program that maps addresses to source locations. rsprof MUST:

1. Parse all compilation units
2. Execute the line number program for each unit
3. Build an index of address ranges to (file, line, column) tuples
4. Store the index sorted by address for binary search

Example index structure:

```
Address Range              File                Line  Column
─────────────────────────────────────────────────────────────
0x0000000000001000-001050  src/main.rs         10    1
0x0000000000001050-001100  src/main.rs         11    5
0x0000000000001100-001200  src/parser.rs       42    1
0x0000000000001200-001250  src/parser.rs       43    12
...
```

### 4.2.3 Function Name Resolution

Function names come from the symbol table. rsprof MUST:

1. Read `.symtab` (static symbols) and `.dynsym` (dynamic symbols)
2. Filter to function symbols (`STT_FUNC`)
3. Build address range → name mapping
4. Demangle Rust symbols using `rustc-demangle`

### 4.2.4 Inlined Functions

When functions are inlined, a single address may correspond to multiple logical source locations. The DWARF `.debug_info` section contains `DW_TAG_inlined_subroutine` entries that describe the inlining chain.

rsprof SHOULD resolve to the innermost (original source) location by default. rsprof MAY provide an option to show the full inline stack.

Example:
```rust
// src/util.rs:10
#[inline(always)]
fn helper() { /* sample taken here */ }

// src/main.rs:50  
fn process() {
    helper();  // inlined
}
```

A sample at the inlined `helper` code SHOULD be attributed to `src/util.rs:10`, not `src/main.rs:50`.

## 4.3 ASLR Handling

### 4.3.1 Problem

Linux enables ASLR by default. The executable's load address varies each run:

```
DWARF addresses:  0x0000000000001000 (relative to binary base)
Runtime address:  0x000055a4b2c01000 (actual in process memory)
Offset:           0x000055a4b2c00000
```

### 4.3.2 Solution

rsprof MUST calculate the ASLR offset at startup:

1. Read `/proc/[pid]/maps`
2. Find the first executable mapping (`r-xp`) of the target binary
3. Subtract the expected base address (usually 0 for PIE binaries)

Example `/proc/[pid]/maps` entry:
```
55a4b2c00000-55a4b2c50000 r-xp 00000000 08:01 1234567 /path/to/binary
```

The ASLR offset is `0x55a4b2c00000`.

### 4.3.3 Address Translation

For each sample address `addr`:
```
debug_addr = addr - aslr_offset
location = index.lookup(debug_addr)
```

## 4.4 Symbol Caching

### 4.4.1 Lookup Cache

Symbol resolution is called for every sample. rsprof SHOULD maintain a cache:

```rust
struct SymbolCache {
    // Most recent lookups (LRU)
    recent: LruCache<u64, Location>,
    // Full index for cache misses
    index: AddressIndex,
}
```

Cache hit rates above 90% are typical since programs spend most time in hot loops.

### 4.4.2 Index Structure

The address index MUST support efficient range queries. Recommended structure:

```rust
struct AddressIndex {
    // Sorted by start address
    ranges: Vec<AddressRange>,
}

struct AddressRange {
    start: u64,
    end: u64,
    file_id: u32,
    line: u32,
    function_id: u32,
}
```

Binary search gives O(log n) lookup. With ~100K ranges, this is ~17 comparisons per lookup.

## 4.5 Path Simplification

Debug info often contains full paths like:
```
/home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/serde-1.0.152/src/de/mod.rs
```

rsprof SHOULD simplify paths for display:

1. Strip common prefixes (`/home/user/.cargo/registry/src/...` → `serde-1.0.152/src/de/mod.rs`)
2. Recognize `src/` as project root
3. Show crate name for dependencies

Display priority:
1. `src/foo.rs:42` - project source
2. `serde/src/de/mod.rs:100` - dependency source
3. `[libc]` - system library (no line info)
4. `??:0` - unknown location

## 4.6 Error Handling

### 4.6.1 Missing Debug Info

If the target binary lacks DWARF info, rsprof MUST:

1. Print a clear error message
2. Suggest recompiling with `debug = true` in `Cargo.toml`
3. Exit with non-zero status

### 4.6.2 Partial Debug Info

If some addresses cannot be resolved (e.g., dynamically loaded libraries), rsprof SHOULD:

1. Continue profiling
2. Report unresolved addresses as `[unknown]`
3. Optionally attempt to load debug info for shared libraries

### 4.6.3 Stripped Binaries

If the binary is stripped (`strip` removes symbols but not DWARF), function names may be unavailable. rsprof SHOULD:

1. Still resolve file:line from DWARF
2. Display `<unknown>` for function names
3. Warn the user about limited information
