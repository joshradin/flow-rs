use crate::action::IntoAction;
use crate::backend::flow_backend::{FlowBackend, FlowBackendError};
use crate::backend::flow_listener_shim::FlowListenerShim;
use crate::backend::job::{BackendJob, JobId, SingleOutput, JobError, TypedOutput};
use crate::job_ordering::{format_cycle, FlowGraph, JobOrderingError};
use crate::job_ordering::{DefaultTaskOrderer, JobOrderer, SteppedTaskOrderer};
use crate::listener::FlowListener;
use crate::pool::FlowThreadPool;
use crate::promise::{IntoPromise, PromiseExt};
use fortuples::fortuples;
use parking_lot::{Mutex, RwLock};
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use thiserror::Error;

type StrongFlowBackend<T = DefaultTaskOrderer> = Rc<RwLock<FlowBackend<T>>>;
type WeakFlowBackend<T = DefaultTaskOrderer> = Weak<RwLock<FlowBackend<T>>>;

/// Builds a [`Flow`]
pub struct FlowBuilder<I, O, T = DefaultTaskOrderer> {
    pool: Option<FlowThreadPool>,
    orderer: Option<T>,
    _marker: PhantomData<(I, O)>,
}

impl<I, O> Default for FlowBuilder<I, O> {
    fn default() -> Self {
        Self::new()
    }
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
    pub fn with_task_orderer<U: JobOrderer>(self, orderer: U) -> FlowBuilder<I, O, U> {
        FlowBuilder {
            pool: self.pool,
            orderer: Some(orderer),
            _marker: PhantomData,
        }
    }

    /// Sets the flow thread pool
    pub fn with_thread_pool(mut self, pool: FlowThreadPool) -> Self {
        self.pool = Some(pool);
        self
    }
}

impl<I: Send, O: Send, T: JobOrderer + Default> FlowBuilder<I, O, T> {
    pub fn build(self) -> Flow<I, O, T> {
        let pool = self.pool.unwrap_or_default();
        let orderer = self.orderer.unwrap_or_default();
        Flow {
            _marker: Default::default(),
            nicknames: Default::default(),
            listeners: vec![],
            backend: Rc::new(RwLock::new(FlowBackend::with_task_orderer_and_worker_pool(
                orderer, pool,
            ))),
        }
    }
}

impl FlowBuilder<(), (), ()> {}

/// Create a flow graph, with an input and an ultimate output
///
/// Flows are built from a set of tasks that can be run in a concurrent manner.
pub struct Flow<I: Send = (), O: Send = (), T: JobOrderer = DefaultTaskOrderer> {
    _marker: PhantomData<fn(I) -> O>,
    nicknames: HashMap<String, JobId>,
    listeners: Vec<Box<dyn FlowListener + Send + Sync>>,
    backend: StrongFlowBackend<T>,
}

impl<I: Send, O: Send> Default for Flow<I, O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Send, O: Send> Flow<I, O> {
    /// Creates a flow builder
    pub fn builder() -> FlowBuilder<I, O> {
        FlowBuilder::new()
    }

    /// Creates a new flow
    pub fn new() -> Self {
        let pool: FlowThreadPool = Default::default();
        Self {
            _marker: PhantomData,
            nicknames: Default::default(),
            listeners: vec![],
            backend: Rc::new(RwLock::new(FlowBackend::with_task_orderer_and_worker_pool(
                SteppedTaskOrderer,
                pool,
            ))),
        }
    }
}

impl<I: Send + Sync + 'static, O: Send + Sync + 'static, T: JobOrderer> Flow<I, O, T> {
    /// Gets a representation of the input of a flow
    pub fn input(&self) -> FlowInput<I, T> {
        FlowInput::new(&self.backend)
    }

    /// Gets the representation of the output of a flow
    pub fn output(&self) -> FlowOutput<O, T> {
        FlowOutput::new(&self.backend)
    }

    /// Sets a depends on relationship
    pub fn depends_on(&mut self, t: JobId, u: JobId) -> Result<(), FlowError> {
        let mut locked = self.backend.write();
        let t = locked
            .get_mut(t)
            .ok_or_else(|| FlowError::TaskNotFound(t))?;
        t.input_mut().depends_on(u);
        Ok(())
    }

    /// Sets a depends on relationship
    pub fn depends_on_all<It: IntoIterator<Item = JobId>>(
        &mut self,
        t: JobId,
        iter: It,
    ) -> Result<(), FlowError> {
        let mut locked = self.backend.write();
        let t = locked
            .get_mut(t)
            .ok_or_else(|| FlowError::TaskNotFound(t))?;
        for u in iter {
            t.input_mut().depends_on(u);
        }
        Ok(())
    }

