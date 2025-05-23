//! Disjointed output

use crate::backend::job::{Data, Input, InputKind, InputSource};
use crate::backend::overlap_checking::OverlapChecker;
use crate::sync::once_lock::OnceLock;
use crate::sync::promise::{BoxPromise, IntoPromise, MapPromise, PollPromise, Promise};
use crate::{InputFlavor, JobError};
use parking_lot::Mutex;
use std::any::type_name;
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;
use thiserror::Error;

/// Disjointed partial promise
pub struct Disjointed<'lf, T, P: Promise<Output = Vec<T>> + 'lf = BoxPromise<'lf, Vec<T>>>
where
    T: Send + 'lf,
{
    inner: Arc<DisjointedInner<'lf, T, P>>,
}

impl<'lf, T, P: Promise<Output = Vec<T>> + 'lf> Disjointed<'lf, T, P>
where
    T: Send + 'lf,
{
    /// Creates a new disjointed, along with a setter
    pub fn new(source: P) -> Self {
        Self {
            inner: Arc::new(DisjointedInner::new(source)),
        }
    }

    pub fn get(&self, index: usize) -> Result<DisjointedIndex<'lf, T, P>, DisjointedError> {
        if self.inner.checker.lock().insert(index..(index + 1)) {
            Ok(DisjointedIndex {
                index,
                inner: self.inner.clone(),
            })
        } else {
            Err(DisjointedError::IndexAlreadyUsed(index))
        }
    }

    pub fn get_range<R: RangeBounds<usize> + Send + 'lf>(
        &self,
        index: R,
    ) -> Result<DisjointedRange<'lf, T, R, P>, DisjointedError> {
        let (start, end) = (index.start_bound().cloned(), index.end_bound().cloned());
        if self.inner.checker.lock().insert((start, end)) {
            Ok(DisjointedRange {
                range: index,
                inner: self.inner.clone(),
            })
        } else {
            Err(DisjointedError::RangeOverlaps(start, end))
        }
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

pub struct DisjointedIndex<'lf, T: Send + 'lf, P: Promise<Output = Vec<T>> + 'lf> {
    index: usize,
    inner: Arc<DisjointedInner<'lf, T, P>>,
}

impl<'lf, T: Send, P: Promise<Output = Vec<T>> + 'lf> Debug for DisjointedIndex<'lf, T, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisjointedIndex")
            .field("index", &self.index)
            .finish()
    }
}

impl<'lf, T: Send, P: Promise<Output = Vec<T>> + 'lf> Promise for DisjointedIndex<'lf, T, P> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self.inner.get() {
            None => PollPromise::Pending,
            Some(ready) => match ready[self.index].lock().take() {
                None => {
                    panic!("Tried to poll empty disjointed index");
                }
                Some(g) => PollPromise::Ready(g),
            },
        }
    }
}

impl<P: Promise<Output = Vec<Data>> + 'static> InputSource<DisjointedIndex<'static, Data, P>>
    for Input
{
    type Data = Data;

    fn use_as_input_source(
        &mut self,
        other: DisjointedIndex<'static, Data, P>,
    ) -> Result<(), JobError> {
        let data_promise = other.into_promise();
        match (self.flavor, &mut self.kind) {
            (InputFlavor::None, _) => Err(JobError::UnexpectedInput),
            (InputFlavor::Single, to_set @ InputKind::None) => {
                *to_set = InputKind::Single(Box::new(data_promise));
                Ok(())
            }
            (InputFlavor::Single, InputKind::Single(_)) => Err(JobError::InputAlreadySet),
            (InputFlavor::Funnel, InputKind::Funnel(funnel)) => {
                funnel.insert(data_promise);
                Ok(())
            }
            _ => {
                panic!("input kind and flavor mismatch")
            }
        }
    }
}

pub struct DisjointedRange<
    'lf,
    T: Send + 'lf,
    R: RangeBounds<usize> + Send + 'lf,
    P: Promise<Output = Vec<T>> + 'lf,
> {
    range: R,
    inner: Arc<DisjointedInner<'lf, T, P>>,
}

