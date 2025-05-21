//! Actual flow backend

use crate::backend::job::{BackendJob, Data, JobError, JobId, Output};
use crate::backend::recv_promise::RecvPromise;
use crate::job_ordering::{DefaultTaskOrderer, FlowGraph};
use crate::job_ordering::{JobOrderer, JobOrdering, JobOrderingError};
use crate::pool::{FlowThreadPool, WorkerPool};
use crate::promise::{BoxPromise, IntoPromise, PollPromise, PromiseSet};
use crossbeam::channel::{Receiver, SendError, Sender, TryRecvError, bounded};
use parking_lot::Mutex;
use static_assertions::assert_impl_all;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tracing::{debug, error_span, trace};

/// Executes flow
#[derive(Debug)]
pub struct FlowBackend<T: JobOrderer = DefaultTaskOrderer, P: WorkerPool = FlowThreadPool> {
    worker_pool: P,
    orderer: T,
    listeners: Vec<Mutex<Box<dyn FlowBackendListener>>>,
    tasks: HashMap<JobId, BackendJob>,
    input: FlowBackendInput,
    output: FlowBackendOutput,
}

assert_impl_all!(FlowBackend: Send);

impl FlowBackend {
    /// Creates a new flow backend
    pub fn new() -> Self {
        Self::with_worker_pool(FlowThreadPool::default())
    }
}

impl<P: WorkerPool> FlowBackend<DefaultTaskOrderer, P> {
    /// Creates the new flow backend with the given worker pool
    pub fn with_worker_pool(worker_pool: P) -> Self {
        Self::with_task_orderer_and_worker_pool(DefaultTaskOrderer::default(), worker_pool)
    }
}

impl<T: JobOrderer, P: WorkerPool> FlowBackend<T, P> {
    /// Creates the new flow backend with the given worker pool
    pub fn with_task_orderer_and_worker_pool(task_orderer: T, worker_pool: P) -> Self {
        Self {
            worker_pool,
            orderer: task_orderer,
            listeners: vec![],
            tasks: HashMap::new(),
            input: FlowBackendInput::default(),
            output: FlowBackendOutput::default(),
        }
    }

    /// Add a task to this flow backend
    pub fn add(&mut self, task: BackendJob) {
        self.tasks.insert(task.id(), task);
    }

    /// Gets the task by id
    pub fn get(&self, task_id: JobId) -> Option<&BackendJob> {
        self.tasks.get(&task_id)
    }

    /// Gets a mutable reference to a task by id
    pub fn get_mut(&mut self, task_id: JobId) -> Option<&mut BackendJob> {
        self.tasks.get_mut(&task_id)
    }

    /// Gets two mutable reference to tasks by id, if the ids are disjoint
    pub fn get_mut2(
        &mut self,
        task_id_1: JobId,
        task_id_2: JobId,
    ) -> Option<(&mut BackendJob, &mut BackendJob)> {
        let [a, b] = self.tasks.get_disjoint_mut([&task_id_1, &task_id_2]);
        a.zip(b)
    }

    /// Gets `N` mutable reference to tasks by id, if the ids are disjoint
    pub fn get_mut_disjoint<const N: usize>(
        &mut self,
        task_ids: [JobId; N],
    ) -> Option<[&mut BackendJob; N]> {
        let references = task_ids.each_ref();
        let tasks = self.tasks.get_disjoint_mut(references);
        let mut ret: Vec<&mut BackendJob> = vec![];

        for task in tasks {
            match task {
                None => {
                    return None;
                }
                Some(b) => {
                    ret.push(b);
                }
            }
        }

        <[&mut BackendJob; N]>::try_from(ret).ok()
    }

    /// Gets the given task id and the flow input
    pub fn get_mut_and_input(
        &mut self,
        task_id: JobId,
    ) -> (Option<&mut BackendJob>, &mut FlowBackendInput) {
        let Self { tasks, input, .. } = self;

        (tasks.get_mut(&task_id), input)
    }
    /// Gets the given task id and the flow output
    pub fn get_mut_and_output(
        &mut self,
        task_id: JobId,
    ) -> (Option<&mut BackendJob>, &mut FlowBackendOutput) {
        let Self { tasks, output, .. } = self;

        (tasks.get_mut(&task_id), output)
    }

