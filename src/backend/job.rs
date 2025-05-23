//! A task within the backend

use crate::actions::{action, Action, BoxAction, Runnable};
use crate::backend::disjointed::Disjointed;
use crate::backend::flow_backend::{FlowBackendError, FlowBackendInput};
use crate::backend::funnel::BackendFunnel;
use crate::backend::recv_promise::RecvPromise;
use crate::backend::reusable;
use crate::backend::reusable::Reusable;
use crate::private::Sealed;
use crate::sync::promise::{BoxPromise, PollPromise, Promise, PromiseExt, PromiseSet};
use crate::sync::promise::{IntoPromise, MapPromise};
use crossbeam::channel::{bounded, Receiver, RecvError, SendError, Sender};
use fortuples::fortuples;
use std::any::{type_name, Any, TypeId};
use std::collections::HashSet;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZero;
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;
use tracing::trace;

static JOB_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// A task id
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct JobId(NonZero<usize>);

impl Display for JobId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "T#{}", self.0)
    }
}

impl JobId {
    /// Creates a new task id
    pub(crate) fn new() -> Self {
        let id = JOB_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        if id == 0 {
            panic!("task ID overflowed");
        }
        let non_zero = NonZero::new(id).expect("Should never be zero");
        JobId(non_zero)
    }
}

/// This is the data that is used
pub type Data = Box<dyn Any + Send>;

/// A backend task
pub struct BackendJob {
    id: JobId,
    nickname: String,
    input: Input,
    output: Output,
    action_input_sender: Sender<Data>,
    action_output_receiver: Receiver<Data>,

    action: BoxAction<'static, (), ()>,
}

impl Debug for BackendJob {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendTask")
            .field("id", &self.id)
            .field("nickname", &self.nickname)
            .finish()
    }
}

impl BackendJob {
    pub fn new<A, I, O>(
        name: impl AsRef<str>,
        output: impl AsOutputFlavor<Data = O>,
        action: A,
    ) -> BackendJob
    where
        I: Send + 'static,
        O: Send + 'static,
        A: Action<Input = I, Output = O> + 'static,
    {
        let id = JobId::new();
        let input = action.input_flavor();
        let nickname = name.as_ref().to_owned();
        let (input_sender, input_receiver) = bounded::<Data>(1);
        let (output_sender, output_receiver) = bounded::<Data>(1);

        let erased_action: BoxAction<(), ()> = match input {
            InputFlavor::None => {
                assert_eq!(
                    TypeId::of::<I>(),
                    TypeId::of::<()>(),
                    "illegal input type for flavor {:?}",
                    input
                );
                let mut action = action;
                let safe_action = crate::actions::action(move |_: ()| -> O {
                    let fake = unsafe {
                        assert_eq!(size_of::<I>(), 0, "input must have zero size");
                        let fake: I = std::mem::zeroed();
                        fake
                    };
                    action.apply(fake)
                });
                Box::new(safe_action.chain(SendOutputAction::new(output_sender)))
            }
            InputFlavor::Single => Box::new(
                ReceiveInputAction::new(input_receiver)
                    .chain(action)
                    .chain(SendOutputAction::new(output_sender)),
            ),
            InputFlavor::Funnel => {
                todo!("Directly creating funnel input not supported")
            }
        };
        let output = output.to_output(id);

        Self {
            id,
            nickname,
            input: Input::new::<I>(input),
            output,
            action_input_sender: input_sender,
            action_output_receiver: output_receiver,
            action: erased_action,
        }
    }

    /// Gets the id of this task
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Gets the dependencies for this task
    pub fn dependencies(&self) -> &HashSet<JobId> {
        self.input.dependencies()
    }

    /// Runs this task
    pub fn run(&mut self) -> Result<(), JobError> {
        if self.input.input_required() {
            match std::mem::replace(&mut self.input.kind, InputKind::None) {
                InputKind::None => return Err(JobError::NoInput),
                InputKind::Single(s) => {
                    let data = s.try_get().map_err(|_| JobError::InputNotReady)?;
                    self.action_input_sender.send(data)?;
                }
                InputKind::Funnel(m) => {
                    let promise = m.into_promise();
                    let data = promise.try_get().map_err(|_| JobError::InputNotReady)?;
                    self.action_input_sender.send(Box::new(data) as Data)?;
                }
            }
        } else if !matches!(self.input.kind, InputKind::None) {
            return Err(JobError::UnexpectedInput);
        }

        self.action.run();

        let output_receiver = self.action_output_receiver.recv()?;

        self.output
            .set_output_fn
            .take()
            .expect("can not set output multiple times")
            .accept(output_receiver)?;

        Ok(())
    }

