use crate::action::{Action, IntoAction};
use crate::backend::flow_backend::{FlowBackend, FlowBackendError};
use crate::backend::task::{
    BackendTask, Input, InputSource, Output, SingleOutput, TaskError, TaskId, TypedOutput,
};
use crate::flow::private::Sealed;
use crate::pool::{ThreadPool, WorkerPool};
use crate::task_ordering::{DefaultTaskOrderer, SteppedTaskOrderer, TaskOrderer};
use fortuples::fortuples;
use parking_lot::RwLock;
use static_assertions::{assert_impl_all, assert_not_impl_all};
use std::any::TypeId;
use std::marker::PhantomData;
use std::ops::Index;
use std::sync::{Arc, Weak};
use thiserror::Error;

type StrongFlowBackend<T = DefaultTaskOrderer> = Arc<RwLock<FlowBackend<T>>>;
type WeakFlowBackend<T = DefaultTaskOrderer> = Weak<RwLock<FlowBackend<T>>>;

/// Builds a [`Flow`]
pub struct FlowBuilder<I, O, T = DefaultTaskOrderer> {
    pool: Option<ThreadPool>,
    orderer: Option<T>,
    _marker: PhantomData<(I, O)>,
}

impl<I, O> FlowBuilder<I, O> {
    /// A flow builder
    pub fn new() -> FlowBuilder<I, O> {
        Self {
            pool: None,
            orderer: None,
            _marker: PhantomData,
        }
    }
}

impl<I, O, T> FlowBuilder<I, O, T> {
    pub fn with_task_orderer<U: TaskOrderer>(self, orderer: U) -> FlowBuilder<I, O, U> {
        FlowBuilder {
            pool: self.pool,
            orderer: Some(orderer),
            _marker: PhantomData,
        }
    }
}

impl<I: Send, O: Send, T: TaskOrderer + Default> FlowBuilder<I, O, T> {
    pub fn build(self) -> Flow<I, O, T> {
        let pool = self.pool.unwrap_or_default();
        let orderer = self.orderer.unwrap_or_default();
        Flow {
            _marker: Default::default(),
            backend: Arc::new(RwLock::new(FlowBackend::with_task_orderer_and_worker_pool(
                orderer, pool,
            ))),
        }
    }
}

impl FlowBuilder<(), (), ()> {}

/// Create a flow graph, with an input and an ultimate output
///
/// Flows are built from a set of tasks that can be run in a concurrent manner.
pub struct Flow<
    I: Send = (),
    O: Send = (),
    T: TaskOrderer = DefaultTaskOrderer,
> {
    _marker: PhantomData<fn(I) -> O>,
    backend: StrongFlowBackend<T>,
}

impl<I: Send, O: Send> Flow<I, O> {
    /// Creates a new flow
    pub fn new() -> Self {
        let pool: ThreadPool = Default::default();
        Self {
            _marker: PhantomData,
            backend: Arc::new(RwLock::new(FlowBackend::with_task_orderer_and_worker_pool(
                SteppedTaskOrderer::default(),
                pool,
            ))),
        }
    }
}

