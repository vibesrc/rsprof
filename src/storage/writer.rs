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

/// Storage writer for profiling data
pub struct Storage {
    conn: Connection,
    start_time: Instant,
    checkpoint_id: i64,
    /// Pending CPU samples: location_id -> count
    pending_cpu: HashMap<i64, u64>,
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
        schema::set_meta(
            &conn,
            "start_time",
            &chrono::Utc::now().to_rfc3339(),
        )?;
        schema::set_meta(&conn, "cpu_freq_hz", &cpu_freq.to_string())?;

        Ok(Storage {
            conn,
            start_time: Instant::now(),
            checkpoint_id: 0,
            pending_cpu: HashMap::new(),
            location_cache: HashMap::new(),
        })
    }

    /// Get or create location_id for a (file, line, function)
    fn get_location_id(&mut self, location: &Location) -> i64 {
        let key = (location.file.clone(), location.line, location.function.clone());

        if let Some(&id) = self.location_cache.get(&key) {
            return id;
        }

        // Insert or get existing
        self.conn.execute(
            "INSERT OR IGNORE INTO locations (file, line, function) VALUES (?, ?, ?)",
            rusqlite::params![&location.file, location.line as i64, &location.function],
        ).ok();

        let id: i64 = self.conn.query_row(
            "SELECT id FROM locations WHERE file = ? AND line = ? AND function = ?",
            rusqlite::params![&location.file, location.line as i64, &location.function],
            |row| row.get(0),
        ).unwrap_or(0);

        self.location_cache.insert(key, id);
        id
    }

    /// Record a CPU sample (aggregates by location_id)
    pub fn record_cpu_sample(&mut self, _addr: u64, location: &Location) {
        let location_id = self.get_location_id(location);
        *self.pending_cpu.entry(location_id).or_insert(0) += 1;
    }

    /// Flush pending data to a new checkpoint
    pub fn flush_checkpoint(&mut self) -> Result<()> {
        if self.pending_cpu.is_empty() {
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

        tx.commit()?;
        Ok(())
    }

    /// Get total samples recorded
    pub fn total_samples(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(count), 0) FROM cpu_samples", [], |row| {
                row.get(0)
            })?;
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
pub fn query_top_cpu_live(
    conn: &Connection,
    limit: usize,
) -> rusqlite::Result<Vec<CpuEntry>> {
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
