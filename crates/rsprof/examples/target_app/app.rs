//! Main application logic.

use crate::analytics::AnalyticsEngine;
use crate::audit::AuditLog;
use crate::cache::DataCache;
use crate::checkout::CheckoutEngine;
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
            0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 => Route::Search,
            9 | 10 | 11 | 12 | 13 => Route::Checkout,
            14 | 15 | 16 => Route::Analytics,
            _ => Route::Health,
        };

        let user_id = (self.tick % 512) + 1;
        let session_id = (self.tick % 2048) + 10;
        let key = format!("r{}_{}", user_id, self.tick % 200);

        let mut payload = vec![0u8; 96 + (self.tick % 128) as usize];
        utils::fill_payload(&mut payload, self.tick as u64);

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
