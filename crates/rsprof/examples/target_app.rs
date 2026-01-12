//! Example target application for testing rsprof
//!
//! Simulates a service with multiple caches and allocation patterns.
//!
//! Build: make target
//! Run:   make run-target
//! Profile: make profile (in another terminal)

use std::collections::HashMap;
use std::time::{Duration, Instant};

// Enable CPU + heap profiling with one line
rsprof_trace::profiler!();

fn main() {
    println!("=== Cache Service Demo ===");
    println!("PID: {}", std::process::id());
    println!();
    println!("Simulating a service with multiple allocation patterns.");
    println!("Press Ctrl-C to stop.");
    println!();

    // Local caches - keeps allocations alive
    let mut user_cache: HashMap<u64, Vec<String>> = HashMap::new();
    let mut session_cache: HashMap<u64, Vec<u8>> = HashMap::new();
    let mut query_results: Vec<Vec<String>> = Vec::new();

    let start = Instant::now();
    let mut tick = 0u64;

    loop {
        tick += 1;

        // User cache operations - slow growth
        handle_user_cache(tick, &mut user_cache);

        // Session cache - high churn
        handle_session_cache(tick, &mut session_cache);

        // Query results - large allocations
        handle_query_cache(tick, &mut query_results);

        // CPU work
        do_computation(tick);

        // Status every 1000 ticks (reduce I/O noise in profiling)
        if tick.is_multiple_of(1000) {
            println!(
                "[{:>5.1}s] tick={:<5} users={:<4} sessions={:<4} queries={:<4}",
                start.elapsed().as_secs_f64(),
                tick,
                user_cache.len(),
                session_cache.len(),
                query_results.len()
            );
        }

        // Throttle - ~30-40% CPU usage
        std::thread::sleep(Duration::from_millis(3));
    }
}

// =============================================================================
// USER CACHE - Grows slowly, accumulates data
// =============================================================================

#[inline(never)]
fn handle_user_cache(tick: u64, cache: &mut HashMap<u64, Vec<String>>) {
    let user_id = tick % 200;

    if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(user_id) {
        // New user - allocate profile data
        let profile = create_user_profile(user_id);
        e.insert(profile);
    } else if tick.is_multiple_of(25) {
        // Existing user - add activity
        add_user_activity(user_id, tick, cache);
    }
}

#[inline(never)]
fn create_user_profile(user_id: u64) -> Vec<String> {
    vec![
        format!("user_{}", user_id),
        format!("email_{}@example.com", user_id),
        format!("pref_{}", user_id % 10),
    ]
}

#[inline(never)]
fn add_user_activity(user_id: u64, tick: u64, cache: &mut HashMap<u64, Vec<String>>) {
    if let Some(profile) = cache.get_mut(&user_id) {
        profile.push(format!("activity_{}", tick));
        // Keep profile bounded
        if profile.len() > 20 {
            profile.remove(3); // Keep first 3 (id, email, pref)
        }
    }
}

// =============================================================================
// SESSION CACHE - High churn, frequent create/expire
// =============================================================================

#[inline(never)]
fn handle_session_cache(tick: u64, cache: &mut HashMap<u64, Vec<u8>>) {
    let session_id = tick % 100;

    // Create or refresh session
    if tick.is_multiple_of(3) {
        let session_data = create_session_data(session_id);
        cache.insert(session_id, session_data);
    }

    // Expire old sessions periodically
    if tick.is_multiple_of(50) {
        expire_sessions(tick, cache);
    }
}

#[inline(never)]
fn create_session_data(session_id: u64) -> Vec<u8> {
    // Simulate session with some data
    vec![0u8; 256 + (session_id % 128) as usize]
}

#[inline(never)]
fn expire_sessions(tick: u64, cache: &mut HashMap<u64, Vec<u8>>) {
    // Remove ~20% of sessions
    let threshold = tick % 100;
    cache.retain(|id, _| *id > threshold.saturating_sub(20));
}

// =============================================================================
// QUERY CACHE - Large results, bounded size
// =============================================================================

#[inline(never)]
fn handle_query_cache(tick: u64, cache: &mut Vec<Vec<String>>) {
    match tick % 5 {
        0 => run_small_query(cache),
        1 => run_medium_query(cache),
        2 => run_large_query(cache),
        _ => {} // No query
    }

    // Bound cache size
    if cache.len() > 100 {
        evict_old_queries(cache);
    }
}

#[inline(never)]
fn run_small_query(cache: &mut Vec<Vec<String>>) {
    let results: Vec<String> = (0..5).map(|i| format!("small_result_{}", i)).collect();
    cache.push(results);
}

#[inline(never)]
fn run_medium_query(cache: &mut Vec<Vec<String>>) {
    let results: Vec<String> = (0..25).map(|i| format!("medium_result_{}", i)).collect();
    cache.push(results);
}

#[inline(never)]
fn run_large_query(cache: &mut Vec<Vec<String>>) {
    // This should show up prominently in heap profile
    let results: Vec<String> = (0..100)
        .map(|i| format!("large_result_row_{}_with_extra_data", i))
        .collect();
    cache.push(results);
}

#[inline(never)]
fn evict_old_queries(cache: &mut Vec<Vec<String>>) {
    // Remove oldest 30 entries
    cache.drain(0..30);
}

// =============================================================================
// CPU WORK - Shows up in CPU profile
// =============================================================================

#[inline(never)]
fn do_computation(tick: u64) {
    // Do work every tick for better profiling visibility
    compute_hash(tick);
    compute_sort(tick);
    if tick.is_multiple_of(3) {
        compute_math(tick);
    }
}

#[inline(never)]
fn compute_hash(tick: u64) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    for i in 0..2000 {
        (tick + i).hash(&mut hasher);
    }
    let _ = hasher.finish();
}

#[inline(never)]
fn compute_sort(tick: u64) {
    let mut data: Vec<i32> = (0..500).map(|i| (tick as i32 * 7 + i) % 1000).collect();
    data.sort();
    let _ = data.iter().sum::<i32>();
}

#[inline(never)]
fn compute_math(tick: u64) {
    let mut sum = 0u64;
    for i in 0u64..1000 {
        sum = sum.wrapping_add(i.wrapping_mul(tick));
        sum = sum.wrapping_mul(31).wrapping_add(17);
    }
    let _ = sum;
}
