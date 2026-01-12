use crate::error::{Error, Result};
use gimli::{EndianSlice, RunTimeEndian};
use object::{Object, ObjectSection};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

/// Parsed DWARF debug information
pub struct DwarfInfo {
    /// Address ranges mapped to source locations
    pub ranges: Vec<AddressRange>,
    /// Function names by address
    pub functions: HashMap<u64, String>,
    /// Function declarations: function name -> (file, line)
    pub function_decls: HashMap<String, (String, u32)>,
}

/// An address range mapped to a source location
#[derive(Debug, Clone)]
pub struct AddressRange {
    pub start: u64,
    pub end: u64,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl DwarfInfo {
    /// Parse DWARF info from an ELF file
    pub fn parse(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(Error::Io)?;

        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(Error::Io)?;
        let mmap = Arc::new(mmap);

        let object = object::File::parse(&**mmap)
            .map_err(|e| Error::SymbolResolution(format!("Failed to parse ELF: {}", e)))?;

        // Check for debug info
        if object.section_by_name(".debug_info").is_none() {
            return Err(Error::MissingDebugInfo {
                path: path.display().to_string(),
            });
        }

        let endian = if object.is_little_endian() {
            RunTimeEndian::Little
        } else {
            RunTimeEndian::Big
        };

        // Parse function names from symbol table first (doesn't need DWARF)
        let functions = Self::parse_functions(&object);

        // Parse line info using a helper that owns the data
        let ranges = Self::parse_line_info_from_object(&object, endian)?;

        // Parse function declarations from DWARF DIEs
        let function_decls = Self::parse_function_decls_from_object(&object, endian)?;

        Ok(DwarfInfo {
            ranges,
            functions,
            function_decls,
        })
    }

    fn parse_line_info_from_object(
        object: &object::File<'_>,
        endian: RunTimeEndian,
    ) -> Result<Vec<AddressRange>> {
        // Helper to load a section's data
        let load_section = |name: &str| -> &[u8] {
            object
                .section_by_name(name)
                .and_then(|s| s.data().ok())
                .unwrap_or(&[])
        };

        // Load all sections we need
        let debug_abbrev = load_section(".debug_abbrev");
        let debug_info = load_section(".debug_info");
        let debug_line = load_section(".debug_line");
        let debug_str = load_section(".debug_str");
        let debug_line_str = load_section(".debug_line_str");

        // Create DWARF context
        let dwarf = gimli::Dwarf {
            debug_abbrev: gimli::DebugAbbrev::new(debug_abbrev, endian),
            debug_info: gimli::DebugInfo::new(debug_info, endian),
            debug_line: gimli::DebugLine::new(debug_line, endian),
            debug_str: gimli::DebugStr::new(debug_str, endian),
            debug_line_str: gimli::DebugLineStr::new(debug_line_str, endian),
            ..Default::default()
        };

        Self::parse_line_info(&dwarf)
    }

    fn parse_line_info(
        dwarf: &gimli::Dwarf<EndianSlice<'_, RunTimeEndian>>,
    ) -> Result<Vec<AddressRange>> {
        let mut ranges = Vec::new();
        let mut units = dwarf.units();

        while let Ok(Some(header)) = units.next() {
            let unit = dwarf
                .unit(header)
                .map_err(|e| Error::SymbolResolution(format!("Failed to parse unit: {}", e)))?;

            if let Some(program) = unit.line_program.clone() {
                let mut rows = program.rows();
                let mut prev_row: Option<(u64, String, u32, u32)> = None;

                while let Ok(Some((header, row))) = rows.next_row() {
                    let addr = row.address();

                    // Get file path
                    let file = row.file(header).map(|f| {
                        let mut path = String::new();

                        if let Some(dir) = f.directory(header)
                            && let Ok(dir_str) = dwarf.attr_string(&unit, dir)
                            && let Ok(s) = dir_str.to_string()
                        {
                            path.push_str(s);
                            if !path.ends_with('/') {
                                path.push('/');
                            }
                        }

                        if let Ok(name) = dwarf.attr_string(&unit, f.path_name())
                            && let Ok(s) = name.to_string()
                        {
                            path.push_str(s);
                        }

                        path
                    });

                    let file = file.unwrap_or_default();
                    let line = row.line().map(|l| l.get() as u32).unwrap_or(0);
                    let column = match row.column() {
                        gimli::ColumnType::LeftEdge => 0,
                        gimli::ColumnType::Column(c) => c.get() as u32,
                    };

                    // Create range from previous row to this one
                    if let Some((prev_addr, prev_file, prev_line, prev_col)) = prev_row.take()
                        && addr > prev_addr
                        && !prev_file.is_empty()
                    {
                        ranges.push(AddressRange {
                            start: prev_addr,
                            end: addr,
                            file: prev_file,
                            line: prev_line,
                            column: prev_col,
                        });
                    }

                    if !row.end_sequence() {
                        prev_row = Some((addr, file, line, column));
                    }
                }
            }
        }

        // Sort by start address for binary search
        ranges.sort_by_key(|r| r.start);
        Ok(ranges)
    }

    fn parse_functions(object: &object::File<'_>) -> HashMap<u64, String> {
        use object::ObjectSymbol;

        let mut functions = HashMap::new();

        for symbol in object.symbols() {
            if symbol.kind() == object::SymbolKind::Text
                && let Ok(name) = symbol.name()
            {
                let demangled = rustc_demangle::demangle(name).to_string();
                functions.insert(symbol.address(), demangled);
            }
        }

        functions
    }

    fn parse_function_decls_from_object(
        object: &object::File<'_>,
        endian: RunTimeEndian,
    ) -> Result<HashMap<String, (String, u32)>> {
        // Helper to load a section's data
        let load_section = |name: &str| -> &[u8] {
            object
                .section_by_name(name)
                .and_then(|s| s.data().ok())
                .unwrap_or(&[])
        };

        // Load all sections we need (same as parse_line_info_from_object)
        let debug_abbrev = load_section(".debug_abbrev");
        let debug_info = load_section(".debug_info");
        let debug_line = load_section(".debug_line");
        let debug_str = load_section(".debug_str");
        let debug_line_str = load_section(".debug_line_str");

        // Create DWARF context
        let dwarf = gimli::Dwarf {
            debug_abbrev: gimli::DebugAbbrev::new(debug_abbrev, endian),
            debug_info: gimli::DebugInfo::new(debug_info, endian),
            debug_line: gimli::DebugLine::new(debug_line, endian),
            debug_str: gimli::DebugStr::new(debug_str, endian),
            debug_line_str: gimli::DebugLineStr::new(debug_line_str, endian),
            ..Default::default()
        };

        Self::parse_function_decls(&dwarf)
    }

    fn parse_function_decls(
        dwarf: &gimli::Dwarf<EndianSlice<'_, RunTimeEndian>>,
    ) -> Result<HashMap<String, (String, u32)>> {
        let mut function_decls: HashMap<String, (String, u32)> = HashMap::new();
        let mut units = dwarf.units();

        while let Ok(Some(header)) = units.next() {
            let unit = match dwarf.unit(header) {
                Ok(u) => u,
                Err(_) => continue,
            };

            // Get the compilation unit's directory for resolving relative paths
            let comp_dir = unit
                .comp_dir
                .as_ref()
                .and_then(|d| d.to_string().ok())
                .unwrap_or_default();

            // Get line program for file table (optional - we can still get function names without it)
            let line_program = unit.line_program.clone();

            let mut entries = unit.entries();
            while let Ok(Some((_, entry))) = entries.next_dfs() {
                // Look for DW_TAG_subprogram (function definitions)
                if entry.tag() != gimli::DW_TAG_subprogram {
                    continue;
                }

                // Get function name
                let name = entry
                    .attr_value(gimli::DW_AT_linkage_name)
                    .ok()
                    .flatten()
                    .or_else(|| entry.attr_value(gimli::DW_AT_name).ok().flatten());

                let func_name = match name {
                    Some(gimli::AttributeValue::DebugStrRef(offset)) => dwarf
                        .debug_str
                        .get_str(offset)
                        .ok()
                        .and_then(|s| s.to_string().ok()),
                    Some(gimli::AttributeValue::String(s)) => s.to_string().ok(),
                    _ => None,
                };

                let func_name = match func_name {
                    Some(n) => rustc_demangle::demangle(n).to_string(),
                    None => continue,
                };

                // Get file index from DW_AT_decl_file
                let file_idx = match entry.attr_value(gimli::DW_AT_decl_file).ok().flatten() {
                    Some(gimli::AttributeValue::FileIndex(idx)) => idx,
                    Some(gimli::AttributeValue::Udata(idx)) => idx,
                    _ => continue,
                };

                // Get line from DW_AT_decl_line
                let line = match entry.attr_value(gimli::DW_AT_decl_line).ok().flatten() {
                    Some(gimli::AttributeValue::Udata(l)) => l as u32,
                    _ => 0,
                };

                // Resolve file path from line program's file table
                let file_path = if let Some(lp) = line_program.as_ref()
                    && file_idx > 0
                {
                    let header = lp.header();
                    header.file(file_idx).and_then(|file_entry| {
                        let mut path = String::new();

                        // Get directory
                        if let Some(dir) = file_entry.directory(header)
                            && let Ok(dir_str) = dwarf.attr_string(&unit, dir)
                            && let Ok(s) = dir_str.to_string()
                        {
                            // Handle relative paths
                            if !s.starts_with('/') && !comp_dir.is_empty() {
                                path.push_str(comp_dir);
                                if !path.ends_with('/') {
                                    path.push('/');
                                }
                            }
                            path.push_str(s);
                            if !path.ends_with('/') {
                                path.push('/');
                            }
                        }

                        // Get filename
                        if let Ok(name) = dwarf.attr_string(&unit, file_entry.path_name())
                            && let Ok(s) = name.to_string()
                        {
                            path.push_str(s);
                        }

                        if path.is_empty() { None } else { Some(path) }
                    })
                } else {
                    None
                };

                if let Some(file) = file_path {
                    // Only store if we don't already have an entry, or if this one is "more user"
                    // (prefer non-stdlib paths)
                    let should_insert = match function_decls.get(&func_name) {
                        Some((existing_file, _)) => {
                            Self::is_stdlib_path(existing_file) && !Self::is_stdlib_path(&file)
                        }
                        None => true,
                    };

                    if should_insert {
                        function_decls.insert(func_name, (file, line));
                    }
                }
            }
        }

        Ok(function_decls)
    }

    /// Check if a path looks like stdlib/library code
    fn is_stdlib_path(path: &str) -> bool {
        path.contains("/rustc/")
            || path.contains("/.cargo/")
            || path.contains("/rust/library/")
            || path.starts_with("<")
    }
}
