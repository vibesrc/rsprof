use anyhow::Context;
use clap::Parser;
use rsprof::cli::{Cli, Command};
use rsprof::error::exit_code;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::from(exit_code::SUCCESS as u8),
        Err(e) => {
            eprintln!("Error: {e:#}");
            if let Some(rsprof_err) = e.downcast_ref::<rsprof::Error>() {
                ExitCode::from(rsprof_err.exit_code() as u8)
            } else {
                ExitCode::from(exit_code::GENERAL_ERROR as u8)
            }
        }
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Validate CLI arguments
    cli.validate()
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("Invalid arguments")?;

    match cli.command {
        Some(Command::Top {
            metric,
            file,
            top,
            threshold,
            since,
            until,
            json,
            csv,
            filter,
        }) => {
            rsprof::commands::top::run(
                &file, metric, top, threshold, since, until, json, csv, filter,
            )?;
        }
        Some(Command::Query { file, sql }) => {
            rsprof::commands::query::run(&file, &sql)?;
        }
        Some(Command::View { file }) => {
            let profile_path = match file {
                Some(f) => f,
                None => {
                    // Find most recent profile
                    rsprof::commands::list::most_recent_profile(std::path::Path::new("."))?
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "No profiles found. Run 'rsprof list' to see available profiles."
                            )
                        })?
                }
            };
            rsprof::commands::view::run(&profile_path)?;
        }
        Some(Command::List { dir }) => {
            rsprof::commands::list::run(dir.as_deref())?;
        }
        Some(Command::Completions { shell }) => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "rsprof", &mut std::io::stdout());
        }
        None => {
            // Recording mode
            run_profiler(&cli)?;
        }
    }

    Ok(())
}

fn run_profiler(cli: &Cli) -> anyhow::Result<()> {
    // Resolve PID
    let pid = match (cli.pid, &cli.process) {
        (Some(pid), _) => pid,
        (_, Some(name)) => rsprof::process::find_process_by_name(name)?,
        _ => unreachable!("validated in cli"),
    };

    // Verify process exists and get info
    let proc_info = rsprof::process::ProcessInfo::new(pid)?;
    eprintln!(
        "Attaching to {} (PID {})",
        proc_info.name(),
        proc_info.pid()
    );

    // Determine output path
    let output_path = cli.output.clone().unwrap_or_else(|| {
        let timestamp = chrono::Local::now().format("%y%m%d%H%M%S");
        std::path::PathBuf::from(format!("rsprof.{}.{}.db", proc_info.name(), timestamp))
    });
    eprintln!("Output: {}", output_path.display());

    // Load symbols
    eprintln!("Loading debug symbols...");
    let resolver = rsprof::symbols::SymbolResolver::new(&proc_info)?;
    eprintln!(
        "Loaded {} address ranges from DWARF",
        resolver.range_count()
    );
    eprintln!("ASLR offset: 0x{:x}", resolver.aslr_offset());

    // Initialize storage
    let storage = rsprof::storage::Storage::new(&output_path, &proc_info, cli.cpu_freq)?;

    // Try to initialize shared memory sampler (rsprof-trace) first
    // This provides both CPU and heap profiling from self-instrumented targets
    let shm_sampler = match rsprof::heap::ShmHeapSampler::new(pid, proc_info.exe_path()) {
        Ok(shm) => {
            eprintln!("Profiling enabled (rsprof-trace: CPU + heap via shared memory)");
            Some(shm)
        }
        Err(_) => None,
    };

    // Initialize perf-based CPU sampler as fallback
    let perf_sampler = if shm_sampler.is_none() {
        match rsprof::cpu::CpuSampler::new(pid, cli.cpu_freq) {
            Ok(s) => {
                eprintln!("CPU profiling enabled (perf_event)");
                Some(s)
            }
            Err(e) => {
                eprintln!("CPU profiling disabled: {}", e);
                None
            }
        }
    } else {
        None // Don't need perf when we have rsprof-trace
    };

    // Try eBPF heap sampler as fallback (requires root/CAP_BPF)
    let heap_sampler = if shm_sampler.is_none() {
        match rsprof::heap::HeapSampler::new(pid, proc_info.exe_path()) {
            Ok(hs) => {
                eprintln!("Heap profiling enabled (eBPF uprobes)");
                Some(hs)
            }
            Err(e) => {
                eprintln!("Heap profiling disabled: {}", e);
                None
            }
        }
    } else {
        None // Don't need eBPF when we have rsprof-trace
    };

    // Run profiler
    if cli.quiet {
        run_headless(
            perf_sampler,
            heap_sampler,
            shm_sampler,
            resolver,
            storage,
            cli.interval,
            cli.duration,
        )?;
    } else {
        rsprof::tui::run(
            perf_sampler,
            heap_sampler,
            shm_sampler,
            resolver,
            storage,
            cli.interval,
            cli.duration,
        )?;
    }

    Ok(())
}

