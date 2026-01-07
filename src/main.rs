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

    // Run profiler
    if cli.quiet {
        run_headless(sampler, resolver, storage, cli.interval, cli.duration)?;
    } else {
        rsprof::tui::run(sampler, resolver, storage, cli.interval, cli.duration)?;
    }

    Ok(())
}

fn run_headless(
    mut sampler: rsprof::cpu::CpuSampler,
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

        // Read samples
        let samples = sampler.read_samples()?;
        total_samples += samples.len() as u64;

        // Resolve and accumulate
        for addr in samples {
            let location = resolver.resolve(addr);
            storage.record_cpu_sample(addr, &location);
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
