//! Audit logging module - Contains a MEMORY LEAK
//!
//! This module "safely" flushes old entries but the flush is buggy
//! and retains references, causing memory to grow unboundedly.

use std::sync::{Mutex, OnceLock};

static AUDIT_LOG: OnceLock<Mutex<AuditLogger>> = OnceLock::new();

struct AuditLogger {
    /// Current entries awaiting flush
    pending_entries: Vec<AuditEntry>,
    /// "Archived" entries - supposedly flushed but we keep copies "for safety"
    archived_entries: Vec<AuditEntry>,
    /// Flush counter
    flush_count: u64,
}

#[derive(Clone)]
struct AuditEntry {
    timestamp: u64,
    operation: String,
    details: String,
    /// "Backup" of the data for compliance - this is the leak!
    backup_blob: Vec<u8>,
}

impl AuditLogger {
    fn new() -> Self {
        Self {
            pending_entries: Vec::new(),
            archived_entries: Vec::new(),
            flush_count: 0,
        }
    }
}

/// Log an audit event - called on every request
#[inline(never)]
pub fn log_audit_event(operation: &str, key: &str, payload: &[u8]) {
    let logger = AUDIT_LOG.get_or_init(|| Mutex::new(AuditLogger::new()));

    if let Ok(mut guard) = logger.lock() {
        let entry = create_audit_entry(operation, key, payload);
        guard.pending_entries.push(entry);

        // "Flush" every 50 entries for safety
        if guard.pending_entries.len() >= 50 {
            flush_audit_log_unsafe(&mut guard);
        }
    }
}

/// MEMORY LEAK: Creates audit entry with backup blob
#[inline(never)]
fn create_audit_entry(operation: &str, key: &str, payload: &[u8]) -> AuditEntry {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    // Build detailed audit string
    let details = build_audit_details(operation, key, payload.len());

    // Create expanded backup with metadata
    let backup = create_backup_blob(payload);

    AuditEntry {
        timestamp,
        operation: operation.to_string(),
        details,
        backup_blob: backup,
    }
}

/// MEMORY LEAK: Creates bloated backup blob with headers
#[inline(never)]
fn create_backup_blob(payload: &[u8]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(payload.len() + 128);
    // Add "header" metadata
    blob.extend_from_slice(b"AUDIT_BACKUP_V1:");
    blob.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    blob.extend_from_slice(b":DATA:");
    blob.extend_from_slice(payload);
    blob.extend_from_slice(b":END");
    blob
}

/// MEMORY LEAK: Build verbose audit details string
#[inline(never)]
fn build_audit_details(operation: &str, key: &str, size: usize) -> String {
    format!(
        "OP={} KEY={} SIZE={} THREAD={:?} TIME={:?}",
        operation,
        key,
        size,
        std::thread::current().id(),
        std::time::SystemTime::now(),
    )
}

/// MEMORY LEAK: "Flushes" but keeps archived copies
#[inline(never)]
fn flush_audit_log_unsafe(logger: &mut AuditLogger) {
    logger.flush_count += 1;

    // "Archive" the entries instead of deleting - for "compliance"
    // This is the memory leak!
    for entry in logger.pending_entries.drain(..) {
        // Clone into archive "for safety" - doubles the memory!
        logger.archived_entries.push(entry.clone());
    }

    // "Cleanup" that doesn't actually help - only trims if huge
    if logger.archived_entries.len() > 10000 {
        // Only remove 10% - leak continues!
        let drain_count = logger.archived_entries.len() / 10;
        logger.archived_entries.drain(0..drain_count);
    }
}

/// Get current memory pressure (for debugging)
#[inline(never)]
pub fn get_audit_memory_usage() -> usize {
    let logger = AUDIT_LOG.get_or_init(|| Mutex::new(AuditLogger::new()));
    if let Ok(guard) = logger.lock() {
        let pending: usize = guard.pending_entries.iter()
            .map(|e| e.backup_blob.len() + e.details.len() + e.operation.len())
            .sum();
        let archived: usize = guard.archived_entries.iter()
            .map(|e| e.backup_blob.len() + e.details.len() + e.operation.len())
            .sum();
        pending + archived
    } else {
        0
    }
}
