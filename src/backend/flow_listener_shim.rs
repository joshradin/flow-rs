use crate::JobId;
use crate::backend::flow_backend::{FlowBackendError, FlowBackendListener};
use crate::backend::job::JobError;
use crate::listener::FlowListener;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct FlowListenerShim {
    flow_listeners: Arc<Mutex<Vec<Box<dyn FlowListener + Send>>>>,
}

impl FlowListenerShim {
    pub fn new(flow_listeners: &Arc<Mutex<Vec<Box<dyn FlowListener + Send>>>>) -> Self {
        Self {
            flow_listeners: flow_listeners.clone(),
        }
    }
}

impl FromIterator<Box<dyn FlowListener + Send>> for FlowListenerShim {
    fn from_iter<T: IntoIterator<Item = Box<dyn FlowListener + Send>>>(iter: T) -> Self {
        Self {
            flow_listeners: Arc::new(Mutex::new(iter.into_iter().collect())),
        }
    }
}

impl FlowBackendListener for FlowListenerShim {
    fn started(&mut self) {
        self.flow_listeners
            .lock()
            .iter_mut()
            .for_each(|f| f.started());
    }

    fn task_started(&mut self, id: JobId, nickname: &str) {
        self.flow_listeners
            .lock()
            .iter_mut()
            .for_each(|f| f.task_started(id, nickname));
    }

    fn task_finished(&mut self, id: JobId, nickname: &str, result: Result<(), &JobError>) {
        self.flow_listeners
            .lock()
            .iter_mut()
            .for_each(|f| f.task_finished(id, nickname, result));
    }

    fn finished(&mut self, _result: Result<(), &FlowBackendError>) {
        // handled separately
    }
}