    /// Add a backend listener
    pub fn add_listener<L: FlowBackendListener + 'static>(&mut self, listener: L) {
        self.listeners.push(Mutex::new(Box::new(listener)));
    }

    /// calculates the task ordering for this flow backend
    pub fn ordering(&self) -> Result<(T::JobOrdering, BackendFlowGraph), FlowBackendError> {
        let flow_graph = BackendFlowGraph::new(self.tasks.values());
        let ordering = self
            .orderer
            .create_ordering(flow_graph.clone(), self.worker_pool.max_size())?;
        Ok((ordering, flow_graph))
    }

    /// Executes this flow
    pub fn execute(&mut self) -> Result<(), FlowBackendError> {
        let result = (|| {
            let span = error_span!("execute");
            let _enter = span.enter();
            self.listeners.iter().for_each(|i| i.lock().started());
            debug!(
                "Calculating task ordering for {} tasks...",
                self.tasks.len()
            );
            let instant = Instant::now();
            let (mut ordering, graph) = self.ordering()?;
            debug!("Task ordering took: {:.3}ms", instant.elapsed().as_millis());

            // let mut tasks = std::mem::replace(&mut self.tasks, Default::default());
            let listeners = Arc::new(self.listeners.drain(..).collect::<Vec<_>>());
            let mut promises = PromiseSet::new();

            let mut step: usize = 1;
            let mut open_tasks = BinaryHeap::<Weighted<JobId>>::new();
            let mut tasks_completed = 0;
            let mut tasks_running = 0;

            trace!("starting task execution");

            while !ordering.empty() {
                let newly_ready = ordering.poll()?;
                let new_adds = !newly_ready.is_empty();
                for task_id in newly_ready {
                    let dependents = graph.dependents(&task_id).len();
                    open_tasks.push(Weighted {
                        t: task_id,
                        dependents: dependents + 1,
                        step,
                    });
                }

                let max = self.worker_pool.max_size() - self.worker_pool.running();

                let len = open_tasks.len();
                let to_drain = max.min(len);
                let batch = {
                    let mut batch = Vec::with_capacity(to_drain);
                    for _ in 0..to_drain {
                        batch.push(open_tasks.pop().unwrap().t);
                    }
                    batch
                };

                for task_id in batch {
                    trace!("sending task {}", task_id);
                    tasks_running += 1;
                    let mut task = self.tasks.remove(&task_id).expect("Task not found");
                    let name = task.nickname().to_string();
                    let listeners = listeners.clone();
                    let promise = self.worker_pool.submit(move || {
                        debug!("Task {:?} started", task.nickname());
                        listeners.iter().for_each(|i| {
                            i.lock().task_started(task.id(), task.nickname());
                        });
                        let r = task.run();
                        listeners.iter().for_each(|i| {
                            i.lock().task_finished(
                                task.id(),
                                task.nickname(),
                                r.as_ref().map(|s| *s),
                            );
                        });
                        (task_id, name, r)
                    });
                    trace!("task {task_id} submitted");
                    promises.insert(promise);
                }
                let mut any_finished = false;
                loop {
                    match promises.poll_any() {
                        None | Some(PollPromise::Pending) => {
                            break;
                        }
                        Some(PollPromise::Ready((done, name, result))) => {
                            tasks_completed += 1;
                            tasks_running -= 1;
                            debug!(tasks_completed=%tasks_completed, name=name, result=?result, "Task {:?} finished", done);
                            if let Err(e) = result {
                                return Err(FlowBackendError::TaskError {
                                    id: done,
                                    nickname: name,
                                    error: e,
                                });
                            }
                            any_finished = true;

                            ordering.offer(done)?;
                        }
                    }
                }
                if any_finished || new_adds {
                    let (next, overflowed) = step.overflowing_add(1);
                    if overflowed {
                        step = 1;
                    } else {
                        step = next;
                    }
                    debug!(
                        running = tasks_running,
                        completed = tasks_completed,
                        waiting = open_tasks.len(),
                        "incremented step because (task finished: {} || new ads: {})",
                        any_finished,
                        new_adds
                    );
                }
            }

            Ok(())
        })();
        self.listeners.iter().for_each(|i| {
            i.lock().finished(result.as_ref().map(|_| ()));
        });
        result
    }

    pub fn output(&self) -> &FlowBackendOutput {
        &self.output
    }

    pub fn output_mut(&mut self) -> &mut FlowBackendOutput {
        &mut self.output
    }

    #[allow(unused)]
    pub fn input(&self) -> &FlowBackendInput {
        &self.input
    }

    pub fn input_mut(&mut self) -> &mut FlowBackendInput {
        &mut self.input
    }
}

