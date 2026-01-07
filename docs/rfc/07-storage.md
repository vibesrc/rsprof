# Section 7: Storage

## 7.1 Overview

rsprof stores all profiling data in SQLite databases. This enables:

- Queryable data with standard SQL
- Post-hoc analysis with different filters
- Sharing profiles for debugging
- Comparison across runs

## 7.2 File Naming

### 7.2.1 Default Pattern

```
rsprof.{process_name}.{timestamp}.db
```

Where:
- `process_name`: From `/proc/[pid]/comm`, sanitized for filesystem
- `timestamp`: `YYMMDDhhmmss` format (sorts chronologically)

Examples:
```
rsprof.my-app.250106143022.db
rsprof.nginx.250106144518.db
rsprof.my-app.250107091200.db
```

### 7.2.2 Process Name Resolution

1. Read `/proc/[pid]/comm` for the process name
2. Sanitize: replace non-alphanumeric (except `-_`) with `-`
3. Truncate to 32 characters
4. Fall back to PID if `/proc/[pid]/comm` is unreadable

```rust
fn get_process_name(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| sanitize(&s.trim()))
        .unwrap_or_else(|_| pid.to_string())
}
```

### 7.2.3 Custom Output

When `-o` is specified, use that path directly:

```bash
rsprof --pid 123 -o /tmp/debug.db
```

## 7.3 Schema

### 7.3.1 Metadata Table

```sql
CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

Required keys:
| Key | Description | Example |
|-----|-------------|---------|
| `version` | Schema version | `1` |
| `pid` | Target process ID | `123456` |
| `process_name` | From /proc/pid/comm | `my-app` |
| `exe_path` | Full executable path | `/usr/bin/my-app` |
| `start_time` | Recording start (ISO 8601) | `2025-01-06T14:30:22Z` |
| `checkpoint_interval_ms` | Interval between checkpoints | `1000` |
| `cpu_freq_hz` | CPU sampling frequency | `99` |

### 7.3.2 Checkpoints Table

```sql
CREATE TABLE checkpoints (
    id INTEGER PRIMARY KEY,
    timestamp_ms INTEGER NOT NULL  -- offset from start_time
);
```

Each checkpoint represents one collection interval (default 1 second).

### 7.3.3 Symbols Table

```sql
CREATE TABLE symbols (
    addr INTEGER PRIMARY KEY,
    file TEXT,
    line INTEGER,
    function TEXT
);
```

Addresses are stored as-is (pre-ASLR-adjustment). Symbol resolution happens during recording as new addresses are encountered.

### 7.3.4 CPU Samples Table

```sql
CREATE TABLE cpu_samples (
    checkpoint_id INTEGER NOT NULL,
    addr INTEGER NOT NULL,
    count INTEGER NOT NULL,
    FOREIGN KEY (checkpoint_id) REFERENCES checkpoints(id),
    FOREIGN KEY (addr) REFERENCES symbols(addr)
);

CREATE INDEX idx_cpu_checkpoint ON cpu_samples(checkpoint_id);
CREATE INDEX idx_cpu_addr ON cpu_samples(addr);
```

Each row represents the number of CPU samples at a given address during a checkpoint interval.

### 7.3.5 Heap Events Table

```sql
CREATE TABLE heap_events (
    checkpoint_id INTEGER NOT NULL,
    addr INTEGER NOT NULL,
    alloc_bytes INTEGER NOT NULL DEFAULT 0,
    free_bytes INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (checkpoint_id) REFERENCES checkpoints(id),
    FOREIGN KEY (addr) REFERENCES symbols(addr)
);

CREATE INDEX idx_heap_checkpoint ON heap_events(checkpoint_id);
CREATE INDEX idx_heap_addr ON heap_events(addr);
```

Stores allocation and free totals per address per checkpoint. Live bytes are computed by summing `alloc_bytes - free_bytes` across checkpoints.

## 7.4 Write Path

### 7.4.1 Initialization

```sql
PRAGMA journal_mode = WAL;      -- Allow concurrent reads during writes
PRAGMA synchronous = NORMAL;    -- Balance durability and performance
```

### 7.4.2 Checkpoint Flush

Every checkpoint interval (default 1s):

```rust
fn flush_checkpoint(&mut self) -> Result<()> {
    let tx = self.conn.transaction()?;
    
    // Insert checkpoint
    tx.execute(
        "INSERT INTO checkpoints (timestamp_ms) VALUES (?)",
        [self.elapsed_ms()],
    )?;
    let checkpoint_id = tx.last_insert_rowid();
    
    // Batch insert CPU samples
    let mut stmt = tx.prepare_cached(
        "INSERT INTO cpu_samples (checkpoint_id, addr, count) VALUES (?, ?, ?)"
    )?;
    for (addr, count) in self.pending_cpu.drain() {
        self.ensure_symbol(&tx, addr)?;
        stmt.execute([checkpoint_id, addr as i64, count as i64])?;
    }
    
    // Batch insert heap events
    let mut stmt = tx.prepare_cached(
        "INSERT INTO heap_events (checkpoint_id, addr, alloc_bytes, free_bytes) 
         VALUES (?, ?, ?, ?)"
    )?;
    for (addr, (alloc, free)) in self.pending_heap.drain() {
        self.ensure_symbol(&tx, addr)?;
        stmt.execute([checkpoint_id, addr as i64, alloc, free])?;
    }
    
    tx.commit()
}
```

### 7.4.3 Symbol Resolution

New addresses are resolved and inserted lazily:

```rust
fn ensure_symbol(&mut self, tx: &Transaction, addr: u64) -> Result<()> {
    if self.known_addrs.contains(&addr) {
        return Ok(());
    }
    
    let loc = self.resolver.resolve(addr);
    tx.execute(
        "INSERT OR IGNORE INTO symbols (addr, file, line, function) VALUES (?, ?, ?, ?)",
        params![addr as i64, loc.file, loc.line, loc.function],
    )?;
    
    self.known_addrs.insert(addr);
    Ok(())
}
```

## 7.5 Read Path

### 7.5.1 Top CPU Query

```sql
-- Full run
SELECT s.file, s.line, s.function, SUM(c.count) as samples
FROM cpu_samples c
JOIN symbols s ON c.addr = s.addr
GROUP BY c.addr
ORDER BY samples DESC
LIMIT ?;

