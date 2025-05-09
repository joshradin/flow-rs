use crate::action::{Action, IntoAction};
use crate::backend::flow_backend::FlowBackend;
use crate::backend::task;
use crate::backend::task::{
    AsOutputFlavor, BackendTask, InputFlavor, ReusableOutput, SingleOutput, TaskId,
};
use crate::flow::private::Sealed;
use std::any::TypeId;
use std::marker::PhantomData;
use std::ops::Index;
use thiserror::Error;

/// Create a flow graph, with an input and an ultimate output
///
/// Flows are built from a set of tasks that can be run in a concurrent manner.
pub struct Flow<I: Send = (), O: Send = ()> {
    _marker: PhantomData<fn(I) -> O>,
    backend: FlowBackend,
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
    pub fn create<AI, AO, M, A: IntoAction<AI, AO, M> + 'static>(
        &mut self,
        name: impl AsRef<str>,
        step: A,
    ) -> StepReference<AI, AO>
    where
        AI: Input + 'static,
        AO: Output + 'static,
        A::Action: 'static,
    {
        let action = step.into_action();
        let bk = BackendTask::new::<_, AI, AO>(name, AI::flavor(), AO::flavor(), action);
        let s = StepReference::<AI, AO>::new(&bk);
        self.backend.add(bk);
        s
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
pub struct StepReference<I: Input, O> {
    id: TaskId,
    name: String,
    input_ty: TypeId,
    output_ty: TypeId,
    _marker: PhantomData<fn(I) -> O>,
}

impl<I: Input, O> StepReference<I, O> {
    fn new(backend_task: &BackendTask) -> Self {
        Self {
            id: backend_task.id(),
            name: backend_task.nickname().to_string(),
            input_ty: backend_task.input().input_ty(),
            output_ty: backend_task.output().output_ty(),
            _marker: Default::default(),
        }
    }
}

impl<I: Input, O> Clone for StepReference<I, O> {
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

/// Used for representing no output or no input
pub enum Void {}

pub trait Input: Send + Sealed {
    type Data: Send;
    fn flavor() -> InputFlavor;
}

impl Sealed for () {}
impl Input for () {
    type Data = Void;

    fn flavor() -> InputFlavor {
        InputFlavor::None
    }
}

/// An input for an action
pub struct In<T: Send>(pub T);
impl<T: Send> Sealed for In<T> {}
impl<T: Send> Input for In<T> {
    type Data = T;
    fn flavor() -> InputFlavor {
        InputFlavor::Single
    }
}

pub trait Output: Send + Sealed {
    fn flavor() -> impl AsOutputFlavor<Data = Self>;
}

#[derive(Clone)]
pub struct Out<T: Send + Clone>(pub T);

impl<T: Send + Clone + 'static> Sealed for Out<T> {}
impl<T: Send + Clone + 'static> Output for Out<T> {
    fn flavor() -> impl AsOutputFlavor<Data = Self> {
        ReusableOutput::<Out<T>>::new()
    }
}
/// An output that can only be used once
#[derive(Debug)]
pub struct Once<T: Send + 'static>(pub T);

impl<T: Send + 'static> Sealed for Once<T> {}
impl<T: Send + 'static> Output for Once<T> {
    fn flavor() -> impl AsOutputFlavor<Data = Self> {
        SingleOutput::<Once<T>>::new()
    }
}

impl Output for () {
    fn flavor() -> impl AsOutputFlavor<Data = Self> {
        task::NoOutput
    }
}

impl<I: Input, O> Sealed for StepReference<I, O> {}

#[derive(Debug, Error)]
pub enum FlowError {}


#[diagnostic::on_unimplemented(
    message = "`{Self}` can not flow into `{Other}`"
)]
pub trait FlowsInto<Other>: Sealed + Sized {
    fn flows_into(self, other: Other);
}

pub trait FlowFrom<Other>: Sealed + Sized {
    fn flows_from(self, other: Other) ;
}

impl<T: Sealed, R: Sealed> FlowFrom<R> for T
    where R: FlowsInto<T>
{
    fn flows_from(self, other: R) {
        other.flows_into(self)
    }
}

impl<T1: Input, R1: Send + Clone, T2: Input, R2: Output> FlowsInto<StepReference<T2, R2>>
    for StepReference<T1, Out<R1>>
{
    fn flows_into(self, other: StepReference<T2, R2>) {
        todo!()
    }
}


impl<T1: Input, R1: Send + Clone, T2: Input, R2: Output> FlowsInto<StepReference<T2, R2>>
    for StepReference<T1, Once<R1>>
{
    fn flows_into(self, other: StepReference<T2, R2>) {
        todo!()
    }
}

mod private {
    pub trait Sealed {}
    impl<T: Sealed> Sealed for &T {}
    impl<T: Sealed> Sealed for &mut T {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_checking() {
        let mut flow: Flow = Flow::new();

        let t1 = flow.create("create_i32", || Out(12_i32));
        let mut t2 = flow.create("consume_i32", |In(i): In<i32>| {});

        // t1.flows_into(&mut t2);
        // t1.flows_into(t2);
        t2.flows_from(t1);
    }
}
