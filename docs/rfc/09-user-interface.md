# Section 9: User Interface

## 9.1 Overview

rsprof provides two interface modes:

1. **Live TUI** - Real-time display during recording
2. **`rsprof top`** - Query recorded data from database

Both share similar output formats but serve different purposes.

## 9.2 Live TUI Layout

### 9.2.1 Side-by-Side View

The TUI shows CPU and heap rankings simultaneously:

```
┌─ rsprof ─────────────────────────────────────────────────────────────────────┐
│ PID: 12345 (my-app) | CPU: 99Hz | Heap: eBPF | Recording: 00:05:23           │
│ Output: rsprof.my-app.250106143022.db                                        │
├──────────────────────────────────────┬───────────────────────────────────────┤
│  Top CPU                             │  Top Heap                             │
├──────────────────────────────────────┼───────────────────────────────────────┤
│ 18.4%  src/parser.rs:142             │ 48.3 MB  src/buffer.rs:34             │
│        parse_header                  │          Buffer::extend               │
│ 12.1%  src/parser.rs:89              │ 12.8 MB  src/cache.rs:201             │
│        tokenize                      │          Cache::insert                │
│  8.2%  src/buffer.rs:34              │  2.1 MB  src/parser.rs:142            │
│        Buffer::extend                │          parse_header                 │
│  6.4%  src/main.rs:67                │  1.2 MB  src/conn.rs:78               │
│        process_batch                 │          Connection::read             │
│  4.1%  src/cache.rs:201              │ 64.0 KB  src/main.rs:89               │
│        Cache::insert                 │          handle_request               │
│  ...                                 │  ...                                  │
├──────────────────────────────────────┴───────────────────────────────────────┤
│ Samples: 31,842 | Live heap: 89.4 MB | Peak: 102.1 MB | Checkpoints: 323     │
│ [q]uit  [1]cpu  [2]heap  [f]ilter  [p]ause                                   │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 9.2.2 Single-Column Views

Press `1` for CPU-only or `2` for heap-only expanded view:

```
┌─ rsprof ─ CPU ───────────────────────────────────────────────────────────────┐
│ PID: 12345 (my-app) | Recording: 00:05:23                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│   CPU%   Location                                    Function                │
├──────────────────────────────────────────────────────────────────────────────┤
│  18.4%   src/parser.rs:142                           parse_header            │
│  12.1%   src/parser.rs:89                            tokenize                │
│   8.2%   src/buffer.rs:34                            Buffer::extend          │
│   6.4%   src/main.rs:67                              process_batch           │
│   4.1%   src/cache.rs:201                            Cache::insert           │
│   3.8%   src/parser.rs:156                           validate                │
│   2.9%   src/conn.rs:78                              Connection::read        │
│   2.1%   alloc/vec.rs:1842                           Vec::reserve            │
│   1.8%   src/main.rs:89                              handle_request          │
│   1.5%   core/str/mod.rs:234                         <str>::parse            │
│   ...                                                                        │
├──────────────────────────────────────────────────────────────────────────────┤
│ Total samples: 31,842 | Showing top 20                                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

## 9.3 `rsprof top` Output

### 9.3.1 CPU Output

```bash
$ rsprof top cpu profile.db --top 10
```

```
rsprof.my-app.250106143022.db
Duration: 5m23s | Samples: 31,842

  CPU%   Location                           Function
──────────────────────────────────────────────────────────────
 18.4%   src/parser.rs:142                  parse_header
 12.1%   src/parser.rs:89                   tokenize
  8.2%   src/buffer.rs:34                   Buffer::extend
  6.4%   src/main.rs:67                     process_batch
  4.1%   src/cache.rs:201                   Cache::insert
  3.8%   src/parser.rs:156                  validate
  2.9%   src/conn.rs:78                     Connection::read
  2.1%   alloc/vec.rs:1842                  Vec::reserve
  1.8%   src/main.rs:89                     handle_request
  1.5%   core/str/mod.rs:234                <str>::parse
```

### 9.3.2 Heap Output

```bash
$ rsprof top heap profile.db --top 10
```

```
rsprof.my-app.250106143022.db
Duration: 5m23s | Live heap: 89.4 MB | Peak: 102.1 MB

    Heap   Location                           Function
──────────────────────────────────────────────────────────────
 48.3 MB   src/buffer.rs:34                   Buffer::extend
 12.8 MB   src/cache.rs:201                   Cache::insert
  2.1 MB   src/parser.rs:142                  parse_header
  1.2 MB   src/conn.rs:78                     Connection::read
 64.0 KB   src/main.rs:89                     handle_request
 32.0 KB   src/parser.rs:89                   tokenize
 16.0 KB   alloc/vec.rs:1842                  Vec::reserve
  8.0 KB   src/main.rs:67                     process_batch
  4.0 KB   core/str/mod.rs:234                <str>::parse
  2.0 KB   src/parser.rs:156                  validate
```

