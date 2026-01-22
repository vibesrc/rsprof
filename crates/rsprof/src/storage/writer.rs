use super::schema::{self, SCHEMA_VERSION};
use crate::error::Result;
use crate::process::ProcessInfo;
use crate::symbols::Location;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

/// Key for aggregating samples: (file, line, function)
type LocationKey = (String, u32, String);

/// Pending heap sample data: (alloc_bytes, free_bytes, live_bytes, alloc_count, free_count)
type HeapSampleData = (i64, i64, i64, u64, u64);

/// Storage writer for profiling data
pub struct Storage {
    conn: Connection,
    start_time: Instant,
    checkpoint_id: i64,
    /// Pending CPU samples: location_id -> count
    pending_cpu: HashMap<i64, u64>,
    /// Pending heap samples: location_id -> (alloc_bytes, free_bytes, live_bytes)
    pending_heap: HashMap<i64, HeapSampleData>,
    /// Cache: (file, line, function) -> location_id
    location_cache: HashMap<LocationKey, i64>,
}

impl Storage {
    /// Create a new storage file
    pub fn new(path: &Path, proc_info: &ProcessInfo, cpu_freq: u64) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for concurrent reads during writes
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        // Checkpoint and truncate any existing WAL to clear stale state
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

        // Create tables (drops existing tables first)
        schema::create_tables(&conn)?;

        // Set metadata
        schema::set_meta(&conn, "version", &SCHEMA_VERSION.to_string())?;
        schema::set_meta(&conn, "pid", &proc_info.pid().to_string())?;
        schema::set_meta(&conn, "process_name", proc_info.name())?;
        schema::set_meta(
            &conn,
            "exe_path",
            &proc_info.exe_path().display().to_string(),
        )?;
        schema::set_meta(&conn, "start_time", &chrono::Utc::now().to_rfc3339())?;
        schema::set_meta(&conn, "cpu_freq_hz", &cpu_freq.to_string())?;

