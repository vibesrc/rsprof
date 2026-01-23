//! Buffer pool that accumulates memory and periodically flushes.
//!
//! Patterns:
//! - BufferPool: Sawtooth - allocates 10MB every 5s, flushes at 100MB
//! - SineWavePool: Smooth sine oscillation 0-50MB
//! - SawtoothPool: Ramps up then drops (distinct from BufferPool's periodic flush)
//! - SquarePool: Alternates between min and max allocation
//! - DepthPool: Allocations at varying call depths

use std::f64::consts::PI;
use std::time::{Duration, Instant};

const CHUNK_SIZE: usize = 10 * 1024 * 1024; // 10MB per allocation
const MAX_SIZE: usize = 100 * 1024 * 1024; // 100MB threshold
const ALLOC_INTERVAL: Duration = Duration::from_secs(5);

pub struct BufferPool {
    buffers: Vec<Vec<u8>>,
    last_alloc: Option<Instant>,
    total_bytes: usize,
}

impl BufferPool {
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
            last_alloc: None, // None = allocate immediately on first tick
            total_bytes: 0,
        }
    }

    /// Called each tick - allocates memory on schedule and flushes when full
    pub fn tick(&mut self) {
        // Check if it's time to allocate (None = first tick, allocate immediately)
        let should_alloc = match self.last_alloc {
            None => true,
            Some(t) => t.elapsed() >= ALLOC_INTERVAL,
        };

        if should_alloc {
            self.allocate_chunk();
            self.last_alloc = Some(Instant::now());
        }

        // Flush if we've hit the threshold
        if self.total_bytes >= MAX_SIZE {
            self.flush();
        }
    }

    /// Allocate a new 10MB chunk
    #[inline(never)]
    fn allocate_chunk(&mut self) {
        let chunk = self.create_buffer();
        self.total_bytes += chunk.len();
        self.buffers.push(chunk);
        eprintln!(
            "[BufferPool] Allocated 10MB, total: {}MB",
            self.total_bytes / (1024 * 1024)
        );
    }

    /// Create the actual buffer (separate function for clear stack traces)
    #[inline(never)]
    fn create_buffer(&self) -> Vec<u8> {
        // Fill with pattern so it's not optimized away
        vec![0xAB; CHUNK_SIZE]
    }

    /// Flush all buffers back to zero
    #[inline(never)]
    fn flush(&mut self) {
        let count = self.buffers.len();
        self.buffers.clear();
        self.total_bytes = 0;
        eprintln!("[BufferPool] Flushed {} buffers, total: 0MB", count);
    }

    #[allow(dead_code)]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    #[allow(dead_code)]
    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }
}

// =============================================================================
// Sine wave memory pattern - smoothly oscillates memory usage
// =============================================================================

const SINE_PERIOD: f64 = 10.0; // 10 second period
const SINE_MAX_MB: usize = 50; // Peak at 50MB
const SINE_CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks for granularity

pub struct SineWavePool {
    buffers: Vec<Vec<u8>>,
    start_time: Instant,
}

impl SineWavePool {
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
            start_time: Instant::now(),
        }
    }

    /// Called each tick - adjusts memory to follow sine wave
    pub fn tick(&mut self) {
        let target_bytes = self.compute_target();
        let current_bytes = self.buffers.len() * SINE_CHUNK_SIZE;

        if current_bytes < target_bytes {
            // Need to allocate more
            let chunks_needed = (target_bytes - current_bytes) / SINE_CHUNK_SIZE;
            for _ in 0..chunks_needed {
                self.allocate_chunk();
            }
        } else if current_bytes > target_bytes + SINE_CHUNK_SIZE {
            // Need to free some
            let chunks_to_free = (current_bytes - target_bytes) / SINE_CHUNK_SIZE;
            self.free_chunks(chunks_to_free);
        }
    }

    /// Compute target bytes based on sine wave
    fn compute_target(&self) -> usize {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let phase = (elapsed / SINE_PERIOD) * 2.0 * PI;
        // sin goes -1 to 1, map to 0 to MAX
        let normalized = (phase.sin() + 1.0) / 2.0;
        (normalized * (SINE_MAX_MB * 1024 * 1024) as f64) as usize
    }

    #[inline(never)]
    fn allocate_chunk(&mut self) {
        let chunk = self.create_sine_buffer();
        self.buffers.push(chunk);
    }

    #[inline(never)]
    fn create_sine_buffer(&self) -> Vec<u8> {
        vec![0xCD; SINE_CHUNK_SIZE]
    }

    #[inline(never)]
    fn free_chunks(&mut self, count: usize) {
        for _ in 0..count.min(self.buffers.len()) {
            self.buffers.pop();
        }
    }
}

// =============================================================================
// Sawtooth memory pattern - ramps up then drops completely
// =============================================================================

const SAWTOOTH_PERIOD: f64 = 8.0; // 8 second ramp
const SAWTOOTH_MAX_MB: usize = 40;
const SAWTOOTH_CHUNK_SIZE: usize = 512 * 1024; // 512KB chunks

