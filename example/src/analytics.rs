//! Analytics path with allocation-heavy tracking and periodic cleanup.

use crate::model::{Request, Response};
use crate::utils;

pub struct AnalyticsEngine {
    events: Vec<AnalyticsEvent>,
    shadow_log: Vec<String>,
    flush_counter: u64,
}

#[allow(dead_code)] // Stored for analysis/replay; not read in the hot path.
struct AnalyticsEvent {
    session_id: u64,
    size: usize,
    payload: Vec<u8>,
}

impl AnalyticsEngine {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            shadow_log: Vec::new(),
            flush_counter: 0,
        }
    }

    #[inline(never)]
    pub fn handle(&mut self, request: &Request, _headers: &[(String, String)]) -> Response {
        self.record_event(request);
        let summary = self.build_summary(request);
        Response {
            status: 202,
            body: summary.into_bytes(),
            cacheable: false,
        }
    }

    #[inline(never)]
    fn record_event(&mut self, request: &Request) {
        let payload = request.payload.clone();
        let mut checksum = 0u64;
        for chunk in payload.chunks(16) {
            checksum = checksum.wrapping_add(utils::slow_hash(chunk));
        }
        let event = AnalyticsEvent {
            session_id: request.session_id,
            size: payload.len(),
            payload,
        };
        self.events.push(event);
        if checksum & 0x3 == 0 {
            self.shadow_log.push(format!("checksum={:x}", checksum));
        }

        if self.events.len() >= 1500 {
            self.compact();
        }
    }

    #[inline(never)]
    fn compact(&mut self) {
        self.flush_counter += 1;

        let mut total_bytes = 0usize;
        for event in &self.events {
            total_bytes += event.size;
        }

        let note = format!(
            "flush={} sessions={} bytes={}",
            self.flush_counter,
            self.events.len(),
            utils::format_bytes(total_bytes)
        );
        self.shadow_log.push(note);

        // Keep only the most recent events.
        let keep = 500;
        if self.events.len() > keep {
            let drain_count = self.events.len() - keep;
            self.events.drain(0..drain_count);
        }

        // Periodically clear shadow log to keep memory bounded.
        if self.shadow_log.len() > 200 {
            self.shadow_log.drain(0..100);
        }
    }

    #[inline(never)]
    fn build_summary(&self, request: &Request) -> String {
        let trace_id = utils::generate_trace_id(request.id);
        format!(
            "[{}] analytics session={} size={} events={} payload_preview={:?}",
            trace_id,
            request.session_id,
            utils::format_bytes(request.payload.len()),
            self.events.len(),
            &request.payload[..request.payload.len().min(16)],
        )
    }
}
