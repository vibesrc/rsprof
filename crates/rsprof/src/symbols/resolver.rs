use super::dwarf::{AddressRange, DwarfInfo};
use crate::error::Result;
use crate::process::{MemoryMaps, ProcessInfo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Function declarations: function name -> (file, line)
    function_decls: HashMap<String, (String, u32)>,
    /// ASLR offset to subtract from runtime addresses
    aslr_offset: u64,
    /// LRU cache for recent lookups
    cache: HashMap<u64, Location>,
    /// Root directory for the target app's source (used to filter dependencies)
    target_root: Option<PathBuf>,
}

impl SymbolResolver {
    /// Create a new symbol resolver for a process
    pub fn new(proc_info: &ProcessInfo) -> Result<Self> {
        // Parse DWARF info from executable
        // Use proc_exe_path which works even if binary was deleted/rebuilt
        let dwarf = DwarfInfo::parse(proc_info.proc_exe_path())?;
        let target_root = detect_target_root(&dwarf, proc_info.exe_path());

        // Get ASLR offset from memory maps
        let maps = MemoryMaps::for_pid(proc_info.pid())?;
        let aslr_offset = maps.aslr_offset(proc_info.exe_path())?;

        Ok(SymbolResolver {
            ranges: dwarf.ranges,
            functions: dwarf.functions,
            function_decls: dwarf.function_decls,
            aslr_offset,
            cache: HashMap::new(),
            target_root,
        })
    }

    /// Number of address ranges loaded
    pub fn range_count(&self) -> usize {
        self.ranges.len()
    }

    /// Get the ASLR offset being used
    pub fn aslr_offset(&self) -> u64 {
        self.aslr_offset
    }

    /// Resolve a runtime address to a source location
    pub fn resolve(&self, addr: u64) -> Location {
        // Check cache first
        if let Some(loc) = self.cache.get(&addr) {
            return loc.clone();
        }

        // Adjust for ASLR
        let debug_addr = addr.saturating_sub(self.aslr_offset);

        // Get function name first
        let function = self.find_function(debug_addr);

        // Binary search for the address range

        match self.ranges.binary_search_by(|r| {
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
                let file = simplify_path(&range.file);
                let line = range.line;

                // Check if line info points to stdlib but function is user code
                // If so, try to use the function's declaration location instead
                if is_stdlib_path(&file)
                    && !is_stdlib_function(&function)
                    && let Some((decl_file, decl_line)) = self.function_decls.get(&function)
                {
                    if !self.is_target_path(decl_file) {
                        return Location::unknown();
                    }
                    let simplified_decl = simplify_path(decl_file);
                    // Only use decl location if it's a user file
                    if !is_stdlib_path(&simplified_decl) {
                        return Location {
                            file: simplified_decl,
                            line: *decl_line,
                            column: 0,
                            function,
                        };
                    }
                }

                if !self.is_target_path(&range.file) {
                    return Location::unknown();
                }

                Location {
                    file,
                    line,
                    column: range.column,
                    function,
                }
            }
            Err(_) => {
                // No line info, try to use function declaration if available
                if function != "[unknown]" {
                    if let Some((decl_file, decl_line)) = self.function_decls.get(&function) {
                        if !self.is_target_path(decl_file) {
                            return Location::unknown();
                        }
                        let simplified = simplify_path(decl_file);
                        if !is_stdlib_path(&simplified) {
                            return Location {
                                file: simplified,
                                line: *decl_line,
                                column: 0,
                                function,
                            };
                        }
                    }
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
        }
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

    fn is_target_path(&self, path: &str) -> bool {
        let Some(root) = &self.target_root else {
            return true;
        };
        if path.is_empty() {
            return false;
        }
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            return candidate.starts_with(root);
        }
        // Best-effort: treat relative paths as in-target only if they exist under root.
        root.join(candidate).exists()
    }
}

fn detect_target_root(dwarf: &DwarfInfo, exe_path: &Path) -> Option<PathBuf> {
    if let Some(root) = root_from_main_decl(dwarf) {
        return Some(root);
    }
    cargo_root_from_exe(exe_path)
}

fn root_from_main_decl(dwarf: &DwarfInfo) -> Option<PathBuf> {
    let main_decl = dwarf.function_decls.iter().find_map(|(name, (file, _))| {
        if name == "main" || name.ends_with("::main") {
            Some(file.as_str())
        } else {
            None
        }
    })?;
    root_from_source_path(main_decl)
}

fn root_from_source_path(path: &str) -> Option<PathBuf> {
    let idx = path.find("/src/")?;
    let root = &path[..idx];
    if root.is_empty() {
        return None;
    }
    Some(PathBuf::from(root))
}

fn cargo_root_from_exe(exe_path: &Path) -> Option<PathBuf> {
    let exe_name = exe_path.file_stem()?.to_string_lossy();
    let mut current = exe_path.parent();
    let mut fallback_root: Option<PathBuf> = None;

    while let Some(dir) = current {
        let cargo_path = dir.join("Cargo.toml");
        if cargo_path.exists() {
            if fallback_root.is_none() {
                fallback_root = Some(dir.to_path_buf());
            }
            if cargo_toml_matches_exe(&cargo_path, &exe_name) {
                return Some(dir.to_path_buf());
            }
        }
        current = dir.parent();
    }

    fallback_root
}

fn cargo_toml_matches_exe(cargo_path: &Path, exe_name: &str) -> bool {
    let contents = match std::fs::read_to_string(cargo_path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let needle = format!("name = \"{}\"", exe_name);
    contents.contains(&needle)
}

/// Check if a path looks like stdlib/library code
fn is_stdlib_path(path: &str) -> bool {
    path.contains("/rustc/")
        || path.contains("/.cargo/")
        || path.contains("/rust/library/")
        || path.starts_with("<std>")
        || path.starts_with("<hashbrown>")
        || path.starts_with("<")
        || path.contains("library/core/")
        || path.contains("library/std/")
        || path.contains("library/alloc/")
}

/// Check if a function name looks like stdlib/internal code
fn is_stdlib_function(name: &str) -> bool {
    // Check starts_with patterns
    name.starts_with("std::")
        || name.starts_with("core::")
        || name.starts_with("alloc::")
        || name.starts_with("hashbrown::")
        || name.starts_with("__rust")
        || name.starts_with("_Unwind")
        || name.starts_with("rust_eh_personality")
        || name.starts_with("addr2line::")
        || name.starts_with("gimli::")
        || name.starts_with("object::")
        || name == "[unknown]"
        // Trait impls: <Type as std::*>::method or <std::*>::method
        || name.starts_with("<std::")
        || name.starts_with("<core::")
        || name.starts_with("<alloc::")
        // Trait impls with generic params: <T as core::fmt::Display>::fmt
        || name.contains(" as core::")
        || name.contains(" as std::")
        || name.contains(" as alloc::")
}

/// Simplify a file path for display
fn simplify_path(path: &str) -> String {
    // Remove common prefixes
    let prefixes_to_strip = ["/rustc/", "/.cargo/registry/src/", "/.cargo/git/checkouts/"];

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