/// Patterns for internal/profiler/library functions to skip
/// These functions should be attributed to the user code that calls them
const SKIP_FUNCTION_PATTERNS: &[&str] = &[
    // Rust allocator entry points
    "__rust_alloc",
    "__rust_dealloc",
    "__rust_realloc",
    "__rustc",
    // Rust alloc crate internals
    "alloc::alloc::",
    "alloc::raw_vec::",
    "alloc::vec::",
    "alloc::string::",
    "alloc::collections::",
    "<alloc::",
    "alloc::fmt::",
    "alloc::ffi::", // format! and CString internals
    // Hashmap/collections internals
    "hashbrown::",
    "std::collections::hash",
    // Core library internals
    "core::ptr::",
    "core::slice::",
    "core::iter::",
    "core::sync::",  // atomics, etc.
    "core::option::",
    "core::result::",
    "<core::",
    "core::ops::function::",
    "core::ops::drop::",
    "core::ffi::",
    "core::fmt::",
    "core::num::",
    "core::str::",
    "core::hash::",
    "core::mem::",
    // Std library internals
    "std::io::",
    "std::fmt::",
    "std::sys::",
    "std::thread::",
    "std::sync::",
    "<std::",
    "fmt::num::",
    "fmt::Write::",
    // Trait implementations (raw DWARF names)
    " as core::fmt::",   // <T as core::fmt::Display>::fmt
    " as std::fmt::",    // <T as std::fmt::Write>::write
    " as core::hash::",  // <T as core::hash::Hash>::hash
    " as alloc::",       // <T as alloc::*>::method
    // Trait implementations on generic types
    "<_>::", // any method on trait objects
    // Libc functions
    "malloc",
    "calloc",
    "realloc",
    "free",
    "memcpy",
    "memmove",
    "memset",
    "memchr",
    "_start",
    "__libc_start_main",
    // Exception/unwinding
    "_Unwind_",
    "__cxa_",
    "_fini",
    "_init",
    "rust_eh_personality",
    // Profiler internals (rsprof-trace)
    "addr2line::",
    "gimli::",
    "object::",
    "miniz_oxide::",
    "rustc_demangle::",  // demangling library
    "rsprof_alloc::",
    "rsprof_trace::",  // profiling library
    "profiling::",
    "rsprof::",
    // Sorting internals
    "sort::shared::smallsort::",
    // Generic patterns for generated code
    "::{{closure}}", // closures attributed to parent
];

/// Check if a file path looks like internal/library code
fn is_internal_file(file: &str) -> bool {
    file.is_empty()
        || file.starts_with('[')
        || file.starts_with('<')  // <std>/, <hashbrown>/, etc
        || file.contains("/rustc/")
        || file.contains("/.cargo/registry/")
        || file.contains("/rust/library/")
        || file.contains("rsprof-alloc")  // profiler internals
        || file.contains("rsprof-trace")  // profiler internals
        || file.contains("profiling.rs")  // profiler internals
        // Bare filenames without path context are usually library code
        || file == "lib.rs"
        || file == "time.rs"
        || file == "unix.rs"
        // Common library source files
        || file.ends_with("memchr.rs")
        || file.ends_with("maybe_uninit.rs")
        || file.ends_with("methods.rs")
        || (file.ends_with("mod.rs") && !file.contains("/src/")) // lib mod.rs, not user mod.rs
}