    /// Turns this into a funnel input
    pub fn make_funnel<
        T: Send + 'static,
        I: FromIterator<T> + IntoIterator<Item = T, IntoIter: Send> + Send + 'static,
    >(
        &mut self,
    ) -> Result<(), JobError> {
        if !matches!(self.input.flavor, InputFlavor::Single) {
            return Err(JobError::UnexpectedInput);
        }
        if TypeId::of::<I>() != self.input.input_ty {
            return Err(JobError::UnexpectedType {
                expected: self.input.input_ty_str,
                received: type_name::<T>(),
                comment: None,
            });
        }
        self.input.flavor = InputFlavor::Funnel;
        let mut funnel = BackendFunnel::new();
        match std::mem::replace(&mut self.input.kind, InputKind::None) {
            InputKind::None => {}
            InputKind::Single(input) => {
                funnel.insert_iter(input.map(|data| {
                    // this is actually I
                    let d = *data.downcast::<I>().unwrap_or_else(|b| {
                        panic!("failed to downcast {b:?} to `{}`", type_name::<I>())
                    });
                    d.into_iter().map(|i| Box::new(i) as Data)
                }));
            }
            InputKind::Funnel(_) => {
                unreachable!()
            }
        }
        let action = std::mem::replace(&mut self.action, Box::new(action(|_| {})));
        let (new_sender, new_receiver) = bounded::<Data>(1);
        let old_sender = std::mem::replace(&mut self.action_input_sender, new_sender);

        self.action =
            Box::new(ReceiveFunnelInputAction::<T, I>::new(new_receiver, old_sender).chain(action));
        self.input.kind = InputKind::Funnel(funnel);
        Ok(())
    }

    /// Gets the input for this task
    #[must_use]
    pub fn input_mut(&mut self) -> &mut Input {
        &mut self.input
    }

    /// Gets the input for this task
    #[must_use]
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// Gets the output of this task
    #[must_use]
    pub fn output_mut(&mut self) -> &mut Output {
        &mut self.output
    }

    /// Gets the output of this task
    #[must_use]
    pub fn output(&self) -> &Output {
        &self.output
    }

    pub fn nickname(&self) -> &str {
        &self.nickname
    }
}

struct ReceiveInputAction<T> {
    receiver: Receiver<Data>,
    _marker: PhantomData<T>,
}

impl<T> ReceiveInputAction<T> {
    fn new(receiver: Receiver<Data>) -> Self {
        Self {
            receiver,
            _marker: PhantomData,
        }
    }
}

impl<T: Send + 'static> Action for ReceiveInputAction<T> {
    type Input = ();
    type Output = T;

    fn apply(&mut self, _: Self::Input) -> Self::Output {
        let i = self
            .receiver
            .recv()
            .expect("failed to receive input")
            .downcast::<T>()
            .unwrap_or_else(|e| {
                panic!(
                    "failed to downcast {:?} ({:?}) to `{}`",
                    e,
                    (*e).type_id(),
                    type_name::<T>()
                )
            });

        *i
    }

    fn input_flavor(&self) -> InputFlavor {
        InputFlavor::None
    }
}

struct ReceiveFunnelInputAction<T, I: FromIterator<T>> {
    receiver: Receiver<Data>,
    sender: Sender<Data>,
    _marker: PhantomData<(T, I)>,
}

impl<T, I: FromIterator<T>> ReceiveFunnelInputAction<T, I> {
    fn new(receiver: Receiver<Data>, sender: Sender<Data>) -> Self {
        Self {
            receiver,
            sender,
            _marker: PhantomData,
        }
    }
}

impl<T: Send + 'static, I: 'static + FromIterator<T> + Send> Action
    for ReceiveFunnelInputAction<T, I>
{
    type Input = ();
    type Output = ();

    fn apply(&mut self, _: Self::Input) -> Self::Output {
        let i = *self
            .receiver
            .recv()
            .expect("failed to receive input")
            .downcast::<Vec<Data>>()
            .unwrap_or_else(|e| panic!("failed to downcast {:?} to `{}`", e, type_name::<T>()));

        let rebuilt = i
            .into_iter()
            .map(|data| {
                *data.downcast::<T>().unwrap_or_else(|e| {
                    panic!("failed to downcast {e:?} to `{}`", type_name::<T>())
                })
            })
            .collect::<I>();
        self.sender.send(Box::new(rebuilt) as Data).unwrap();
    }

    fn input_flavor(&self) -> InputFlavor {
        InputFlavor::None
    }
}