// impl<R: RangeBounds<usize> + Send, P: Promise<Output = Vec<Data>> + 'static>
//     InputSource<DisjointedRange<'static, Data, R, P>> for Input
// {
//     type Data = Vec<Data>;
//
//     fn use_as_input_source(
//         &mut self,
//         other: DisjointedRange<'static, Data, R, P>,
//     ) -> Result<(), JobError> {
//         let data_promise = other.into_promise().map(|d| Box::new(d) as Data);
//         match (self.flavor, &mut self.kind) {
//             (InputFlavor::None, _) => Err(JobError::UnexpectedInput),
//             (InputFlavor::Single, to_set @ InputKind::None) => {
//                 *to_set = InputKind::Single(Box::new(data_promise));
//                 Ok(())
//             }
//             (InputFlavor::Single, InputKind::Single(_)) => Err(JobError::InputAlreadySet),
//             (InputFlavor::Funnel, InputKind::Funnel(funnel)) => {
//                 // funnel.insert(data_promise);
//                 Ok(())
//             }
//             _ => {
//                 panic!("input kind and flavor mismatch")
//             }
//         }
//     }
// }

impl<'lf, T: Send + 'lf, R: RangeBounds<usize> + Send + 'lf, P: Promise<Output = Vec<T>> + 'lf>
    Debug for DisjointedRange<'lf, T, R, P>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisjointedRange")
            .field("start", &self.range.start_bound())
            .field("end", &self.range.end_bound())
            .finish()
    }
}

impl<'lf, T: Send, R: RangeBounds<usize> + Send + 'lf, P: Promise<Output = Vec<T>> + 'lf> Promise
    for DisjointedRange<'lf, T, R, P>
{
    type Output = Vec<T>;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self.inner.get() {
            None => PollPromise::Pending,
            Some(ready) => {
                let (start, end) = (
                    self.range.start_bound().cloned(),
                    self.range.end_bound().cloned(),
                );
                match ready[(start, end)]
                    .iter()
                    .try_fold(vec![], |mut accum, next| {
                        let t = next.lock().take()?;
                        accum.push(t);
                        Some(accum)
                    }) {
                    Some(vec) => PollPromise::Ready(vec),
                    None => panic!("trying to poll already executed promise"),
                }
            }
        }
    }
}

impl<R: RangeBounds<usize> + Send + 'static, P: Promise<Output = Vec<Data>> + 'static>
    DisjointedRange<'static, Data, R, P>
{
    pub fn downcast_elements<T: 'static>(self) -> CastedDisjointedRange<T, R, P> {
        CastedDisjointedRange {
            inner: self,
            _marker: PhantomData,
        }
    }
}

pub struct CastedDisjointedRange<
    T: 'static,
    R: RangeBounds<usize> + Send + 'static,
    P: Promise<Output = Vec<Data>> + 'static,
> {
    inner: DisjointedRange<'static, Data, R, P>,
    _marker: PhantomData<T>,
}

impl<
    T: 'static + Send,
    R: RangeBounds<usize> + Send + 'static,
    P: Promise<Output = Vec<Data>> + 'static,
> InputSource<CastedDisjointedRange<T, R, P>> for Input
{
    type Data = Vec<T>;

    fn use_as_input_source(
        &mut self,
        other: CastedDisjointedRange<T, R, P>,
    ) -> Result<(), JobError> {
        let inner = other
            .inner
            .map(|inner| {
                inner
                    .into_iter()
                    .map(|data| match data.downcast::<T>() {
                        Ok(data) => *data,
                        Err(e) => {
                            panic!("failed to downcast {:?} to `{}`", e, type_name::<T>())
                        }
                    })
                    .collect::<Vec<T>>()
            })
            .map(|vec| Box::new(vec) as Data);

        let promise: BoxPromise<'static, Data> = Box::new(inner);
        match (self.flavor, &mut self.kind) {
            (InputFlavor::None, _) => Err(JobError::UnexpectedInput),
            (InputFlavor::Single, to_set @ InputKind::None) => {
                *to_set = InputKind::Single(Box::new(promise));
                Ok(())
            }
            (InputFlavor::Single, InputKind::Single(_)) => Err(JobError::InputAlreadySet),
            (InputFlavor::Funnel, InputKind::Funnel(funnel)) => {
                funnel.insert(promise);
                Ok(())
            }
            _ => {
                panic!("input kind and flavor mismatch")
            }
        }
    }
}

type Buf<T> = Vec<Mutex<Option<T>>>;

