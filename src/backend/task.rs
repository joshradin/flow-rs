//! A task within the backend

use crate::action::{Action, BoxAction, Runnable};
use crate::backend::reusable::Reusable;
use crate::promise::{BoxPromise, GetPromise, PollPromise, Promise};
use crate::promise::{IntoPromise, MapPromise};
use crossbeam::channel::{bounded, Receiver, RecvError, SendError, Sender};
use parking_lot::Mutex;
use std::any::{type_name, Any, TypeId};
use std::collections::HashSet;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZero;
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;
use crate::backend::task::private::Sealed;
use crate::Void;

static TASK_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// A task id
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct TaskId(NonZero<usize>);

impl Display for TaskId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "T#{}", self.0)
    }
}

impl TaskId {
    /// Creates a new task id
    pub fn new() -> Self {
        let id = TASK_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        if id == 0 {
            panic!("task ID overflowed");
        }
        let non_zero = NonZero::new(id).expect("Should never be zero");
        TaskId(non_zero)
    }
}

/// This is the data that is used
pub type Data = Box<dyn Any + Send>;

/// A backend task
pub struct BackendTask {
    id: TaskId,
    nickname: String,
    input: Input,
    output: Output,
    action_input_sender: Sender<Data>,
    action_output_receiver: Receiver<Data>,

    action: BoxAction<'static, (), ()>,
}

impl Debug for BackendTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendTask")
            .field("id", &self.id)
            .field("nickname", &self.nickname)
            .finish()
    }
}

impl BackendTask {
    pub fn new<A, I: 'static, O: 'static>(
        name: impl AsRef<str>,
        input: InputFlavor,
        output: impl AsOutputFlavor<Data = O>,
        action: A,
    ) -> BackendTask
    where
        I: Send,
        O: Send,
        A: Action<Input = I, Output = O> + 'static,
    {
        let id = TaskId::new();
        let nickname = name.as_ref().to_owned();
        let (input_sender, input_receiver) = bounded::<Data>(1);
        let (output_sender, output_receiver) = bounded::<Data>(1);

        let erased_action = ReceiveInputAction::new(input_receiver)
            .chain(action)
            .chain(SendOutputAction::new(output_sender));
        let output = output.to_output(id);

        Self {
            id,
            nickname,
            input: Input::new::<I>(input),
            output,
            action_input_sender: input_sender,
            action_output_receiver: output_receiver,
            action: Box::new(erased_action) as BoxAction<_, _>,
        }
    }

    /// Gets the id of this task
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Gets the dependencies for this task
    pub fn dependencies(&self) -> &HashSet<TaskId> {
        self.input.dependencies()
    }



    /// Runs this task
    pub fn run(&mut self) -> Result<(), TaskError> {
        if self.input.input_required() {
            match std::mem::replace(&mut self.input.kind, InputKind::None) {
                InputKind::None => return Err(TaskError::NoInput),
                InputKind::Single(s) => {
                    let data = s.get();
                    self.action_input_sender.send(data)?;
                }
                InputKind::Multi(m) => {
                    todo!("multi input")
                }
            }
        } else if !matches!(self.input.kind, InputKind::None) {
            return Err(TaskError::UnexpectedInput);
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
            .unwrap_or_else(|e| panic!("failed to downcast {:?} to `{}`", e, type_name::<T>()));

        *i
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
}

/// Input flavor for this task
#[derive(Debug, Copy, Clone)]
pub enum InputFlavor {
    None,
    Single,
}

/// An input source
pub trait InputSource<T> {
    type Data;

    fn use_as_input_source(&mut self, other: T) -> Result<(), TaskError>;
}

/// Task input data
#[derive(Debug)]
pub struct Input {
    input_ty: TypeId,
    input_ty_str: &'static str,
    task_dependencies: HashSet<TaskId>,
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
            kind: InputKind::None,
        }
    }

