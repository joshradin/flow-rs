//! Disjointed output

use crate::sync::lazy::Lazy;
use crate::sync::promise::{BoxPromise, GetPromise, PollPromise, Promise};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::ops::{Deref, RangeBounds};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use parking_lot::Mutex;
use thiserror::Error;

/// Disjointed partial promise
pub struct Disjointed<'lf, T, P : Promise<Output=Vec<T>> + 'lf = BoxPromise<'lf, Vec<T>>>
where
    T: Send + 'lf,
{
    inner: Arc<DisjointedInner<'lf, T, P>>,
}

impl<'lf, T, P : Promise<Output=Vec<T>> + 'lf> Disjointed<'lf, T, P>
where
    T: Send + 'lf,
{
    /// Creates a new disjointed, along with a setter
    pub fn new(source: P) -> Self {
        todo!()
    }

    pub fn get(&self, index: usize) -> Result<DisjointedIndex<'lf, T>, DisjointedError> {
        todo!()
    }
    pub fn get_range<R: RangeBounds<usize>>(
        &self,
        index: R,
    ) -> Result<DisjointedRange<'lf, T>, DisjointedError> {
        todo!()
    }
}

impl<'lf, T> Debug for Disjointed<'lf, T>
where
    T: Send + 'lf,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Disjointed").finish()
    }
}

pub struct DisjointedIndex<'lf, T: Send> {
    inner: Arc<DisjointedInner<'lf, T, BoxPromise<'lf, Vec<T>>>>,
}

impl<'lf, T: Send> Promise for DisjointedIndex<'lf, T> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        todo!()
    }
}

pub struct DisjointedRange<'lf, T: Send> {
    inner: Arc<DisjointedInner<'lf, T, BoxPromise<'lf, Vec<T>>>>,
}

impl<'lf, T: Send> Promise for DisjointedRange<'lf, T> {
    type Output = Vec<T>;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        todo!()
    }
}

type Buf<T> = Vec<Mutex<Option<T>>>;
type BoxLazyInitializer<T> = Box<dyn FnOnce() -> Buf<T> + Send>;

struct DisjointedInner<'lf, T: Send + 'lf, P: Promise<Output=Vec<T>> + 'lf> {
    promise: Arc<Mutex<Option<P>>>,
    inner: OnceLock<Buf<T>>,
    _marker: PhantomData<&'lf ()>,
}

impl<'lf, T: Send + 'lf, P: Promise<Output=Vec<T>> + 'lf>  DisjointedInner<'lf, T, P> {
    //// Creates a new promise
    fn new(promise: P) -> Self {
        Self {
            promise: Arc::new(Mutex::new(Some(promise))),
            inner: Default::default(),
            _marker: Default::default(),
        }
    }

    fn get(&self) -> Option<&Buf<T>> {
        // let promise =self.promise.clone();
        // let f = || {
        //
        // };
        // self.inner.get_or_init(f)
        todo!()
    }
}
// unsafe impl<'lf, T: Send, P: Promise<Output=Vec<T>> + 'lf> Sync for DisjointedInner<'lf, T, P> {}

#[derive(Debug, Error)]
pub enum DisjointedError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crossbeam::channel::bounded;
    use crate::backend::disjointed::{Buf, Disjointed};
    use crate::backend::job::Data;
    use static_assertions::assert_impl_all;
    use crate::backend::recv_promise::RecvPromise;

    assert_impl_all!(Disjointed<'static, Data>: Send);
    assert_impl_all!(Buf<Data>: Sync);

    #[test]
    fn test_disjointed_inner() {
        let (tx, rx) = bounded::<Vec<usize>>(1);
        let receiver_promise = RecvPromise::new(rx);
        let inner = Arc::new(DisjointedInner::new(receiver_promise));

        assert!(inner.get().is_none());

    }
}