struct DisjointedInner<'lf, T: Send + 'lf, P: Promise<Output = Vec<T>> + 'lf> {
    checker: Mutex<OverlapChecker<usize>>,
    promise: Arc<Mutex<P>>,
    inner: OnceLock<Buf<T>>,
    _marker: PhantomData<&'lf ()>,
}

impl<'lf, T: Send + 'lf, P: Promise<Output = Vec<T>> + 'lf> DisjointedInner<'lf, T, P> {
    //// Creates a new promise
    fn new(promise: P) -> Self {
        Self {
            checker: Default::default(),
            promise: Arc::new(Mutex::new(promise)),
            inner: OnceLock::new(),
            _marker: Default::default(),
        }
    }

    fn get(&self) -> Option<&Buf<T>> {
        let promise = self.promise.clone();
        let f = move || match promise.lock().poll() {
            PollPromise::Ready(ready) => Some(into_buf(ready)),
            PollPromise::Pending => None,
        };
        self.inner.get_or_try_init_opt(f)
    }
}

fn into_buf<T: Send>(v: Vec<T>) -> Buf<T> {
    v.into_iter().map(|t| Mutex::new(Some(t))).collect()
}

#[derive(Debug, Error)]
pub enum DisjointedError {
    #[error("{0} is already is use")]
    IndexAlreadyUsed(usize),
    #[error("({0:?}, {1:?}) is already in use")]
    RangeOverlaps(Bound<usize>, Bound<usize>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::disjointed::{Buf, Disjointed};
    use crate::backend::job::Data;
    use crate::backend::recv_promise::RecvPromise;
    use crossbeam::channel::bounded;
    use static_assertions::assert_impl_all;
    use std::sync::Arc;

    assert_impl_all!(Disjointed<'static, Data>: Send);
    assert_impl_all!(Buf<Data>: Sync);

    #[test]
    fn test_disjointed_inner() {
        let (tx, rx) = bounded::<Vec<usize>>(1);
        let receiver_promise = RecvPromise::new(rx);
        let inner = Arc::new(DisjointedInner::new(receiver_promise));

        assert!(inner.get().is_none());
        tx.send(vec![1, 2, 3]).unwrap();
        assert!(inner.get().is_some());
        let Some(buf) = inner.get() else {
            panic!(
                "Should be ready, and calling get multiple times shouldn't cause problems once promise is ready"
            )
        };
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn test_disjointed_indices() {
        let (tx, rx) = bounded::<Vec<usize>>(1);
        let receiver_promise = RecvPromise::new(rx);
        let inner = Disjointed::new(receiver_promise);

        let a = inner.get(0).unwrap();
        let b = inner.get(1).unwrap();
        inner.get(0).expect_err("should result in error");
        tx.send(vec![1, 2, 3]).unwrap();

        let a = a.get();
        let b = b.get();
        assert_eq!(a, 1);
        assert_eq!(b, 2);
    }

    #[test]
    fn test_disjointed_ranges() {
        let (tx, rx) = bounded::<Vec<usize>>(1);
        let receiver_promise = RecvPromise::new(rx);
        let inner = Disjointed::new(receiver_promise);

        let a = inner.get_range(0..3).unwrap();
        let b = inner.get_range(3..).unwrap();
        inner.get(0).expect_err("should result in error");
        inner.get_range(2..=4).expect_err("should result in error");
        tx.send(vec![1, 2, 3, 4, 5, 6]).unwrap();

        let a = a.get();
        let b = b.get();
        assert_eq!(a, [1, 2, 3]);
        assert_eq!(b, [4, 5, 6]);
    }

    #[test]
    fn test_disjointed_mixed() {
        let (tx, rx) = bounded::<Vec<usize>>(1);
        let receiver_promise = RecvPromise::new(rx);
        let inner = Disjointed::new(receiver_promise);

        let a = inner.get_range(0..3).unwrap();
        let b = inner.get(3).unwrap();
        let c = inner.get(4).unwrap();
        let d = inner.get_range(5..).unwrap();
        tx.send(vec![1, 2, 3, 4, 5, 6, 7]).unwrap();

        let a = a.get();
        let b = b.get();
        let c = c.get();
        let d = d.get();
        assert_eq!(a, [1, 2, 3]);
        assert_eq!(b, 4);
        assert_eq!(c, 5);
        assert_eq!(d, [6, 7]);
    }
}
