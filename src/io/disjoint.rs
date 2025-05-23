//! The [`Disjoint`] type.
//!
//! ```rust
//! use jobflow::{Flow, FlowsInto};
//! use jobflow::io::{Disjoint, DisjointableJob};
//! let mut flow = Flow::new();
//! let split = flow.create("split", || -> Vec<i32> { vec![0, 1, 2] }).disjointed();
//!
//! split.get(0).flows_into(flow.create("1", |i: i32| { assert_eq!(i, 0)})).unwrap();
//! split.get(1..=2).flows_into(flow.create("1", |i: Vec<i32>| { assert_eq!(i, [1, 2])})).unwrap();
//!
//! flow.run().expect("failed to run");
//!
//! ```

use crate::flow::private::JobRefWithBackend;
use crate::private::Sealed;
use std::marker::PhantomData;
use std::ops::RangeInclusive;
use std::sync::Arc;

/// Easily use multiple split outputs
pub struct Disjoint<T: Send> {
    inner: Arc<DisjointInner<T>>,
}

impl<T: Send> Disjoint<T> {}

impl<T: Send> FromIterator<T> for Disjoint<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        todo!()
    }
}

struct DisjointInner<T: Send> {
    _marker: PhantomData<T>,
}

/// Disjointed trait
pub trait DisjointableJob: Sized + Sealed {
    /// Converts this into a disjointed object
    fn disjointed(self) -> Disjointed<Self>;
}

pub struct Disjointed<T>(T);

impl<S> Disjointed<S> {
    pub fn get<U>(&self, index: U) -> U::Output
    where
        U: DisjointIndex<Self>,
    {
        index.get(self)
    }
}

impl<S> DisjointableJob for S
where
    S: JobRefWithBackend,
    S::Out: IntoIterator,
{
    fn disjointed(self) -> Disjointed<Self> {
        todo!()
    }
}

/// Helper trait for indexing into [`Disjointed`] types.
pub trait DisjointIndex<T>: Sealed
where
    T: ?Sized,
{
    type Output;

    /// Returns the index part
    fn get(self, t: &T) -> Self::Output;
}

impl<S: DisjointableJobRefWithBackend> DisjointIndex<Disjointed<S>> for usize {
    type Output = DisjointedElement<S::Element>;

    fn get(self, t: &Disjointed<S>) -> Self::Output {
        todo!()
    }
}

impl<S: DisjointableJobRefWithBackend> DisjointIndex<Disjointed<S>> for RangeInclusive<usize> {
    type Output = DisjointedElements<S::Element>;

    fn get(self, t: &Disjointed<S>) -> Self::Output {
        todo!()
    }
}


impl<S: JobRefWithBackend<Out = Vec<T>>, T: Send> DisjointableJobRefWithBackend for S {
    type Element = T;
}

#[derive(Debug)]
pub struct DisjointedElement<T: Send> {
    _marker: PhantomData<T>,
}

#[derive(Debug)]
pub struct DisjointedElements<T: Send> {
    _marker: PhantomData<T>,
}

// #[diagnostic::do_not_recommend]
impl<T, U, TO: JobOrderer> FlowsInto<U> for DisjointedElement<T>
where
    T: Send,
    U: JobRefWithBackend<In=T, TO = TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        // crate::flow::try_set_flow(&self, &other)?;
        Ok(other)
    }
}

// #[diagnostic::do_not_recommend]
impl<T, U, I, TO: JobOrderer> FlowsInto<U> for DisjointedElements<T>
where
    T: Send,
    I: FromIterator<T>,
    U: JobRefWithBackend<In=I, TO = TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        // crate::flow::try_set_flow(&self, &other)?;
        Ok(other)
    }
}

mod private {
    use crate::flow::private::JobRefWithBackend;

    pub trait DisjointableJobRefWithBackend: JobRefWithBackend {
        type Element: Send;
    }
}
use private::*;
use crate::{FlowError, FlowsInto};
use crate::job_ordering::JobOrderer;

#[cfg(test)]
mod tests {
    use crate::io::disjoint::DisjointableJobRefWithBackend;
    use crate::JobReference;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    assert_impl_all!(JobReference<(), Vec<usize>>: DisjointableJobRefWithBackend);
    assert_not_impl_any!(JobReference<(), usize>: DisjointableJobRefWithBackend);
}
