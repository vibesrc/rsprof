use crate::error::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Profile info extracted from a database file
pub struct ProfileInfo {
    pub path: PathBuf,
    pub process_name: String,
    pub pid: u32,
    pub duration_secs: f64,
    pub samples: u64,
    pub created: String,
}

/// Find all rsprof profile databases in a directory
pub fn find_profiles(dir: &Path) -> Result<Vec<ProfileInfo>> {
    let mut profiles = Vec::new();

    let entries = std::fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "db").unwrap_or(false) {
            // Check if filename matches rsprof.*.db pattern
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.starts_with("rsprof.")
                && let Ok(info) = get_profile_info(&path)
            {
                profiles.push(info);
            }
        }
    }

    // Sort by modification time (most recent first)
    profiles.sort_by(|a, b| b.created.cmp(&a.created));

    Ok(profiles)
}

/// Get the most recent profile in a directory
pub fn most_recent_profile(dir: &Path) -> Result<Option<PathBuf>> {
    let profiles = find_profiles(dir)?;
    Ok(profiles.into_iter().next().map(|p| p.path))
}

/// Extract metadata from a profile database
fn get_profile_info(path: &Path) -> Result<ProfileInfo> {
    let conn = Connection::open(path)?;

    let process_name: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'process_name'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "unknown".to_string());

    let pid: u32 = conn
        .query_row("SELECT value FROM meta WHERE key = 'pid'", [], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let created: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'start_time'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "unknown".to_string());

    let duration_ms: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(timestamp_ms), 0) FROM checkpoints",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let samples: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(count), 0) FROM cpu_samples",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    Ok(ProfileInfo {
        path: path.to_path_buf(),
        process_name,
        pid,
        duration_secs: duration_ms as f64 / 1000.0,
        samples: samples as u64,
        created,
    })
}

/// Run the list command
pub fn run(dir: Option<&Path>) -> Result<()> {
    let search_dir = dir.unwrap_or_else(|| Path::new("."));
    let profiles = find_profiles(search_dir)?;

    if profiles.is_empty() {
        println!("No rsprof profiles found in {}", search_dir.display());
        return Ok(());
    }

    println!(
        "{:<40} {:>12} {:>10} {:>10}",
        "FILE", "PROCESS", "DURATION", "SAMPLES"
    );
    println!("{}", "-".repeat(76));

    for profile in profiles {
        let filename = profile
            .path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();

        let duration = if profile.duration_secs >= 60.0 {
            format!(
                "{:.0}m{:.0}s",
                profile.duration_secs / 60.0,
                profile.duration_secs % 60.0
            )
        } else {
            format!("{:.1}s", profile.duration_secs)
        };

        println!(
            "{:<40} {:>12} {:>10} {:>10}",
            filename, profile.process_name, duration, profile.samples
        );
    }

    Ok(())
}
