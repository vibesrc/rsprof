mod app;
mod ui;

use crate::cpu::CpuSampler;
use crate::error::Result;
use crate::storage::Storage;
use crate::symbols::SymbolResolver;
use std::time::Duration;

pub use app::App;

/// Run the TUI profiler
pub fn run(
    sampler: CpuSampler,
    resolver: SymbolResolver,
    storage: Storage,
    checkpoint_interval: Duration,
    max_duration: Option<Duration>,
) -> Result<()> {
    let mut app = App::new(sampler, resolver, storage, checkpoint_interval, max_duration);
    app.run()
}