struct SendOutputAction<T> {
    sender: Sender<Data>,
    _marker: PhantomData<T>,
}

impl<T> SendOutputAction<T> {
    fn new(sender: Sender<Data>) -> Self {
        Self {
            sender,
            _marker: PhantomData,
        }
    }
}

impl<T: Send + 'static> Action for SendOutputAction<T> {
    type Input = T;
    type Output = ();

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        let data = Box::new(input) as Data;
        self.sender.send(data).expect("failed to send data");
    }

    fn input_flavor(&self) -> InputFlavor {
        InputFlavor::Single
    }
}

/// Input flavor for this task
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InputFlavor {
    None,
    Single,
    Funnel,
}

/// An input source
pub trait InputSource<T> {
    type Data;

    fn use_as_input_source(&mut self, other: T) -> Result<(), JobError>;
}

/// Task input data
#[derive(Debug)]
pub struct Input {
    input_ty: TypeId,
    input_ty_str: &'static str,
    task_dependencies: HashSet<JobId>,
    flavor: InputFlavor,
    kind: InputKind,
}

impl Input {
    fn new<T: 'static>(input_flavor: InputFlavor) -> Self {
        let ty = TypeId::of::<T>();
        Self {
            input_ty: ty,
            input_ty_str: type_name::<T>(),
            task_dependencies: HashSet::new(),
            flavor: input_flavor,
            kind: match input_flavor {
                InputFlavor::Funnel => InputKind::Funnel(BackendFunnel::new()),
                _ => InputKind::None,
            },
        }
    }

    fn input_required(&self) -> bool {
        match self.flavor {
            InputFlavor::None => false,
            InputFlavor::Single | InputFlavor::Funnel => true,
        }
    }

    #[cfg(test)]
    #[allow(unused)]
    fn check_type<T: 'static>(&self) -> Result<(), JobError> {
        if TypeId::of::<T>() != self.input_ty {
            Err(JobError::UnexpectedType {
                expected: self.input_ty_str,
                received: type_name::<T>(),
                comment: None,
            })
        } else {
            Ok(())
        }
    }

    /// Gets the list of task id dependencies for this task
    pub fn dependencies(&self) -> &HashSet<JobId> {
        &self.task_dependencies
    }

    /// Sets an explicit task ordering
    pub fn depends_on(&mut self, task_id: JobId) {
        self.task_dependencies.insert(task_id);
    }

    /// Sets an explicit task ordering
    #[inline]
    pub fn depends_on_all<I: IntoIterator<Item = JobId>>(&mut self, task_ids: I) {
        task_ids.into_iter().for_each(|task_id| {
            self.depends_on(task_id);
        })
    }

    pub fn set_source<T: 'static + Send, S>(&mut self, source: S) -> Result<(), JobError>
    where
        Self: InputSource<S, Data = T>,
    {
        self.use_as_input_source(source)
    }

    pub fn input_ty(&self) -> TypeId {
        self.input_ty
    }
}

pub trait AsOutputFlavor: Sealed {
    type Data: 'static;

    fn to_output(self, id: JobId) -> Output;
}

pub struct SingleOutput<T: 'static + Send>(PhantomData<T>);

impl<T: 'static + Send> SingleOutput<T> {
    /// Create a new single output
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: 'static + Send> Sealed for SingleOutput<T> {}

impl<T: 'static + Send> AsOutputFlavor for SingleOutput<T> {
    type Data = T;

    fn to_output(self, id: JobId) -> Output {
        let (send, receive) = bounded::<Data>(1);
        let promise = RecvPromise::new(receive);

        let f = move |data: Data| -> Result<(), JobError> {
            send.send(data)?;
            Ok(())
        };

        Output {
            task_id: id,
            output_ty: TypeId::of::<T>(),
            output_ty_str: type_name::<T>(),
            flavor: OutputFlavor::Single,
            kind: OutputKind::Once(Some(Box::new(promise))),
            set_output_fn: Some(SetOutputFn::new(f)),
        }
    }
}

pub struct NoOutput;

impl Sealed for NoOutput {}
impl AsOutputFlavor for NoOutput {
    type Data = ();

    fn to_output(self, id: JobId) -> Output {
        Output {
            task_id: id,
            output_ty: TypeId::of::<()>(),
            output_ty_str: type_name::<()>(),
            flavor: OutputFlavor::None,
            kind: OutputKind::None,
            set_output_fn: None,
        }
    }
}

