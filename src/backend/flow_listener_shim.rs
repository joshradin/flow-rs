use crate::backend::flow_backend::{FlowBackendError, FlowBackendListener};
use crate::backend::task::TaskError;
use crate::listener::FlowListener;
use crate::TaskId;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct FlowListenerShim {
    flow_listeners: Arc<Mutex<Vec<Box<dyn FlowListener + Send + Sync>>>>,
}

impl FlowListenerShim {
    pub fn new(flow_listeners: &Arc<Mutex<Vec<Box<dyn FlowListener + Send + Sync>>>>) -> Self {
        Self { flow_listeners: flow_listeners.clone() }
    }
}

impl FromIterator<Box<dyn FlowListener + Send + Sync>> for FlowListenerShim {
    fn from_iter<T: IntoIterator<Item=Box<dyn FlowListener + Send + Sync>>>(iter: T) -> Self {
        Self {
            flow_listeners: Arc::new(Mutex::new(iter.into_iter().collect())),
        }
    }
}


impl FlowBackendListener for FlowListenerShim {
    fn started(&mut self) {
        self.flow_listeners.lock().iter_mut().for_each(|f| f.started());
    }

    fn task_started(&mut self, id: TaskId, nickname: &str) {
        self.flow_listeners.lock()
            .iter_mut()
            .for_each(|f| f.task_started(id, nickname));
    }

    fn task_finished(&mut self, id: TaskId, nickname: &str, result: Result<(), &TaskError>) {
        self.flow_listeners.lock()
            .iter_mut()
            .for_each(|f| f.task_finished(id, nickname, result));
    }

    fn finished(&mut self, result: Result<(), &FlowBackendError>) {
        // handled separately
    }
}
