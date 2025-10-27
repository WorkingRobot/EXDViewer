#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

pub struct Stopwatch {
    name: String,
    start: Instant,
}

impl Stopwatch {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let ret = Self {
            name: name.into(),
            start: Instant::now(),
        };
        log::debug!("{}: Start", ret.name);
        ret
    }

    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Default for Stopwatch {
    fn default() -> Self {
        Self::new("Stopwatch")
    }
}

impl Drop for Stopwatch {
    fn drop(&mut self) {
        log::info!(
            "{}: {:.4}ms",
            self.name,
            self.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

//pub type RepeatedStopwatch = WorkingRepeatedStopwatch;
pub type RepeatedStopwatch = DummyRepeatedStopwatch;

pub struct WorkingRepeatedStopwatch {
    name: &'static str,
    duration_ns: AtomicU64,
    count: AtomicUsize,
}

impl WorkingRepeatedStopwatch {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            duration_ns: AtomicU64::new(0),
            count: AtomicUsize::new(0),
        }
    }

    pub fn record(&self, duration: Duration) {
        self.duration_ns
            .fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.duration_ns.store(0, Ordering::SeqCst);
        self.count.store(0, Ordering::SeqCst);
    }

    pub fn start(&'_ self) -> RepeatedStopwatchGuard<'_> {
        RepeatedStopwatchGuard {
            parent: self,
            start: Instant::now(),
        }
    }

    pub fn report(&self) {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            log::info!("{}: No recorded measurements", self.name);
        } else {
            let total_ns = self.duration_ns.load(Ordering::Relaxed);
            let avg_ns = total_ns / count as u64;
            log::info!(
                "{}: {} measurements, total {:.4}ms, average {:.4}ms",
                self.name,
                count,
                (total_ns as f64) / 1_000_000.0,
                (avg_ns as f64) / 1_000_000.0
            );
        }
    }
}

pub struct RepeatedStopwatchGuard<'a> {
    parent: &'a WorkingRepeatedStopwatch,
    start: Instant,
}

impl Drop for RepeatedStopwatchGuard<'_> {
    fn drop(&mut self) {
        self.parent.record(self.start.elapsed());
    }
}

pub struct DummyRepeatedStopwatch;

impl DummyRepeatedStopwatch {
    #[must_use]
    pub const fn new(_name: &'static str) -> Self {
        Self
    }

    pub fn record(&self, _duration: Duration) {}

    pub fn reset(&self) {}

    pub fn start(&'_ self) -> () {}

    pub fn report(&self) {}
}

pub mod stopwatches {
    use super::RepeatedStopwatch;

    pub static FILTER_ROW_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Filter Row");

    pub static FILTER_CELL_ITER_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Cell Iter");

    pub static FILTER_CELL_GRAB_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Cell Grab");

    pub static FILTER_CELL_CREATE_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Cell Create");

    pub static FILTER_CELL_READ_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Cell Read");

    pub static FILTER_KEY_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Compare Key (Inner)");

    pub static FILTER_MATCH_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Match Cell");

    pub static FILTER_TOTAL_STOPWATCH: RepeatedStopwatch =
        RepeatedStopwatch::new("Sheet Table Total Filter");

    // pub static MULTILINE_STOPWATCH: RepeatedStopwatch = RepeatedStopwatch::new("Cell Multiline Size");
    // pub static MULTILINE2_STOPWATCH: RepeatedStopwatch =
    //     RepeatedStopwatch::new("Cell Multiline Size Actual");
    // pub static MULTILINE3_STOPWATCH: RepeatedStopwatch =
    //     RepeatedStopwatch::new("Cell Multiline Galley Layout");
    // pub static MULTILINE4_STOPWATCH: RepeatedStopwatch =
    //     RepeatedStopwatch::new("Cell Multiline Size Estimate");
}
