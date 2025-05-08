use crate::action::Action;
use crate::flow::private::Sealed;
use std::any::TypeId;
use std::marker::PhantomData;
use std::num::NonZero;
use std::ops::Index;
use thiserror::Error;

/// Create a flow graph, with an input and an ultimate output
pub struct Flow<I: Send = (), O: Send = ()> {
    _marker: PhantomData<fn(I) -> O>,
}

impl<I: Send, O: Send> Flow<I, O> {
    /// Creates a new flow
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }

    /// Gets a representation of the input of a flow
    pub fn input(&self) -> FlowInput<I> {
        todo!()
    }

    /// Gets the representation of the output of a flow
    pub fn output(&self) -> FlowOutput<O> {
        todo!()
    }

    /// Adds a step to this flow
    pub fn add<A: Action + 'static>(
        &mut self,
        name: impl AsRef<str>,
        step: A,
    ) -> StepReference<A::Input, A::Output> {
        todo!()
    }

    /// Applies this flow with a given input
    pub fn apply(self, i: I) -> Result<O, FlowError> {
        todo!()
    }
}



impl<O: Send> Flow<(), O> {
    /// Gets the end result of this flow
    #[inline]
    pub fn get(self) -> Result<O, FlowError> {
        self.apply(())
    }
}

impl<I: Send> Flow<I, ()> {
    /// Runs this flow with the given input
    #[inline]
    pub fn accept(self, i: I) -> Result<(), FlowError> {
        self.apply(i)
    }
}

pub struct FlowInput<I> {
    flavor: DataFlavor<I>,
}

impl<I> FlowInput<Vec<I>> {
    /// Gets the nth index of this input
    pub fn nth(&self, i: usize) -> FlowInput<I> {
        todo!()
    }
}
impl<I> Sealed for FlowInput<I> {}
impl<I: Send> DataOutput<I> for FlowInput<I> {}

enum DataFlavor<T> {
    Single(SingleData<T>),
    Vec(VecData<T>),
}

struct SingleData<I> {
    _marker: PhantomData<I>,
}

struct VecData<T> {
    _marker: PhantomData<Vec<T>>,
}

pub struct FlowOutput<T> {
    marker: PhantomData<T>,
}

impl<T> Sealed for FlowOutput<T> {}
impl<T: Send, D: DataOutput<T>> FlowsFrom<D> for FlowOutput<T> {
    fn flows_from(&mut self, i: D) {
        todo!()
    }
}

/// Represents something that can be *used* as an input for a step
pub trait DataInput<I: Send> {
    #[doc(hidden)]
    fn id(&self) -> usize;

}
/// Represents the output of a step
pub trait DataOutput<O: Send> {}

impl<D: DataOutput<T>, T: Send> DataOutput<Vec<T>> for Vec<D> {}
impl<D1: DataOutput<T>, D2: DataOutput<U>, T: Send, U: Send> DataOutput<(T, U)> for (D1, D2) {}

/// A trait for a type that gets data from something
pub trait FlowsFrom<I> {
    fn flows_from(&mut self, i: I);
}

pub trait FlowsInto<O> {
    fn flows_into(&mut self, o: O);
}

impl<I: Send, O: Send, T: DataOutput<I>> FlowsFrom<T> for StepReference<I, O> {
    fn flows_from(&mut self, i: T) {
        todo!()
    }
}

// impl<I: Send, O: Send, T: DataOutput<O>> FlowsFrom<T> for StepReference<I, O> {
//     fn flows_from(&mut self, i: T) {
//         todo!()
//     }
// }

/// A reference to a step
pub struct StepReference<I: Send, O: Send> {
    id: NonZero<usize>,
    name: String,
    input_ty: TypeId,
    output_ty: TypeId,
    _marker: PhantomData<fn(I) -> O>,
}

impl<I: Send, O: Send> Clone for StepReference<I, O> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            input_ty: self.input_ty.clone(),
            output_ty: self.output_ty.clone(),
            _marker: PhantomData,
        }
    }
}

impl<I: Send, O: Send> Sealed for StepReference<I, O> {}
impl<I: Send, O: Send> DataOutput<O> for StepReference<I, O> {}

#[derive(Debug, Error)]
pub enum FlowError {}

mod private {
    pub trait Sealed {}
}
