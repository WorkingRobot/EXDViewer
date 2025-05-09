use super::TrackedPromise;
use std::{cell::RefCell, sync::Arc};

#[derive(Clone)]
pub struct BackgroundInitializer<T: 'static>(Arc<BackgroundInitializerImpl<T>>);

struct BackgroundInitializerImpl<T: 'static> {
    value: RefCell<Option<Arc<T>>>,
    initializer: TrackedPromise<anyhow::Result<()>>,
}

impl<T: 'static> BackgroundInitializer<T> {
    pub fn new(
        ctx: &egui::Context,
        future: impl Future<Output = anyhow::Result<T>> + 'static,
    ) -> Self {
        Self(Arc::new_cyclic(|this| {
            let this = this.clone();
            BackgroundInitializerImpl {
                value: RefCell::new(None),
                initializer: TrackedPromise::spawn_local(ctx.clone(), async move {
                    let val = future.await?;
                    let this: Arc<BackgroundInitializerImpl<T>> =
                        this.upgrade().ok_or(anyhow::anyhow!("self dropped"))?;
                    *this.value.borrow_mut() = Some(Arc::new(val));
                    Ok(())
                }),
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