/// Check if a location is internal (profiler/library code)
fn is_internal_location(loc: &rsprof::symbols::Location) -> bool {
    if is_internal_file(&loc.file) {
        return true;
    }
    SKIP_FUNCTION_PATTERNS
        .iter()
        .any(|p| loc.function.contains(p))
}

/// Patterns for utility functions that should be attributed to their callers
const UTILITY_PATTERNS: &[&str] = &[
    // Derived trait methods - attribute to caller
    ">::clone",      // Clone::clone on any type
    ">::fmt",        // Debug/Display::fmt
    ">::hash",       // Hash::hash
    ">::eq",         // PartialEq::eq
    ">::partial_cmp", // PartialOrd
    ">::cmp",        // Ord
    // Common utility functions
    "::utils::",
    "::to_string",
    "::to_owned",
    "::into",
    "format_bytes",
    "format_size",
    "sanitize_",
    "generate_trace_id",
];

/// Check if a function is a utility function (should attribute to caller)
fn is_utility_function(func: &str) -> bool {
    UTILITY_PATTERNS.iter().any(|p| func.contains(p))
}

/// Find the best "user" frame in a stack trace.
/// If the first user frame is a utility function, return its caller instead.
fn find_user_frame(
    stack: &[u64],
    resolver: &rsprof::symbols::SymbolResolver,
) -> rsprof::symbols::Location {
    let mut first_user_frame: Option<rsprof::symbols::Location> = None;
    let mut first_user_idx: Option<usize> = None;

    // FIRST PASS: Find the first user frame
    for (i, &addr) in stack.iter().enumerate() {
        let loc = resolver.resolve(addr);
        // Skip internal files and functions
        if is_internal_file(&loc.file) || is_internal_location(&loc) {
            continue;
        }
        if !loc.function.is_empty() && loc.function != "[unknown]" {
            first_user_frame = Some(loc);
            first_user_idx = Some(i);
            break;
        }
    }

    // If first user frame is a utility function, look for its caller
    if let (Some(first_loc), Some(first_idx)) = (&first_user_frame, first_user_idx) {
        if is_utility_function(&first_loc.function) {
            // Look for the next user frame (caller of the utility)
            for &addr in stack.iter().skip(first_idx + 1) {
                let loc = resolver.resolve(addr);
                let has_internal_fn = SKIP_FUNCTION_PATTERNS
                    .iter()
                    .any(|p| loc.function.contains(p));
                if !has_internal_fn && !loc.function.is_empty() && loc.function != "[unknown]" {
                    // Found the caller - return it
                    return loc;
                }
            }
        }
        // Return the first user frame if no better caller found
        return first_user_frame.unwrap();
    }

    // Fallback: look for frames with real source paths
    for &addr in stack {
        let loc = resolver.resolve(addr);
        if !is_internal_file(&loc.file) && !is_internal_location(&loc) {
            return loc;
        }
    }

    // No user frame found - return a marker that will be filtered out
    // by is_internal_location (empty function name or internal file)
    rsprof::symbols::Location {
        file: "[internal]".to_string(),
        line: 0,
        column: 0,
        function: "[internal]".to_string(),
    }
}

