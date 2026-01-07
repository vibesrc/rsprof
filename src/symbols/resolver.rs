use super::dwarf::{AddressRange, DwarfInfo};
use crate::error::Result;
use crate::process::{MemoryMaps, ProcessInfo};
use std::collections::HashMap;

/// A resolved source location
#[derive(Debug, Clone, Default)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub function: String,
}

impl Location {
    pub fn unknown() -> Self {
        Location {
            file: "[unknown]".to_string(),
            line: 0,
            column: 0,
            function: "[unknown]".to_string(),
        }
    }

    /// Format as file:line
    pub fn as_file_line(&self) -> String {
        if self.line > 0 {
            format!("{}:{}", self.file, self.line)
        } else {
            self.file.clone()
        }
    }

    /// Simplify the file path for display
    pub fn simplified_file(&self) -> String {
        simplify_path(&self.file)
    }
}

/// Symbol resolver using DWARF debug info
pub struct SymbolResolver {
    /// DWARF address ranges (sorted by start address)
    ranges: Vec<AddressRange>,
    /// Function names by address
    functions: HashMap<u64, String>,
    /// ASLR offset to subtract from runtime addresses
    aslr_offset: u64,
    /// LRU cache for recent lookups
    cache: HashMap<u64, Location>,
}

impl SymbolResolver {
    /// Create a new symbol resolver for a process
    pub fn new(proc_info: &ProcessInfo) -> Result<Self> {
        // Parse DWARF info from executable
        let dwarf = DwarfInfo::parse(proc_info.exe_path())?;

        // Get ASLR offset from memory maps
        let maps = MemoryMaps::for_pid(proc_info.pid())?;
        let aslr_offset = maps.aslr_offset(proc_info.exe_path())?;

        Ok(SymbolResolver {
            ranges: dwarf.ranges,
            functions: dwarf.functions,
            aslr_offset,
            cache: HashMap::new(),
        })
    }

    /// Number of address ranges loaded
    pub fn range_count(&self) -> usize {
        self.ranges.len()
    }

    /// Resolve a runtime address to a source location
    pub fn resolve(&self, addr: u64) -> Location {
        // Check cache first
        if let Some(loc) = self.cache.get(&addr) {
            return loc.clone();
        }

        // Adjust for ASLR
        let debug_addr = addr.saturating_sub(self.aslr_offset);

        // Binary search for the address range
        let location = match self.ranges.binary_search_by(|r| {
            if debug_addr < r.start {
                std::cmp::Ordering::Greater
            } else if debug_addr >= r.end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(idx) => {
                let range = &self.ranges[idx];
                let function = self.find_function(debug_addr);
                Location {
                    file: simplify_path(&range.file),
                    line: range.line,
                    column: range.column,
                    function,
                }
            }
            Err(_) => {
                // No line info, try to at least get function name
                let function = self.find_function(debug_addr);
                if function != "[unknown]" {
                    Location {
                        file: "[no line info]".to_string(),
                        line: 0,
                        column: 0,
                        function,
                    }
                } else {
                    Location::unknown()
                }
            }
        };

        location
    }

    /// Resolve and cache (mutable version)
    pub fn resolve_cached(&mut self, addr: u64) -> Location {
        if let Some(loc) = self.cache.get(&addr) {
            return loc.clone();
        }

        let location = self.resolve(addr);
        self.cache.insert(addr, location.clone());
        location
    }

    fn find_function(&self, addr: u64) -> String {
        // Find the function containing this address
        // Functions are stored by their start address, so we need to find
        // the largest start address <= addr
        let mut best: Option<(&u64, &String)> = None;

        for (func_addr, name) in &self.functions {
            if *func_addr <= addr {
                match best {
                    None => best = Some((func_addr, name)),
                    Some((best_addr, _)) if func_addr > best_addr => {
                        best = Some((func_addr, name));
                    }
                    _ => {}
                }
            }
        }

        best.map(|(_, name)| name.clone())
            .unwrap_or_else(|| "[unknown]".to_string())
    }
}

/// Simplify a file path for display
fn simplify_path(path: &str) -> String {
    // Remove common prefixes
    let prefixes_to_strip = [
        "/rustc/",
        "/.cargo/registry/src/",
        "/.cargo/git/checkouts/",
    ];

    let mut result = path.to_string();

    for prefix in &prefixes_to_strip {
        if let Some(idx) = result.find(prefix) {
            // Find the crate name after the prefix
            let after_prefix = &result[idx + prefix.len()..];
            if let Some(slash_idx) = after_prefix.find('/') {
                result = after_prefix[slash_idx + 1..].to_string();
            }
        }
    }

    // Try to extract just the relevant part
    // e.g., "/home/user/project/src/main.rs" -> "src/main.rs"
    if let Some(idx) = result.find("/src/") {
        result = result[idx + 1..].to_string();
    }

    result
}