impl Default for FlowBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct FlowBackendInput {
    sender: Sender<Data>,
    receiver: Option<Receiver<Data>>,
}

impl FlowBackendInput {
    /// Send data
    pub fn send(&mut self, d: Data) -> Result<(), FlowBackendError> {
        self.sender.send(d)?;
        Ok(())
    }

    /// Takes the promise
    pub fn take_promise(&mut self) -> Result<RecvPromise<Data>, FlowBackendError> {
        match self.receiver.take() {
            None => Err(FlowBackendError::FlowInputTaskAlreadySet),
            Some(recv) => Ok(RecvPromise::new(recv)),
        }
    }
}

impl Default for FlowBackendInput {
    fn default() -> Self {
        let (tx, rx) = bounded(1);
        Self {
            sender: tx,
            receiver: Some(rx),
        }
    }
}

#[derive(Debug, Default)]
pub struct FlowBackendOutput {
    promise: Option<BoxPromise<'static, Data>>,
}

impl FlowBackendOutput {
    pub fn set_source(&mut self, source: &mut Output) -> Result<(), FlowBackendError> {
        let promise = source.into_promise();
        match &self.promise {
            None => {
                let _ = self.promise.insert(Box::new(promise));
            }
            Some(_) => return Err(FlowBackendError::FlowOutputTaskAlreadySet),
        }

        Ok(())
    }

    pub fn has_source(&self) -> bool {
        self.promise.is_some()
    }
}

impl IntoPromise for &mut FlowBackendOutput {
    type Output = Data;
    type IntoPromise = BoxPromise<'static, Data>;

    fn into_promise(self) -> Self::IntoPromise {
        match self.promise.take() {
            None => {
                panic!("FlowBackend output has no promise")
            }
            Some(promise) => promise,
        }
    }
}

#[derive(Debug)]
struct Weighted<T> {
    t: T,
    dependents: usize,
    step: usize,
}

impl<T> PartialEq for Weighted<T> {
    fn eq(&self, other: &Self) -> bool {
        self.dependents == other.dependents && self.step == other.step
    }
}
impl<T> Eq for Weighted<T> {}
impl<T> PartialOrd for Weighted<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Weighted<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // consider step to denominator and dependents to numerator
        let this = self.dependents.saturating_mul(other.step);
        let other = other.dependents.saturating_mul(self.step);
        this.cmp(&other)
    }
}

