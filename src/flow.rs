use crate::action::{Action, IntoAction};
use crate::backend::flow_backend::FlowBackend;
use crate::backend::task::{
    BackendTask, InputFlavor, SingleOutput, TaskError, TaskId,
};
use crate::flow::private::Sealed;
use crate::pool::ThreadPool;
use parking_lot::RwLock;
use static_assertions::{assert_impl_all, assert_not_impl_all};
use std::any::TypeId;
use std::marker::PhantomData;
use std::ops::Index;
use std::sync::{Arc, Weak};
use thiserror::Error;

type StrongFlowBackend<P = ThreadPool> = Arc<RwLock<FlowBackend<P>>>;
type WeakFlowBackend<P = ThreadPool> = Weak<RwLock<FlowBackend<P>>>;

/// Create a flow graph, with an input and an ultimate output
///
/// Flows are built from a set of tasks that can be run in a concurrent manner.
pub struct Flow<I: Send = (), O: Send = ()> {
    _marker: PhantomData<fn(I) -> O>,
    backend: StrongFlowBackend,
}

impl<I: Send, O: Send> Flow<I, O> {
    /// Creates a new flow
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
            backend: Arc::new(RwLock::new(FlowBackend::new())),
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
        AI: Send + Sync + 'static,
        AO: Send + Sync + 'static,
        A::Action: 'static,
    {
        let action = step.into_action();
        let bk =
            BackendTask::new::<_, AI, AO>(name, InputFlavor::Single, SingleOutput::new(), action);
        let s = StepReference::<AI, AO>::new(&bk, &self.backend);
        self.backend.write().add(bk);
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
pub struct StepReference<I, O> {
    backend: WeakFlowBackend,
    id: TaskId,
    name: String,
    input_ty: TypeId,
    output_ty: TypeId,
    _marker: PhantomData<fn(I) -> O>,
}

impl<I, O> StepReference<I, O> {
    fn new(backend_task: &BackendTask, cl: &StrongFlowBackend) -> Self {
        Self {
            backend: Arc::downgrade(cl),
            id: backend_task.id(),
            name: backend_task.nickname().to_string(),
            input_ty: backend_task.input().input_ty(),
            output_ty: backend_task.output().output_ty(),
            _marker: Default::default(),
        }
    }

    /// Makes this step reusable if it hasn't already been used as input
    pub fn reusable(self) -> Result<Reusable<Self>, FlowError>
    where
        O: Clone + Sync + Send + 'static,
    {
        let id = self.id;
        transaction_mut(&self.backend, |backend| {
            let option = backend.get_mut(id).expect("backend task must exist");
            option.output_mut().make_reusable::<O>()
        })?;

        Ok(Reusable(Self {
            backend: self.backend.clone(),
            id: self.id,
            name: self.name.clone(),
            input_ty: self.input_ty,
            output_ty: self.output_ty,
            _marker: Default::default(),
        }))
    }

    /// Makes this step reusable if it hasn't already been used as input
    pub fn funnelled<T: Send + Sync + 'static>(self) -> Result<Funneled<Self>, FlowError>
    where
        I: FromIterator<T> + IntoIterator<Item = T, IntoIter: Send + Sync> + Send + Sync + 'static,
    {
        let id = self.id;
        transaction_mut(&self.backend, |backend| {
            let option = backend.get_mut(id).expect("backend task must exist");
            option.make_funnel::<T, I>()
        })?;

        Ok(Funneled(Self {
            backend: self.backend.clone(),
            id: self.id,
            name: self.name.clone(),
            input_ty: self.input_ty,
            output_ty: self.output_ty,
            _marker: Default::default(),
        }))
    }
}

fn transaction_mut<T, F: FnOnce(&mut FlowBackend) -> T>(weak: &WeakFlowBackend, f: F) -> T {
    let upgrade = weak.upgrade().expect("Weak instance lost");
    let mut backend = upgrade.write();
    f(&mut backend)
}

impl<I, O> Sealed for StepReference<I, O> {}

#[derive(Debug, Error)]
pub enum FlowError {
    #[error(transparent)]
    TaskError(#[from] TaskError),
}

/// Used for wrapping a step with a re-usable output.
pub struct Funneled<T>(T);

impl<T, R: Clone + Send + Sync + 'static> Funneled<StepReference<T, R>> {
    pub fn reusable(self) -> Result<Reusable<Self>, FlowError> {
        let id = self.0.id;
        transaction_mut(&self.0.backend, |backend| {
            let option = backend.get_mut(id).expect("backend task must exist");
            option.output_mut().make_reusable::<R>()
        })?;

        Ok(Reusable::from_funnelled(self))
    }
}

/// Used for wrapping a step with a re-usable output.
pub struct Reusable<T>(T);

impl<I, T: Clone> Clone for Reusable<StepReference<I, T>> {
    fn clone(&self) -> Self {
        Reusable(StepReference {
            backend: self.0.backend.clone(),
            id: self.0.id,
            name: self.0.name.clone(),
            input_ty: self.0.input_ty,
            output_ty: self.0.output_ty,
            _marker: Default::default(),
        })
    }
}

impl<I, T: Clone> Clone for Reusable<Funneled<StepReference<I, T>>> {
    fn clone(&self) -> Self {
        Reusable(Funneled(StepReference {
            backend: self.0.0.backend.clone(),
            id: self.0.0.id,
            name: self.0.0.name.clone(),
            input_ty: self.0.0.input_ty,
            output_ty: self.0.0.output_ty,
            _marker: Default::default(),
        }))
    }
}

impl<I, T: Clone> Reusable<StepReference<I, T>> {
    pub(crate) fn from_step_reference(step: StepReference<I, T>) -> Self {
        Self(step)
    }
}

impl<I, T: Clone> Reusable<Funneled<StepReference<I, T>>> {
    pub(crate) fn from_funnelled(step: Funneled<StepReference<I, T>>) -> Self {
        Self(step)
    }
}

impl<I, T: Clone> AsRef<Self> for Reusable<StepReference<I, T>> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<I, T: Clone> AsMut<Self> for Reusable<StepReference<I, T>> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

impl<I, T: Clone> AsRef<Self> for Reusable<Funneled<StepReference<I, T>>> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<I, T: Clone> AsMut<Self> for Reusable<Funneled<StepReference<I, T>>> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

#[diagnostic::on_unimplemented(message = "`{Self}` can not flow into `{Other}`")]
pub trait FlowsInto<Other>: Sized {
    type Out;

    fn flows_into(self, other: Other) -> Self::Out;
}
fn try_set_flow(
    this_id: TaskId,
    other_id: TaskId,
    weak: &WeakFlowBackend,
) -> Result<(), FlowError> {
    transaction_mut(&weak, move |backend| -> Result<(), FlowError> {
        let (this, other) = backend.get_mut_disjoint(this_id, other_id).unwrap();
        other.input_mut().set_source(this.output_mut())?;
        Ok(())
    })?;
    Ok(())
}

impl<I, T, O> FlowsInto<StepReference<T, O>> for StepReference<I, T> {
    type Out = Result<StepReference<T, O>, FlowError>;

    fn flows_into(self, other: StepReference<T, O>) -> Self::Out {
        let this_id = self.id;
        let other_id = other.id;

        let weak = self.backend;
        try_set_flow(this_id, other_id, &weak)?;
        Ok(other)
    }
}

impl<I, T, O: Clone> FlowsInto<Reusable<StepReference<T, O>>> for StepReference<I, T> {
    type Out = Result<Reusable<StepReference<T, O>>, FlowError>;

    fn flows_into(self, Reusable(other): Reusable<StepReference<T, O>>) -> Self::Out {
        let this_id = self.id;
        let other_id = other.id;

        let weak = self.backend;
        try_set_flow(this_id, other_id, &weak)?;
        Ok(Reusable(other))
    }
}

impl<'a, I, T, O: Clone> FlowsInto<&'a Reusable<StepReference<T, O>>> for StepReference<I, T> {
    type Out = Result<&'a Reusable<StepReference<T, O>>, FlowError>;

    fn flows_into(self, other: &'a Reusable<StepReference<T, O>>) -> Self::Out {
        let this_id = self.id;
        let other_id = other.0.id;

        let weak = self.backend;
        try_set_flow(this_id, other_id, &weak)?;
        Ok(other)
    }
}

impl<I, T: Clone, O: Clone> FlowsInto<Reusable<StepReference<T, O>>>
    for Reusable<StepReference<I, T>>
{
    type Out = Result<Reusable<StepReference<T, O>>, FlowError>;

    fn flows_into(self, other: Reusable<StepReference<T, O>>) -> Self::Out {
        let this_id = self.0.id;
        let other_id = other.0.id;

        let weak = self.0.backend;
        try_set_flow(this_id, other_id, &weak)?;
        Ok(other)
    }
}

impl<I, T: Clone, O> FlowsInto<StepReference<T, O>> for Reusable<StepReference<I, T>> {
    type Out = Result<StepReference<T, O>, FlowError>;

    fn flows_into(self, other: StepReference<T, O>) -> Self::Out {
        let this_id = self.0.id;
        let other_id = other.id;

        let weak = self.0.backend;
        try_set_flow(this_id, other_id, &weak)?;
        Ok(other)
    }
}

impl<'a, I, T: Clone, O: Clone> FlowsInto<&'a Reusable<StepReference<T, O>>>
    for Reusable<StepReference<I, T>>
{
    type Out = Result<&'a Reusable<StepReference<T, O>>, FlowError>;

    fn flows_into(self, other: &'a Reusable<StepReference<T, O>>) -> Self::Out {
        let this_id = self.0.id;
        let other_id = other.0.id;

        let weak = self.0.backend;
        try_set_flow(this_id, other_id, &weak)?;
        Ok(other)
    }
}

impl<'b, I, T: Clone, O: Clone> FlowsInto<Reusable<StepReference<T, O>>>
    for &'b Reusable<StepReference<I, T>>
{
    type Out = Result<Reusable<StepReference<T, O>>, FlowError>;

    fn flows_into(self, other: Reusable<StepReference<T, O>>) -> Self::Out {
        let this_id = self.0.id;
        let other_id = other.0.id;

        let weak = &self.0.backend;
        try_set_flow(this_id, other_id, weak)?;
        Ok(other)
    }
}

impl<'b, I, T, O> FlowsInto<StepReference<T, O>> for &'b Reusable<StepReference<I, T>> {
    type Out = Result<StepReference<T, O>, FlowError>;

    fn flows_into(self, other: StepReference<T, O>) -> Self::Out {
        let this_id = self.0.id;
        let other_id = other.id;

        let weak = &self.0.backend;
        try_set_flow(this_id, other_id, weak)?;
        Ok(other)
    }
}

impl<'a, 'b, I, T, O> FlowsInto<&'a Reusable<StepReference<T, O>>>
    for &'b Reusable<StepReference<I, T>>
{
    type Out = Result<&'a Reusable<StepReference<T, O>>, FlowError>;

    fn flows_into(self, other: &'a Reusable<StepReference<T, O>>) -> Self::Out {
        let this_id = self.0.id;
        let other_id = other.0.id;

        let weak = &self.0.backend;
        try_set_flow(this_id, other_id, weak)?;
        Ok(other)
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

    pub struct Move<T>(T);

    assert_impl_all!(StepReference<(), Move<i32>>: FlowsInto<StepReference<Move<i32>, ()>>);
    assert_impl_all!(StepReference<(), i32>: FlowsInto<Reusable<StepReference<i32, ()>>>);
    assert_impl_all!(StepReference<(), i32>: FlowsInto<&'static Reusable<StepReference<i32, ()>>>);
    assert_impl_all!(Reusable<StepReference<(), i32>>: FlowsInto<&'static Reusable<StepReference<i32, ()>>>);
    assert_impl_all!(Reusable<StepReference<(), i32>>: FlowsInto<Reusable<StepReference<i32, ()>>>);
    assert_impl_all!(Reusable<StepReference<(), i32>>: FlowsInto<StepReference<i32, ()>>);
    assert_impl_all!(&Reusable<StepReference<(), i32>>: FlowsInto<&'static Reusable<StepReference<i32, ()>>>);
    assert_impl_all!(&Reusable<StepReference<(), i32>>: FlowsInto<Reusable<StepReference<i32, ()>>>);
    assert_impl_all!(&Reusable<StepReference<(), i32>>: FlowsInto<StepReference<i32, ()>>);
    assert_not_impl_all!(StepReference<(), i32>: FlowsInto<StepReference<(), ()>>);

    #[test]
    fn test_type_checking_non_clone() {
        let mut flow: Flow = Flow::new();

        let t1 = flow.create("create_i32", || Move(12_i32));
        let mut t2 = flow.create("consume_i32", |Move(i): Move<i32>| {});

        t1.flows_into(t2);
    }

    #[test]
    fn test_type_checking_cloneable() {
        let mut flow: Flow = Flow::new();

        let mut t1 = flow
            .create("create_i32", || 12_i32)
            .reusable()
            .unwrap();
        let t2 = flow.create("consume_i32", |i: i32| {});
        let t3 = flow.create("consume_i32", |i: i32| {});

        t1.as_ref().flows_into(t2).expect("Should be Ok");
        t1.as_ref().flows_into(t3).expect("Should be Ok");
    }

    #[test]
    fn test_type_checking_funnelable() {
        let mut flow: Flow = Flow::new();

        let mut t1 = flow
            .create("create_i32", |i: Vec<i32>| 12_i32)
            .funnelled()
            .expect("Should be Ok")
            .reusable()
            .unwrap();
        let t2 = flow.create("consume_i32", |i: i32| {});
        let t3 = flow.create("consume_i32", |i: i32| {});

        // t1.as_ref().flows_into(t2).expect("Should be Ok");
        // t1.as_ref().flows_into(t3).expect("Should be Ok");
    }
}
