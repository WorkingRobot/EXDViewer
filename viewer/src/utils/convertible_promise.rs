use either::Either::{self, Left, Right};
use poll_promise::Promise;

/// A trait that defines the behavior of a convertible promise.
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

/// A promise that whose value will be converted to a different type when accessed.
#[derive(Debug)]
pub struct ConvertiblePromise<P: PromiseKind, T>(Either<P, T>);

impl<P: PromiseKind, T> ConvertiblePromise<P, T> {
    pub fn new(value: T) -> Self {
        Self(Either::Right(value))
    }

    pub fn new_promise(promise: P) -> Self {
        Self(Either::Left(promise))
    }

    pub fn should_swap(&self) -> bool {
        matches!(&self.0, Left(promise) if promise.ready())
    }

    pub fn converted(&self) -> bool {
        matches!(&self.0, Right(_))
    }

    fn convert(&mut self, converter: impl FnOnce(P::Output) -> T) {
        if self.should_swap() {
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

    pub fn try_get(&self) -> Result<&T, &P> {
        match &self.0 {
            Left(promise) => Err(promise),
            Right(value) => Ok(value),
        }
    }

    pub fn try_get_mut(&mut self) -> Result<&mut T, &mut P> {
        match &mut self.0 {
            Left(promise) => Err(promise),
            Right(value) => Ok(value),
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

    pub fn get_mut_with<'a, 'b, P2: PromiseKind, T2>(
        &'a mut self,
        other: &'b mut ConvertiblePromise<P2, T2>,
        converter: impl FnOnce(Either<P::Output, T>, Either<P2::Output, T2>) -> (T, T2),
    ) -> Option<(&'a mut T, &'b mut T2)> {
        if (!self.converted() && !self.should_swap())
            || (!other.converted() && !other.should_swap())
        {
            return None;
        }

        // Convert both at the same time
        replace_with::replace_with_or_abort(&mut self.0, |this| {
            let this_input = this.map_left(PromiseKind::block_and_take);

            let mut converted_this_val = None;

            replace_with::replace_with_or_abort(&mut other.0, |other| {
                let other_input = other.map_left(PromiseKind::block_and_take);

                let (converted_this, converted_other) = converter(this_input, other_input);

                converted_this_val = Some(converted_this);

                Right(converted_other)
            });

            Right(converted_this_val.expect("Converter must always be called"))
        });

        self.0.as_mut().right().zip(other.0.as_mut().right())
    }
}
