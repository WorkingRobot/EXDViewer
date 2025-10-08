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

pub struct RepeatedStopwatch {
    name: &'static str,
    duration_ns: AtomicU64,
    count: AtomicUsize,
}

impl RepeatedStopwatch {
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

    pub fn average(&self) -> Duration {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            Duration::ZERO
        } else {
            let total_ns = self.duration_ns.load(Ordering::Relaxed);
            Duration::from_nanos(total_ns / count as u64)
        }
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
    parent: &'a RepeatedStopwatch,
    start: Instant,
}

impl Drop for RepeatedStopwatchGuard<'_> {
    fn drop(&mut self) {
        self.parent.record(self.start.elapsed());
    }
}