pub struct SawtoothPool {
    buffers: Vec<Vec<u8>>,
    start_time: Instant,
}

impl SawtoothPool {
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
            start_time: Instant::now(),
        }
    }

    pub fn tick(&mut self) {
        let target_bytes = self.compute_target();
        let current_bytes = self.buffers.len() * SAWTOOTH_CHUNK_SIZE;

        if current_bytes < target_bytes {
            let chunks_needed = (target_bytes - current_bytes) / SAWTOOTH_CHUNK_SIZE;
            for _ in 0..chunks_needed {
                self.allocate_sawtooth_chunk();
            }
        } else if current_bytes > target_bytes + SAWTOOTH_CHUNK_SIZE {
            let chunks_to_free = (current_bytes - target_bytes) / SAWTOOTH_CHUNK_SIZE;
            self.free_sawtooth_chunks(chunks_to_free);
        }
    }

    fn compute_target(&self) -> usize {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        // Sawtooth: ramps 0→MAX over period, then drops to 0
        let phase = (elapsed % SAWTOOTH_PERIOD) / SAWTOOTH_PERIOD;
        (phase * (SAWTOOTH_MAX_MB * 1024 * 1024) as f64) as usize
    }

    #[inline(never)]
    fn allocate_sawtooth_chunk(&mut self) {
        let chunk = self.create_sawtooth_buffer();
        self.buffers.push(chunk);
    }

    #[inline(never)]
    fn create_sawtooth_buffer(&self) -> Vec<u8> {
        vec![0xAA; SAWTOOTH_CHUNK_SIZE]
    }

    #[inline(never)]
    fn free_sawtooth_chunks(&mut self, count: usize) {
        for _ in 0..count.min(self.buffers.len()) {
            self.buffers.pop();
        }
    }
}

// =============================================================================
// Square wave memory pattern - alternates between min and max
// =============================================================================

const SQUARE_PERIOD: f64 = 6.0; // 6 second period (3s low, 3s high)
const SQUARE_LOW_MB: usize = 5;
const SQUARE_HIGH_MB: usize = 35;
const SQUARE_CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks

pub struct SquarePool {
    buffers: Vec<Vec<u8>>,
    start_time: Instant,
}

impl SquarePool {
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
            start_time: Instant::now(),
        }
    }

    pub fn tick(&mut self) {
        let target_bytes = self.compute_target();
        let current_bytes = self.buffers.len() * SQUARE_CHUNK_SIZE;

        if current_bytes < target_bytes {
            let chunks_needed = (target_bytes - current_bytes) / SQUARE_CHUNK_SIZE;
            for _ in 0..chunks_needed {
                self.allocate_square_chunk();
            }
        } else if current_bytes > target_bytes + SQUARE_CHUNK_SIZE {
            let chunks_to_free = (current_bytes - target_bytes) / SQUARE_CHUNK_SIZE;
            self.free_square_chunks(chunks_to_free);
        }
    }

    fn compute_target(&self) -> usize {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let phase = (elapsed % SQUARE_PERIOD) / SQUARE_PERIOD;
        let target_mb = if phase < 0.5 {
            SQUARE_LOW_MB
        } else {
            SQUARE_HIGH_MB
        };
        target_mb * 1024 * 1024
    }

    #[inline(never)]
    fn allocate_square_chunk(&mut self) {
        let chunk = self.create_square_buffer();
        self.buffers.push(chunk);
    }

    #[inline(never)]
    fn create_square_buffer(&self) -> Vec<u8> {
        vec![0xBB; SQUARE_CHUNK_SIZE]
    }

    #[inline(never)]
    fn free_square_chunks(&mut self, count: usize) {
        for _ in 0..count.min(self.buffers.len()) {
            self.buffers.pop();
        }
    }
}

// =============================================================================
// Depth pool - allocations at varying call stack depths
// =============================================================================

const DEPTH_PERIOD: f64 = 12.0; // 12 seconds to cycle through depths
const DEPTH_CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2MB chunks
const DEPTH_TARGET_MB: usize = 20; // Each depth level targets ~20MB

pub struct DepthPool {
    depth_1_buffers: Vec<Vec<u8>>,
    depth_2_buffers: Vec<Vec<u8>>,
    depth_3_buffers: Vec<Vec<u8>>,
    depth_4_buffers: Vec<Vec<u8>>,
    start_time: Instant,
}

impl DepthPool {
    pub fn new() -> Self {
        Self {
            depth_1_buffers: Vec::new(),
            depth_2_buffers: Vec::new(),
            depth_3_buffers: Vec::new(),
            depth_4_buffers: Vec::new(),
            start_time: Instant::now(),
        }
    }