        Ok(Storage {
            conn,
            start_time: Instant::now(),
            checkpoint_id: 0,
            pending_cpu: HashMap::new(),
            pending_heap: HashMap::new(),
            location_cache: HashMap::new(),
        })
    }

    /// Get or create location_id for a (file, line, function)
    fn get_location_id(&mut self, location: &Location) -> i64 {
        let key = (
            location.file.clone(),
            location.line,
            location.function.clone(),
        );

        if let Some(&id) = self.location_cache.get(&key) {
            return id;
        }

        // Insert or get existing
        self.conn
            .execute(
                "INSERT OR IGNORE INTO locations (file, line, function) VALUES (?, ?, ?)",
                rusqlite::params![&location.file, location.line as i64, &location.function],
            )
            .ok();

        let id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM locations WHERE file = ? AND line = ? AND function = ?",
                rusqlite::params![&location.file, location.line as i64, &location.function],
                |row| row.get(0),
            )
            .unwrap_or(0);

        self.location_cache.insert(key, id);
        id
    }

    /// Record a CPU sample (aggregates by location_id)
    pub fn record_cpu_sample(&mut self, _addr: u64, location: &Location) -> i64 {
        let location_id = self.get_location_id(location);
        *self.pending_cpu.entry(location_id).or_insert(0) += 1;
        location_id
    }

    /// Record a heap sample (aggregates by location_id)
    /// Called once per checkpoint with cumulative stats from sampler.
    /// Multiple stack keys that resolve to the same location are summed.
    pub fn record_heap_sample(
        &mut self,
        location: &Location,
        alloc_bytes: i64,
        free_bytes: i64,
        live_bytes: i64,
        alloc_count: u64,
        free_count: u64,
    ) -> i64 {
        let location_id = self.get_location_id(location);
        let entry = self
            .pending_heap
            .entry(location_id)
            .or_insert((0, 0, 0, 0, 0));
        // Sum values from different stack keys that resolve to same location
        entry.0 += alloc_bytes;
        entry.1 += free_bytes;
        entry.2 += live_bytes;
        entry.3 += alloc_count;
        entry.4 += free_count;
        location_id
    }

    /// Flush pending data to a new checkpoint
    pub fn flush_checkpoint(&mut self) -> Result<()> {
        if self.pending_cpu.is_empty() && self.pending_heap.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;

        // Create checkpoint
        let timestamp_ms = self.start_time.elapsed().as_millis() as i64;
        tx.execute(
            "INSERT INTO checkpoints (timestamp_ms) VALUES (?)",
            [timestamp_ms],
        )?;
        self.checkpoint_id = tx.last_insert_rowid();

        // Insert CPU samples (just checkpoint_id, location_id, count)
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO cpu_samples (checkpoint_id, location_id, count) VALUES (?, ?, ?)",
            )?;

            for (location_id, count) in self.pending_cpu.drain() {
                stmt.execute(rusqlite::params![
                    self.checkpoint_id,
                    location_id,
                    count as i64
                ])?;
            }
        }

        // Insert heap samples
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO heap_samples (checkpoint_id, location_id, alloc_bytes, free_bytes, live_bytes, alloc_count, free_count) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;

            for (location_id, (alloc, free, live, alloc_cnt, free_cnt)) in self.pending_heap.drain()
            {
                stmt.execute(rusqlite::params![
                    self.checkpoint_id,
                    location_id,
                    alloc,
                    free,
                    live,
                    alloc_cnt as i64,
                    free_cnt as i64
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Get total samples recorded
    pub fn total_samples(&self) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(count), 0) FROM cpu_samples",
            [],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// Get number of checkpoints
    pub fn checkpoint_count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Query top CPU consumers with both total and instant percentages
    pub fn query_top_cpu_live(&self, limit: usize) -> Vec<CpuEntry> {
        query_top_cpu_live(&self.conn, limit).unwrap_or_default()
    }

    /// Query top CPU consumers - cumulative only (for `top` command)
    pub fn query_top_cpu(&self, limit: usize) -> Vec<CpuEntry> {
        query_top_cpu(&self.conn, limit, 0.0).unwrap_or_default()
    }

    /// Query top heap consumers with live bytes and delta
    pub fn query_top_heap_live(&self, limit: usize) -> Vec<HeapEntry> {
        query_top_heap_live(&self.conn, limit).unwrap_or_default()
    }

    /// Query combined CPU + Heap data for "Both" view
    pub fn query_combined_live(&self, limit: usize) -> Vec<CombinedEntry> {
        query_combined_live(&self.conn, limit).unwrap_or_default()
    }

    /// Query heap time series aggregated into buckets
    pub fn query_heap_timeseries_aggregated(
        &self,
        location_id: i64,
        start_ms: i64,
        end_ms: i64,
        num_buckets: usize,
    ) -> Vec<(f64, f64)> {
        query_heap_timeseries_aggregated(&self.conn, location_id, start_ms, end_ms, num_buckets)
    }

    /// Query sparkline data for all heap locations (recent N checkpoints)
    pub fn query_heap_sparklines(&self, num_points: usize) -> HashMap<i64, Vec<i64>> {
        query_heap_sparklines(&self.conn, num_points)
    }

    /// Query sparkline data for specific locations with zero-fill for missing checkpoints
    pub fn query_heap_sparklines_for_locations(
        &self,
        num_points: usize,
        location_ids: &[i64],
    ) -> HashMap<i64, Vec<i64>> {
        query_heap_sparklines_for_locations(&self.conn, num_points, location_ids)
    }

    /// Query time series for a specific location (time_sec, cpu_pct)
    /// Returns all checkpoints for this location
    pub fn query_location_timeseries(&self, location_id: i64) -> Vec<(f64, f64)> {
        let query_result: rusqlite::Result<Vec<(f64, f64)>> = (|| {
            let mut stmt = self.conn.prepare(
                r#"
                SELECT c.timestamp_ms,
                       CAST(cs.count AS REAL) * 100.0 / (
                           SELECT SUM(count) FROM cpu_samples WHERE checkpoint_id = c.id
                       ) as pct
                FROM checkpoints c
                JOIN cpu_samples cs ON cs.checkpoint_id = c.id AND cs.location_id = ?1
                ORDER BY c.timestamp_ms ASC
                "#,
            )?;

            let rows = stmt.query_map([location_id], |row| {
                let ts_ms: i64 = row.get(0)?;
                let pct: f64 = row.get::<_, Option<f64>>(1)?.unwrap_or(0.0);
                Ok((ts_ms as f64 / 1000.0, pct))
            })?;

            Ok(rows.filter_map(|r| r.ok()).collect())
        })();

        query_result.unwrap_or_default()
    }

    /// Query time series aggregated into buckets at the database level
    /// Returns exactly `num_buckets` points covering the time range
    pub fn query_location_timeseries_aggregated(
        &self,
        location_id: i64,
        start_ms: i64,
        end_ms: i64,
        num_buckets: usize,
    ) -> Vec<(f64, f64)> {
        if num_buckets == 0 || start_ms >= end_ms {
            return Vec::new();
        }

        let bucket_ms = (end_ms - start_ms) / num_buckets as i64;
        if bucket_ms == 0 {
            return Vec::new();
        }

        let query_result: rusqlite::Result<Vec<(f64, f64)>> = (|| {
            // Aggregate by time bucket, taking MAX cpu% in each bucket
            let mut stmt = self.conn.prepare(
                r#"
                WITH bucket_data AS (
                    SELECT
                        ((c.timestamp_ms - ?2) / ?4) as bucket_idx,
                        CAST(cs.count AS REAL) * 100.0 / (
                            SELECT SUM(count) FROM cpu_samples WHERE checkpoint_id = c.id
                        ) as pct
                    FROM checkpoints c
                    JOIN cpu_samples cs ON cs.checkpoint_id = c.id AND cs.location_id = ?1
                    WHERE c.timestamp_ms >= ?2 AND c.timestamp_ms < ?3
                )
                SELECT bucket_idx, MAX(pct) as max_pct
                FROM bucket_data
                GROUP BY bucket_idx
                ORDER BY bucket_idx ASC
                "#,
            )?;

            let rows = stmt.query_map(
                rusqlite::params![location_id, start_ms, end_ms, bucket_ms],
                |row| {
                    let bucket_idx: i64 = row.get(0)?;
                    let pct: f64 = row.get::<_, Option<f64>>(1)?.unwrap_or(0.0);
                    // Convert bucket index back to time (center of bucket)
                    let time_ms = start_ms + bucket_idx * bucket_ms + bucket_ms / 2;
                    Ok((time_ms as f64 / 1000.0, pct))
                },
            )?;

            Ok(rows.filter_map(|r| r.ok()).collect())
        })();

        query_result.unwrap_or_default()
    }
}

/// Query results for top CPU consumers
#[derive(Debug, Clone)]
pub struct CpuEntry {
    pub location_id: i64,
    pub file: String,
    pub line: u32,
    pub function: String,
    pub total_samples: u64,
    pub total_percent: f64,
    pub instant_percent: f64,
}

/// Query results for top heap consumers
#[derive(Debug, Clone)]
pub struct HeapEntry {
    pub location_id: i64,
    pub file: String,
    pub line: u32,
    pub function: String,
    pub live_bytes: i64,
    pub total_alloc_bytes: i64,
    pub total_free_bytes: i64,
    pub alloc_count: u64,
    pub free_count: u64,
}

/// Combined CPU + Heap entry for "Both" view
#[derive(Debug, Clone)]
pub struct CombinedEntry {
    pub location_id: i64,
    pub file: String,
    pub line: u32,
    pub function: String,
    pub cpu_total_pct: f64,
    pub cpu_instant_pct: f64,
    /// Total heap allocations over all time (sum of alloc_bytes)
    pub heap_total: i64,
    /// Current slice heap usage (live_bytes at current checkpoint)
    pub heap_instant: i64,
}

/// Time-series data point for a function
#[derive(Debug, Clone)]
pub struct TimeSeriesPoint {
    pub timestamp_ms: i64,
    pub percent: f64,
}

/// Query CPU% over time for a specific location
pub fn query_cpu_timeseries(
    conn: &Connection,
    location_id: i64,
) -> rusqlite::Result<Vec<TimeSeriesPoint>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT c.timestamp_ms,
               CAST(cs.count AS REAL) * 100.0 / (
                   SELECT SUM(count) FROM cpu_samples WHERE checkpoint_id = c.id
               ) as pct
        FROM checkpoints c
        JOIN cpu_samples cs ON cs.checkpoint_id = c.id AND cs.location_id = ?
        ORDER BY c.timestamp_ms
        "#,
    )?;

    let rows = stmt.query_map([location_id], |row| {
        Ok(TimeSeriesPoint {
            timestamp_ms: row.get(0)?,
            percent: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
        })
    })?;

    let mut points = Vec::new();
    for row in rows {
        points.push(row?);
    }
    Ok(points)
}

