use crate::cli::TopMetric;
use crate::error::Result;
use crate::storage::query_top_cpu;
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

pub fn run(
    file: &Path,
    metric: TopMetric,
    limit: usize,
    threshold: f64,
    _since: Option<Duration>,
    _until: Option<Duration>,
    json: bool,
    csv: bool,
    _filter: Option<String>,
) -> Result<()> {
    let conn = Connection::open(file)?;

    // Get metadata
    let duration_ms: Option<i64> = conn
        .query_row(
            "SELECT MAX(timestamp_ms) FROM checkpoints",
            [],
            |row| row.get(0),
        )
        .ok();

    let total_samples: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(count), 0) FROM cpu_samples",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    match metric {
        TopMetric::Cpu => {
            let entries = query_top_cpu(&conn, limit, threshold)?;

            if json {
                print_cpu_json(file, duration_ms, total_samples, &entries);
            } else if csv {
                print_cpu_csv(&entries);
            } else {
                print_cpu_table(file, duration_ms, total_samples, &entries);
            }
        }
        TopMetric::Heap => {
            eprintln!("Heap profiling not yet implemented");
        }
    }

    Ok(())
}

fn print_cpu_table(
    file: &Path,
    duration_ms: Option<i64>,
    total_samples: i64,
    entries: &[crate::storage::CpuEntry],
) {
    // Header comment
    println!("# {}", file.display());
    if let Some(ms) = duration_ms {
        let secs = ms / 1000;
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        println!("# Duration: {}m{:02}s | Samples: {}", mins, remaining_secs, total_samples);
    }
    println!();

    // Simple aligned output - LLM-friendly
    println!("{:>6}  {:<30}  {}", "CPU%", "LOCATION", "FUNCTION");
    println!("{}", "-".repeat(80));

    for entry in entries {
        let location = format_location(&entry.file, entry.line);
        let function = format_function(&entry.function);
        println!("{:>5.1}%  {:<30}  {}", entry.total_percent, location, function);
    }
}

fn print_cpu_json(
    file: &Path,
    duration_ms: Option<i64>,
    total_samples: i64,
    entries: &[crate::storage::CpuEntry],
) {
    println!("{{");
    println!("  \"file\": \"{}\",", file.display());
    if let Some(ms) = duration_ms {
        println!("  \"duration_ms\": {},", ms);
    }
    println!("  \"total_samples\": {},", total_samples);
    println!("  \"entries\": [");

    for (i, entry) in entries.iter().enumerate() {
        let comma = if i < entries.len() - 1 { "," } else { "" };
        println!(
            "    {{ \"cpu_pct\": {:.1}, \"file\": \"{}\", \"line\": {}, \"function\": \"{}\" }}{}",
            entry.total_percent,
            entry.file.replace('\\', "\\\\").replace('"', "\\\""),
            entry.line,
            entry.function.replace('\\', "\\\\").replace('"', "\\\""),
            comma
        );
    }

    println!("  ]");
    println!("}}");
}

fn print_cpu_csv(entries: &[crate::storage::CpuEntry]) {
    println!("cpu_pct,file,line,function");
    for entry in entries {
        println!(
            "{:.1},{},{},\"{}\"",
            entry.total_percent, entry.file, entry.line, entry.function
        );
    }
}

/// Format a file path for display - keep the most relevant parts
fn format_location(file: &str, line: u32) -> String {
    let simplified = simplify_path(file);
    if line > 0 {
        format!("{}:{}", simplified, line)
    } else {
        simplified
    }
}

/// Simplify a file path - extract the most meaningful part
fn simplify_path(path: &str) -> String {
    // Handle [no line info] and similar
    if path.starts_with('[') {
        return path.to_string();
    }

    // Extract just filename for stdlib paths
    if path.contains("/rust/library/") || path.contains("/rustc/") {
        if let Some(filename) = path.rsplit('/').next() {
            return format!("<std>/{}", filename);
        }
    }

    // For cargo dependencies, extract crate name and file
    if path.contains("/.cargo/") {
        // Try to find the crate name
        if let Some(idx) = path.find("/src/") {
            let before_src = &path[..idx];
            if let Some(crate_start) = before_src.rfind('/') {
                let crate_name = &before_src[crate_start + 1..];
                let after_src = &path[idx + 5..]; // skip "/src/"
                return format!("<{}>/{}", crate_name, after_src);
            }
        }
    }

    // For local paths, try to find src/
    if let Some(idx) = path.find("/src/") {
        return path[idx + 1..].to_string(); // keep "src/..."
    }

    // For examples/
    if let Some(idx) = path.find("/examples/") {
        return path[idx + 1..].to_string();
    }

    // Fallback: just the filename
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// Format a function name - remove hash suffix and simplify
fn format_function(func: &str) -> String {
    let mut result = func.to_string();

    // Remove the hash suffix (e.g., "::h1234567890abcdef")
    if let Some(idx) = result.rfind("::h") {
        let suffix = &result[idx + 3..];
        if suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
            result = result[..idx].to_string();
        }
    }

    // Simplify common prefixes
    let prefixes_to_shorten = [
        ("core::slice::sort::", "sort::"),
        ("core::ptr::", "ptr::"),
        ("core::fmt::", "fmt::"),
        ("core::iter::", "iter::"),
        ("core::hash::", "hash::"),
        ("core::str::", "str::"),
        ("core::num::", "num::"),
        ("alloc::vec::", "Vec::"),
        ("alloc::string::", "String::"),
        ("alloc::alloc::", "alloc::"),
        ("hashbrown::raw::", "hashbrown::"),
        ("std::collections::hash_map::", "HashMap::"),
    ];

    for (prefix, replacement) in prefixes_to_shorten {
        if result.starts_with(prefix) {
            result = format!("{}{}", replacement, &result[prefix.len()..]);
            break;
        }
    }

    // Remove <...> generic parameters for readability
    while let (Some(start), Some(end)) = (result.find('<'), result.rfind('>')) {
        if start < end {
            // Check if it's simple enough to keep
            let generic = &result[start..=end];
            if generic.len() > 20 || generic.contains("::") {
                result = format!("{}<_>{}", &result[..start], &result[end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    result
}


