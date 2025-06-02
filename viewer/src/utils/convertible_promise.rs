use either::Either::{self, Left, Right};
use poll_promise::Promise;

pub trait PromiseKind
where
    Self: Sized,
{
    type Output: 'static;

    fn ready(&self) -> bool;
    fn block_and_take(self) -> Self::Output;

    fn try_take(self) -> Result<Self::Output, Self> {
        if self.ready() {
            Ok(self.block_and_take())
        } else {
            Err(self)
        }
    }
}

impl<R: Send + 'static> PromiseKind for Promise<R> {
    type Output = R;

    fn ready(&self) -> bool {
        self.ready().is_some()
    }

    fn block_and_take(self) -> R {
        self.block_and_take()
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

    fn convert(&mut self, converter: impl FnOnce(P::Output) -> T) {
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
    }

    pub fn get_mut(&mut self, converter: impl FnOnce(P::Output) -> T) -> Option<&mut T> {
        self.convert(converter);
        self.0.as_mut().right()
    }

    pub fn get(&mut self, converter: impl FnOnce(P::Output) -> T) -> Option<&T> {
        self.convert(converter);
        self.0.as_ref().right()
    }
}
