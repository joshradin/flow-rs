//! Actual flow backend

use crate::backend::task::{BackendTask, TaskError, TaskId};
use crate::pool::{ThreadPool, WorkerPool};
use crate::promise::{GetPromise, MapPromise, PollPromise, PromiseSet};
use crate::task_ordering::{DefaultTaskOrderer, FlowGraph};
use crate::task_ordering::{TaskOrderer, TaskOrdering, TaskOrderingError};
use petgraph::visit::NodeRef;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tracing::{debug, debug_span, error_span, info, Span};

/// Executes flow
#[derive(Debug)]
pub struct FlowBackend<T: TaskOrderer = DefaultTaskOrderer, P: WorkerPool = ThreadPool> {
    worker_pool: P,
    orderer: T,
    listeners: Vec<Box<dyn FlowBackendListener>>,
    tasks: HashMap<TaskId, BackendTask>,
}

impl FlowBackend {
    /// Creates a new flow backend
    pub fn new() -> Self {
        Self::with_worker_pool(ThreadPool::default())
    }
}

impl<P: WorkerPool> FlowBackend<DefaultTaskOrderer, P> {
    /// Creates the new flow backend with the given worker pool
    pub fn with_worker_pool(worker_pool: P) -> Self {
        Self::with_task_orderer_and_worker_pool(DefaultTaskOrderer::default(), worker_pool)
    }
}

impl<T: TaskOrderer, P: WorkerPool> FlowBackend<T, P> {
    /// Creates the new flow backend with the given worker pool
    pub fn with_task_orderer_and_worker_pool(task_orderer: T, worker_pool: P) -> Self {
        Self {
            worker_pool,
            orderer: task_orderer,
            listeners: vec![],
            tasks: HashMap::new(),
        }
    }

    /// Add a task to this flow backend
    pub fn add(&mut self, task: BackendTask) {
        self.tasks.insert(task.id(), task);
    }

    /// Gets the task by id
    pub fn get(&self, task_id: TaskId) -> Option<&BackendTask> {
        self.tasks.get(&task_id)
    }

    /// Gets a mutable reference to a task by id
    pub fn get_mut(&mut self, task_id: TaskId) -> Option<&mut BackendTask> {
        self.tasks.get_mut(&task_id)
    }

    /// Gets two mutable reference to tasks by id, if the ids are disjoint
    pub fn get_mut2(
        &mut self,
        task_id_1: TaskId,
        task_id_2: TaskId,
    ) -> Option<(&mut BackendTask, &mut BackendTask)> {
        let [a, b] = self.tasks.get_disjoint_mut([&task_id_1, &task_id_2]);
        a.zip(b)
    }

    /// Gets `N` mutable reference to tasks by id, if the ids are disjoint
    pub fn get_mut_disjoint<const N: usize>(
        &mut self,
        task_ids: [TaskId; N],
    ) -> Option<[&mut BackendTask; N]> {
        let references = task_ids.each_ref();
        let tasks = self.tasks.get_disjoint_mut(references);
        let mut ret: Vec<&mut BackendTask> = vec![];

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

        <[&mut BackendTask; N]>::try_from(ret).ok()
    }

