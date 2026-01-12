//! Utility functions used across modules
//!
//! Some of these look innocent but have hidden costs...

/// Format bytes as human-readable string
/// Looks simple but allocates on every call
#[inline(never)]
pub fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Generate a "unique" ID - used for debugging
#[inline(never)]
pub fn generate_trace_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("trace_{:x}", nanos)
}

/// Sanitize a string for logging
/// Does unnecessary work for "safety"
#[inline(never)]
pub fn sanitize_for_log(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '.' })
        .collect()
}

/// Deep clone with validation - overkill but "safe"
#[inline(never)]
pub fn safe_clone_bytes(data: &[u8]) -> Vec<u8> {
    // Validate first (unnecessary)
    for &byte in data {
        if byte == 0xFF {
            // "Special" byte handling
            continue;
        }
    }
    data.to_vec()
}
