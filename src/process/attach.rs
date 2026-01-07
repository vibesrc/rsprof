use crate::error::{Error, Result};
use std::fs;
use std::path::PathBuf;

/// Information about a target process
pub struct ProcessInfo {
    pid: u32,
    name: String,
    exe_path: PathBuf,
}

impl ProcessInfo {
    /// Create ProcessInfo for a given PID
    pub fn new(pid: u32) -> Result<Self> {
        let proc_path = format!("/proc/{}", pid);

        // Check process exists
        if !std::path::Path::new(&proc_path).exists() {
            return Err(Error::ProcessNotFound(format!("PID {}", pid)));
        }

        // Get process name from /proc/[pid]/comm
        let name = fs::read_to_string(format!("{}/comm", proc_path))
            .map_err(|_| Error::ProcessNotFound(format!("Cannot read comm for PID {}", pid)))?
            .trim()
            .to_string();

        // Get executable path from /proc/[pid]/exe
        let exe_path = fs::read_link(format!("{}/exe", proc_path))
            .map_err(|e| Error::PermissionDenied(format!("Cannot read exe for PID {}: {}", pid, e)))?;

        Ok(ProcessInfo {
            pid,
            name,
            exe_path,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn exe_path(&self) -> &PathBuf {
        &self.exe_path
    }

    /// Get all thread IDs for this process
    pub fn thread_ids(&self) -> Result<Vec<u32>> {
        let task_path = format!("/proc/{}/task", self.pid);
        let mut tids = Vec::new();

        for entry in fs::read_dir(&task_path)
            .map_err(|e| Error::ProcessNotFound(format!("Cannot read tasks for PID {}: {}", self.pid, e)))?
        {
            if let Ok(entry) = entry {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(tid) = name.parse::<u32>() {
                        tids.push(tid);
                    }
                }
            }
        }

        Ok(tids)
    }
}

/// Find a process by name (pgrep-style matching)
pub fn find_process_by_name(pattern: &str) -> Result<u32> {
    let mut matches: Vec<(u32, String)> = Vec::new();

    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Check if it's a PID directory
        if let Ok(pid) = name_str.parse::<u32>() {
            let comm_path = format!("/proc/{}/comm", pid);
            if let Ok(comm) = fs::read_to_string(&comm_path) {
                let comm = comm.trim();
                // Substring match like pgrep
                if comm.contains(pattern) {
                    matches.push((pid, comm.to_string()));
                }
            }
        }
    }

    match matches.len() {
        0 => Err(Error::ProcessNotFound(format!("No process matching '{}'", pattern))),
        1 => Ok(matches[0].0),
        _ => {
            let match_list = matches
                .iter()
                .map(|(pid, name)| format!("  PID {}: {}", pid, name))
                .collect::<Vec<_>>()
                .join("\n");
            Err(Error::MultipleProcesses {
                pattern: pattern.to_string(),
                matches: match_list,
            })
        }
    }
}

/// Sanitize process name for use in filenames
#[allow(dead_code)]
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(32)
        .collect()
}