/// Query CPU% over time aggregated into buckets (for chart rendering)
/// Returns at most `num_buckets` points, each representing the MAX value in that time bucket
pub fn query_cpu_timeseries_aggregated(
    conn: &Connection,
    location_id: i64,
    start_ms: i64,
    end_ms: i64,
    num_buckets: usize,
) -> Vec<(f64, f64)> {
    if num_buckets == 0 || start_ms >= end_ms {
        return Vec::new();
    }

    let bucket_ms = (end_ms - start_ms) / num_buckets as i64;
    if bucket_ms == 0 {
        return Vec::new();
    }

    let query_result: rusqlite::Result<Vec<(f64, f64)>> = (|| {
        let mut stmt = conn.prepare(
            r#"
            WITH bucket_data AS (
                SELECT
                    ((c.timestamp_ms - ?2) / ?4) as bucket_idx,
                    CAST(cs.count AS REAL) * 100.0 / (
                        SELECT SUM(count) FROM cpu_samples WHERE checkpoint_id = c.id
                    ) as pct
                FROM checkpoints c
                JOIN cpu_samples cs ON cs.checkpoint_id = c.id AND cs.location_id = ?1
                WHERE c.timestamp_ms >= ?2 AND c.timestamp_ms < ?3
            )
            SELECT bucket_idx, MAX(pct) as max_pct
            FROM bucket_data
            GROUP BY bucket_idx
            ORDER BY bucket_idx ASC
            "#,
        )?;

        let rows = stmt.query_map(
            rusqlite::params![location_id, start_ms, end_ms, bucket_ms],
            |row| {
                let bucket_idx: i64 = row.get(0)?;
                let pct: f64 = row.get::<_, Option<f64>>(1)?.unwrap_or(0.0);
                let time_ms = start_ms + bucket_idx * bucket_ms + bucket_ms / 2;
                Ok((time_ms as f64 / 1000.0, pct))
            },
        )?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    })();

    query_result.unwrap_or_default()
}

