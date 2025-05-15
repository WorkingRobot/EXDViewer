use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use poll_promise::Promise;

use super::convertible_promise::PromiseKind;

/// A wrapper around `poll_promise::Promise` that tracks the number of running promises.
/// Use for notifying the UI when promises are running and redraws are needed.
pub struct TrackedPromise<T: Send + 'static>(Promise<T>);

static RUNNING_PROMISES: AtomicUsize = AtomicUsize::new(0);

/// Call this inside `App::update()`
pub fn tick_promises(ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    poll_promise::tick_local();

    if RUNNING_PROMISES.load(Ordering::SeqCst) != 0 {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl<T: Send + 'static> TrackedPromise<T> {
    pub fn spawn_local(future: impl Future<Output = T> + 'static) -> Self {
        Self(Promise::spawn_local(async move {
            Self::increment();
            let result = future.await;
            Self::decrement();
            result
        }))
    }

    pub fn try_get(&self) -> Option<&T> {
        self.0.ready()
    }

    fn increment() {
        RUNNING_PROMISES.fetch_add(1, Ordering::SeqCst);
    }

    fn decrement() {
        RUNNING_PROMISES.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<R: Send + 'static> PromiseKind for TrackedPromise<R> {
    type Output = R;

    fn ready(&self) -> bool {
        self.0.ready().is_some()
    }

    fn block_and_take(self) -> R {
        self.0.block_and_take()
    }
}
