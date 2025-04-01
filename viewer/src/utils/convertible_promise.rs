use std::ops::Deref;

use either::Either::{self, Left, Right};
use poll_promise::Promise;

pub trait PromiseKind {
    type Output: Send + 'static;

    fn ready(&self) -> bool;
    fn block_and_take(self) -> Self::Output;
}

impl<P: Into<Promise<R>> + Deref<Target = Promise<R>>, R: Send + 'static> PromiseKind for P {
    type Output = R;

    fn ready(&self) -> bool {
        self.deref().ready().is_some()
    }

    fn block_and_take(self) -> R {
        self.into().block_and_take()
    }
}

pub struct ConvertiblePromise<P: PromiseKind, T>(Either<P, T>);

impl<P: PromiseKind, T> ConvertiblePromise<P, T> {
    pub fn new(value: T) -> Self {
        Self(Either::Right(value))
    }

    pub fn new_promise(promise: P) -> Self {
        Self(Either::Left(promise))
    }

    pub fn get(&mut self, converter: impl FnOnce(P::Output) -> T) -> Option<&mut T> {
        let should_swap = if let Left(promise) = &self.0 {
            promise.ready()
        } else {
            false
        };

        if should_swap {
            replace_with::replace_with_or_abort(&mut self.0, |this| {
                let promise = match this {
                    Left(promise) => promise,
                    Right(_) => unreachable!(),
                };
                let result = promise.block_and_take();
                Right(converter(result))
            });
        }

        self.0.as_mut().right()
    }
}