### 9.3.3 Column Widths

| Column | Width | Format |
|--------|-------|--------|
| CPU% | 6 | `XX.X%` or `<0.1%` |
| Heap | 8 | `X.X MB`, `X.X KB`, `X B` |
| Location | 35 | Truncated with `…` |
| Function | remainder | Truncated with `...` |

## 9.4 Keyboard Controls (Live TUI)

| Key | Action |
|-----|--------|
| `q` | Quit (finalize database) |
| `1` | Full-screen CPU view |
| `2` | Full-screen Heap view |
| `3` | Side-by-side view (default) |
| `f` | Filter by pattern |
| `/` | Same as `f` |
| `p` | Pause/resume display updates |
| `↑`/`↓` | Scroll list |
| `PgUp`/`PgDn` | Scroll page |
| `Esc` | Clear filter / cancel input |

## 9.5 Filtering

### 9.5.1 Live TUI Filter

Pressing `f` or `/` opens a filter input:

```
┌─ Filter ────────────────────────┐
│ Pattern: parser█               │
│ [Enter] apply  [Esc] cancel     │
└─────────────────────────────────┘
```

### 9.5.2 CLI Filter (rsprof top)

```bash
rsprof top cpu profile.db --filter parser
rsprof top heap profile.db --filter "buffer.rs"
```

### 9.5.3 Filter Matching

The filter is a case-insensitive substring match against:
1. File path
2. Function name

Examples:
- `parser` matches `src/parser.rs:142` and `MyParser::parse`
- `vec` matches `alloc/vec.rs:1842` and `Vec::push`
- `::new` matches any `new` function

## 9.6 Output Formats

### 9.6.1 JSON Output

```bash
$ rsprof top cpu profile.db --json
```

```json
{
  "file": "rsprof.my-app.250106143022.db",
  "duration_ms": 323000,
  "total_samples": 31842,
  "entries": [
    {
      "cpu_pct": 18.4,
      "file": "src/parser.rs",
      "line": 142,
      "function": "parse_header"
    },
    ...
  ]
}
```

### 9.6.2 CSV Output

```bash
$ rsprof top cpu profile.db --csv
```

```csv
cpu_pct,file,line,function
18.4,src/parser.rs,142,parse_header
12.1,src/parser.rs,89,tokenize
8.2,src/buffer.rs,34,Buffer::extend
```

### 9.6.3 Heap JSON

```bash
$ rsprof top heap profile.db --json
```

```json
{
  "file": "rsprof.my-app.250106143022.db",
  "duration_ms": 323000,
  "live_bytes": 93782016,
  "peak_bytes": 107071308,
  "entries": [
    {
      "bytes": 50659328,
      "file": "src/buffer.rs",
      "line": 34,
      "function": "Buffer::extend"
    },
    ...
  ]
}
```

## 9.7 Refresh Behavior (Live TUI)

### 9.7.1 Refresh Rate

Default: 100ms (10 Hz display refresh)

The TUI queries SQLite at this rate. Checkpoint writes happen at `--interval` (default 1s).

### 9.7.2 Pause Mode

Pressing `p` pauses display updates:

```
│ ▐▐ PAUSED | Recording continues | Press p to resume                        │
```

Recording continues in the background; only display updates are paused.

## 9.8 Terminal Compatibility

### 9.8.1 Minimum Size

rsprof REQUIRES minimum terminal size: 80 columns × 24 rows.

If terminal is smaller:
```
Terminal too small. Minimum: 80x24, Current: 60x20
```

### 9.8.2 Color Support

rsprof detects color support and adapts:

| Terminal | Behavior |
|----------|----------|
| True color (24-bit) | Full color highlighting |
| 256 color | Reduced palette |
| 16 color | Basic highlighting |
| No color / `NO_COLOR` set | Monochrome |

### 9.8.3 Unicode

rsprof uses Unicode box drawing by default. Falls back to ASCII if `LANG` doesn't indicate UTF-8.

## 9.9 Byte Formatting

```rust
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1e3)
    } else {
        format!("{} B", bytes)
    }
}
```

## 9.10 Location Formatting

Priority order for path display:
1. `src/...` - Project source files
2. `crate_name/src/...` - Dependency files  
3. `[unknown]` - Unresolved addresses

Examples:
```
src/parser.rs:142           ← project file
serde/src/de/mod.rs:89      ← dependency
std/src/io/mod.rs:234       ← standard library
[libc.so.6]                 ← system library
[unknown]                   ← unresolved
```
