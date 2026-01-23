//! Main application logic.

use crate::analytics::AnalyticsEngine;
use crate::audit::AuditLog;
use crate::buffer_pool::{BufferPool, DepthPool, SawtoothPool, SineWavePool, SquarePool};
use crate::cache::DataCache;
use crate::checkout::CheckoutEngine;
use crate::cpu_patterns::{DepthCpuLoad, SawtoothCpuLoad, SineCpuLoad, SquareCpuLoad, StepCpuLoad};
use crate::model::{Request, Response, Route, Stats};
use crate::search::SearchEngine;
use crate::utils;
use crate::validation::InputValidator;

pub struct Application {
    tick: u64,
    stats: Stats,
    cache: DataCache,
    validator: InputValidator,
    search: SearchEngine,
    checkout: CheckoutEngine,
    analytics: AnalyticsEngine,
    audit: AuditLog,
    // Memory patterns
    buffer_pool: BufferPool,
    sine_pool: SineWavePool,
    sawtooth_pool: SawtoothPool,
    square_pool: SquarePool,
    depth_pool: DepthPool,
    // CPU patterns
    sine_cpu: SineCpuLoad,
    step_cpu: StepCpuLoad,
    sawtooth_cpu: SawtoothCpuLoad,
    square_cpu: SquareCpuLoad,
    depth_cpu: DepthCpuLoad,
}

impl Application {
    pub fn new() -> Self {
        Self {
            tick: 0,
            stats: Stats {
                requests: 0,
                cache_hits: 0,
                cache_misses: 0,
                errors: 0,
            },
            cache: DataCache::new(),
            validator: InputValidator::new(),
            search: SearchEngine::new(),
            checkout: CheckoutEngine::new(),
            analytics: AnalyticsEngine::new(),
            audit: AuditLog::new(),
            // Memory patterns
            buffer_pool: BufferPool::new(),
            sine_pool: SineWavePool::new(),
            sawtooth_pool: SawtoothPool::new(),
            square_pool: SquarePool::new(),
            depth_pool: DepthPool::new(),
            // CPU patterns
            sine_cpu: SineCpuLoad::new(),
            step_cpu: StepCpuLoad::new(),
            sawtooth_cpu: SawtoothCpuLoad::new(),
            square_cpu: SquareCpuLoad::new(),
            depth_cpu: DepthCpuLoad::new(),
        }
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    #[inline(never)]
    pub fn tick(&mut self) {
        self.tick += 1;
        self.stats.requests += 1;

        // Memory patterns
        self.buffer_pool.tick(); // Periodic flush: 10MB every 5s, flush at 100MB
        self.sine_pool.tick(); // Sine wave: 0→50MB→0 over 10s
        self.sawtooth_pool.tick(); // Sawtooth: ramp 0→40MB over 8s, drop
        self.square_pool.tick(); // Square: alternates 5MB/35MB every 3s
        self.depth_pool.tick(); // Varying depths: cycles through 4 depths over 12s

        // CPU patterns
        self.sine_cpu.tick(); // Sine wave CPU load over 10s
        self.step_cpu.tick(); // Step between low/high every 5s
        self.sawtooth_cpu.tick(); // Sawtooth: ramps up over 6s
        self.square_cpu.tick(); // Square: alternates low/high every 2s
        self.depth_cpu.tick(); // Varying depths: cycles through 4 depths over 8s

        let request = self.generate_request();
        let headers = utils::parse_headers(&request.payload);

        if !self.validator.validate(&request, &headers) {
            self.stats.errors += 1;
            return;
        }

        if let Some(cached) = self.cache.get(&request.key) {
            self.stats.cache_hits += 1;
            let _ = cached.len();
            return;
        }
        self.stats.cache_misses += 1;

        let response = self.handle_request(&request, &headers);

        if response.cacheable {
            self.cache.put(request.key.clone(), response.body.clone());
        }

        self.audit.record_event(&request, &response);
    }

    #[inline(never)]
    fn handle_request(&mut self, request: &Request, headers: &[(String, String)]) -> Response {
        match request.route {
            Route::Search => self.search.handle(request, headers),
            Route::Checkout => self.checkout.handle(request, headers),
            Route::Analytics => self.analytics.handle(request, headers),
            Route::Health => Response {
                status: 200,
                body: b"ok".to_vec(),
                cacheable: true,
            },
        }
    }

    #[inline(never)]
    fn generate_request(&self) -> Request {
        // Mix routes so multiple hotspots show up (search is still hottest, but not dominant).
        let route = match self.tick % 20 {
            0..=8 => Route::Search,
            9..=13 => Route::Checkout,
            14..=16 => Route::Analytics,
            _ => Route::Health,
        };

        let user_id = (self.tick % 512) + 1;
        let session_id = (self.tick % 2048) + 10;
        let key = format!("r{}_{}", user_id, self.tick % 200);

        let mut payload = vec![0u8; 96 + (self.tick % 128) as usize];
        utils::fill_payload(&mut payload, self.tick);

        Request {
            id: self.tick,
            user_id,
            session_id,
            key,
            payload,
            route,
            flags: (self.tick % 4) as u8,
        }
    }
}