-- With time window (last N seconds)
SELECT s.file, s.line, s.function, SUM(c.count) as samples
FROM cpu_samples c
JOIN symbols s ON c.addr = s.addr
JOIN checkpoints cp ON c.checkpoint_id = cp.id
WHERE cp.timestamp_ms >= (SELECT MAX(timestamp_ms) - ? FROM checkpoints)
GROUP BY c.addr
ORDER BY samples DESC
LIMIT ?;

-- With threshold (only locations > X% of total)
WITH totals AS (
    SELECT SUM(count) as total FROM cpu_samples
    WHERE checkpoint_id IN (SELECT id FROM checkpoints WHERE timestamp_ms >= ?)
)
SELECT s.file, s.line, s.function, 
       SUM(c.count) as samples,
       CAST(SUM(c.count) AS REAL) / totals.total * 100 as pct
FROM cpu_samples c
JOIN symbols s ON c.addr = s.addr
CROSS JOIN totals
WHERE c.checkpoint_id IN (SELECT id FROM checkpoints WHERE timestamp_ms >= ?)
GROUP BY c.addr
HAVING pct >= ?
ORDER BY samples DESC
LIMIT ?;
```

### 7.5.2 Top Heap Query

```sql
-- Live bytes at end of recording
SELECT s.file, s.line, s.function,
       SUM(h.alloc_bytes) - SUM(h.free_bytes) as live_bytes
FROM heap_events h
JOIN symbols s ON h.addr = s.addr
GROUP BY h.addr
HAVING live_bytes > 0
ORDER BY live_bytes DESC
LIMIT ?;

-- Live bytes at specific checkpoint
SELECT s.file, s.line, s.function,
       SUM(h.alloc_bytes) - SUM(h.free_bytes) as live_bytes
FROM heap_events h
JOIN symbols s ON h.addr = s.addr
WHERE h.checkpoint_id <= ?
GROUP BY h.addr
HAVING live_bytes > 0
ORDER BY live_bytes DESC
LIMIT ?;
```

### 7.5.3 Time Series Query

```sql
-- CPU samples over time for a specific location
SELECT cp.timestamp_ms, c.count
FROM cpu_samples c
JOIN checkpoints cp ON c.checkpoint_id = cp.id
WHERE c.addr = ?
ORDER BY cp.timestamp_ms;

-- Heap growth over time
SELECT cp.timestamp_ms,
       SUM(h.alloc_bytes) - SUM(h.free_bytes) as live_bytes
FROM heap_events h
JOIN checkpoints cp ON h.checkpoint_id = cp.id
WHERE h.checkpoint_id <= cp.id
GROUP BY cp.id
ORDER BY cp.timestamp_ms;
```

## 7.6 Concurrency

### 7.6.1 WAL Mode

SQLite WAL (Write-Ahead Logging) mode allows:
- Single writer (sampler thread)
- Multiple concurrent readers (TUI, queries)

The TUI can query the database while recording is in progress.

### 7.6.2 Connection Pooling

```rust
struct Storage {
    // Writer connection (sampler thread only)
    writer: Connection,
    // Reader connection (TUI/query thread)
    reader: Connection,
}
```

Both connections open the same file. WAL mode ensures they don't block each other.

## 7.7 Performance Considerations

### 7.7.1 Batch Inserts

All inserts within a checkpoint are batched in a single transaction. This is critical for performance - individual inserts would be 100x slower.

### 7.7.2 Prepared Statements

Use `prepare_cached()` for repeated queries to avoid re-parsing SQL.

### 7.7.3 Index Strategy

Indexes are chosen for the common query patterns:
- `idx_cpu_checkpoint`: Filter by time range
- `idx_cpu_addr`: Aggregate by location
- `idx_heap_checkpoint`: Filter by time range
- `idx_heap_addr`: Aggregate by location

### 7.7.4 Expected Sizes

| Duration | Checkpoints | CPU rows | Heap rows | File size |
|----------|-------------|----------|-----------|-----------|
| 1 min | 60 | ~60K | ~60K | ~5 MB |
| 10 min | 600 | ~600K | ~600K | ~50 MB |
| 1 hour | 3600 | ~3.6M | ~3.6M | ~300 MB |

Assumes ~1000 active locations per checkpoint. Actual sizes vary with workload.

## 7.8 Schema Versioning

The `meta.version` key tracks schema version. If rsprof opens a database with a newer schema version, it MUST fail with a clear error suggesting upgrade.

Future schema changes:
1. Add new tables/columns with defaults
2. Increment version
3. Include migration logic for older versions
