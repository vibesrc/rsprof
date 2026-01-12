//! CTF-style target application for testing rsprof
//!
//! This app has hidden performance bottlenecks. Can you find them?
//!
//! Build: cargo build --release --example target_app -p rsprof
//! Run:   ./target/release/examples/target_app
//! Profile: rsprof -p $(pgrep target_app)
//!
//! CHALLENGE: Find the 3 major optimization targets!

mod app;
mod cache;
mod metrics;
mod processing;
mod utils;
mod validation;

use std::time::{Duration, Instant};

// Enable CPU + heap profiling
rsprof_trace::profiler!();

fn main() {
    println!("=== Performance CTF ===");
    println!("PID: {}", std::process::id());
    println!();
    println!("This app has hidden performance bottlenecks.");
    println!("Use rsprof to find the 3 major optimization targets!");
    println!();
    println!("Press Ctrl-C to stop.");
    println!();

    let mut application = app::Application::new();
    let start = Instant::now();

    loop {
        application.tick();

        if application.tick_count() % 500 == 0 {
            println!(
                "[{:>5.1}s] processed={:<6} cache_hits={:<5} errors={:<3}",
                start.elapsed().as_secs_f64(),
                application.tick_count(),
                application.stats().cache_hits,
                application.stats().errors,
            );
        }

        std::thread::sleep(Duration::from_millis(2));
    }
}
