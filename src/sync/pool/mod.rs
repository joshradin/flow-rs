//! The worker pool used by jobflow.

use crate::backend::recv_promise::RecvPromise;
use crate::sync::pool::inner_thread_pool::InnerThreadPool;
use crate::sync::pool::settings::ThreadPoolSettings;
use crate::sync::promise::{PollPromise, Promise};
use crossbeam::channel::bounded;
use std::sync::Arc;
use std::time::Duration;

mod inner_thread_pool;
mod settings;

/// Pool trait
pub trait WorkerPool {
    fn max_size(&self) -> usize;

    /// Active threads
    fn active(&self) -> usize;
    /// Running threads (have tasks)
    fn running(&self) -> usize;

    /// Submits some work into the worker pool
    fn submit<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
        &self,
        f: F,
    ) -> impl Promise<Output = T> + use<Self, F, T>;
}

/// Default [`WorkerPool`] implementation.
#[derive(Clone)]
pub struct FlowThreadPool {
    settings: ThreadPoolSettings,
    inner: Arc<InnerThreadPool>,
}

impl Default for FlowThreadPool {
    fn default() -> Self {
        Self::with_settings(ThreadPoolSettings::default())
    }
}

impl FlowThreadPool {
    /// Create a new thread pool
    pub fn new(core_size: usize, max_size: usize, timeout: Duration) -> Self {
        FlowThreadPool::with_settings(ThreadPoolSettings::new(core_size, max_size, timeout))
    }

    fn with_settings(settings: ThreadPoolSettings) -> Self {
        Self {
            settings,
            inner: InnerThreadPool::new(settings),
        }
    }
}

impl WorkerPool for FlowThreadPool {
    // type Promise<T: Send + 'static> = ;

    fn max_size(&self) -> usize {
        self.settings.max_size()
    }

    fn active(&self) -> usize {
        self.inner.active()
    }

    fn running(&self) -> usize {
        self.inner.running()
    }

    fn submit<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
        &self,
        f: F,
    ) -> impl Promise<Output = T> + use<F, T> {
        let (tx, rx) = bounded::<T>(1);
        let recv_promise = RecvPromise::new(rx);
        self.inner
            .submit(move || {
                let t = f();
                let _ = tx.try_send(t);
            })
            .expect("failed to submit promise");
        FlowThreadPoolPromise {
            wrapped: recv_promise,
        }
    }
}

pub struct FlowThreadPoolPromise<T> {
    wrapped: RecvPromise<T>,
}

impl<T: Send> Promise for FlowThreadPoolPromise<T> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        self.wrapped.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::promise::{GetPromise, PromiseSet};
    use static_assertions::assert_impl_all;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread::yield_now;
    use tracing::{info, info_span};

    assert_impl_all!(FlowThreadPool: Sync);
    #[test]
    fn test_thread_pool_executor() {
        let pool = FlowThreadPool::default();
        let barrier = Arc::new(Barrier::new(2));
        let mut promises = vec![];
        for _ in 0..2 {
            let barrier = barrier.clone();
            let promise = pool.submit(move || {
                barrier.wait();
            });
            promises.push(promise);
        }
        for promise in promises {
            promise.get();
        }
    }

    #[test]
    fn test_thread_pool_more_than_core_count() {
        let span = info_span!("thread pool");
        let pool = FlowThreadPool::new(4, 4, Duration::ZERO);
        let count = Arc::new(AtomicUsize::new(0));
        let mut promises = vec![];

        let _enter = span.enter();
        for idx in 0..128 {
            let count = count.clone();
            let promise = pool.submit(move || {
                let i = count.fetch_add(1, Ordering::SeqCst);
                info!("job #{:3} (): {} -> {}", idx, i, i + 1);
            });
            promises.push(promise);
        }
        let _ = PromiseSet::from_iter(promises)
            .get()
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(count.load(Ordering::SeqCst), 128);
    }

    #[test]
    fn test_thread_pool_counts() {
        let thread_pool = FlowThreadPool::new(1, 1, Duration::ZERO);
        assert_eq!(thread_pool.running(), 0);
        assert_eq!(thread_pool.active(), 0);
        let barrier = Arc::new(Barrier::new(2));
        let promise = thread_pool.submit({
            let barrier = barrier.clone();
            move || {
                barrier.wait();
            }
        });
        while thread_pool.running() < 1 {
            yield_now()
        }
        assert_eq!(thread_pool.running(), 1);
        assert_eq!(thread_pool.active(), 1);
        barrier.wait();
        promise.get();
        while thread_pool.running() > 0 {
            yield_now()
        }
        assert_eq!(thread_pool.running(), 0);
        assert_eq!(thread_pool.active(), 1);
    }

    #[test]
    fn test_thread_pool_return_value() {
        let pool = FlowThreadPool::default();
        let result: Result<_, Infallible> = pool.submit(|| Ok(42)).get();
        assert_eq!(result.unwrap(), 42);
    }
}
