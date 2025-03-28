use futures_util::{
    FutureExt,
    future::{LocalBoxFuture, Shared},
};

#[derive(Clone)]
pub struct SharedFuture<T: Clone + 'static>(Shared<LocalBoxFuture<'static, T>>);

impl<T: Clone + 'static> SharedFuture<T> {
    pub fn new(future: impl Future<Output = T> + 'static) -> Self {
        Self(future.boxed_local().shared())
    }

    pub fn into_shared(self) -> Shared<LocalBoxFuture<'static, T>> {
        self.0
    }
}

impl<T: Clone + 'static> From<SharedFuture<T>> for Shared<LocalBoxFuture<'static, T>> {
    fn from(shared_future: SharedFuture<T>) -> Self {
        shared_future.0
    }
}
