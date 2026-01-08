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
                        .ok_or_else(|| anyhow::anyhow!("No profiles found. Run 'rsprof list' to see available profiles."))?
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

    // Initialize storage
    let storage = rsprof::storage::Storage::new(&output_path, &proc_info, cli.cpu_freq)?;

    // Initialize CPU sampler
    let sampler = rsprof::cpu::CpuSampler::new(pid, cli.cpu_freq)?;

    // Try to initialize heap sampler (requires root/CAP_BPF)
    let heap_sampler = match rsprof::heap::HeapSampler::new(pid, proc_info.proc_exe_path()) {
        Ok(hs) => {
            eprintln!("Heap profiling enabled (eBPF uprobes)");
            Some(hs)
        }
        Err(e) => {
            eprintln!("Heap profiling disabled: {}", e);
            None
        }
    };

    // Run profiler
    if cli.quiet {
        run_headless(sampler, heap_sampler, resolver, storage, cli.interval, cli.duration)?;
    } else {
        rsprof::tui::run(sampler, heap_sampler, resolver, storage, cli.interval, cli.duration)?;
    }

    Ok(())
}

/// Find the first "user" frame in a stack trace (not allocator internals)
fn find_user_frame(stack: &[u64], resolver: &rsprof::symbols::SymbolResolver) -> rsprof::symbols::Location {
    // Skip internal allocator/library frames - find first user code
    let skip_function_patterns = [
        "__rust_alloc", "__rust_dealloc", "__rust_realloc",
        "alloc::alloc::", "alloc::raw_vec::", "alloc::vec::",
        "alloc::string::", "alloc::collections::", "<alloc::",
        "hashbrown::", "std::collections::hash",
        "core::ptr::", "core::slice::", "core::iter::", "<core::",
        "core::ops::function::", // FnOnce::call_once etc
        "_Unwind_", "__cxa_", "_fini", "_init",
        "addr2line::", "gimli::", "object::", "miniz_oxide::",
        "sort::shared::smallsort::", // sorting internals
    ];

    /// Check if a file path looks like internal/library code
    fn is_internal_file(file: &str) -> bool {
        file.is_empty()
            || file.starts_with('[')
            || file.starts_with('<')  // <std>/, <hashbrown>/, etc
            || file.contains("/rustc/")
            || file.contains("/.cargo/registry/")
            || file.contains("/rust/library/")
    }

    // First pass: look for user frames with real source paths
    for &addr in stack {
        let loc = resolver.resolve(addr);
        if is_internal_file(&loc.file) {
            continue;
        }
        let is_internal_fn = skip_function_patterns.iter().any(|p| loc.function.contains(p));
        if !is_internal_fn && (loc.file.contains("/src/") || loc.file.contains("/examples/")) {
            return loc;
        }
    }

    // Second pass: any non-internal frame with real file info
    for &addr in stack {
        let loc = resolver.resolve(addr);
        if is_internal_file(&loc.file) {
            continue;
        }
        let is_internal_fn = skip_function_patterns.iter().any(|p| loc.function.contains(p));
        if !is_internal_fn {
            return loc;
        }
    }

    // Third pass: first frame with any non-internal function name
    for &addr in stack {
        let loc = resolver.resolve(addr);
        let is_internal_fn = skip_function_patterns.iter().any(|p| loc.function.contains(p));
        if !is_internal_fn && !loc.function.is_empty() && loc.function != "[unknown]" {
            return loc;
        }
    }

    // Last resort: first address
    if !stack.is_empty() {
        return resolver.resolve(stack[0]);
    }

    resolver.resolve(0)
}

fn run_headless(
    mut sampler: rsprof::cpu::CpuSampler,
    heap_sampler: Option<rsprof::heap::HeapSampler>,
    resolver: rsprof::symbols::SymbolResolver,
    mut storage: rsprof::storage::Storage,
    checkpoint_interval: std::time::Duration,
    duration: Option<std::time::Duration>,
) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .context("Failed to set Ctrl-C handler")?;

    let start = std::time::Instant::now();
    let mut last_checkpoint = std::time::Instant::now();
    let mut total_samples = 0u64;

    eprintln!("Recording (Ctrl-C to stop)...");

    while running.load(Ordering::SeqCst) {
        // Check duration limit
        if let Some(max_duration) = duration {
            if start.elapsed() >= max_duration {
                break;
            }
        }

        // Read CPU samples
        let samples = sampler.read_samples()?;
        total_samples += samples.len() as u64;

        // Resolve and accumulate CPU samples
        for addr in samples {
            let location = resolver.resolve(addr);
            storage.record_cpu_sample(addr, &location);
        }

        // Read heap stats if available
        if let Some(ref hs) = heap_sampler {
            let heap_stats = hs.read_stats();
            let inline_stacks = hs.read_inline_stacks();

            // Debug: show stack info on first iteration
            static DEBUG_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !DEBUG_ONCE.swap(true, std::sync::atomic::Ordering::SeqCst) && !inline_stacks.is_empty() {
                eprintln!("\n=== DEBUG: Inline stack samples ===");
                for (key_addr, stack) in inline_stacks.iter().take(3) {
                    eprintln!("Key addr: 0x{:x}, stack depth: {}", key_addr, stack.len());
                    for (i, &addr) in stack.iter().enumerate() {
                        let loc = resolver.resolve(addr);
                        eprintln!("  [{}] 0x{:x} -> {}:{} ({})", i, addr, loc.file, loc.line, loc.function);
                    }
                }
                eprintln!("=================================\n");
            }

            for (key_addr, stats) in heap_stats {
                // Use inline stack for better resolution if available
                let location = if let Some(stack) = inline_stacks.get(&key_addr) {
                    find_user_frame(stack, &resolver)
                } else {
                    resolver.resolve(key_addr)
                };
                storage.record_heap_sample(
                    &location,
                    stats.total_alloc_bytes as i64,
                    stats.total_free_bytes as i64,
                    stats.live_bytes,
                );
            }
        }

        // Checkpoint
        if last_checkpoint.elapsed() >= checkpoint_interval {
            storage.flush_checkpoint()?;
            last_checkpoint = std::time::Instant::now();
            eprint!(
                "\rSamples: {} | Elapsed: {:?}",
                total_samples,
                start.elapsed()
            );
        }

        // Sleep briefly to avoid busy-waiting
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Final flush
    storage.flush_checkpoint()?;
    eprintln!("\nRecording complete. Total samples: {}", total_samples);

    Ok(())
}
