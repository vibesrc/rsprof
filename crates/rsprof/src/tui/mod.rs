mod app;
mod ui;

use crate::cpu::CpuSampler;
use crate::error::Result;
use crate::heap::{HeapSampler, ShmHeapSampler};
use crate::storage::Storage;
use crate::symbols::SymbolResolver;
use std::time::Duration;

pub use app::App;

/// Run the TUI profiler
pub fn run(
    perf_sampler: Option<CpuSampler>,
    heap_sampler: Option<HeapSampler>,
    shm_sampler: Option<ShmHeapSampler>,
    resolver: SymbolResolver,
    storage: Storage,
    checkpoint_interval: Duration,
    max_duration: Option<Duration>,
) -> Result<()> {
    let mut app = App::new(
        perf_sampler,
        heap_sampler,
        shm_sampler,
        resolver,
        storage,
        checkpoint_interval,
        max_duration,
    );
    app.run()
}
