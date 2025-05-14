use futures_util::FutureExt as _;
use pinned::oneshot::Receiver;

use super::{TrackedPromise, convertible_promise::PromiseKind};

pub struct UnsendPromise<T: 'static> {
    rx: Receiver<T>,
    promise: TrackedPromise<()>,
}

impl<T: 'static> UnsendPromise<T> {
    pub fn new(future: impl Future<Output = T> + 'static) -> Self {
        let (tx, rx) = pinned::oneshot::channel();
        let promise = TrackedPromise::spawn_local(async move {
            log::info!("UnsendPromise spawned");
            if tx.send(future.await).is_err() {
                unreachable!("UnsendPromise value already set");
            }
            log::info!("UnsendPromise completed");
        });
        Self { rx, promise }
    }
}

impl<T: 'static> PromiseKind for UnsendPromise<T> {
    type Output = T;

    fn ready(&self) -> bool {
        self.promise.ready()
    }

    fn block_and_take(self) -> Self::Output {
        let Self { rx, promise } = self;
        promise.block_and_take();
        rx.now_or_never()
            .expect("UnsendPromise value not set")
            .expect("UnsendPromise rx channel closed")
    }
}
