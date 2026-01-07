use super::perf::PerfEvent;
use crate::error::Result;

/// CPU sampler that reads perf_event samples
pub struct CpuSampler {
    /// Per-thread perf events
    events: Vec<PerfEvent>,
}

impl CpuSampler {
    /// Create a new CPU sampler for all threads of a process
    pub fn new(pid: u32, freq: u64) -> Result<Self> {
        // For now, just sample the main thread
        // TODO: Sample all threads by enumerating /proc/[pid]/task/
        let event = PerfEvent::open(pid as i32, freq)?;

        Ok(CpuSampler {
            events: vec![event],
        })
    }

    /// Read all available samples from all threads
    pub fn read_samples(&mut self) -> Result<Vec<u64>> {
        let mut all_samples = Vec::new();

        for event in &mut self.events {
            let samples = event.read_samples();
            all_samples.extend(samples);
        }

        Ok(all_samples)
    }
}
