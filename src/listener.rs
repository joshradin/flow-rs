//! Listens to [`Flow`](crate::Flow) events

use crate::{FlowError, TaskError, TaskId};
use std::any::Any;

/// Listens to a [`Flow`] instance.
pub trait FlowListener {
    /// Ran as a flow starts
    fn started(&mut self) {}

    /// A task started
    fn task_started(&mut self, id: TaskId, nickname: &str) {}

    /// A task finished
    fn task_finished(&mut self, id: TaskId, nickname: &str, result: Result<(), &TaskError>) {}
    /// Ran once the entire flow is done
    fn finished(&mut self, result: Result<(), &FlowError>) {}
}

pub struct PrintTaskListener;

impl FlowListener for PrintTaskListener {
    fn task_started(&mut self, id: TaskId, nickname: &str) {
        println!("== started: {} ==", nickname);
    }

    fn task_finished(&mut self, id: TaskId, nickname: &str, result: Result<(), &TaskError>) {
        println!("== finished: {}. {result:?} ==", nickname);
    }
}

