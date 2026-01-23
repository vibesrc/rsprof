//! More realistic CTF target with divergent/convergent paths and bounded leaks.
//!
//! Build: cargo build --release --example ctf_app -p rsprof
//! Run:   ./target/release/examples/ctf_app
//! Profile: rsprof -p $(pgrep ctf_app)

mod analytics;
mod app;
mod audit;
mod cache;
mod checkout;
mod model;
mod search;
mod utils;
mod validation;

use std::time::{Duration, Instant};

// Enable CPU + heap profiling
rsprof_trace::profiler!();

fn main() {
    println!("=== Performance CTF (Realistic) ===");
    println!("PID: {}", std::process::id());
    println!();
    println!("Multiple hot/cold paths with bounded leaks.");
    println!("Find the top CPU and heap targets.");
    println!();
    println!("Press Ctrl-C to stop.");
    println!();

    let mut application = app::Application::new();
    let start = Instant::now();

    loop {
        application.tick();

        if application.tick_count().is_multiple_of(500) {
            let stats = application.stats();
            println!(
                "[{:>5.1}s] processed={:<6} cache_hits={:<5} errors={:<3}",
                start.elapsed().as_secs_f64(),
                stats.requests,
                stats.cache_hits,
                stats.errors,
            );
        }

        std::thread::sleep(Duration::from_millis(2));
    }
}
