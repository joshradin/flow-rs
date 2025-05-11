use crate::backend::task::Data;
use crate::promise::MapPromise;
use crate::promise::{IntoPromise, PollPromise, Promise, PromiseSet};

/// The backend funnel implementation.
///
/// Funnels accepts inputs from tasks, and produces one output
#[derive(Debug)]
pub struct BackendFunnel {
    promises: PromiseSet<'static, Vec<Data>>,
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
        self.promises.insert(p.into_promise().map(|d| vec![d]));
    }

    /// Inserts a promise into this backend funnel
    pub fn insert_iter<
        I: IntoIterator<Item = Data> + 'static,
        P: IntoPromise<Output = I> + 'static,
    >(
        &mut self,
        p: P,
    ) {
        self.promises
            .insert(p.into_promise().map(|i: I| i.into_iter().collect()));
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
    set: PromiseSet<'static, Vec<Data>>,
}

impl Promise for BackendFunnelPromise {
    type Output = Vec<Data>;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self.set.poll() {
            PollPromise::Ready(ready) => PollPromise::Ready(ready.into_iter().flatten().collect()),
            PollPromise::Pending => PollPromise::Pending,
        }
    }
}
