//! Listens to [`Flow`](crate::Flow) events

use crate::{FlowError, JobError, JobId};

/// Listens to a [`Flow`] instance.
pub trait FlowListener {
    /// Ran as a flow starts
    fn started(&mut self) {}

    /// A task started
    fn task_started(&mut self, id: JobId, nickname: &str) {
        let _id = id;
        let _nickname = nickname;
    }

    /// A task finished
    fn task_finished(&mut self, id: JobId, nickname: &str, result: Result<(), &JobError>) {
        let _id  = id;
        let _nickname = nickname;
        let _result = result;
    }
    /// Ran once the entire flow is done
    fn finished(&mut self, result: Result<(), &FlowError>) {
        let _ = result;
    }
}

pub struct PrintTaskListener;

impl FlowListener for PrintTaskListener {
    fn task_started(&mut self, _id: JobId, nickname: &str) {
        println!("== started: {} ==", nickname);
    }

    fn task_finished(&mut self, _id: JobId, nickname: &str, result: Result<(), &JobError>) {
        println!("== finished: {}. {result:?} ==", nickname);
    }
}

