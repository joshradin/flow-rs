//! Actual flow backend

use crate::backend::task::{BackendTask, TaskError, TaskId};
use crate::backend::task_ordering::{TaskOrdering, TaskOrderingError};
use crate::pool::{ThreadPool, WorkerPool};
use crate::promise::{GetPromise, MapPromise, PromiseSet};
use petgraph::visit::NodeRef;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tracing::{debug, debug_span, error_span, Span};

/// Executes flow
#[derive(Debug)]
pub struct FlowBackend<P: WorkerPool = ThreadPool> {
    worker_pool: P,
    listeners: Vec<Box<dyn FlowBackendListener>>,
    tasks: HashMap<TaskId, BackendTask>,
}

impl FlowBackend {
    /// Creates a new flow backend
    pub fn new() -> Self {
        Self::with_worker_pool(ThreadPool::default())
    }
}

impl<P: WorkerPool> FlowBackend<P> {
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

    /// Add a backend listener
    pub fn add_listener<L: FlowBackendListener + 'static>(&mut self, listener: L) {
        self.listeners.push(Box::new(listener));
    }

    /// calculates the task ordering for this flow backend
    fn ordering(&self) -> Result<TaskOrdering, FlowBackendError> {
        Ok(TaskOrdering::new(
            self.tasks.values(),
            self.worker_pool.max_size(),
        )?)
    }

    /// Executes this flow
    pub fn execute(&mut self) -> Result<(), FlowBackendError> {
        error_span!("execute").in_scope(|| {
            debug!(
                "Calculating task ordering for {} tasks...",
                self.tasks.len()
            );
            let instant = Instant::now();
            let ordering = self.ordering()?;
            debug!("Task ordering took: {:.3}ms", instant.elapsed().as_millis());

            // let mut tasks = std::mem::replace(&mut self.tasks, Default::default());
            let listeners = Arc::new(self.listeners.drain(..).collect::<Vec<_>>());

            for (step, task_ids) in ordering.into_iter().enumerate() {
                debug_span!("step", step = step).in_scope(|| -> Result<(), FlowBackendError> {
                    debug!("executing {} tasks: {:?}", task_ids.len(), task_ids);
                    let mut promises = PromiseSet::new();
                    for task in task_ids {
                        let mut task = self.tasks.remove(&task).expect("Task not found");
                        let listeners = listeners.clone();
                        let promise = self.worker_pool.submit(move || {
                            debug!("Starting execution of task {:?}", task);
                            listeners.iter().for_each(|i| {
                                i.task_started(task.id(), task.nickname());
                            });
                            let r = task.run();
                            listeners.iter().for_each(|i| {
                                i.task_finished(task.id(), task.nickname(), &r);
                            });
                            r
                        });
                        promises.insert(promise);
                    }
                    for result in promises.get() {
                        result?;
                    }
                    Ok(())
                })?;
            }

            Ok(())
        })
    }
}

impl<P: WorkerPool> FlowBackend<P> {
    /// Creates the new flow backend with the given worker pool
    pub fn with_worker_pool(worker_pool: P) -> Self {
        Self {
            worker_pool,
            listeners: vec![],
            tasks: HashMap::new(),
        }
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
    #[error(transparent)]
    TaskError(#[from] TaskError),
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
            .input()
            .set_source(MockTaskInput(12))
            .expect("failed to set input");
        task2
            .input()
            .set_source(task1.output())
            .expect("failed to set output for task 2");
        task3
            .input()
            .set_source(task1.output())
            .expect("failed to set output for task 3");
        backend.add(task1);
        backend.add(task2);
        backend.add(task3);
        backend
    }
}