/// Query top CPU consumers with both total and instant percentages (for live TUI)
pub fn query_top_cpu_live(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<CpuEntry>> {
    // Get totals
    let grand_total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(count), 0.0) FROM cpu_samples",
        [],
        |row| row.get(0),
    )?;

    if grand_total == 0.0 {
        return Ok(vec![]);
    }

    // Get last checkpoint for instant %
    let last_checkpoint: Option<i64> = conn
        .query_row(
            "SELECT id FROM checkpoints ORDER BY timestamp_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let instant_total: f64 = if let Some(cp_id) = last_checkpoint {
        conn.query_row(
            "SELECT COALESCE(SUM(count), 0.0) FROM cpu_samples WHERE checkpoint_id = ?",
            [cp_id],
            |row| row.get(0),
        )?
    } else {
        0.0
    };

    // Query with both totals and instant counts
    let mut stmt = conn.prepare(
        r#"
        SELECT
            l.id, l.file, l.line, l.function,
            SUM(cs.count) as total_samples,
            COALESCE((
                SELECT count FROM cpu_samples
                WHERE location_id = l.id AND checkpoint_id = ?1
            ), 0) as instant_samples
        FROM cpu_samples cs
        JOIN locations l ON cs.location_id = l.id
        GROUP BY cs.location_id
        ORDER BY total_samples DESC
        LIMIT ?2
        "#,
    )?;

    let cp_id = last_checkpoint.unwrap_or(0);
    let rows = stmt.query_map(rusqlite::params![cp_id, limit as i64], |row| {
        let total_samples: i64 = row.get(4)?;
        let instant_samples: i64 = row.get(5)?;
        Ok(CpuEntry {
            location_id: row.get(0)?,
            file: row.get(1)?,
            line: row.get::<_, i64>(2)? as u32,
            function: row.get(3)?,
            total_samples: total_samples as u64,
            total_percent: (total_samples as f64 / grand_total) * 100.0,
            instant_percent: if instant_total > 0.0 {
                (instant_samples as f64 / instant_total) * 100.0
            } else {
                0.0
            },
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }

    Ok(entries)
}

/// Query top CPU consumers - cumulative only (for `top` command)
pub fn query_top_cpu(
    conn: &Connection,
    limit: usize,
    threshold: f64,
) -> rusqlite::Result<Vec<CpuEntry>> {
    let total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(count), 0.0) FROM cpu_samples",
        [],
        |row| row.get(0),
    )?;

    if total == 0.0 {
        return Ok(vec![]);
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT l.id, l.file, l.line, l.function, SUM(cs.count) as samples
        FROM cpu_samples cs
        JOIN locations l ON cs.location_id = l.id
        GROUP BY cs.location_id
        ORDER BY samples DESC
        LIMIT ?
        "#,
    )?;

    let rows = stmt.query_map([limit as i64], |row| {
        let samples: i64 = row.get(4)?;
        let percent = (samples as f64 / total) * 100.0;
        Ok(CpuEntry {
            location_id: row.get(0)?,
            file: row.get(1)?,
            line: row.get::<_, i64>(2)? as u32,
            function: row.get(3)?,
            total_samples: samples as u64,
            total_percent: percent,
            instant_percent: 0.0, // Not used in `top` command
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let entry = row?;
        if entry.total_percent >= threshold {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// Query top heap consumers with totals
pub fn query_top_heap_live(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<HeapEntry>> {
    // Get the most recent checkpoint for live_bytes
    let last_checkpoint: Option<i64> = conn
        .query_row(
            "SELECT id FROM checkpoints ORDER BY timestamp_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let mut stmt = conn.prepare(
        r#"
        SELECT
            l.id, l.file, l.line, l.function,
            COALESCE((
                SELECT live_bytes FROM heap_samples
                WHERE location_id = l.id AND checkpoint_id = ?1
            ), 0) as live,
            SUM(hs.alloc_bytes) as total_alloc,
            SUM(hs.free_bytes) as total_free,
            SUM(hs.alloc_count) as total_alloc_count,
            SUM(hs.free_count) as total_free_count
        FROM heap_samples hs
        JOIN locations l ON hs.location_id = l.id
        GROUP BY hs.location_id
        ORDER BY live DESC, total_alloc DESC
        LIMIT ?2
        "#,
    )?;

    let cp_id = last_checkpoint.unwrap_or(0);
    let rows = stmt.query_map(rusqlite::params![cp_id, limit as i64], |row| {
        Ok(HeapEntry {
            location_id: row.get(0)?,
            file: row.get(1)?,
            line: row.get::<_, i64>(2)? as u32,
            function: row.get(3)?,
            live_bytes: row.get(4)?,
            total_alloc_bytes: row.get(5)?,
            total_free_bytes: row.get(6)?,
            alloc_count: row.get::<_, i64>(7)? as u64,
            free_count: row.get::<_, i64>(8)? as u64,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }

    Ok(entries)
}

/// Query combined CPU + Heap data for "Both" view
pub fn query_combined_live(
    conn: &Connection,
    limit: usize,
) -> rusqlite::Result<Vec<CombinedEntry>> {
    // Get CPU totals
    let cpu_grand_total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(count), 0.0) FROM cpu_samples",
        [],
        |row| row.get(0),
    )?;

    // Get last checkpoint for instant values
    let last_checkpoint: Option<i64> = conn
        .query_row(
            "SELECT id FROM checkpoints ORDER BY timestamp_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let cpu_instant_total: f64 = if let Some(cp_id) = last_checkpoint {
        conn.query_row(
            "SELECT COALESCE(SUM(count), 0.0) FROM cpu_samples WHERE checkpoint_id = ?",
            [cp_id],
            |row| row.get(0),
        )?
    } else {
        0.0
    };

    // Combined query joining CPU and Heap data
    // heap_total = sum of all allocations over time (alloc_bytes)
    // heap_instant = current slice's live bytes (live_bytes at current checkpoint)
    let mut stmt = conn.prepare(
        r#"
        SELECT
            l.id, l.file, l.line, l.function,
            COALESCE((SELECT SUM(count) FROM cpu_samples WHERE location_id = l.id), 0) as cpu_total,
            COALESCE((SELECT count FROM cpu_samples WHERE location_id = l.id AND checkpoint_id = ?1), 0) as cpu_instant,
            COALESCE((SELECT SUM(alloc_bytes) FROM heap_samples WHERE location_id = l.id), 0) as heap_total,
            COALESCE((SELECT live_bytes FROM heap_samples WHERE location_id = l.id AND checkpoint_id = ?1), 0) as heap_instant
        FROM locations l
        WHERE l.id IN (
            SELECT DISTINCT location_id FROM cpu_samples
            UNION
            SELECT DISTINCT location_id FROM heap_samples
        )
        ORDER BY cpu_total DESC
        LIMIT ?2
        "#,
    )?;

    let cp_id = last_checkpoint.unwrap_or(0);

    let rows = stmt.query_map(rusqlite::params![cp_id, limit as i64], |row| {
        let cpu_total: i64 = row.get(4)?;
        let cpu_instant: i64 = row.get(5)?;
        let heap_total: i64 = row.get(6)?;
        let heap_instant: i64 = row.get(7)?;

        Ok(CombinedEntry {
            location_id: row.get(0)?,
            file: row.get(1)?,
            line: row.get::<_, i64>(2)? as u32,
            function: row.get(3)?,
            cpu_total_pct: if cpu_grand_total > 0.0 {
                (cpu_total as f64 / cpu_grand_total) * 100.0
            } else {
                0.0
            },
            cpu_instant_pct: if cpu_instant_total > 0.0 {
                (cpu_instant as f64 / cpu_instant_total) * 100.0
            } else {
                0.0
            },
            heap_total,
            heap_instant,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }

    Ok(entries)
}

/// Query heap bytes over time aggregated into buckets (for chart rendering)
pub fn query_heap_timeseries_aggregated(
    conn: &Connection,
    location_id: i64,
    start_ms: i64,
    end_ms: i64,
    num_buckets: usize,
) -> Vec<(f64, f64)> {
    if num_buckets == 0 || start_ms >= end_ms {
        return Vec::new();
    }

    let bucket_ms = (end_ms - start_ms) / num_buckets as i64;
    if bucket_ms == 0 {
        return Vec::new();
    }

    let query_result: rusqlite::Result<Vec<(f64, f64)>> = (|| {
        let mut stmt = conn.prepare(
            r#"
            WITH bucket_data AS (
                SELECT
                    ((c.timestamp_ms - ?2) / ?4) as bucket_idx,
                    hs.live_bytes
                FROM checkpoints c
                JOIN heap_samples hs ON hs.checkpoint_id = c.id AND hs.location_id = ?1
                WHERE c.timestamp_ms >= ?2 AND c.timestamp_ms < ?3
            )
            SELECT bucket_idx, MAX(live_bytes) as max_bytes
            FROM bucket_data
            GROUP BY bucket_idx
            ORDER BY bucket_idx ASC
            "#,
        )?;

        let rows = stmt.query_map(
            rusqlite::params![location_id, start_ms, end_ms, bucket_ms],
            |row| {
                let bucket_idx: i64 = row.get(0)?;
                let bytes: i64 = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
                let time_ms = start_ms + bucket_idx * bucket_ms + bucket_ms / 2;
                Ok((time_ms as f64 / 1000.0, bytes as f64))
            },
        )?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    })();

    query_result.unwrap_or_default()
}

/// Query sparkline data for all heap locations (recent N checkpoints)
/// Returns HashMap<location_id, Vec<live_bytes>> for sparkline rendering
pub fn query_heap_sparklines(conn: &Connection, num_points: usize) -> HashMap<i64, Vec<i64>> {
    query_heap_sparklines_for_locations(conn, num_points, &[])
}

/// Query sparkline data for specific locations (or all if location_ids is empty)
/// Returns HashMap<location_id, Vec<live_bytes>> with exactly num_points values per location
/// Missing checkpoints are filled with 0
pub fn query_heap_sparklines_for_locations(
    conn: &Connection,
    num_points: usize,
    location_ids: &[i64],
) -> HashMap<i64, Vec<i64>> {
    let query_result: rusqlite::Result<HashMap<i64, Vec<i64>>> = (|| {
        // Get the last N checkpoints in chronological order
        let mut cp_stmt =
            conn.prepare("SELECT id FROM checkpoints ORDER BY timestamp_ms DESC LIMIT ?")?;
        let checkpoint_ids: Vec<i64> = cp_stmt
            .query_map([num_points as i64], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if checkpoint_ids.is_empty() {
            return Ok(HashMap::new());
        }

        // Reverse to get chronological order (oldest first)
        let checkpoint_ids: Vec<i64> = checkpoint_ids.into_iter().rev().collect();
        let num_checkpoints = checkpoint_ids.len();

        // Create a map from checkpoint_id to index for quick lookup
        let cp_index: std::collections::HashMap<i64, usize> = checkpoint_ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        // Build query based on whether we have specific location_ids
        let cp_placeholders = checkpoint_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");

        let query = if location_ids.is_empty() {
            format!(
                r#"
                SELECT hs.location_id, hs.checkpoint_id, hs.live_bytes
                FROM heap_samples hs
                WHERE hs.checkpoint_id IN ({})
                "#,
                cp_placeholders
            )
        } else {
            let loc_placeholders = location_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            format!(
                r#"
                SELECT hs.location_id, hs.checkpoint_id, hs.live_bytes
                FROM heap_samples hs
                WHERE hs.checkpoint_id IN ({})
                AND hs.location_id IN ({})
                "#,
                cp_placeholders, loc_placeholders
            )
        };

        let mut stmt = conn.prepare(&query)?;

        // Build parameter list
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = checkpoint_ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::ToSql>)
            .collect();

        for loc_id in location_ids {
            params.push(Box::new(*loc_id));
        }

        let params_ref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        // Collect all data points with their checkpoint index
        let mut raw_data: HashMap<i64, Vec<(usize, i64)>> = HashMap::new();

        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?, // location_id
                row.get::<_, i64>(1)?, // checkpoint_id
                row.get::<_, i64>(2)?, // live_bytes
            ))
        })?;

        for row in rows {
            if let Ok((loc_id, cp_id, live_bytes)) = row
                && let Some(&idx) = cp_index.get(&cp_id)
            {
                raw_data.entry(loc_id).or_default().push((idx, live_bytes));
            }
        }

        // Build result with zeros for missing checkpoints
        let mut result: HashMap<i64, Vec<i64>> = HashMap::new();

        // For specified locations, ensure they all have entries (even if all zeros)
        for &loc_id in location_ids {
            result.insert(loc_id, vec![0i64; num_checkpoints]);
        }

        // Fill in actual data
        for (loc_id, data_points) in raw_data {
            let values = result
                .entry(loc_id)
                .or_insert_with(|| vec![0i64; num_checkpoints]);
            for (idx, live_bytes) in data_points {
                values[idx] = live_bytes;
            }
        }

        Ok(result)
    })();

    query_result.unwrap_or_default()
}
