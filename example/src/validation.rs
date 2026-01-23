//! Validation module with expensive debug tracing (bounded leak).

use crate::model::Request;
use crate::utils;

pub struct InputValidator {
    rules: Vec<ValidationRule>,
    recent: Vec<ValidationRecord>,
    leak_guard: usize,
}

struct ValidationRule {
    name: String,
    min_len: usize,
    max_len: usize,
}

#[allow(dead_code)] // Captured for troubleshooting; not used in the hot path.
struct ValidationRecord {
    key: String,
    passed: bool,
    debug_info: String,
}

impl InputValidator {
    pub fn new() -> Self {
        Self {
            rules: vec![
                ValidationRule {
                    name: "key".into(),
                    min_len: 2,
                    max_len: 128,
                },
                ValidationRule {
                    name: "payload".into(),
                    min_len: 32,
                    max_len: 4096,
                },
            ],
            recent: Vec::new(),
            leak_guard: 0,
        }
    }

    #[inline(never)]
    pub fn validate(&mut self, request: &Request, headers: &[(String, String)]) -> bool {
        let passed = self.check_rules(request, headers);
        self.record_validation(request, headers, passed);
        passed
    }

    #[inline(never)]
    fn check_rules(&self, request: &Request, headers: &[(String, String)]) -> bool {
        let header_len = headers.len();
        for rule in &self.rules {
            let len = match rule.name.as_str() {
                "key" => request.key.len() + header_len,
                "payload" => request.payload.len(),
                _ => 0,
            };
            if len < rule.min_len || len > rule.max_len {
                return false;
            }
        }
        true
    }

    #[inline(never)]
    fn record_validation(&mut self, request: &Request, headers: &[(String, String)], passed: bool) {
        let record = ValidationRecord {
            key: request.key.clone(),
            passed,
            debug_info: self.generate_debug_info(request, headers),
        };
        self.recent.push(record);
        self.leak_guard += 1;

        if self.recent.len() > 4000 {
            let drain_count = 1200;
            self.recent.drain(0..drain_count);
        }

        if self.leak_guard >= 2500 {
            self.recent.shrink_to(3000);
            self.leak_guard = 0;
        }
    }

    #[inline(never)]
    fn generate_debug_info(&self, request: &Request, headers: &[(String, String)]) -> String {
        let trace_id = utils::generate_trace_id(request.id);
        let sanitized_key = utils::sanitize_for_log(&request.key);
        let size_str = utils::format_bytes(request.payload.len());
        let header_keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();

        format!(
            "[{}] key={} size={} headers={:?} route={:?} flags={} payload_preview={:?}",
            trace_id,
            sanitized_key,
            size_str,
            header_keys,
            request.route,
            request.flags,
            &request.payload[..request.payload.len().min(24)],
        )
    }
}
