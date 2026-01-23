//! CPU-intensive patterns for testing CPU profiling attribution.
//!
//! - SineCpuLoad: Varies CPU work in a sine wave pattern
//! - StepCpuLoad: Steps between low and high CPU usage
//! - DepthCpuLoad: CPU work at varying call depths

use std::f64::consts::PI;
use std::time::Instant;

const SINE_PERIOD: f64 = 10.0; // 10 second period
const STEP_PERIOD: f64 = 5.0; // 5 seconds per step
const DEPTH_PERIOD: f64 = 8.0; // 8 seconds to cycle through depths

// =============================================================================
// Sine wave CPU load
// =============================================================================

pub struct SineCpuLoad {
    start_time: Instant,
}

impl SineCpuLoad {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
        }
    }

    /// Called each tick - does variable CPU work based on sine wave
    #[inline(never)]
    pub fn tick(&self) {
        let iterations = self.compute_iterations();
        self.do_sine_work(iterations);
    }

    fn compute_iterations(&self) -> u32 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let phase = (elapsed / SINE_PERIOD) * 2.0 * PI;
        // sin goes -1 to 1, map to 100 to 10000 iterations
        let normalized = (phase.sin() + 1.0) / 2.0;
        (100.0 + normalized * 9900.0) as u32
    }

    #[inline(never)]
    fn do_sine_work(&self, iterations: u32) {
        let mut sum = 0u64;
        for i in 0..iterations {
            sum = sum.wrapping_add(self.sine_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn sine_compute(&self, x: u32) -> u64 {
        // Some busy work that won't be optimized away
        let mut val = x as u64;
        for _ in 0..50 {
            val = val.wrapping_mul(1103515245).wrapping_add(12345);
            val ^= val >> 16;
        }
        val
    }
}

// =============================================================================
// Step CPU load - alternates between low and high
// =============================================================================

pub struct StepCpuLoad {
    start_time: Instant,
}

impl StepCpuLoad {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
        }
    }

    /// Called each tick - alternates between low and high CPU work
    #[inline(never)]
    pub fn tick(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let step = (elapsed / STEP_PERIOD) as u32;

        if step.is_multiple_of(2) {
            self.do_low_work();
        } else {
            self.do_high_work();
        }
    }

    #[inline(never)]
    fn do_low_work(&self) {
        let mut sum = 0u64;
        for i in 0..500 {
            sum = sum.wrapping_add(self.step_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn do_high_work(&self) {
        let mut sum = 0u64;
        for i in 0..5000 {
            sum = sum.wrapping_add(self.step_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn step_compute(&self, x: u32) -> u64 {
        let mut val = x as u64;
        for _ in 0..100 {
            val = val.wrapping_mul(6364136223846793005).wrapping_add(1);
            val ^= val >> 33;
        }
        val
    }
}

// =============================================================================
// Sawtooth CPU load - ramps up then drops
// =============================================================================

const SAWTOOTH_PERIOD: f64 = 6.0; // 6 second ramp

pub struct SawtoothCpuLoad {
    start_time: Instant,
}

impl SawtoothCpuLoad {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
        }
    }

    #[inline(never)]
    pub fn tick(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        // Sawtooth: ramps 0→1 over period, then drops
        let phase = (elapsed % SAWTOOTH_PERIOD) / SAWTOOTH_PERIOD;
        let iterations = (100.0 + phase * 9900.0) as u32;
        self.do_sawtooth_work(iterations);
    }

    #[inline(never)]
    fn do_sawtooth_work(&self, iterations: u32) {
        let mut sum = 0u64;
        for i in 0..iterations {
            sum = sum.wrapping_add(self.sawtooth_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn sawtooth_compute(&self, x: u32) -> u64 {
        let mut val = x as u64;
        for _ in 0..50 {
            val = val
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            val ^= val >> 21;
        }
        val
    }
}

// =============================================================================
// Square wave CPU load - alternates between min and max
// =============================================================================

const SQUARE_PERIOD: f64 = 4.0; // 4 second period (2s low, 2s high)

pub struct SquareCpuLoad {
    start_time: Instant,
}

impl SquareCpuLoad {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
        }
    }

    #[inline(never)]
    pub fn tick(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let phase = (elapsed % SQUARE_PERIOD) / SQUARE_PERIOD;

        if phase < 0.5 {
            self.do_square_low();
        } else {
            self.do_square_high();
        }
    }

    #[inline(never)]
    fn do_square_low(&self) {
        let mut sum = 0u64;
        for i in 0..200 {
            sum = sum.wrapping_add(self.square_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn do_square_high(&self) {
        let mut sum = 0u64;
        for i in 0..8000 {
            sum = sum.wrapping_add(self.square_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn square_compute(&self, x: u32) -> u64 {
        let mut val = x as u64;
        for _ in 0..80 {
            val = val
                .wrapping_mul(3935559000370003845)
                .wrapping_add(1442695040888963407);
            val ^= val >> 27;
        }
        val
    }
}

// =============================================================================
// Depth CPU load - calls work at varying call depths
// =============================================================================

pub struct DepthCpuLoad {
    start_time: Instant,
}

impl DepthCpuLoad {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
        }
    }

    #[inline(never)]
    pub fn tick(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        // Cycle through depths 1-4 over 8 seconds
        let depth = ((elapsed % DEPTH_PERIOD) / 2.0) as u32 % 4;

        match depth {
            0 => self.depth_1_work(),
            1 => self.depth_2_work(),
            2 => self.depth_3_work(),
            _ => self.depth_4_work(),
        }
    }

    #[inline(never)]
    fn depth_1_work(&self) {
        self.do_depth_compute(3000);
    }

    #[inline(never)]
    fn depth_2_work(&self) {
        self.depth_2_inner();
    }

    #[inline(never)]
    fn depth_2_inner(&self) {
        self.do_depth_compute(3000);
    }

    #[inline(never)]
    fn depth_3_work(&self) {
        self.depth_3_a();
    }

    #[inline(never)]
    fn depth_3_a(&self) {
        self.depth_3_b();
    }

    #[inline(never)]
    fn depth_3_b(&self) {
        self.do_depth_compute(3000);
    }

    #[inline(never)]
    fn depth_4_work(&self) {
        self.depth_4_a();
    }

    #[inline(never)]
    fn depth_4_a(&self) {
        self.depth_4_b();
    }

    #[inline(never)]
    fn depth_4_b(&self) {
        self.depth_4_c();
    }

    #[inline(never)]
    fn depth_4_c(&self) {
        self.do_depth_compute(3000);
    }

    #[inline(never)]
    fn do_depth_compute(&self, iterations: u32) {
        let mut sum = 0u64;
        for i in 0..iterations {
            sum = sum.wrapping_add(self.depth_compute(i));
        }
        std::hint::black_box(sum);
    }

    #[inline(never)]
    fn depth_compute(&self, x: u32) -> u64 {
        let mut val = x as u64;
        for _ in 0..60 {
            val = val
                .wrapping_mul(1442695040888963407)
                .wrapping_add(7046029254386353087);
            val ^= val >> 29;
        }
        val
    }
}
