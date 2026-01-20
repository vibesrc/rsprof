//! Validation module - Contains BOTTLENECK #2 (Memory)

use crate::app::Request;
use crate::utils;

pub struct InputValidator {
    // Looks like a simple validator...
    rules: Vec<ValidationRule>,
    history: Vec<ValidationRecord>,
}

struct ValidationRule {
    name: String,
    min_len: usize,
    max_len: usize,
}

#[allow(dead_code)] // Fields are intentionally unused - it's a memory leak!
struct ValidationRecord {
    key: String,
    passed: bool,
    timestamp: u64,
    // BOTTLENECK #2: Storing way too much data per validation
    debug_info: String,
}

impl InputValidator {
    pub fn new() -> Self {
        Self {
            rules: vec![
                ValidationRule {
                    name: "length".into(),
                    min_len: 1,
                    max_len: 1000,
                },
                ValidationRule {
                    name: "payload".into(),
                    min_len: 0,
                    max_len: 10000,
                },
            ],
            history: Vec::new(),
        }
    }

    #[inline(never)]
    pub fn validate(&mut self, request: &Request) -> bool {
        let passed = self.check_rules(request);

        // Record validation - this is the memory leak!
        self.record_validation(request, passed);

        passed
    }

    #[inline(never)]
    fn check_rules(&self, request: &Request) -> bool {
        for rule in &self.rules {
            let len = match rule.name.as_str() {
                "length" => request.key.len(),
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
    fn record_validation(&mut self, request: &Request, passed: bool) {
        // BOTTLENECK #2: Accumulating history forever with bloated records
        let record = ValidationRecord {
            key: request.key.clone(),
            passed,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            // This generates a huge string every time!
            debug_info: self.generate_debug_info(request),
        };
        self.history.push(record);

        // "Cleanup" that doesn't actually help much
        if self.history.len() > 10000 {
            self.history.drain(0..1000);
        }
    }

    #[inline(never)]
    fn generate_debug_info(&self, request: &Request) -> String {
        // Creates a large string for every single validation
        // Uses utils functions for extra overhead
        let trace_id = utils::generate_trace_id();
        let sanitized_key = utils::sanitize_for_log(&request.key);
        let size_str = utils::format_bytes(request.payload.len());

        format!(
            "[{}] Validated request '{}' with {} payload. \
             Rules checked: {:?}. Priority level: {}. \
             Current history size: {}. Timestamp: {:?}. \
             Payload preview: {:?}",
            trace_id,
            sanitized_key,
            size_str,
            self.rules.iter().map(|r| &r.name).collect::<Vec<_>>(),
            request.priority,
            self.history.len(),
            std::time::SystemTime::now(),
            &request.payload[..request.payload.len().min(32)],
        )
    }
}