    /// Adds a step to this flow
    pub fn create<AI, AO, M, A: IntoAction<AI, AO, M> + 'static>(
        &mut self,
        name: impl AsRef<str>,
        step: A,
    ) -> JobReference<AI, AO, T>
    where
        AI: Send + Sync + 'static,
        AO: Send + Sync + 'static,
        A::Action: 'static,
    {
        let (input_flavor, action) = step.into_action();
        let bk = BackendJob::new::<_, AI, AO>(name, input_flavor, SingleOutput::new(), action);
        self.nicknames.insert(bk.nickname().to_string(), bk.id());
        let s = JobReference::<AI, AO, T>::new(&bk, &self.backend);
        self.backend.write().add(bk);
        s
    }

    /// Creates a flow graph at the given moment for this [`Flow`]
    pub fn flow_graph(&self) -> Result<impl FlowGraph, FlowError> {
        let (_to, fg) = self.backend.read().ordering()?;
        Ok(fg)
    }

    /// Attempts to find a task with the given name
    pub fn find(&self, name: impl AsRef<str>) -> Option<JobId> {
        self.nicknames.get(name.as_ref()).copied()
    }

    /// Add a [`FlowListener`] to this flow
    pub fn add_listener<L>(&mut self, listener: L)
    where
        L: FlowListener + Send + Sync + 'static,
    {
        self.listeners.push(Box::new(listener));
    }

    fn apply_(self, i: I, expect_result: bool) -> Result<Option<O>, FlowError> {
        let mut backend = self.backend.write();
        let listeners = Arc::new(Mutex::new(self.listeners));
        let shim = FlowListenerShim::new(&listeners);
        backend.add_listener(shim);
        backend.input_mut().send(Box::new(i))?;
        let result = backend.execute().map_err(|e| match e {
            FlowBackendError::TaskOrdering(JobOrderingError::CyclicTasks { cycle }) => {
                let cycled = cycle
                    .into_iter()
                    .map(|id| {
                        let id = backend.get(id).unwrap();
                        id.nickname().to_string()
                    })
                    .collect();
                FlowError::CycleError { members: cycled }
            }
            other => FlowError::from(other),
        });
        listeners
            .lock()
            .iter_mut()
            .for_each(|l| l.finished(result.as_ref().map(|_| ())));
        result?;

        if expect_result {
            if !backend.output().has_source() {
                return Err(FlowError::NoOutputTask);
            }

            let promise = backend.output_mut().into_promise();
            match promise.try_get() {
                Ok(ok) => {
                    let o: O = *ok.downcast().expect("Failed to downcast result");
                    Ok(Some(o))
                }
                Err(_) => Err(FlowError::OutputExpected),
            }
        } else {
            Ok(None)
        }
    }

    /// Applies this flow with a given input
    pub fn apply(self, i: I) -> Result<O, FlowError> {
        self.apply_(i, true)?
            .ok_or_else(|| FlowError::OutputExpected)
    }
}

impl<O: Send + Sync + 'static, TO: JobOrderer> Flow<(), O, TO> {
    /// Gets the end result of this flow
    #[inline]
    pub fn get(self) -> Result<O, FlowError> {
        self.apply(())
    }
}

impl<I: Send + Sync + 'static, TO: JobOrderer> Flow<I, (), TO> {
    /// Runs this flow with the given input
    #[inline]
    pub fn accept(self, i: I) -> Result<(), FlowError> {
        self.apply_(i, false)?;
        Ok(())
    }
}

impl<TO: JobOrderer> Flow<(), (), TO> {
    /// Runs this flow with the given input
    #[inline]
    pub fn run(self) -> Result<(), FlowError> {
        self.apply_((), false)?;
        Ok(())
    }
}

/// Represents the input of a flow
pub struct FlowInput<I, T: JobOrderer = DefaultTaskOrderer> {
    backend: WeakFlowBackend<T>,
    _marker: PhantomData<fn(I)>,
}

impl<I, T: JobOrderer> FlowInput<I, T> {
    fn new(b: &StrongFlowBackend<T>) -> Self {
        Self {
            backend: Rc::downgrade(b),
            _marker: PhantomData,
        }
    }
}

impl<I> FlowInput<Vec<I>> {}

/// Represents the output of a flow
pub struct FlowOutput<I, T: JobOrderer = DefaultTaskOrderer> {
    _backend: WeakFlowBackend<T>,
    _marker: PhantomData<fn(I)>,
}

