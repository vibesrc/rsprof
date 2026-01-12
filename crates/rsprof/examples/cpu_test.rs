//! Simple CPU profiling test - no custom allocator

fn main() {
    println!("=== CPU Test ===");
    println!("PID: {}", std::process::id());
    println!("Press Ctrl-C to stop.");

    loop {
        // CPU work that should show up in profiles
        let primes = calculate_primes(2000);
        let sorted = bubble_sort(300);

        if primes + sorted > 0 {
            // Prevent optimizer from removing the work
        }

        // No sleep - burn CPU continuously for profiling test
        // std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[inline(never)]
fn calculate_primes(limit: usize) -> usize {
    let mut count = 0;
    for n in 2..=limit {
        if is_prime(n) {
            count += 1;
        }
    }
    count
}

#[inline(never)]
fn is_prime(n: usize) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 {
        return true;
    }
    if n.is_multiple_of(2) {
        return false;
    }

    let limit = (n as f64).sqrt() as usize + 1;
    for i in (3..=limit).step_by(2) {
        if n.is_multiple_of(i) {
            return false;
        }
    }
    true
}

#[inline(never)]
fn bubble_sort(size: usize) -> usize {
    let mut data: Vec<i32> = (0..size as i32).rev().collect();

    for i in 0..data.len() {
        for j in 0..data.len() - 1 - i {
            if data[j] > data[j + 1] {
                data.swap(j, j + 1);
            }
        }
    }

    data.iter().sum::<i32>() as usize
}