    fn input_required(&self) -> bool {
        match self.flavor {
            InputFlavor::None => false,
            InputFlavor::Single => true,
        }
    }

    fn check_type<T: 'static>(&self) -> Result<(), TaskError> {
        if TypeId::of::<T>() != self.input_ty {
            Err(TaskError::UnexpectedType {
                expected: self.input_ty_str,
                received: type_name::<T>(),
            })
        } else {
            Ok(())
        }
    }



    /// Gets the list of task id dependencies for this task
    pub fn dependencies(&self) -> &HashSet<TaskId> {
        &self.task_dependencies
    }

    /// Sets an explicit task ordering
    pub fn depends_on(&mut self, task_id: TaskId) {
        self.task_dependencies.insert(task_id);
    }

    pub fn set_source<T: 'static + Send, S>(&mut self, source: S) -> Result<(), TaskError>
    where
        Self: InputSource<S, Data = T>,
    {
        self.use_as_input_source(source)
    }

    pub fn input_ty(&self) -> TypeId {
        self.input_ty
    }
}

pub trait AsOutputFlavor : Sealed {
    type Data: 'static;

    fn to_output(self, id: TaskId) -> Output;
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

    fn to_output(self, id: TaskId) -> Output {
        let (send, receive) = bounded::<Data>(1);
        let promise = RecvPromise::new(receive);

        let f = move |data: Data| -> Result<(), TaskError> {
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

    fn to_output(self, id: TaskId) -> Output {
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

struct RecvPromise<T> {
    receiver: Receiver<T>,
}

impl<T> RecvPromise<T> {
    fn new(receiver: Receiver<T>) -> Self {
        Self { receiver }
    }
}

impl<T: Send> Promise for RecvPromise<T> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        if !self.receiver.is_empty() {
            PollPromise::Ready(self.receiver.recv().expect("failed to receive promise"))
        } else {
            PollPromise::Pending
        }
    }
}

pub struct ReusableOutput<T: 'static + Send + Clone>(PhantomData<T>);

impl<T: 'static + Send + Clone> Sealed for ReusableOutput<T> {}

impl<T: 'static + Send + Clone> AsOutputFlavor for ReusableOutput<T> {
    type Data = T;

    fn to_output(self, id: TaskId) -> Output {
        let (send, receive) = bounded::<T>(1);
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
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

#[derive(Debug)]
enum InputKind {
    None,
    Single(BoxPromise<'static, Data>),
    Multi(Mutex<Vec<BoxPromise<'static, Data>>>),
}

#[derive(Debug, Copy, Clone)]
pub enum OutputFlavor {
    None,
    Single,
    Reusable,
}

pub struct Output {
    task_id: TaskId,
    output_ty: TypeId,
    output_ty_str: &'static str,
    flavor: OutputFlavor,
    kind: OutputKind,
    set_output_fn: Option<SetOutputFn>,
}

impl Output {
    pub fn make_reusable<T: Send + Clone + 'static>(&mut self) -> Result<(), TaskError> {
        if TypeId::of::<T>() != self.output_ty {
            return Err(TaskError::UnexpectedType {
                expected: self.output_ty_str,
                received: type_name::<T>(),
            });
        } else if matches!(self.kind, OutputKind::Once(None)) {
            return Err(TaskError::OutputAlreadyUsed);
        }

        self.flavor = OutputFlavor::Reusable;

        let (promise, f) = create_reusable::<T>();

        self.set_output_fn = Some(SetOutputFn::new(f));
        self.kind = OutputKind::Reusable(promise);
        Ok(())
    }

    pub fn output_ty(&self) -> TypeId {
        self.output_ty
    }
}

fn create_reusable<T: Send + Clone + 'static>() -> (Reusable, impl FnOnce(Data) -> Result<(), TaskError> + Send + 'static) {
    let (send, receive) = bounded::<T>(1);
    let promise = Reusable::new(RecvPromise::new(receive));

    let f = move |data: Data| -> Result<(), TaskError> {
        let as_t = *data.downcast::<T>().expect("failed to downcast to");
        send.send(as_t)
            .map_err(|SendError(e)| SendError(Box::new(e) as Data))?;
        Ok(())
    };
    (promise, f)
}

struct SetOutputFn(Box<dyn FnOnce(Data) -> Result<(), TaskError> + Send>);

impl SetOutputFn {
    fn new<F: FnOnce(Data) -> Result<(), TaskError> + Send + 'static>(f: F) -> Self {
        let boxed = Box::new(f) as Box<dyn FnOnce(Data) -> Result<(), TaskError> + Send>;
        Self(boxed)
    }

    fn accept(self, data: Data) -> Result<(), TaskError> {
        (self.0)(data)
    }
}

impl InputSource<&mut Output> for Input {
    type Data = Data;

    fn use_as_input_source(&mut self, other: &mut Output) -> Result<(), TaskError> {
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
                    None => Err(TaskError::OutputCanNotBeReused),
                    Some(some) => {
                        self.kind = InputKind::Single(some);
                        Ok(())
                    }
                }
            }
            (i, o) => Err(TaskError::OutputCanNotBeUsedAsInput {
                output_flavor: o,
                input_flavor: i,
            }),
        }
    }
}

