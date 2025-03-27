use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use egui::Id;
use poll_promise::Promise;

#[derive(Clone)]
pub struct BackgroundInitializer<T: 'static>(Arc<BackgroundInitializerImpl<T>>);

struct BackgroundInitializerImpl<T: 'static> {
    value: RefCell<Option<Arc<T>>>,
    initializer: TrackedPromise<anyhow::Result<()>>,
}

impl<T: 'static> BackgroundInitializer<T> {
    pub fn new(
        ctx: Option<&egui::Context>,
        future: impl Future<Output = anyhow::Result<T>> + 'static,
    ) -> Self {
        Self(Arc::new_cyclic(|this| {
            let this = this.clone();
            BackgroundInitializerImpl {
                value: RefCell::new(None),
                initializer: if let Some(ctx) = ctx {
                    TrackedPromise::spawn_local(ctx.clone(), async move {
                        let val = future.await?;
                        let this: Arc<BackgroundInitializerImpl<T>> =
                            this.upgrade().ok_or(anyhow::anyhow!("self dropped"))?;
                        *this.value.borrow_mut() = Some(Arc::new(val));
                        Ok(())
                    })
                } else {
                    TrackedPromise::spawn_local_untracked(async move {
                        let val = future.await?;
                        let this: Arc<BackgroundInitializerImpl<T>> =
                            this.upgrade().ok_or(anyhow::anyhow!("self dropped"))?;
                        *this.value.borrow_mut() = Some(Arc::new(val));
                        Ok(())
                    })
                },
            }
        }))
    }

    pub fn value(&self) -> Option<Arc<T>> {
        self.0.value.borrow().as_ref().cloned()
    }

    pub fn result(&self) -> Option<Result<Arc<T>, &anyhow::Error>> {
        self.0.initializer.ready().map(|r| match r {
            Ok(()) => Ok(self.value().unwrap()),
            Err(e) => Err(e),
        })
    }
}

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