impl<I, T: JobOrderer> FlowOutput<I, T> {
    fn new(backend: &StrongFlowBackend<T>) -> Self {
        Self {
            _backend: Rc::downgrade(backend),
            _marker: PhantomData,
        }
    }
}

/// A reference to a job
pub struct JobReference<I, O, T: JobOrderer = DefaultTaskOrderer> {
    backend: WeakFlowBackend<T>,
    id: JobId,
    name: String,
    input_ty: TypeId,
    output_ty: TypeId,
    _marker: PhantomData<fn(I) -> O>,
}

impl<I, O, TO: JobOrderer> JobReference<I, O, TO> {
    fn new(backend_task: &BackendJob, cl: &StrongFlowBackend<TO>) -> Self {
        Self {
            backend: Rc::downgrade(cl),
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

fn transaction_mut<T, U: JobOrderer, F: FnOnce(&mut FlowBackend<U>) -> T>(
    weak: &WeakFlowBackend<U>,
    f: F,
) -> T {
    let upgrade = weak.upgrade().expect("Weak instance lost");
    let mut backend = upgrade.write();
    f(&mut backend)
}

/// An error occurred with this flow
#[derive(Debug, Error)]
pub enum FlowError {
    #[error("{}", format_cycle(.members))]
    CycleError { members: Vec<String> },
    #[error(transparent)]
    TaskError(#[from] JobError),
    #[error(transparent)]
    BackendFlowError(#[from] FlowBackendError),
    #[error("Tasks are not disjoint")]
    NondisjointedTasks,
    #[error("An output was expected for this flow, but none was found")]
    OutputExpected,
    #[error("No task is being used as an output")]
    NoOutputTask,
    #[error("Task with id {} was not found", .0)]
    TaskNotFound(JobId),
}

/// Used for wrapping a step with a re-usable output.
pub struct Funneled<T>(T);

impl<T, R: Clone + Send + Sync + 'static, TO: JobOrderer> Funneled<JobReference<T, R, TO>> {
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

impl<I, T: Clone, TO: JobOrderer> Clone for Reusable<JobReference<I, T, TO>> {
    fn clone(&self) -> Self {
        Reusable(JobReference {
            backend: self.0.backend.clone(),
            id: self.0.id,
            name: self.0.name.clone(),
            input_ty: self.0.input_ty,
            output_ty: self.0.output_ty,
            _marker: Default::default(),
        })
    }
}

impl<I, T: Clone, TO: JobOrderer> Clone for Reusable<Funneled<JobReference<I, T, TO>>> {
    fn clone(&self) -> Self {
        Reusable(Funneled(JobReference {
            backend: self.0.0.backend.clone(),
            id: self.0.0.id,
            name: self.0.0.name.clone(),
            input_ty: self.0.0.input_ty,
            output_ty: self.0.0.output_ty,
            _marker: Default::default(),
        }))
    }
}

impl<I, T: Clone, TO: JobOrderer> Reusable<Funneled<JobReference<I, T, TO>>> {
    pub(crate) fn from_funnelled(step: Funneled<JobReference<I, T, TO>>) -> Self {
        Self(step)
    }
}

impl<I, T: Clone, TO: JobOrderer> AsRef<Self> for Reusable<JobReference<I, T, TO>> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<I, T: Clone, TO: JobOrderer> AsMut<Self> for Reusable<JobReference<I, T, TO>> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

impl<I, T: Clone, TO: JobOrderer> AsRef<Self> for Reusable<Funneled<JobReference<I, T, TO>>> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<I, T: Clone, TO: JobOrderer> AsMut<Self> for Reusable<Funneled<JobReference<I, T, TO>>> {
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

fn try_set_flow<T, U, TO: JobOrderer>(this: &T, other: &U) -> Result<(), FlowError>
where
    T: JobRefWithBackend<TO = TO>,
    U: JobRefWithBackend<TO = TO>,
{
    let weak = this.backend();
    let this_id = *this.id();
    let other_id = *other.id();
    transaction_mut(weak, move |backend| -> Result<(), FlowError> {
        let (this, other) = backend.get_mut2(this_id, other_id).unwrap();
        other.input_mut().set_source(this.output_mut())?;
        Ok(())
    })?;
    Ok(())
}

#[diagnostic::do_not_recommend]
impl<T, U, TO: JobOrderer> FlowsInto<U> for T
where
    T: JobRefWithBackend<TO = TO, Out = U::In>,
    U: JobRefWithBackend<TO = TO>,
{
    type Out = Result<U, FlowError>;

    fn flows_into(self, other: U) -> Self::Out {
        try_set_flow(&self, &other)?;
        Ok(other)
    }
}
#[diagnostic::do_not_recommend]
impl<T, I, U, TO: JobOrderer> FlowsInto<U> for Vec<T>
where
    T: JobRefWithBackend<Out = I, TO = TO>,
    U: FunneledJobRef<In: FromIterator<I>, TO = TO>,
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
    #[diagnostic::do_not_recommend]
    #[tuples::min_size(1)]
    impl<O> FlowsInto<O> for #Tuple
        where
            #(#Member: JobRefWithBackend<Out: Send + Sync + 'static>,)*
            O: JobRefWithBackend<In=(#(#Member::Out,)*)>,
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

/// Sets a depends on relation
pub trait DependsOn<U> {
    fn depends_on(&mut self, u: U) -> Result<(), FlowError>;
}

impl<T, U> DependsOn<&U> for T
where
    T: JobRefWithBackend,
    U: JobRefWithBackend<TO = T::TO>,
{
    fn depends_on(&mut self, u: &U) -> Result<(), FlowError> {
        let t_id = *self.id();
        let u_id = *u.id();
        transaction_mut(self.backend(), move |backend| -> Result<(), FlowError> {
            let (t, u) = backend
                .get_mut2(t_id, u_id)
                .ok_or_else(|| FlowError::NondisjointedTasks)?;
            t.input_mut().depends_on(u.id());
            Ok(())
        })
    }
}

impl<T: JobRefWithBackend> DependsOn<JobId> for T {
    fn depends_on(&mut self, u: JobId) -> Result<(), FlowError> {
        let t_id = *self.id();
        transaction_mut(self.backend(), move |backend| -> Result<(), FlowError> {
            let t = backend
                .get_mut(t_id)
                .ok_or_else(|| FlowError::TaskNotFound(t_id))?;
            t.input_mut().depends_on(u);
            Ok(())
        })
    }
}

/// A type that has an associated job id
pub trait JobRef {
    /// Gets the id of this job ref
    fn id(&self) -> &JobId;
}

struct AnyStepRef<I, O, TO: JobOrderer>(WeakFlowBackend<TO>, JobId, PhantomData<(I, O)>);

impl<I, O, TO: JobOrderer> JobRefWithBackend for AnyStepRef<I, O, TO> {
    type In = I;
    type Out = O;
    type TO = TO;

    fn backend(&self) -> &WeakFlowBackend<TO> {
        &self.0
    }
}

impl<I, O, TO: JobOrderer> JobRef for AnyStepRef<I, O, TO> {
    fn id(&self) -> &JobId {
        &self.1
    }
}

impl<I, O, TO: JobOrderer> JobRefWithBackend for JobReference<I, O, TO> {
    type In = I;
    type Out = O;
    type TO = TO;

    fn backend(&self) -> &WeakFlowBackend<TO> {
        &self.backend
    }
}

impl<I, O, TO: JobOrderer> JobRef for JobReference<I, O, TO> {
    fn id(&self) -> &JobId {
        &self.id
    }
}

impl<T: JobRefWithBackend> JobRefWithBackend for Reusable<T> {
    type In = <T as JobRefWithBackend>::In;
    type Out = <T as JobRefWithBackend>::Out;
    type TO = <T as JobRefWithBackend>::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}

impl<T: JobRefWithBackend> JobRef for Reusable<T> {
    fn id(&self) -> &JobId {
        self.0.id()
    }
}

impl<T: JobRefWithBackend> JobRefWithBackend for &Reusable<T> {
    type In = <T as JobRefWithBackend>::In;
    type Out = <T as JobRefWithBackend>::Out;
    type TO = <T as JobRefWithBackend>::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}

impl<T: JobRefWithBackend> JobRef for &Reusable<T> {
    fn id(&self) -> &JobId {
        self.0.id()
    }
}

impl<T: JobRefWithBackend> JobRefWithBackend for Funneled<T> {
    type In = <T as JobRefWithBackend>::In;
    type Out = <T as JobRefWithBackend>::Out;
    type TO = <T as JobRefWithBackend>::TO;

    fn backend(&self) -> &WeakFlowBackend<Self::TO> {
        self.0.backend()
    }
}

impl<T: JobRefWithBackend> JobRef for Funneled<T> {
    fn id(&self) -> &JobId {
        self.0.id()
    }
}

impl<T: JobRefWithBackend> FunneledJobRef for Funneled<T> {}
impl<T: FunneledJobRef> FunneledJobRef for Reusable<T> {}

impl<I, T: JobRefWithBackend<In = I>> FlowsInto<T> for FlowInput<I, T::TO> {
    type Out = Result<T, FlowError>;

    fn flows_into(self, other: T) -> Self::Out {
        transaction_mut(&self.backend, |backend| -> Result<(), FlowError> {
            let (task, input) = backend.get_mut_and_input(*other.id());
            let t = task.ok_or_else(|| FlowError::TaskNotFound(*other.id()))?;
            t.input_mut().set_source(input)?;
            Ok(())
        })?;

        Ok(other)
    }
}

impl<O, T: JobRefWithBackend<Out = O>> FlowsInto<FlowOutput<O, T::TO>> for T {
    type Out = Result<(), FlowError>;

    fn flows_into(self, _other: FlowOutput<O, T::TO>) -> Self::Out {
        transaction_mut(self.backend(), |backend| -> Result<(), FlowError> {
            let (task, output) = backend.get_mut_and_output(*self.id());
            let t = task.ok_or_else(|| FlowError::TaskNotFound(*self.id()))?;
            output.set_source(t.output_mut())?;
            Ok(())
        })?;
        Ok(())
    }
}

/// private trait implementations
mod private {
    use super::*;

    pub trait JobRefWithBackend: JobRef {
        type In;
        type Out;
        type TO: JobOrderer;

        fn backend(&self) -> &WeakFlowBackend<Self::TO>;
    }

    pub trait FunneledJobRef: JobRefWithBackend {}
}
use private::*;

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::{assert_impl_all, assert_not_impl_all};

    pub struct Move<T>(T);

    assert_impl_all!(JobReference<(), Move<i32>>: FlowsInto<JobReference<Move<i32>, ()>>);

    assert_impl_all!((JobReference<(), Move<i32>>, JobReference<(), Move<f32>>):
        FlowsInto<JobReference<(Move<i32>, Move<f32>), ()>>);

    assert_impl_all!(JobReference<(), i32>: FlowsInto<Reusable<JobReference<i32, ()>>>);
    assert_impl_all!(JobReference<(), i32>: FlowsInto<Funneled<JobReference<i32, ()>>>);
    assert_impl_all!(JobReference<(), i32>: FlowsInto<Reusable<Funneled<JobReference<i32, ()>>>>);
    assert_impl_all!(JobReference<(), i32>: FlowsInto<&'static Reusable<JobReference<i32, ()>>>);

    assert_impl_all!(Reusable<JobReference<(), i32>>: FlowsInto<&'static Reusable<JobReference<i32, ()>>>);
    assert_impl_all!(Reusable<JobReference<(), i32>>: FlowsInto<Reusable<JobReference<i32, ()>>>);
    assert_impl_all!(Reusable<JobReference<(), i32>>: FlowsInto<JobReference<i32, ()>>);
    assert_impl_all!(&Reusable<JobReference<(), i32>>: FlowsInto<&'static Reusable<JobReference<i32, ()>>>);
    assert_impl_all!(&Reusable<JobReference<(), i32>>: FlowsInto<Reusable<JobReference<i32, ()>>>);
    assert_impl_all!(&Reusable<JobReference<(), i32>>: FlowsInto<JobReference<i32, ()>>);

    assert_not_impl_all!(JobReference<(), i32>: FlowsInto<JobReference<(), ()>>);
    assert_not_impl_all!(&JobReference<(), i32>: FlowsInto<JobReference<i32, ()>>);
    assert_not_impl_all!(&JobReference<(), i32>: FlowsInto<&'static JobReference<i32, ()>>);
    assert_not_impl_all!((JobReference<(), Move<i32>>, JobReference<(), Move<i32>>):
        FlowsInto<JobReference<(Move<i32>, Move<f32>), ()>>);

    #[test]
    fn test_type_checking_non_clone() {
        let mut flow: Flow = Flow::new();

        let t1 = flow.create("create_i32", || Move(12_i32));
        let t2 = flow.create("consume_i32", |Move(i): Move<i32>| {});

        t1.flows_into(t2);
    }

    #[test]
    fn test_type_checking_cloneable() {
        let mut flow: Flow = Flow::new();

        let t1 = flow.create("create_i32", || 12_i32).reusable().unwrap();
        let t2 = flow.create("consume_i32", |i: i32| {});
        let t3 = flow.create("consume_i32", |i: i32| {});

        t1.as_ref().flows_into(t2).expect("Should be Ok");
        t1.as_ref().flows_into(t3).expect("Should be Ok");
    }

    #[test]
    fn test_type_checking_funnelable() {
        let mut flow: Flow = Flow::new();

        let t1 = flow
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
