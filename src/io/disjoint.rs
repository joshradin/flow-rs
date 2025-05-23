//! The [`Disjointed`] type.
//!
//! ```rust
//! use jobflow::{Flow, FlowsInto};
//! use jobflow::actions::Supplier;
//! let mut flow = Flow::new();
//! let split = flow.create("split", || -> Vec<i32> { vec![0, 1, 2] }).disjointed().unwrap();
//!
//! split.gets(0).flows_into(flow.create("0", |i: i32| { assert_eq!(i, 0)})).unwrap();
//! split.gets(1..=2).flows_into(flow.create("1..=2", |i: Vec<i32>| { assert_eq!(i, [1, 2])})).unwrap();
//!
//! flow.run().expect("failed to run");
//!
//! ```

use crate::flow::private::JobRefWithBackend;
use crate::private::Sealed;
use std::collections::Bound;
use std::ops::{Range, RangeFrom, RangeFull, RangeInclusive, RangeTo, RangeToInclusive};



/// A disjointed task is a special type that allows it's outputs to be split
pub struct Disjointed<T>(pub(crate) T);

impl<S> Disjointed<S> {
    /// Gets the disjointed output of a task
    pub fn gets<U>(&self, index: U) -> U::Output<'_>
    where
        U: DisjointIndex<Self>,
    {
        index.get(self)
    }

}


impl<S> Funneled<Disjointed<S>> {
    /// Gets the disjointed output of a funnelled task
    pub fn get<U>(&self, index: U) -> U::Output<'_>
    where
        U: DisjointIndex<Disjointed<S>>,
    {
        index.get(&self.0)
    }

}


impl<I, O, TO: JobOrderer> Disjointed<JobReference<I, O, TO>> {
    /// Makes this step reusable if it hasn't already been used as input
    pub fn funnelled<T: Send + 'static>(self) -> Result<Funneled<Self>, FlowError>
    where
        I: FromIterator<T> + IntoIterator<Item = T, IntoIter: Send> + Send + 'static,
    {
        let id = *self.0.id();
        transaction_mut(&self.0.backend(), |backend| {
            let option = backend.get_mut(id).expect("backend task must exist");
            option.make_funnel::<T, I>()
        })?;

        Ok(Funneled(self))
    }
}

impl<S: Sealed> Sealed for Disjointed<S> {}
impl<S: JobRefWithBackend> JobRef for Disjointed<S> {
    fn id(&self) -> &JobId {
        self.0.id()
    }
}

impl<S: JobRefWithBackend> JobRefWithBackend for Disjointed<S> {
    type In = S::In;
    type Out = S::Out;
    type TO = S::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}



/// Helper trait for indexing into [`Disjointed`] types.
pub trait DisjointIndex<T>: Sealed
where
    T: ?Sized,
{
    type Output<'a>
    where
        T: 'a;

    /// Returns the index part
    fn get<'a>(self, t: &'a T) -> Self::Output<'a>;
}

impl<S: DisjointableJobRefWithBackend> DisjointIndex<Disjointed<S>> for usize {
    type Output<'a>
        = DisjointedElement<'a, S>
    where
        S: 'a;

    fn get(self, t: &Disjointed<S>) -> Self::Output<'_> {
        DisjointedElement {
            index: self,
            disjointed: t,
        }
    }
}

impl<S: DisjointableJobRefWithBackend> DisjointIndex<Funneled<Disjointed<S>>> for usize {
    type Output<'a>
    = DisjointedElement<'a, S>
    where
        S: 'a;

    fn get(self, t: &Funneled<Disjointed<S>>) -> Self::Output<'_> {
        DisjointedElement {
            index: self,
            disjointed: &t.0,
        }
    }
}


macro_rules! impl_disjoint_index_on_range {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<S: DisjointableJobRefWithBackend> DisjointIndex<Disjointed<S>> for $ty {
                type Output<'a> = DisjointedElements<'a, S> where S: 'a;

                fn get(self, t: &Disjointed<S>) -> Self::Output<'_> {
                    use std::ops::RangeBounds as _;
                    DisjointedElements {
                        range: (self.start_bound().cloned(), self.end_bound().cloned()),
                        disjointed: t,
                    }
                }
            }
        )*
    };
}

impl_disjoint_index_on_range!(
    Range<usize>,
    RangeInclusive<usize>,
    RangeFrom<usize>,
    RangeTo<usize>,
    RangeToInclusive<usize>,
    RangeFull,
);

impl<S: JobRefWithBackend<Out = Vec<T>>, T: Send> DisjointableJobRefWithBackend for S {
    type Element = T;
}

pub struct DisjointedElement<'a, T> {
    index: usize,
    disjointed: &'a Disjointed<T>,
}

pub struct DisjointedElements<'a, T> {
    range: (Bound<usize>, Bound<usize>),
    disjointed: &'a Disjointed<T>,
}

// #[diagnostic::do_not_recommend]
impl<T, U, TO: JobOrderer> FlowsInto<U> for DisjointedElement<'_, T>
where
    T: DisjointableJobRefWithBackend,
    U: JobRefWithBackend<In = T::Element, TO = TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        let weak = self.disjointed.backend();
        let this_id = *self.disjointed.id();
        let other_id = *other.id();
        transaction_mut(weak, move |backend| -> Result<(), FlowError> {
            let (this, other) = backend.get_mut2(this_id, other_id).unwrap();
            let OutputKind::Disjointed(d) = &mut this.output_mut().kind else {
                eprintln!("output kind: {:?}", this.output().flavor);
                return Err(FlowError::OutputNotDisjoint)
            };
            let result = d.get(self.index)?;
            other.input_mut().set_source(result)?;
            other.input_mut().depends_on(this.id());
            Ok(())
        })?;
        Ok(other)
    }
}

// #[diagnostic::do_not_recommend]
impl<T, U, I, TO: JobOrderer> FlowsInto<U> for DisjointedElements<'_, T>
where
    T: DisjointableJobRefWithBackend<Element: 'static>,
    I: FromIterator<T::Element>,
    U: JobRefWithBackend<In = I, TO = TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        let weak = self.disjointed.backend();
        let this_id = *self.disjointed.id();
        let other_id = *other.id();
        transaction_mut(weak, move |backend| -> Result<(), FlowError> {
            let (this, other) = backend.get_mut2(this_id, other_id).unwrap();
            let OutputKind::Disjointed(d) = &mut this.output_mut().kind else {
                eprintln!("output kind: {:?}", this.output().flavor);
                return Err(FlowError::OutputNotDisjoint)
            };
            let result = d.get_range(self.range)?
                .downcast_elements::<T::Element>();
            other.input_mut().set_source(result)?;
            other.input_mut().depends_on(this.id());
            Ok(())
        })?;
        Ok(other)
    }
}

mod private {
    use crate::flow::private::JobRefWithBackend;

    pub trait DisjointableJobRefWithBackend: JobRefWithBackend {
        type Element: Send;
    }
}
use crate::backend::job::OutputKind;
use crate::flow::{transaction_mut, WeakFlowBackend};
use crate::job_ordering::JobOrderer;
use crate::{FlowError, FlowsInto, Funneled, JobId, JobRef, JobReference};
use private::*;

#[cfg(test)]
mod tests {
    use crate::io::disjoint::DisjointableJobRefWithBackend;
    use crate::JobReference;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    assert_impl_all!(JobReference<(), Vec<usize>>: DisjointableJobRefWithBackend);
    assert_not_impl_any!(JobReference<(), usize>: DisjointableJobRefWithBackend);
}
