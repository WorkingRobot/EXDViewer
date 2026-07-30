use std::{
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use poll_promise::Promise;

use super::convertible_promise::PromiseKind;

/// A wrapper around `poll_promise::Promise` that tracks the number of running promises.
/// Use for notifying the UI when promises are running and redraws are needed.
pub struct TrackedPromise<T: Send + 'static>(Promise<T>);

static RUNNING_PROMISES: AtomicUsize = AtomicUsize::new(0);
static PROMISE_CTX: OnceLock<egui::Context> = OnceLock::new();

/// Call this inside `App::update()`
pub fn tick_promises(ctx: &egui::Context) {
    PROMISE_CTX.get_or_init(|| ctx.clone());

    #[cfg(not(target_arch = "wasm32"))]
    poll_promise::tick_local();

    if RUNNING_PROMISES.load(Ordering::SeqCst) != 0 {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Holds a promise's place in the running count. Counting from a guard rather than around the await
/// keeps the tally right when the UI abandons a request: dropping a promise cancels its future on
/// native, so a count kept inside the future would never come back down.
struct Running;

impl Running {
    fn new() -> Self {
        RUNNING_PROMISES.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING_PROMISES.fetch_sub(1, Ordering::SeqCst);
        PROMISE_CTX.get().inspect(|ctx| {
            ctx.request_repaint();
        });
    }
}

impl<T: Send + 'static> TrackedPromise<T> {
    pub fn spawn_local(future: impl Future<Output = T> + 'static) -> Self {
        let running = Running::new();
        Self(Promise::spawn_local(async move {
            let _running = running;
            future.await
        }))
    }

    pub fn try_get(&self) -> Option<&T> {
        self.0.ready()
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

impl<T: Send + 'static> std::fmt::Debug for TrackedPromise<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackedPromise").finish_non_exhaustive()
    }
}