impl<I: Send, O: Send, T: TaskOrderer> Flow<I, O, T> {
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
    ) -> TaskReference<AI, AO, T>
    where
        AI: Send + Sync + 'static,
        AO: Send + Sync + 'static,
        A::Action: 'static,
    {
        let (input_flavor, action) = step.into_action();
        let bk = BackendTask::new::<_, AI, AO>(name, input_flavor, SingleOutput::new(), action);
        let s = TaskReference::<AI, AO, T>::new(&bk, &self.backend);
        self.backend.write().add(bk);
        s
    }

    /// Applies this flow with a given input
    pub fn apply(self, i: I) -> Result<O, FlowError> {
        let mut backend = self.backend.write();
        backend.execute()?;

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
pub struct TaskReference<I, O, T: TaskOrderer = DefaultTaskOrderer> {
    backend: WeakFlowBackend<T>,
    id: TaskId,
    name: String,
    input_ty: TypeId,
    output_ty: TypeId,
    _marker: PhantomData<fn(I) -> O>,
}

impl<I, O, TO: TaskOrderer> TaskReference<I, O, TO> {
    fn new(backend_task: &BackendTask, cl: &StrongFlowBackend<TO>) -> Self {
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

fn transaction_mut<T, U: TaskOrderer,  F: FnOnce(&mut FlowBackend<U>) -> T>(weak: &WeakFlowBackend<U>, f: F) -> T {
    let upgrade = weak.upgrade().expect("Weak instance lost");
    let mut backend = upgrade.write();
    f(&mut backend)
}

impl<I, O, TO: TaskOrderer> Sealed for TaskReference<I, O, TO> {}

#[derive(Debug, Error)]
pub enum FlowError {
    #[error(transparent)]
    TaskError(#[from] TaskError),
    #[error(transparent)]
    BackendFlowError(#[from] FlowBackendError),
    #[error("Tasks are not disjoint")]
    NondisjointedTasks,
}

/// Used for wrapping a step with a re-usable output.
pub struct Funneled<T>(T);

impl<T, R: Clone + Send + Sync + 'static, TO: TaskOrderer> Funneled<TaskReference<T, R, TO>> {
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

impl<I, T: Clone, TO: TaskOrderer> Clone for Reusable<TaskReference<I, T, TO>> {
    fn clone(&self) -> Self {
        Reusable(TaskReference {
            backend: self.0.backend.clone(),
            id: self.0.id,
            name: self.0.name.clone(),
            input_ty: self.0.input_ty,
            output_ty: self.0.output_ty,
            _marker: Default::default(),
        })
    }
}

impl<I, T: Clone, TO: TaskOrderer> Clone for Reusable<Funneled<TaskReference<I, T, TO>>> {
    fn clone(&self) -> Self {
        Reusable(Funneled(TaskReference {
            backend: self.0.0.backend.clone(),
            id: self.0.0.id,
            name: self.0.0.name.clone(),
            input_ty: self.0.0.input_ty,
            output_ty: self.0.0.output_ty,
            _marker: Default::default(),
        }))
    }
}

impl<I, T: Clone, TO: TaskOrderer> Reusable<TaskReference<I, T, TO>> {
    pub(crate) fn from_step_reference(step: TaskReference<I, T, TO>) -> Self {
        Self(step)
    }
}

impl<I, T: Clone, TO: TaskOrderer> Reusable<Funneled<TaskReference<I, T, TO>>> {
    pub(crate) fn from_funnelled(step: Funneled<TaskReference<I, T, TO>>) -> Self {
        Self(step)
    }
}

impl<I, T: Clone, TO: TaskOrderer> AsRef<Self> for Reusable<TaskReference<I, T, TO>> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<I, T: Clone, TO: TaskOrderer> AsMut<Self> for Reusable<TaskReference<I, T, TO>> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

impl<I, T: Clone, TO: TaskOrderer> AsRef<Self> for Reusable<Funneled<TaskReference<I, T, TO>>> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<I, T: Clone, TO: TaskOrderer> AsMut<Self> for Reusable<Funneled<TaskReference<I, T, TO>>> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

/// Allows for generalization of types that can flow their data into another
#[diagnostic::on_unimplemented(message = "`{Self}` can not flow into `{Other}`")]
pub trait FlowsInto<Other>: Sized {
    type Out;

    fn flows_into(self, other: Other) -> Self::Out;
}

fn try_set_flow<T, U, TO: TaskOrderer>(this: &T, other: &U) -> Result<(), FlowError>
where
    T: TaskRefWithBackend<TO=TO>,
    U: TaskRefWithBackend<TO=TO>,
{
    let weak = this.backend();
    let this_id = *this.id();
    let other_id = *other.id();
    transaction_mut(&weak, move |backend| -> Result<(), FlowError> {
        let (this, other) = backend.get_mut2(this_id, other_id).unwrap();
        other.input_mut().set_source(this.output_mut())?;
        Ok(())
    })?;
    Ok(())
}

fn try_set_flow_all<Tuple, T, U, const N: usize, const M: usize>(
    these: [&T; N],
    other: &U,
) -> Result<(), FlowError>
where
    Tuple: Send + 'static,
    T: TaskRefWithBackend,
    U: TaskRefWithBackend<TO=T::TO>,
    Input: for<'a> InputSource<(PhantomData<Tuple>, [&'a mut Output; N]), Data = Tuple>,
{
    assert_eq!(N + 1, M, "M must be N + 1");
    let weak = other.backend();
    let this_id = these.map(|this| *this.id());
    let other_id = *other.id();
    let all: [TaskId; M] = this_id
        .into_iter()
        .chain([other_id])
        .collect::<Vec<_>>()
        .try_into()
        .expect("should not fail");
    transaction_mut(&weak, move |backend| -> Result<(), FlowError> {
        let mut all = backend.get_mut_disjoint(all).unwrap();
        let (these, [other]) = all.split_first_chunk_mut::<N>().unwrap() else {
            panic!("Unexpected chunk size");
        };
        let task_outputs = these.each_mut().map(|t| t.output_mut());
        other
            .input_mut()
            .set_source((PhantomData::<Tuple>, task_outputs))?;
        Ok(())
    })?;
    Ok(())
}

impl<T, U, TO: TaskOrderer> FlowsInto<U> for T
where
    T: TaskRefWithBackend<TO=TO,Out = U::In>,
    U: TaskRefWithBackend<TO=TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        try_set_flow(&self, &other)?;
        Ok(other)
    }
}

impl<T, I, U, TO: TaskOrderer> FlowsInto<U> for Vec<T>
where
    T: TaskRefWithBackend<Out = I, TO=TO>,
    U: FunneledStepRef<In: FromIterator<I>, TO=TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        for item in self {
            try_set_flow(&item, &other)?;
        }
        Ok(other)
    }
}

