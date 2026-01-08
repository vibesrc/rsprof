use rusqlite::Connection;

pub const SCHEMA_VERSION: i32 = 2;

/// Create all tables (drops existing tables first to ensure clean state)
pub fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        -- Drop existing tables to ensure clean state for new session
        DROP TABLE IF EXISTS heap_samples;
        DROP TABLE IF EXISTS cpu_samples;
        DROP TABLE IF EXISTS checkpoints;
        DROP TABLE IF EXISTS locations;
        DROP TABLE IF EXISTS meta;

        -- Metadata table
        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Checkpoints (one per interval)
        CREATE TABLE checkpoints (
            id INTEGER PRIMARY KEY,
            timestamp_ms INTEGER NOT NULL
        );

        -- Unique locations (file, line, function) - normalized
        CREATE TABLE locations (
            id INTEGER PRIMARY KEY,
            file TEXT NOT NULL,
            line INTEGER NOT NULL,
            function TEXT NOT NULL,
            UNIQUE(file, line, function)
        );

        -- CPU samples per checkpoint (references location_id)
        CREATE TABLE cpu_samples (
            checkpoint_id INTEGER NOT NULL,
            location_id INTEGER NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (checkpoint_id, location_id),
            FOREIGN KEY (checkpoint_id) REFERENCES checkpoints(id),
            FOREIGN KEY (location_id) REFERENCES locations(id)
        );

        -- Index for timeseries queries by location
        CREATE INDEX idx_cpu_location ON cpu_samples(location_id);

        -- Heap samples per checkpoint (references location_id)
        CREATE TABLE heap_samples (
            checkpoint_id INTEGER NOT NULL,
            location_id INTEGER NOT NULL,
            alloc_bytes INTEGER NOT NULL DEFAULT 0,
            free_bytes INTEGER NOT NULL DEFAULT 0,
            live_bytes INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (checkpoint_id, location_id),
            FOREIGN KEY (checkpoint_id) REFERENCES checkpoints(id),
            FOREIGN KEY (location_id) REFERENCES locations(id)
        );

        -- Index for timeseries queries by location
        CREATE INDEX idx_heap_location ON heap_samples(location_id);
        "#,
    )
}

/// Set a metadata key
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)",
        [key, value],
    )?;
    Ok(())
}

/// Get a metadata key
#[allow(dead_code)]
pub fn get_meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?", [key], |row| {
        row.get(0)
    })
    .optional()
}

trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