pub struct ReusableOutput<T: 'static + Send + Clone>(PhantomData<T>);

impl<T: 'static + Send + Clone> Sealed for ReusableOutput<T> {}

impl<T: 'static + Send + Clone> AsOutputFlavor for ReusableOutput<T> {
    type Data = T;

    fn to_output(self, id: JobId) -> Output {
        let (promise, f) = create_reusable::<T>();

        Output {
            task_id: id,
            output_ty: TypeId::of::<T>(),
            output_ty_str: type_name::<T>(),
            flavor: OutputFlavor::Reusable,
            kind: OutputKind::Reusable(promise),
            set_output_fn: Some(SetOutputFn::new(f)),
        }
    }
}

impl<T: 'static + Send + Clone> ReusableOutput<T> {
    /// Create a new reusable output
    #[allow(unused)]
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

#[derive(Debug)]
enum InputKind {
    None,
    Single(BoxPromise<'static, Data>),
    Funnel(BackendFunnel),
}

#[derive(Debug, Copy, Clone)]
pub enum OutputFlavor {
    None,
    Single,
    Reusable,
    Disjointed,
}

pub struct Output {
    task_id: JobId,
    output_ty: TypeId,
    output_ty_str: &'static str,
    flavor: OutputFlavor,
    kind: OutputKind,
    set_output_fn: Option<SetOutputFn>,
}

impl Output {
    pub fn make_reusable<T: Send + Clone + 'static>(&mut self) -> Result<(), JobError> {
        if TypeId::of::<T>() != self.output_ty {
            return Err(JobError::UnexpectedType {
                expected: self.output_ty_str,
                received: type_name::<T>(),
                comment: None,
            });
        } else if matches!(self.kind, OutputKind::Once(None)) {
            return Err(JobError::OutputAlreadyUsed);
        }

        self.flavor = OutputFlavor::Reusable;

        let (promise, f) = create_reusable::<T>();

        self.set_output_fn = Some(SetOutputFn::new(f));
        self.kind = OutputKind::Reusable(promise);
        Ok(())
    }

    pub fn make_disjointed<T: Send + 'static>(&mut self) -> Result<(), JobError> {
        if TypeId::of::<Vec<T>>() != self.output_ty {
            return Err(JobError::UnexpectedType {
                expected: self.output_ty_str,
                received: type_name::<T>(),
                comment: format!(
                    "To make disjointed, the output must have been declared as type `Vec<{}>`",
                    type_name::<T>()
                )
                .into(),
            });
        } else if matches!(self.kind, OutputKind::Once(None)) {
            return Err(JobError::OutputAlreadyUsed);
        }

        self.flavor = OutputFlavor::Disjointed;

        let (disjointed, f) = create_disjointed::<T>();
        self.set_output_fn = Some(SetOutputFn::new(f));
        self.kind = OutputKind::Disjointed(disjointed);

        Ok(())
    }

    pub fn output_ty(&self) -> TypeId {
        self.output_ty
    }
}

fn create_reusable<T: Send + Clone + 'static>() -> (
    Reusable<'static, Data>,
    impl FnOnce(Data) -> Result<(), JobError> + Send + 'static,
) {
    let (send, receive) = bounded::<T>(1);
    let promise: Reusable<T> = Reusable::new(Box::new(RecvPromise::new(receive)));

    let f = move |data: Data| -> Result<(), JobError> {
        let as_t = *data.downcast::<T>().expect("failed to downcast to");
        send.send(as_t)
            .map_err(|SendError(e)| SendError(Box::new(e) as Data))?;
        Ok(())
    };
    (promise.into_data(), f)
}

fn create_disjointed<T: Send + 'static>() -> (
    Disjointed<'static, Data>,
    impl FnOnce(Data) -> Result<(), JobError>,
) {
    let (send, receive) = bounded::<Vec<T>>(1);
    let inner = RecvPromise::new(receive);
    let as_data_vec: BoxPromise<Vec<Data>> = Box::new(inner.map(|t| {
        t.into_iter()
            .map(|t| Box::new(t) as Data)
            .collect()
    }));

    let disjointed: Disjointed<'static, Data>= Disjointed::new(as_data_vec);
    let f = move |data: Data| -> Result<(), JobError> {
        let as_t = *data.downcast::<Vec<T>>().expect("failed to downcast to vector");
        send.send(as_t)
            .map_err(|SendError(e)| SendError(Box::new(e) as Data))?;
        Ok(())
    };

    (disjointed, f)
}