fortuples! {
    #[tuples::min_size(1)]
    impl<O> FlowsInto<O> for #Tuple
        where
            #(#Member: TaskRefWithBackend<Out: Send + Sync + 'static>,)*
            O: TaskRefWithBackend<In=(#(#Member::Out,)*)>,
    {
        type Out = Result<O, FlowError>;

        fn flows_into(self, other: O) -> Self::Out {
            let backend = other.backend().clone();
            let these_ids = (
                #(*#self.id(),)*
            );
            let other_id = *other.id();
            transaction_mut(&backend, move |backend| -> Result<(), FlowError> {
                let [other_task, #(#Member,)*] = backend.get_mut_disjoint([other_id, #(#these_ids),*]).ok_or_else(|| {
                    FlowError::NondisjointedTasks
                })?;

                let outputs = (#(  TypedOutput::<#Member::Out>::new(#Member.output_mut())  ,)*);
                other_task.input_mut().set_source(outputs)?;
                Ok(())
            })?;

        //     try_set_flow_all::<
        //         (#(#Member::Out,)*),
        // AnyStepRef<(), ()>, _, #len(Tuple), { #len(Tuple) + 1 }>(these.each_ref(), &other)?;
            Ok(other)
        }
    }
}

/// A type that has an associated task id
pub trait TaskRef {
    /// Gets the id of this task ref
    fn id(&self) -> &TaskId;
}

trait TaskRefWithBackend: TaskRef {
    type In;
    type Out;
    type TO: TaskOrderer;

    fn backend(&self) -> &WeakFlowBackend<Self::TO>;
}

struct AnyStepRef<I, O, TO: TaskOrderer>(WeakFlowBackend<TO>, TaskId, PhantomData<(I, O)>);

impl<I, O, TO: TaskOrderer> AnyStepRef<I, O, TO> {
    fn erase_types(self) -> AnyStepRef<(), (), TO> {
        AnyStepRef(self.0, self.1, PhantomData)
    }
}

impl<I, O, TO: TaskOrderer> TaskRefWithBackend for AnyStepRef<I, O, TO> {
    type In = I;
    type Out = O;
    type TO = TO;


    fn backend(&self) -> &WeakFlowBackend<TO> {
        &self.0
    }
}

impl<I, O, TO: TaskOrderer> TaskRef for AnyStepRef<I, O, TO> {
    fn id(&self) -> &TaskId {
        &self.1
    }
}

fn to_any_step_ref<T: TaskRefWithBackend>(step_ref: &T) -> AnyStepRef<T::In, T::Out, T::TO> {
    AnyStepRef(step_ref.backend().clone(), *step_ref.id(), PhantomData)
}

impl<I, O, TO: TaskOrderer> TaskRefWithBackend for TaskReference<I, O, TO> {
    type In = I;
    type Out = O;
    type TO = TO;


    fn backend(&self) -> &WeakFlowBackend<TO> {
        &self.backend
    }
}

impl<I, O, TO: TaskOrderer> TaskRef for TaskReference<I, O, TO> {
    fn id(&self) -> &TaskId {
        &self.id
    }
}

impl<T: TaskRefWithBackend> TaskRefWithBackend for Reusable<T> {
    type In = <T as TaskRefWithBackend>::In;
    type Out = <T as TaskRefWithBackend>::Out;
    type TO = <T as TaskRefWithBackend>::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}

impl<T: TaskRefWithBackend> TaskRef for Reusable<T> {
    fn id(&self) -> &TaskId {
        self.0.id()
    }
}

impl<T: TaskRefWithBackend> TaskRefWithBackend for &Reusable<T> {
    type In = <T as TaskRefWithBackend>::In;
    type Out = <T as TaskRefWithBackend>::Out;
    type TO = <T as TaskRefWithBackend>::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}

impl<T: TaskRefWithBackend> TaskRef for &Reusable<T> {
    fn id(&self) -> &TaskId {
        self.0.id()
    }
}

impl<T: TaskRefWithBackend> TaskRefWithBackend for Funneled<T> {
    type In = <T as TaskRefWithBackend>::In;
    type Out = <T as TaskRefWithBackend>::Out;
    type TO = <T as TaskRefWithBackend>::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}

impl<T: TaskRefWithBackend> TaskRef for Funneled<T> {
    fn id(&self) -> &TaskId {
        self.0.id()
    }
}

trait FunneledStepRef: TaskRefWithBackend {}
impl<T: TaskRefWithBackend> FunneledStepRef for Funneled<T> {}
impl<T: FunneledStepRef> FunneledStepRef for Reusable<T> {}

mod private {
    pub trait Sealed {}
    impl<T: Sealed> Sealed for &T {}
    impl<T: Sealed> Sealed for &mut T {}
}

#[cfg(test)]
mod tests {
    use super::*;

    pub struct Move<T>(T);

    assert_impl_all!(TaskReference<(), Move<i32>>: FlowsInto<TaskReference<Move<i32>, ()>>);

    assert_impl_all!((TaskReference<(), Move<i32>>, TaskReference<(), Move<f32>>):
        FlowsInto<TaskReference<(Move<i32>, Move<f32>), ()>>);

    assert_impl_all!(TaskReference<(), i32>: FlowsInto<Reusable<TaskReference<i32, ()>>>);
    assert_impl_all!(TaskReference<(), i32>: FlowsInto<Funneled<TaskReference<i32, ()>>>);
    assert_impl_all!(TaskReference<(), i32>: FlowsInto<Reusable<Funneled<TaskReference<i32, ()>>>>);
    assert_impl_all!(TaskReference<(), i32>: FlowsInto<&'static Reusable<TaskReference<i32, ()>>>);

    assert_impl_all!(Reusable<TaskReference<(), i32>>: FlowsInto<&'static Reusable<TaskReference<i32, ()>>>);
    assert_impl_all!(Reusable<TaskReference<(), i32>>: FlowsInto<Reusable<TaskReference<i32, ()>>>);
    assert_impl_all!(Reusable<TaskReference<(), i32>>: FlowsInto<TaskReference<i32, ()>>);
    assert_impl_all!(&Reusable<TaskReference<(), i32>>: FlowsInto<&'static Reusable<TaskReference<i32, ()>>>);
    assert_impl_all!(&Reusable<TaskReference<(), i32>>: FlowsInto<Reusable<TaskReference<i32, ()>>>);
    assert_impl_all!(&Reusable<TaskReference<(), i32>>: FlowsInto<TaskReference<i32, ()>>);

    assert_not_impl_all!(TaskReference<(), i32>: FlowsInto<TaskReference<(), ()>>);
    assert_not_impl_all!(&TaskReference<(), i32>: FlowsInto<TaskReference<i32, ()>>);
    assert_not_impl_all!(&TaskReference<(), i32>: FlowsInto<&'static TaskReference<i32, ()>>);
    assert_not_impl_all!((TaskReference<(), Move<i32>>, TaskReference<(), Move<i32>>):
        FlowsInto<TaskReference<(Move<i32>, Move<f32>), ()>>);

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

        let mut t1 = flow.create("create_i32", || 12_i32).reusable().unwrap();
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