pub struct TaskOutputPromise {
    kind: OutputKind,
}

impl Promise for TaskOutputPromise {
    type Output = Data;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        todo!()
    }
}

#[derive(Debug)]
enum OutputKind {
    None,
    /// used when the output can only be used once
    Once(Option<BoxPromise<'static, Data>>),
    /// Used when the output can be used multiple times
    Reusable(Reusable),
}

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("Output can not be re-used")]
    OutputCanNotBeReused,
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
    #[error("Unexpected type (expected: `{expected}`, actual: `{received}`)")]
    UnexpectedType {
        expected: &'static str,
        received: &'static str,
    },
    #[error("{output_flavor:?} can not be used as an input for {input_flavor:?}")]
    OutputCanNotBeUsedAsInput {
        output_flavor: OutputFlavor,
        input_flavor: InputFlavor,
    },
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    use crate::backend::task::{Data, Input, InputFlavor, InputKind, InputSource, TaskError};
    use crate::promise::MapPromise;
    use crate::promise::{BoxPromise, Just};
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

        fn use_as_input_source(&mut self, other: MockTaskInput<T>) -> Result<(), TaskError> {
            self.check_type::<T>()?;
            let as_promise = Just::new(other.into_inner());
            match (self.flavor, &mut self.kind) {
                (InputFlavor::None, _) => return Err(TaskError::UnexpectedInput),
                (InputFlavor::Single, InputKind::None) => {
                    if TypeId::of::<T>() != self.input_ty {
                        return Err(TaskError::UnexpectedType {
                            expected: self.input_ty_str,
                            received: type_name::<T>(),
                        });
                    }

                    let promise = Box::new(as_promise.map(|t| Box::new(t) as Data))
                        as BoxPromise<'static, Data>;
                    self.kind = InputKind::Single(promise);
                }
                (InputFlavor::Single, _) => return Err(TaskError::InputAlreadySet),
            }
            Ok(())
        }
    }
}

mod private {
    pub trait Sealed {}
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::action;
    use crate::backend::task::test_fixtures::MockTaskInput;
    use std::thread;

    #[test]
    fn test_task_id() {
        let TaskId(id) = TaskId::new();
        assert!(id.get() > 0);
    }

    #[test]
    fn test_create_task() {
        let mut task = BackendTask::new(
            "task",
            InputFlavor::Single,
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
    fn test_chain_task() {
        let mut task1 = BackendTask::new(
            "task1",
            InputFlavor::Single,
            SingleOutput::new(),
            action(|i: i32| i * i),
        );
        let mut task2 = BackendTask::new(
            "task2",
            InputFlavor::Single,
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
}