struct SetOutputFn(Box<dyn FnOnce(Data) -> Result<(), JobError> + Send>);

impl SetOutputFn {
    fn new<F: FnOnce(Data) -> Result<(), JobError> + Send + 'static>(f: F) -> Self {
        let boxed = Box::new(f) as Box<dyn FnOnce(Data) -> Result<(), JobError> + Send>;
        Self(boxed)
    }

    fn accept(self, data: Data) -> Result<(), JobError> {
        (self.0)(data)
    }
}

impl InputSource<&mut Output> for Input {
    type Data = Data;

    fn use_as_input_source(&mut self, other: &mut Output) -> Result<(), JobError> {
        match (self.flavor, other.flavor) {
            (InputFlavor::Single, OutputFlavor::Reusable) => {
                let OutputKind::Reusable(reusable) = &mut other.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        other.flavor, other.kind
                    );
                };

                self.kind = InputKind::Single(Box::new(reusable.clone().into_promise()));
                self.depends_on(other.task_id);
                Ok(())
            }
            (InputFlavor::Single, OutputFlavor::Single) => {
                let OutputKind::Once(once) = &mut other.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        other.flavor, other.kind
                    );
                };
                self.depends_on(other.task_id);
                match once.take() {
                    None => Err(JobError::OutputCanNotBeReused),
                    Some(some) => {
                        self.kind = InputKind::Single(some);
                        Ok(())
                    }
                }
            }
            (InputFlavor::Funnel, OutputFlavor::Single) => {
                let OutputKind::Once(once) = &mut other.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        other.flavor, other.kind
                    );
                };
                let InputKind::Funnel(funnel) = &mut self.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        self.flavor, self.kind
                    );
                };

                match once.take() {
                    None => Err(JobError::OutputCanNotBeReused),
                    Some(some) => {
                        funnel.insert(some);
                        self.depends_on(other.task_id);
                        Ok(())
                    }
                }
            }
            (InputFlavor::Funnel, OutputFlavor::Reusable) => {
                let OutputKind::Reusable(reusable) = &mut other.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        other.flavor, other.kind
                    );
                };
                let InputKind::Funnel(funnel) = &mut self.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        self.flavor, self.kind
                    );
                };

                funnel.insert(reusable.clone());
                self.depends_on(other.task_id);
                Ok(())
            }
            (i, o) => Err(JobError::OutputCanNotBeUsedAsInput {
                output_flavor: o,
                input_flavor: i,
            }),
        }
    }
}
impl<P: Promise<Output = Data> + 'static> InputSource<P> for Input {
    type Data = P::Output;

    fn use_as_input_source(&mut self, other: P) -> Result<(), JobError> {
        match (self.flavor, &mut self.kind) {
            (InputFlavor::None, _) => Err(JobError::UnexpectedInput),
            (InputFlavor::Single, to_set @ InputKind::None) => {
                *to_set = InputKind::Single(Box::new(other));
                Ok(())
            }
            (InputFlavor::Single, InputKind::Single(_)) => Err(JobError::InputAlreadySet),
            (InputFlavor::Funnel, InputKind::Funnel(funnel)) => {
                funnel.insert(other);
                Ok(())
            }
            _ => {
                panic!("input kind and flavor mismatch")
            }
        }
    }
}

