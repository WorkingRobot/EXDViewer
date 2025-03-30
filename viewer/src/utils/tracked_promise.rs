use std::ops::{Deref, DerefMut};

use egui::Id;
use poll_promise::Promise;

pub struct TrackedPromise<T: Send + 'static> {
    promise: Promise<T>,
    context: Option<egui::Context>,
}

pub fn tick_promises(ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    poll_promise::tick_local();

    let running_futures = ctx.data(|w| {
        w.get_temp::<u32>(Id::new("running-futures"))
            .unwrap_or_default()
    });
    if running_futures != 0 {
        ctx.request_repaint();
    }
}

impl<T: Send + 'static> TrackedPromise<T> {
    pub fn spawn_local_untracked(future: impl Future<Output = T> + 'static) -> Self {
        Self {
            context: None,
            promise: Promise::spawn_local(future),
        }
    }

    pub fn spawn_local(ctx: egui::Context, future: impl Future<Output = T> + 'static) -> Self {
        Self {
            context: Some(ctx.clone()),
            promise: Promise::spawn_local(async move {
                Self::increment(&ctx);
                let ret = future.await;
                Self::decrement(&ctx);
                ret
            }),
        }
    }

    fn increment(ctx: &egui::Context) {
        ctx.data_mut(|w| {
            *w.get_temp_mut_or_default::<u32>(Id::new("running-futures")) += 1;
        });
    }

    fn decrement(ctx: &egui::Context) {
        ctx.data_mut(|w| {
            *w.get_temp_mut_or_default::<u32>(Id::new("running-futures")) -= 1;
        });
    }
}

impl<T: Send + 'static> From<TrackedPromise<T>> for Promise<T> {
    fn from(promise: TrackedPromise<T>) -> Self {
        promise.promise
    }
}

impl<T: Send + 'static> Deref for TrackedPromise<T> {
    type Target = Promise<T>;

    fn deref(&self) -> &Self::Target {
        &self.promise
    }
}

impl<T: Send + 'static> DerefMut for TrackedPromise<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.promise
    }
}
