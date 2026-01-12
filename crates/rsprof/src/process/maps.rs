use crate::error::{Error, Result};
use std::fs;
use std::path::Path;

/// A parsed memory mapping from /proc/[pid]/maps
#[derive(Debug, Clone)]
pub struct MemoryMapping {
    pub start: u64,
    pub end: u64,
    pub perms: String,
    pub offset: u64,
    pub pathname: Option<String>,
}

impl MemoryMapping {
    pub fn is_executable(&self) -> bool {
        self.perms.contains('x')
    }
}

/// Collection of memory mappings for a process
pub struct MemoryMaps {
    mappings: Vec<MemoryMapping>,
}

impl MemoryMaps {
    /// Parse /proc/[pid]/maps
    pub fn for_pid(pid: u32) -> Result<Self> {
        let path = format!("/proc/{}/maps", pid);
        let content = fs::read_to_string(&path).map_err(|e| {
            Error::ProcessNotFound(format!("Cannot read maps for PID {}: {}", pid, e))
        })?;

        let mappings = content.lines().filter_map(Self::parse_line).collect();

        Ok(MemoryMaps { mappings })
    }

    fn parse_line(line: &str) -> Option<MemoryMapping> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            return None;
        }

        // Parse address range "start-end"
        let addr_parts: Vec<&str> = parts[0].split('-').collect();
        if addr_parts.len() != 2 {
            return None;
        }

        let start = u64::from_str_radix(addr_parts[0], 16).ok()?;
        let end = u64::from_str_radix(addr_parts[1], 16).ok()?;
        let perms = parts[1].to_string();
        let offset = u64::from_str_radix(parts[2], 16).ok()?;

        // Pathname is the last field (if present)
        let pathname = if parts.len() >= 6 {
            Some(parts[5..].join(" "))
        } else {
            None
        };

        Some(MemoryMapping {
            start,
            end,
            perms,
            offset,
            pathname,
        })
    }

    /// Find the ASLR base address offset for the main executable
    pub fn aslr_offset(&self, exe_path: &Path) -> Result<u64> {
        let exe_str = exe_path.to_string_lossy();

        // Find the first executable mapping of the target binary
        for mapping in &self.mappings {
            if mapping.is_executable()
                && let Some(ref pathname) = mapping.pathname
                && (pathname == exe_str.as_ref()
                    || pathname.ends_with(exe_path.file_name().unwrap().to_str().unwrap()))
            {
                // For PIE binaries, the offset is the start address minus the file offset
                // Usually the first executable segment starts at 0 in the file
                return Ok(mapping.start - mapping.offset);
            }
        }

        // If we didn't find a match, the binary might be loaded at address 0 (non-PIE)
        // or we couldn't find it - return 0 as offset
        Ok(0)
    }

    /// Get all executable mappings
    pub fn executable_mappings(&self) -> impl Iterator<Item = &MemoryMapping> {
        self.mappings.iter().filter(|m| m.is_executable())
    }

    /// Check if an address is in an executable region
    pub fn is_executable_addr(&self, addr: u64) -> bool {
        self.mappings
            .iter()
            .any(|m| m.is_executable() && addr >= m.start && addr < m.end)
    }
}
