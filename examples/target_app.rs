//! Example target application for testing rsprof
//!
//! Run with: cargo run --example target_app
//! Then in another terminal: ./target/release/rsprof --pid <PID>

use std::collections::HashMap;
use std::time::{Duration, Instant};

fn main() {
    println!("Target app started. PID: {}", std::process::id());
    println!("Press Ctrl-C to stop.");
    println!();

    let start = Instant::now();
    let mut iteration = 0u64;

    loop {
        iteration += 1;

        // Mix of different workloads
        let result1 = cpu_intensive_math(1000);
        let result2 = string_processing("hello world ".repeat(100));
        let result3 = hash_operations(500);
        let result4 = vector_operations(10000);
        let result5 = recursive_fibonacci(25);

        // Prevent optimizations from removing the work
        if iteration % 100 == 0 {
            println!(
                "[{:>6.1}s] iter={} results={},{},{},{},{}",
                start.elapsed().as_secs_f64(),
                iteration,
                result1,
                result2,
                result3,
                result4,
                result5
            );
        }

        // Small sleep to make output readable
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// CPU-intensive mathematical operations
#[inline(never)]
fn cpu_intensive_math(iterations: usize) -> f64 {
    let mut result = 0.0f64;
    for i in 1..=iterations {
        result += (i as f64).sqrt();
        result += (i as f64).sin();
        result += (i as f64).cos();
        result = result.abs();
    }
    result
}

/// String processing workload
#[inline(never)]
fn string_processing(input: String) -> usize {
    let mut result = 0usize;

    // Multiple string operations
    let upper = input.to_uppercase();
    result += upper.len();

    let words: Vec<&str> = input.split_whitespace().collect();
    result += words.len();

    for word in words {
        result += word.chars().filter(|c| c.is_alphabetic()).count();
    }

    let reversed: String = input.chars().rev().collect();
    result += reversed.len();

    result
}

/// HashMap operations
#[inline(never)]
fn hash_operations(count: usize) -> usize {
    let mut map: HashMap<String, usize> = HashMap::new();

    // Insert
    for i in 0..count {
        map.insert(format!("key_{}", i), i * 2);
    }

    // Lookup
    let mut sum = 0usize;
    for i in 0..count {
        if let Some(v) = map.get(&format!("key_{}", i)) {
            sum += v;
        }
    }

    // Remove half
    for i in 0..count / 2 {
        map.remove(&format!("key_{}", i));
    }

    sum + map.len()
}

/// Vector allocation and operations
#[inline(never)]
fn vector_operations(size: usize) -> usize {
    // Allocate
    let mut vec: Vec<usize> = Vec::with_capacity(size);

    // Fill
    for i in 0..size {
        vec.push(i * 3);
    }

    // Sort (already sorted, but compiler doesn't know)
    vec.sort_unstable();

    // Binary search
    let mut found = 0usize;
    for i in (0..size).step_by(100) {
        if vec.binary_search(&(i * 3)).is_ok() {
            found += 1;
        }
    }

    // Sum
    let sum: usize = vec.iter().sum();

    sum / 1000 + found
}

/// Recursive Fibonacci (intentionally inefficient for CPU load)
#[inline(never)]
fn recursive_fibonacci(n: u32) -> u64 {
    if n <= 1 {
        n as u64
    } else {
        recursive_fibonacci(n - 1) + recursive_fibonacci(n - 2)
    }
}
