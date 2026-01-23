mod app;
mod ui;

use crate::cpu::CpuSampler;
use crate::error::Result;
use crate::heap::ShmHeapSampler;
use crate::storage::Storage;
use crate::symbols::SymbolResolver;
use std::time::Duration;

pub use app::App;

/// Run the TUI profiler
#[allow(clippy::too_many_arguments)]
pub fn run(
    perf_sampler: Option<CpuSampler>,
    shm_sampler: Option<ShmHeapSampler>,
    resolver: SymbolResolver,
    storage: Storage,
    checkpoint_interval: Duration,
    max_duration: Option<Duration>,
    include_internal: bool,
) -> Result<()> {
    let time_offset_secs = storage.time_offset_secs();
    let mut app = App::new(
        perf_sampler,
        shm_sampler,
        resolver,
        storage,
        checkpoint_interval,
        max_duration,
        include_internal,
        time_offset_secs,
    );
    app.run()
}
