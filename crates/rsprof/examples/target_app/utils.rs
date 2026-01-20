//! Utility helpers used across modules.

pub fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn sanitize_for_log(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '.'
            }
        })
        .collect()
}

pub fn generate_trace_id(seed: u64) -> String {
    format!("trace_{:x}", seed.wrapping_mul(0x9e3779b97f4a7c15))
}

pub fn slow_hash(data: &[u8]) -> u64 {
    let mut hash = 0u64;
    for (i, &byte) in data.iter().enumerate() {
        for j in 0..64 {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
            hash ^= (i as u64).wrapping_mul(j as u64);
        }
    }
    hash
}

pub fn fill_payload(payload: &mut [u8], seed: u64) {
    let mut value = seed as u8;
    for byte in payload.iter_mut() {
        value = value.wrapping_mul(37).wrapping_add(17);
        *byte = value;
    }
}

pub fn parse_headers(payload: &[u8]) -> Vec<(String, String)> {
    let mut headers = Vec::with_capacity(6);
    let mut checksum = 0u64;
    for chunk in payload.chunks(12) {
        checksum = checksum.wrapping_add(slow_hash(chunk));
    }
    headers.push(("x-check".to_string(), format!("{:x}", checksum)));
    headers.push(("x-len".to_string(), payload.len().to_string()));
    headers.push(("x-mode".to_string(), format!("{}", payload[0] % 4)));
    headers
}
