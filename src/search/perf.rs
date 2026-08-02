use std::time::Instant;

pub const SEARCH_PERF_LOG: bool = false;

pub struct PerfSpan {
    start: Instant,
    label: &'static str,
}

impl PerfSpan {
    pub fn new(label: &'static str) -> Self {
        Self {
            start: Instant::now(),
            label,
        }
    }
}

impl Drop for PerfSpan {
    fn drop(&mut self) {
        if SEARCH_PERF_LOG {
            eprintln!("[search] {}: {:?}", self.label, self.start.elapsed());
        }
    }
}
