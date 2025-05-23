//! Lazy from promise

use crate::sync::promise::{BoxPromise, IntoPromise, Just, PollPromise, Promise};
use std::marker::PhantomData;

/// A lazy-like promise
pub struct Lazy<'lf, P: Promise<Output: 'lf> + 'lf> {
    state: LazyState<P>,
    _marker: PhantomData<&'lf ()>,
}

enum LazyState<P: Promise> {
    Promise(P),
    Ready(P::Output),
}

impl<'lf, P: Promise<Output: 'lf> + 'lf> Lazy<'lf, P> {
    /// Create a new lazy
    pub fn new<U: IntoPromise<IntoPromise = P>>(promise: U) -> Self {
        Lazy {
            state: LazyState::Promise(promise.into_promise()),
            _marker: PhantomData,
        }
    }

    /// Gets a reference poll promise. Unlike normal promises, ready can be used any amount of time
    pub fn poll_ref(&mut self) -> PollPromise<&P::Output> {
        let state = &mut self.state;
        match state {
            LazyState::Promise(p) => match p.poll() {
                PollPromise::Ready(ready) => {
                    *state = LazyState::Ready(ready);
                    let LazyState::Ready(ready) = state else {
                        unreachable!()
                    };
                    PollPromise::Ready(&*ready)
                }
                PollPromise::Pending => PollPromise::Pending,
            },
            LazyState::Ready(ready) => PollPromise::Ready(&*ready),
        }
    }

    /// Gets a reference poll promise. Unlike normal promises, ready can be used any amount of time
    pub fn poll_mut(&mut self) -> PollPromise<&mut P::Output> {
        let state = &mut self.state;
        match state {
            LazyState::Promise(p) => match p.poll() {
                PollPromise::Ready(ready) => {
                    *state = LazyState::Ready(ready);
                    let LazyState::Ready(ready) = state else {
                        unreachable!()
                    };
                    PollPromise::Ready(ready)
                }
                PollPromise::Pending => PollPromise::Pending,
            },
            LazyState::Ready(ready) => PollPromise::Ready(ready),
        }
    }
}

impl<'lf, P: Promise<Output: 'lf> + 'lf> IntoPromise for Lazy<'lf, P>
where
    P::Output: Send,
{
    type Output = <P as Promise>::Output;
    type IntoPromise = BoxPromise<'lf, <P as Promise>::Output>;

    fn into_promise(self) -> Self::IntoPromise {
        match self.state {
            LazyState::Promise(p) => Box::new(p) as BoxPromise<_>,
            LazyState::Ready(ready) => Box::new(Just::new(ready)) as BoxPromise<_>,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy() {
        let lazy = Lazy::new(Just::new(15));

    }
}