    /// Add a backend listener
    pub fn add_listener<L: FlowBackendListener + 'static>(&mut self, listener: L) {
        self.listeners.push(Box::new(listener));
    }

    /// calculates the task ordering for this flow backend
    fn ordering(&self) -> Result<T::TaskOrdering, FlowBackendError> {
        let flow_graph = BackendFlowGraph::new(self.tasks.values());
        let ordering = self.orderer.create_ordering(flow_graph, self.worker_pool.max_size())?;
        Ok(ordering)
    }

    /// Executes this flow
    pub fn execute(&mut self) -> Result<(), FlowBackendError> {
        error_span!("execute").in_scope(|| {
            debug!(
                "Calculating task ordering for {} tasks...",
                self.tasks.len()
            );
            let instant = Instant::now();
            let mut ordering = self.ordering()?;
            debug!("Task ordering took: {:.3}ms", instant.elapsed().as_millis());

            // let mut tasks = std::mem::replace(&mut self.tasks, Default::default());
            let listeners = Arc::new(self.listeners.drain(..).collect::<Vec<_>>());
            let mut promises = PromiseSet::new();

            while !ordering.empty() {
                let task_ids = ordering.poll()?;
                for task_id in task_ids {
                    let mut task = self.tasks.remove(&task_id).expect("Task not found");
                    let name =task.nickname().to_string();
                    let listeners = listeners.clone();
                    let promise = self.worker_pool.submit(move || {
                        info!("Task {:?} started", task.nickname());
                        listeners.iter().for_each(|i| {
                            i.task_started(task.id(), task.nickname());
                        });
                        let r = task.run();
                        listeners.iter().for_each(|i| {
                            i.task_finished(task.id(), task.nickname(), &r);
                        });
                        (task_id, name, r)
                    });
                    promises.insert(promise);
                }
                loop {
                    match promises.poll_any() {
                        None |  Some(PollPromise::Pending) => {
                            break;
                        }
                        Some(PollPromise::Ready((done, name, result))) => {
                            info!("Task {:?} finished: {:?}", name, result);
                            if let Err(e) = result {
                                return Err(FlowBackendError::TaskError {
                                    id: done,
                                    nickname: name,
                                    error: e,
                                })
                            }

                            ordering.offer(done)?;
                        }
                    }
                }
            }

            // for (step, task_ids) in ordering.into_iter().enumerate() {
            //     debug_span!("step", step = step).in_scope(|| -> Result<(), FlowBackendError> {
            //         debug!("executing {} tasks: {:?}", task_ids.len(), task_ids);
            //         let mut promises = PromiseSet::new();
            //         for task_id in task_ids {
            //             let mut task = self.tasks.remove(&task_id).expect("Task not found");
            //             let name = task.nickname().to_string();
            //             let listeners = listeners.clone();
            //             let promise = self.worker_pool.submit(move || {
            //                 debug!("Starting execution of task {:?}", task);
            //                 listeners.iter().for_each(|i| {
            //                     i.task_started(task.id(), task.nickname());
            //                 });
            //                 let r = task.run();
            //                 listeners.iter().for_each(|i| {
            //                     i.task_finished(task.id(), task.nickname(), &r);
            //                 });
            //                 (task_id, name, r)
            //             });
            //             promises.insert(promise);
            //         }
            //         for (id, name, result)in promises.get() {
            //             if let Err(e) = result {
            //                 return Err(FlowBackendError::TaskError {
            //                     id,
            //                     nickname: name,
            //                     error: e,
            //                 })
            //             }
            //         }
            //         Ok(())
            //     })?;
            // }

            Ok(())
        })
    }
}

impl Default for FlowBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum FlowBackendError {
    #[error(transparent)]
    TaskOrdering(#[from] TaskOrderingError),
    #[error("{nickname}: {error}")]
    TaskError {
        id: TaskId,
        nickname: String,
        error: TaskError,
    },
}

/// Listens to events produced by the flow backend
pub trait FlowBackendListener: Sync + Send {
    /// Run when a task is started
    fn task_started(&self, id: TaskId, nickname: &str);
    /// Run when a task finished
    fn task_finished(&self, id: TaskId, nickname: &str, result: &Result<(), TaskError>);
}

impl Debug for dyn FlowBackendListener {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowBackendListener").finish()
    }
}

pub struct BackendFlowGraph {
    tasks: HashSet<TaskId>,
    dependencies: HashMap<TaskId, HashSet<TaskId>>,
    dependents: HashMap<TaskId, HashSet<TaskId>>,
}

impl BackendFlowGraph {
    pub fn new<'a, I: IntoIterator<Item = &'a BackendTask>>(b_tasks: I) -> Self {
        let mut tasks = HashSet::new();
        let mut dependencies = HashMap::new();
        let mut dependents: HashMap<_, HashSet<_>> = HashMap::new();

        b_tasks.into_iter().for_each(|task| {
            tasks.insert(task.id());
            dependencies.insert(task.id(), task.dependencies().clone());
            for dep in task.dependencies() {
                dependents.entry(dep.clone()).or_default().insert(task.id());
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
    type Tasks = HashSet<TaskId>;
    type DependsOn = HashSet<TaskId>;
    type Dependents = HashSet<TaskId>;

    fn tasks(&self) -> Self::Tasks {
        self.tasks.clone()
    }

    fn dependencies(&self, task: &TaskId) -> Self::DependsOn {
        self.dependencies[task].clone()
    }

    fn dependents(&self, task: &TaskId) -> Self::DependsOn {
        self.dependencies[task].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::action;
    use crate::backend::task::test_fixtures::MockTaskInput;
    use crate::backend::task::{InputFlavor, ReusableOutput, SingleOutput};
    use test_log::test;

    #[test]
    fn test_can_create_default_worker_pool() {
        let _pool = FlowBackend::default();
    }

    #[test]
    fn test_ordering() {
        let backend = create_backend();

        let ordering = backend.ordering().expect("failed to get ordering");
        println!("{:#?}", ordering);
    }

    #[test]
    fn test_run() {
        let mut backend = create_backend();

        backend.execute().expect("failed to execute flow");
    }

    fn create_backend() -> FlowBackend {
        let mut backend = FlowBackend::default();
        let mut task1 = BackendTask::new(
            "task1",
            InputFlavor::Single,
            ReusableOutput::new(),
            action(|i: i32| i * i),
        );
        let mut task2 = BackendTask::new(
            "task2",
            InputFlavor::Single,
            SingleOutput::new(),
            action(|i: i32| {
                println!("task2: {}", i);
                i.to_string()
            }),
        );
        let mut task3 = BackendTask::new(
            "task3",
            InputFlavor::Single,
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
}