fortuples! {
    #[tuples::min_size(1)]
    impl InputSource<#Tuple> for Input
    where
        #(#Member: OutputWithType<T: Send + 'static>,)*
    {
        type Data = (#(#Member::T,)*);

        fn use_as_input_source(&mut self, mut other: #Tuple) -> Result<(), JobError> {
            let (#(#Member,)*) = (#(#other.as_output(),)*);
            let task_ids = [#(#Member.task_id),*];
            let promises = || -> Result<_, JobError> {
                let promise_set = [#(#Member,)*]
                    .iter_mut()
                    .map(|o| match &o.kind {
                        OutputKind::None => Err(JobError::OutputCanNotBeUsedAsInput {
                            output_flavor: OutputFlavor::None,
                            input_flavor: self.flavor,
                        }),
                        OutputKind::Once(None) => Err(JobError::OutputAlreadyUsed),
                        OutputKind::Once(_) | OutputKind::Reusable(_) => Ok(o.into_promise()),
                        OutputKind::Disjointed(disjoints) => {
                            todo!("disjointed can be used as input if not's been partially used")
                        }
                    })
                    .collect::<Result<PromiseSet<_>, JobError >>()?;

                Ok(promise_set
                    .into_promise()
                    .map(|promised: Vec<Data>| {
                        trace!("converting {promised:?} to ({})", [#(type_name::<#Member::T>()),*].join(", "));
                        let [#(#Member,)*] = promised.try_into().expect("failed to convert to array");
                        let ret = (
                            #(
                                *#Member.downcast::<#Member::T>().unwrap(),
                            )*
                        );
                        ret
                    }))
            };


            match self.flavor {
                InputFlavor::None => Err(JobError::OutputCanNotBeUsedAsInput {
                    output_flavor: OutputFlavor::None,
                    input_flavor: self.flavor,
                }),
                InputFlavor::Single => {
                    let promise = promises()?;
                    self.depends_on_all(task_ids);
                    self.kind = InputKind::Single(Box::new(promise.map(|p: Self::Data| Box::new(p) as Data)));
                    Ok(())
                }
                InputFlavor::Funnel => {
                    let promise = promises()?;
                    self.depends_on_all(task_ids);
                    let InputKind::Funnel(funnel) = &mut self.kind else {
                        unreachable!()
                    };
                    funnel.insert(promise.map(|t| Box::new(t) as Data));
                    Ok(())
                }
            }
        }
    }


}

// fortuples! {
//     #[tuples::min_size(1)]
//     impl InputSource<(PhantomData<#Tuple>, [&mut Output; #len(Tuple)])> for Input
//     where
//         #(#Member: OutputWithType,)*
//     {
//         type Data = #Tuple;
//
//         fn use_as_input_source(&mut self, other: (PhantomData<#Tuple>, [&mut Output; #len(Tuple)])) -> Result<(), TaskError> {
//             todo!()
//         }
//     }
// }

impl InputSource<&mut FlowBackendInput> for Input {
    type Data = Data;

    fn use_as_input_source(&mut self, other: &mut FlowBackendInput) -> Result<(), JobError> {
        match self.flavor {
            InputFlavor::None => Err(JobError::UnexpectedInput),
            InputFlavor::Single => {
                self.kind = InputKind::Single(Box::new(other.take_promise()?));
                Ok(())
            }
            InputFlavor::Funnel => {
                let InputKind::Funnel(funnel) = &mut self.kind else {
                    panic!(
                        "flavor and kind mismatch. flavor = {:?}, kind = {:?}",
                        self.flavor, self.kind
                    );
                };
                funnel.insert(other.take_promise()?);
                Ok(())
            }
        }
    }
}

pub trait OutputWithType {
    type T;

    fn as_output(&mut self) -> &mut Output;
}

pub struct TypedOutput<'a, T>(&'a mut Output, PhantomData<T>);

impl<'a, T> TypedOutput<'a, T> {
    pub fn new(output: &'a mut Output) -> Self {
        TypedOutput(output, PhantomData)
    }
}

impl<T> OutputWithType for TypedOutput<'_, T> {
    type T = T;

    fn as_output(&mut self) -> &mut Output {
        self.0
    }
}

pub enum TaskOutputPromise {
    Once(BoxPromise<'static, Data>),
    Reusable(reusable::IntoPromise<'static, Data, BoxPromise<'static, Data>>),
}

impl Promise for TaskOutputPromise {
    type Output = Data;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self {
            TaskOutputPromise::Once(o) => o.poll(),
            TaskOutputPromise::Reusable(r) => Promise::poll(r),
        }
    }
}

impl IntoPromise for &mut Output {
    type Output = Data;
    type IntoPromise = TaskOutputPromise;

    fn into_promise(self) -> Self::IntoPromise {
        match &mut self.kind {
            OutputKind::None => {
                panic!("can not be used as a promise")
            }
            OutputKind::Once(o) => {
                let o = o.take().expect("output already used");
                TaskOutputPromise::Once(o)
            }
            OutputKind::Reusable(s) => {
                let cloned = s.clone();
                TaskOutputPromise::Reusable(cloned.into_promise())
            }
            OutputKind::Disjointed(_) => {
                panic!("Can not infallibly made into a promise")
            }
        }
    }
}

#[derive(Debug)]
enum OutputKind {
    /// no output
    None,
    /// used when the output can only be used once
    Once(Option<BoxPromise<'static, Data>>),
    /// Used when the output can be used multiple times
    Reusable(Reusable<'static, Data>),
    /// Disjointed output
    Disjointed(Disjointed<'static, Data>),
}

impl OutputKind {
    fn as_reusable(&self) -> Option<&Reusable<'static, Data>> {
        if let Self::Reusable(reusable) = self {
            Some(reusable)
        } else {
            None
        }
    }
    fn as_reusable_mut(&mut self) -> Option<&mut Reusable<'static, Data>> {
        if let Self::Reusable(reusable) = self {
            Some(reusable)
        } else {
            None
        }
    }
    fn as_disjointed(&self) -> Option<&Disjointed<'static, Data>> {
        if let Self::Disjointed(d) = self {
            Some(d)
        } else {
            None
        }
    }
    fn as_disjointed_mut(&mut self) -> Option<&mut Disjointed<'static, Data>> {
        if let Self::Disjointed(d) = self {
            Some(d)
        } else {
            None
        }
    }
}

/// An error occurred while executing a job
#[derive(Debug, Error)]
pub enum JobError {
    #[error("Output can not be re-used")]
    OutputCanNotBeReused,
    #[error("The input for this task is yet ready")]
    InputNotReady,
    #[error("No input is expected for this task")]
    NoInput,
    #[error("This task did not expect an input")]
    UnexpectedInput,
    #[error("Can not set this as reusable because output was already used")]
    OutputAlreadyUsed,
    #[error(transparent)]
    SendError(#[from] SendError<Data>),
    #[error(transparent)]
    RecvError(#[from] RecvError),

    #[error("The input for this task was already set")]
    InputAlreadySet,
    #[error("Unexpected type (expected: `{expected}`, actual: `{received}`){}", comment.as_ref().map(|s| format!(": {s}")).unwrap_or(String::new()))]
    UnexpectedType {
        expected: &'static str,
        received: &'static str,
        comment: Option<String>,
    },
    #[error("{output_flavor:?} can not be used as an input for {input_flavor:?}")]
    OutputCanNotBeUsedAsInput {
        output_flavor: OutputFlavor,
        input_flavor: InputFlavor,
    },
    #[error(transparent)]
    FlowBackendError(Box<FlowBackendError>),
}

impl From<FlowBackendError> for JobError {
    fn from(value: FlowBackendError) -> Self {
        JobError::FlowBackendError(Box::new(value))
    }
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    use crate::backend::job::{Data, Input, InputFlavor, InputKind, InputSource, JobError};
    use crate::sync::promise::MapPromise;
    use crate::sync::promise::{BoxPromise, Just};
    use std::any::{type_name, TypeId};

    /// Used for mocking a task input
    pub struct MockTaskInput<T>(pub T);

    impl<T> MockTaskInput<T> {
        pub fn into_inner(self) -> T {
            self.0
        }
    }

    impl<T: Send + 'static> InputSource<MockTaskInput<T>> for Input {
        type Data = T;

        fn use_as_input_source(&mut self, other: MockTaskInput<T>) -> Result<(), JobError> {
            self.check_type::<T>()?;
            let as_promise = Just::new(other.into_inner()).map(|t| Box::new(t) as Data);
            match (self.flavor, &mut self.kind) {
                (InputFlavor::None, _) => return Err(JobError::UnexpectedInput),
                (InputFlavor::Single, InputKind::None) => {
                    if TypeId::of::<T>() != self.input_ty {
                        return Err(JobError::UnexpectedType {
                            expected: self.input_ty_str,
                            received: type_name::<T>(),
                            comment: None,
                        });
                    }

                    let promise = Box::new(as_promise) as BoxPromise<'static, Data>;
                    self.kind = InputKind::Single(promise);
                }
                (InputFlavor::Single, _) => return Err(JobError::InputAlreadySet),
                (InputFlavor::Funnel, InputKind::Funnel(funnel)) => {
                    funnel.insert(as_promise);
                }
                (InputFlavor::Funnel, _) => {
                    panic!("funnel flavor has no funnel kind")
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::action;
    use crate::backend::flow_backend::FlowBackendOutput;
    use crate::backend::job::test_fixtures::MockTaskInput;
    use crate::sync::promise::GetPromise;
    use std::thread;

    #[test]
    fn test_task_id() {
        let JobId(id) = JobId::new();
        assert!(id.get() > 0);
    }

    #[test]
    fn test_create_task() {
        let mut task = BackendJob::new(
            "task",
            SingleOutput::new(),
            action(|i: i32| {
                println!("{}", i);
                i.to_string()
            }),
        );
        task.input_mut()
            .set_source(MockTaskInput(12))
            .expect("failed to set input");
        task.run().expect("failed to run task");
    }

    #[test]
    fn test_no_input_task() {
        let (tx, rx) = bounded::<&str>(1);
        let mut task = BackendJob::new(
            "task",
            SingleOutput::new(),
            action(move || {
                tx.send("Hello, world").expect("failed to send input");
            }),
        );
        task.run().expect("failed to run task");
        let output = rx.try_recv().expect("failed to receive output");
        assert_eq!(output, "Hello, world");
    }

    #[test]
    fn test_flow_input_to_task() {
        let mut input = FlowBackendInput::default();
        let mut task = BackendJob::new(
            "task",
            SingleOutput::new(),
            action(move |i: i32| {
                assert_eq!(i, 32);
            }),
        );
        task.input_mut()
            .set_source(&mut input)
            .expect("failed to set input");
        input.send(Box::new(32_i32)).expect("failed to send input");
        task.run().expect("failed to run task");
    }

    #[test]
    fn test_flow_output_from_task() {
        let mut output = FlowBackendOutput::default();
        let mut task = BackendJob::new("task", SingleOutput::new(), action(move || 32_i32));
        output
            .set_source(task.output_mut())
            .expect("failed to set output");

        task.run().expect("failed to run task");
        let t = *output
            .get()
            .downcast::<i32>()
            .expect("failed to downcast output");
        assert_eq!(t, 32_i32);
    }

    #[test]
    fn test_make_output_disjointed() {
        let mut task = BackendJob::new("task", SingleOutput::new(), action(move || vec![1, 2, 3]));
        task.output_mut()
            .make_disjointed::<char>()
            .expect_err("should fail to make disjoint");
        task.output_mut()
            .make_disjointed::<i32>()
            .expect("failed to make disjoint");
    }

    #[test]
    fn test_disjointed_flow_output() {
        let mut task1 =
            BackendJob::new("task1", SingleOutput::new(), action(move || vec![1, 2, 3]));
        task1
            .output_mut()
            .make_disjointed::<i32>()
            .expect("failed to make disjoint");
        let disjoint = task1
            .output_mut()
            .kind
            .as_disjointed_mut()
            .expect("must be disjoint");

        let mut task2 = BackendJob::new("task2", SingleOutput::new(), action(move |i: i32| {}));
        task2
            .input_mut()
            .set_source(disjoint.get(0).unwrap())
            .expect("failed to set");

        let mut task3 =
            BackendJob::new("task2", SingleOutput::new(), action(move |i: Vec<i32>| {}));
        task3
            .input_mut()
            .set_source(
                disjoint
                    .get_range(1..)
                    .unwrap()
                    .map(|d| Box::new(d) as Data),
            )
            .expect("failed to set");

        task1.run().expect("failed to run task1");
        task2.run().expect("failed to run task2");
        task3.run().expect("failed to run task3");
    }

    #[test]
    fn test_chain_task() {
        let mut task1 = BackendJob::new("task1", SingleOutput::new(), action(|i: i32| i * i));
        let mut task2 = BackendJob::new(
            "task2",
            SingleOutput::new(),
            action(|i: i32| {
                println!("{}", i);
                i.to_string()
            }),
        );
        task1
            .input_mut()
            .set_source(MockTaskInput(12))
            .expect("failed to set input");
        task2
            .input_mut()
            .set_source(task1.output_mut())
            .expect("failed to set output for task 2");
        task1.run().expect("failed to run task1");
        thread::spawn(move || {
            task2.run().expect("failed to run task2");
        })
        .join()
        .expect("failed to join thread");
    }

    #[test]
    fn test_funnel_task() {
        let mut task1 = BackendJob::new("task1", SingleOutput::new(), action(|i: i32| i * i));
        let mut task2 = BackendJob::new("task2", SingleOutput::new(), action(|i: i32| i * i * i));
        let mut task3 = BackendJob::new(
            "task3",
            SingleOutput::new(),
            action(|i: Vec<i32>| {
                assert_eq!(i, [9, 27]);
                i.iter().sum::<i32>()
            }),
        );
        task3
            .make_funnel::<i32, Vec<i32>>()
            .expect("failed to create funnel");
        task1
            .input_mut()
            .set_source(MockTaskInput(3))
            .expect("failed to set input");
        task2
            .input_mut()
            .set_source(MockTaskInput(3))
            .expect("failed to set input for task 2");
        task3
            .input_mut()
            .set_source(task1.output_mut())
            .expect("failed to set input for task 3");
        task3
            .input_mut()
            .set_source(task2.output_mut())
            .expect("failed to set input for task 3");

        task1.run().expect("failed to run task1");
        task2.run().expect("failed to run task2");
        task3.run().expect("failed to run task3");
    }
}
