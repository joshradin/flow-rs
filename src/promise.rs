//! Provides the [`Promise`] type, a synchronous equivalent of a [`Future`].

use std::thread::yield_now;
use std::time::{Duration, Instant};

/// Polls a promise
#[derive(Debug)]
pub enum PollPromise<T> {
    Ready(T),
    Pending,
}

/// A type that eventually resolves to some value
pub trait Promise: Send {
    type Output;

    fn poll(&mut self) -> PollPromise<Self::Output>;
}

pub trait PromiseExt: Promise + Sized {
    /// Tries to get the value of this promise immediately
    fn try_get(mut self) -> Result<Self::Output, Self> {
        match self.poll() {
            PollPromise::Ready(t) => Ok(t),
            PollPromise::Pending => Err(self),
        }
    }

    /// Gets the value of this promise once it's ready
    fn get(mut self) -> Self::Output {
        loop {
            match self.poll() {
                PollPromise::Ready(t) => return t,
                PollPromise::Pending => {}
            }
            yield_now();
        }
    }

    /// Waits if necessary for at most the given computation to complete, then retrieves the result if available.
    fn get_timeout(mut self, timeout: Duration) -> Result<Self::Output, Self> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            match self.poll() {
                PollPromise::Ready(ready) => {
                    return Ok(ready);
                }
                PollPromise::Pending => {}
            }
            yield_now();
        }
        Err(self)
    }
}

impl<T: Promise> PromiseExt for T {}

/// Gets this type as a [`Promise`]
pub trait IntoPromise {
    type Output;
    type Promise: Promise<Output = Self::Output>;

    fn into_promise(self) -> Self::Promise;
}

impl<P: Promise> IntoPromise for P {
    type Output = P::Output;
    type Promise = Self;

    fn into_promise(self) -> Self::Promise {
        self
    }
}

/// A [`Promise`] that immediately returns
#[derive(Debug)]
pub struct Just<T>(Option<T>);

impl<T> Just<T> {
    /// Create a new Just
    pub fn new(t: T) -> Just<T> {
        Just(Some(t))
    }
}

impl<T: Send> Promise for Just<T> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self.0.take() {
            None => {
                panic!("Promise should not be polled after returning data")
            }
            Some(t) => PollPromise::Ready(t),
        }
    }
}

impl<'lf, T> Promise for Box<dyn Promise<Output = T> + 'lf> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        (**self).poll()
    }
}

pub type BoxPromise<'lf, T> = Box<dyn Promise<Output = T> + 'lf>;

pub struct PromiseSet<'lf, T: Send + 'lf> {
    finished: Vec<T>,
    promises: Vec<BoxPromise<'lf, T>>,
}

impl<'lf, T: Send + 'lf> Promise for PromiseSet<'lf, T> {
    type Output = Vec<T>;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        let Self { finished, promises } = self;

        let mut not_done = vec![];
        for mut promise in promises.drain(..) {
            match promise.poll() {
                PollPromise::Ready(ready) => {
                    finished.push(ready);
                }
                PollPromise::Pending => {
                    not_done.push(promise);
                }
            }
        }

        promises.extend(not_done);

        if promises.is_empty() {
            PollPromise::Ready(finished.drain(..).collect())
        } else {
            PollPromise::Pending
        }
    }
}

impl<'lf, T: Send + 'lf, P: Promise<Output=T> + 'lf> FromIterator<P> for PromiseSet<'lf, T> {
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> Self {
        Self {
            finished: vec![],
            promises: iter.into_iter().map(|b| Box::new(b) as BoxPromise<'lf, T>).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::promise::{BoxPromise, Just, PromiseExt};

    #[test]
    fn test_promise() {
        let promise = Just::new(111_i32);
        let resolved = promise.get();
        assert_eq!(resolved, 111);
    }

    #[test]
    fn test_boxed_promise() {
        let promise: BoxPromise<'static, i32> = Box::new(Just::new(111_i32));
        let resolved = promise.get();
        assert_eq!(resolved, 111);
    }
}
