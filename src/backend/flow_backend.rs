//! Actual flow backend

use crate::backend::task::{BackendTask, TaskId};
use crate::backend::task_ordering::{TaskOrdering, TaskOrderingError};
use crate::pool::{ThreadPool, WorkerPool};
use std::collections::HashMap;
use thiserror::Error;

/// Executes flow
#[derive(Debug)]
pub struct FlowBackend<P: WorkerPool = ThreadPool> {
    worker_pool: P,
    tasks: HashMap<TaskId, BackendTask>,
}

impl FlowBackend {
    /// Creates a new flow backend
    pub fn new() -> Self {
        Self::with_worker_pool(ThreadPool::default())
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

    /// calculates the task ordering for this flow backend
    fn ordering(&self) -> Result<TaskOrdering, FlowBackendError> {
        Ok(TaskOrdering::new(
            self.tasks.values(),
            self.worker_pool.max_size(),
        )?)
    }
}

impl<P: WorkerPool> FlowBackend<P> {
    /// Creates the new flow backend with the given worker pool
    pub fn with_worker_pool(worker_pool: P) -> Self {
        Self {
            worker_pool,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::action;
    use crate::backend::task::test_fixtures::MockTaskInput;
    use crate::backend::task::{InputFlavor, ReusableOutput, SingleOutput};

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