    pub fn tick(&mut self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        // Each depth gets a 3-second window within the 12-second period
        let phase = (elapsed % DEPTH_PERIOD) / DEPTH_PERIOD;
        let active_depth = (phase * 4.0) as u32;

        // Each depth level: ramp up during its window, then free
        match active_depth {
            0 => {
                self.grow_depth_1();
                self.shrink_depth_2();
                self.shrink_depth_3();
                self.shrink_depth_4();
            }
            1 => {
                self.shrink_depth_1();
                self.grow_depth_2();
                self.shrink_depth_3();
                self.shrink_depth_4();
            }
            2 => {
                self.shrink_depth_1();
                self.shrink_depth_2();
                self.grow_depth_3();
                self.shrink_depth_4();
            }
            _ => {
                self.shrink_depth_1();
                self.shrink_depth_2();
                self.shrink_depth_3();
                self.grow_depth_4();
            }
        }
    }

    // Depth 1: Direct allocation
    #[inline(never)]
    fn grow_depth_1(&mut self) {
        let target = DEPTH_TARGET_MB * 1024 * 1024;
        let current = self.depth_1_buffers.len() * DEPTH_CHUNK_SIZE;
        if current < target {
            self.allocate_depth_1();
        }
    }

    #[inline(never)]
    fn allocate_depth_1(&mut self) {
        let chunk = self.create_depth_1_buffer();
        self.depth_1_buffers.push(chunk);
    }

    #[inline(never)]
    fn create_depth_1_buffer(&self) -> Vec<u8> {
        vec![0xD1; DEPTH_CHUNK_SIZE]
    }

    #[inline(never)]
    fn shrink_depth_1(&mut self) {
        if !self.depth_1_buffers.is_empty() {
            self.depth_1_buffers.pop();
        }
    }

    // Depth 2: One level of indirection
    #[inline(never)]
    fn grow_depth_2(&mut self) {
        let target = DEPTH_TARGET_MB * 1024 * 1024;
        let current = self.depth_2_buffers.len() * DEPTH_CHUNK_SIZE;
        if current < target {
            self.allocate_depth_2();
        }
    }

    #[inline(never)]
    fn allocate_depth_2(&mut self) {
        self.depth_2_inner_alloc();
    }

    #[inline(never)]
    fn depth_2_inner_alloc(&mut self) {
        let chunk = self.create_depth_2_buffer();
        self.depth_2_buffers.push(chunk);
    }

    #[inline(never)]
    fn create_depth_2_buffer(&self) -> Vec<u8> {
        vec![0xD2; DEPTH_CHUNK_SIZE]
    }

    #[inline(never)]
    fn shrink_depth_2(&mut self) {
        if !self.depth_2_buffers.is_empty() {
            self.depth_2_buffers.pop();
        }
    }

    // Depth 3: Two levels of indirection
    #[inline(never)]
    fn grow_depth_3(&mut self) {
        let target = DEPTH_TARGET_MB * 1024 * 1024;
        let current = self.depth_3_buffers.len() * DEPTH_CHUNK_SIZE;
        if current < target {
            self.allocate_depth_3();
        }
    }

    #[inline(never)]
    fn allocate_depth_3(&mut self) {
        self.depth_3_level_a();
    }

    #[inline(never)]
    fn depth_3_level_a(&mut self) {
        self.depth_3_level_b();
    }

    #[inline(never)]
    fn depth_3_level_b(&mut self) {
        let chunk = self.create_depth_3_buffer();
        self.depth_3_buffers.push(chunk);
    }

    #[inline(never)]
    fn create_depth_3_buffer(&self) -> Vec<u8> {
        vec![0xD3; DEPTH_CHUNK_SIZE]
    }

    #[inline(never)]
    fn shrink_depth_3(&mut self) {
        if !self.depth_3_buffers.is_empty() {
            self.depth_3_buffers.pop();
        }
    }

    // Depth 4: Three levels of indirection
    #[inline(never)]
    fn grow_depth_4(&mut self) {
        let target = DEPTH_TARGET_MB * 1024 * 1024;
        let current = self.depth_4_buffers.len() * DEPTH_CHUNK_SIZE;
        if current < target {
            self.allocate_depth_4();
        }
    }

    #[inline(never)]
    fn allocate_depth_4(&mut self) {
        self.depth_4_level_a();
    }

    #[inline(never)]
    fn depth_4_level_a(&mut self) {
        self.depth_4_level_b();
    }

    #[inline(never)]
    fn depth_4_level_b(&mut self) {
        self.depth_4_level_c();
    }

    #[inline(never)]
    fn depth_4_level_c(&mut self) {
        let chunk = self.create_depth_4_buffer();
        self.depth_4_buffers.push(chunk);
    }

    #[inline(never)]
    fn create_depth_4_buffer(&self) -> Vec<u8> {
        vec![0xD4; DEPTH_CHUNK_SIZE]
    }

    #[inline(never)]
    fn shrink_depth_4(&mut self) {
        if !self.depth_4_buffers.is_empty() {
            self.depth_4_buffers.pop();
        }
    }
}
