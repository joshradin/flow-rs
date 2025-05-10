use crate::backend::task::Data;
use crate::promise::{IntoPromise, PollPromise, Promise, PromiseSet};

/// The backend funnel implementation.
///
/// Funnels accepts inputs from tasks, and produces one output
pub struct BackendFunnel {
    promises: PromiseSet<'static, Data>,
}

impl BackendFunnel {
    /// Creates a new backend funnel
    pub fn new() -> Self {
        Self {
            promises: PromiseSet::new(),
        }
    }

    /// Inserts a promise into this backend funnel
    pub fn insert<P: IntoPromise<Output = Data> + 'static>(&mut self, p: P) {
        self.promises.insert(p.into_promise());
    }
}

impl IntoPromise for BackendFunnel {
    type Output = Vec<Data>;
    type IntoPromise = BackendFunnelPromise;

    fn into_promise(self) -> Self::IntoPromise {
        BackendFunnelPromise { set: self.promises }
    }
}

/// Backend future promise
pub struct BackendFunnelPromise {
    set: PromiseSet<'static, Data>,
}

impl Promise for BackendFunnelPromise {
    type Output = Vec<Data>;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        self.set.poll()
    }
}
