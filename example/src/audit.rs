//! Audit logging with a bounded "leak" that recovers.

use crate::model::{Request, Response};
use crate::utils;

pub struct AuditLog {
    pending: Vec<AuditEntry>,
    archive: Vec<AuditEntry>,
    shadow_bytes: Vec<Vec<u8>>,
    flush_count: u64,
}

#[allow(dead_code)] // Stored for compliance/debugging; not read in normal flow.
struct AuditEntry {
    key: String,
    status: u16,
    details: String,
    backup: Vec<u8>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            archive: Vec::new(),
            shadow_bytes: Vec::new(),
            flush_count: 0,
        }
    }

    #[inline(never)]
    pub fn record_event(&mut self, request: &Request, response: &Response) {
        let entry = self.create_entry(request, response);
        self.pending.push(entry);
        if self.pending.len() >= 64 {
            self.flush();
        }
    }

    #[inline(never)]
    fn create_entry(&self, request: &Request, response: &Response) -> AuditEntry {
        let details = format!(
            "op=handle key={} status={} session={} user={}",
            utils::sanitize_for_log(&request.key),
            response.status,
            request.session_id,
            request.user_id,
        );
        let backup = self.build_backup_blob(&request.payload);
        AuditEntry {
            key: request.key.clone(),
            status: response.status,
            details,
            backup,
        }
    }

    #[inline(never)]
    fn build_backup_blob(&self, payload: &[u8]) -> Vec<u8> {
        let mut blob = Vec::with_capacity(payload.len() + 64);
        blob.extend_from_slice(b"AUDIT_V2:");
        blob.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        blob.extend_from_slice(payload);
        blob
    }

    #[inline(never)]
    fn flush(&mut self) {
        self.flush_count += 1;

        for entry in self.pending.drain(..) {
            self.archive.push(entry);
        }

        // Keep a shadow copy for compliance checks.
        if let Some(entry) = self.archive.last() {
            self.shadow_bytes.push(entry.backup.clone());
        }

        // Bound memory growth by periodic cleanup.
        if self.archive.len() > 2000 {
            self.archive.drain(0..800);
        }

        if self.shadow_bytes.len() > 300 {
            self.shadow_bytes.drain(0..150);
        }
    }
}