#[derive(Debug, Error)]
pub enum FlowBackendError {
    #[error(transparent)]
    TaskOrdering(#[from] JobOrderingError),
    #[error("{nickname}: {error}")]
    TaskError {
        id: JobId,
        nickname: String,
        error: JobError,
    },
    #[error("Only one task can use the flow input")]
    FlowInputTaskAlreadySet,
    #[error("Only one task can use the flow output")]
    FlowOutputTaskAlreadySet,
    #[error(transparent)]
    SendError(#[from] SendError<Data>),
    #[error(transparent)]
    RecvError(#[from] TryRecvError),
}

/// Listens to events produced by the flow backend
pub trait FlowBackendListener: Sync + Send {
    fn started(&mut self);
    /// Run when a task is started
    fn task_started(&mut self, id: JobId, nickname: &str);
    /// Run when a task finished
    fn task_finished(&mut self, id: JobId, nickname: &str, result: Result<(), &JobError>);
    /// Ran once the entire flow is done
    fn finished(&mut self, result: Result<(), &FlowBackendError>);
}

impl Debug for dyn FlowBackendListener {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowBackendListener").finish()
    }
}

#[derive(Clone)]
pub struct BackendFlowGraph {
    tasks: HashSet<JobId>,
    dependencies: HashMap<JobId, HashSet<JobId>>,
    dependents: HashMap<JobId, HashSet<JobId>>,
}

impl BackendFlowGraph {
    pub fn new<'a, I: IntoIterator<Item = &'a BackendJob>>(b_tasks: I) -> Self {
        let mut tasks = HashSet::new();
        let mut dependencies = HashMap::new();
        let mut dependents: HashMap<_, HashSet<_>> = HashMap::new();

        b_tasks.into_iter().for_each(|task| {
            tasks.insert(task.id());
            dependencies.insert(task.id(), task.dependencies().clone());
            for dep in task.dependencies() {
                dependents.entry(*dep).or_default().insert(task.id());
            }
        });

        Self {
            tasks,
            dependencies,
            dependents,
        }
    }
}

impl FlowGraph for BackendFlowGraph {
    type Jobs = HashSet<JobId>;
    type DependsOn = HashSet<JobId>;
    type Dependents = HashSet<JobId>;

    fn jobs(&self) -> Self::Jobs {
        self.tasks.clone()
    }

    fn dependencies(&self, task: &JobId) -> Self::DependsOn {
        self.dependencies.get(task).cloned().unwrap_or_default()
    }

    fn dependents(&self, task: &JobId) -> Self::DependsOn {
        self.dependents.get(task).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::action;
    use crate::backend::job::test_fixtures::MockTaskInput;
    use crate::backend::job::{InputFlavor, ReusableOutput, SingleOutput};
    use test_log::test;

    #[test]
    fn test_can_create_default_worker_pool() {
        let _pool = FlowBackend::default();
    }

    #[test]
    fn test_ordering() {
        let backend = create_backend();

        let (ordering, _) = backend.ordering().expect("failed to get ordering");
        println!("{:#?}", ordering);
    }

    #[test]
    fn test_run() {
        let mut backend = create_backend();

        backend.execute().expect("failed to execute flow");
    }

    fn create_backend() -> FlowBackend {
        let mut backend = FlowBackend::default();
        let mut task1 = BackendJob::new("task1", ReusableOutput::new(), action(|i: i32| i * i));
        let mut task2 = BackendJob::new(
            "task2",
            SingleOutput::new(),
            action(|i: i32| {
                println!("task2: {}", i);
                i.to_string()
            }),
        );
        let mut task3 = BackendJob::new(
            "task3",
            SingleOutput::new(),
            action(|i: i32| {
                println!("task3: {}", i);
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
        task3
            .input_mut()
            .set_source(task1.output_mut())
            .expect("failed to set output for task 3");
        backend.add(task1);
        backend.add(task2);
        backend.add(task3);
        backend
    }

    #[test]
    fn test_weighted_task_id_ordering() {
        let a = Weighted {
            t: (),
            dependents: 1,
            step: 1,
        };
        let b = Weighted {
            t: (),
            dependents: 1,
            step: 2,
        };
        let c = Weighted {
            t: (),
            dependents: 6,
            step: 2,
        };
        let d = Weighted {
            t: (),
            dependents: 6,
            step: 6,
        };
        let e = Weighted {
            t: (),
            dependents: 32,
            step: 64,
        };
        let e = Weighted {
            t: (),
            dependents: 32,
            step: 64,
        };
        assert!(a > b);
        assert!(c > b);
        assert!(c > a);
        assert!(c > d);
        assert!(e < a);
    }
}
