use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "rsprof")]
#[command(about = "Zero-instrumentation profiler for Rust processes")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Process ID to profile
    #[arg(long, short = 'p', global = true, conflicts_with = "process")]
    pub pid: Option<u32>,

    /// Process name to profile (pgrep-style matching)
    #[arg(long, short = 'P', global = true, conflicts_with = "pid")]
    pub process: Option<String>,

    /// Output database path
    #[arg(long, short = 'o', global = true)]
    pub output: Option<PathBuf>,

    /// Checkpoint interval
    #[arg(long, short = 'i', default_value = "1s", value_parser = parse_duration)]
    pub interval: Duration,

    /// Recording duration (default: until Ctrl-C)
    #[arg(long, short = 'd', value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// CPU sampling frequency in Hz
    #[arg(long, default_value = "99")]
    pub cpu_freq: u64,

    /// Disable TUI, record only
    #[arg(long, short = 'q')]
    pub quiet: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// View top CPU or heap consumers from a recorded profile
    Top {
        /// What to display
        #[arg(value_enum)]
        metric: TopMetric,

        /// Profile database file
        file: PathBuf,

        /// Number of entries to display
        #[arg(long, short = 'n', default_value = "20")]
        top: usize,

        /// Minimum percentage to display
        #[arg(long, short = 't', default_value = "0")]
        threshold: f64,

        /// Only include last N duration of recording
        #[arg(long, value_parser = parse_duration)]
        since: Option<Duration>,

        /// Only include first N duration of recording
        #[arg(long, value_parser = parse_duration)]
        until: Option<Duration>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Output as CSV
        #[arg(long)]
        csv: bool,

        /// Filter by file or function name
        #[arg(long, short = 'f')]
        filter: Option<String>,
    },

    /// Execute raw SQL query on a profile database
    Query {
        /// Profile database file
        file: PathBuf,

        /// SQL query to execute
        sql: String,
    },

    /// Interactive TUI viewer for a recorded profile
    View {
        /// Profile database file (defaults to most recent)
        file: Option<PathBuf>,
    },

    /// List saved profile databases
    List {
        /// Directory to search (defaults to current directory)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum TopMetric {
    Cpu,
    Heap,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    // Try humantime first
    if let Ok(d) = humantime::parse_duration(s) {
        return Ok(d);
    }

    // Try bare number as seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }

    Err(format!(
        "Invalid duration '{}'. Examples: 30s, 5m, 2h, 1h30m, 90",
        s
    ))
}

impl Cli {
    pub fn validate(&self) -> Result<(), String> {
        // For recording mode (no subcommand), require either --pid or --process
        if self.command.is_none() && self.pid.is_none() && self.process.is_none() {
            return Err("Either --pid or --process is required for recording".to_string());
        }

        // Validate CPU frequency
        if self.cpu_freq == 0 || self.cpu_freq > 10000 {
            return Err(format!(
                "CPU frequency must be between 1 and 10000 Hz, got {}",
                self.cpu_freq
            ));
        }

        Ok(())
    }
}