fn run_headless(
    mut perf_sampler: Option<rsprof::cpu::CpuSampler>,
    heap_sampler: Option<rsprof::heap::HeapSampler>,
    mut shm_sampler: Option<rsprof::heap::ShmHeapSampler>,
    resolver: rsprof::symbols::SymbolResolver,
    mut storage: rsprof::storage::Storage,
    checkpoint_interval: std::time::Duration,
    duration: Option<std::time::Duration>,
) -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .context("Failed to set Ctrl-C handler")?;

    let start = std::time::Instant::now();
    let mut last_checkpoint = std::time::Instant::now();
    let mut total_cpu_samples = 0u64;
    let mut total_heap_events = 0u64;

    eprintln!("Recording (Ctrl-C to stop)...");

    while running.load(Ordering::SeqCst) {
        // Check duration limit
        if let Some(max_duration) = duration
            && start.elapsed() >= max_duration
        {
            break;
        }

        // Read from shared memory sampler (rsprof-trace) - gets both CPU and heap events
        if let Some(ref mut shm) = shm_sampler {
            let _events = shm.poll_events(std::time::Duration::from_millis(1));

            // Process CPU samples from rsprof-trace
            let cpu_samples = shm.read_cpu_samples();
            for sample in cpu_samples {
                total_cpu_samples += 1;
                // Walk the stack to find the first user frame (skip allocator/profiler internals)
                let location = find_user_frame(&sample.stack, &resolver);
                // Only record if we found a user frame (not internal/library code)
                if !is_internal_location(&location) {
                    storage
                        .record_cpu_sample(sample.stack.first().copied().unwrap_or(0), &location);
                }
            }

            // Just update the event count - heap stats are recorded at checkpoint time
            total_heap_events = shm.read_stats().len() as u64;
        }

        // Fallback to perf-based CPU sampling if no SHM sampler
        if shm_sampler.is_none()
            && let Some(ref mut sampler) = perf_sampler
        {
            let samples = sampler.read_samples()?;
            total_cpu_samples += samples.len() as u64;

            for addr in samples {
                let location = resolver.resolve(addr);
                if !is_internal_location(&location) {
                    storage.record_cpu_sample(addr, &location);
                }
            }
        }

        // Checkpoint - record heap stats and flush
        if last_checkpoint.elapsed() >= checkpoint_interval {
            // Record heap stats from SHM sampler (rsprof-trace)
            if let Some(ref shm) = shm_sampler {
                let heap_stats = shm.read_stats();
                let inline_stacks = shm.read_inline_stacks();
                total_heap_events = heap_stats.len() as u64;

                for (key_addr, stats) in heap_stats {
                    let location = if let Some(stack) = inline_stacks.get(&key_addr) {
                        find_user_frame(stack, &resolver)
                    } else {
                        resolver.resolve(key_addr)
                    };
                    if !is_internal_location(&location) {
                        storage.record_heap_sample(
                            &location,
                            stats.total_alloc_bytes as i64,
                            stats.total_free_bytes as i64,
                            stats.live_bytes,
                            stats.total_allocs,
                            stats.total_frees,
                        );
                    }
                }
            }

            // Record heap stats from eBPF sampler (fallback)
            if shm_sampler.is_none() {
                if let Some(ref hs) = heap_sampler {
                    let heap_stats = hs.read_stats();
                    let inline_stacks = hs.read_inline_stacks();
                    total_heap_events = heap_stats.len() as u64;

                    for (key_addr, stats) in heap_stats {
                        let location = if let Some(stack) = inline_stacks.get(&key_addr) {
                            find_user_frame(stack, &resolver)
                        } else {
                            resolver.resolve(key_addr)
                        };
                        if !is_internal_location(&location) {
                            storage.record_heap_sample(
                                &location,
                                stats.total_alloc_bytes as i64,
                                stats.total_free_bytes as i64,
                                stats.live_bytes,
                                stats.total_allocs,
                                stats.total_frees,
                            );
                        }
                    }
                }
            }

            storage.flush_checkpoint()?;
            last_checkpoint = std::time::Instant::now();
            eprint!(
                "\rCPU samples: {} | Heap sites: {} | Elapsed: {:?}",
                total_cpu_samples,
                total_heap_events,
                start.elapsed()
            );
        }

        // Sleep briefly to avoid busy-waiting
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Final flush
    storage.flush_checkpoint()?;
    eprintln!(
        "\nRecording complete. CPU samples: {}, Heap sites: {}",
        total_cpu_samples, total_heap_events
    );

    Ok(())
}
