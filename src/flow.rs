use crate::action::Action;
use crate::flow::private::Sealed;
use std::any::TypeId;
use std::marker::PhantomData;
use std::num::NonZero;
use std::ops::Index;
use thiserror::Error;
use crate::backend::flow_backend::FlowBackend;

/// Create a flow graph, with an input and an ultimate output
///
/// Flows are built from a set of tasks that can be run in a concurrent manner.
pub struct Flow<I: Send = (), O: Send = ()> {
    _marker: PhantomData<fn(I) -> O>,
    backend: FlowBackend
}

impl<I: Send, O: Send> Flow<I, O> {
    /// Creates a new flow
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
            backend: FlowBackend::new(),
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
    _marker: PhantomData<fn(I)>,
}

impl<I> FlowInput<Vec<I>> {
    /// Gets the nth index of this input
    pub fn nth(&self, i: usize) -> FlowInput<I> {
        todo!()
    }
}

pub struct FlowOutput<I> {
    _marker: PhantomData<fn(I)>,
}

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

#[derive(Debug, Error)]
pub enum FlowError {}

mod private {
    pub trait Sealed {}
}
