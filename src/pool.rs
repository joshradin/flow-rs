//! The worker pool used by flow-rs

use crate::promise::{GetPromise, Just, PollPromise, Promise};
use crossbeam::atomic::AtomicCell;
use crossbeam::channel::{bounded, unbounded, Receiver, Sender, TryRecvError};
use parking_lot::Mutex;
use std::any::Any;
use std::collections::VecDeque;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant};
use std::{panic, thread};
use tracing::{debug, error_span, event, info, instrument, trace, warn, Level, Span};


/// Pool trait
pub trait WorkerPool {
    fn max_size(&self) -> usize;

    /// Submits some work into the worker pool
    fn submit<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
        &self,
        f: F,
    ) -> impl Promise<Output=T> + 'static;
}

#[derive(Debug)]
struct WorkerThread {
    id: ThreadId,
    handle: JoinHandle<()>,
    command: Sender<WorkerThreadCommand>,
    state: Arc<AtomicCell<WorkerThreadState>>,
}

impl Display for WorkerThread {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerThread")
         .field("id", &self.id)
         .field("state", &self.state.load())
         .finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum WorkerThreadState {
    Waiting,
    Running,
    Idle(Instant),
    Panicked,
}

enum WorkerThreadCommand {
    Stop,
    Execute(Box<dyn FnMut() + Send>),
}

impl WorkerThread {
    fn spawn() -> Self {
        let state = Arc::new(AtomicCell::new(WorkerThreadState::Idle(Instant::now())));
        let (sender, receiver) = unbounded::<WorkerThreadCommand>();

        let (id_tx, id_rx) = bounded::<ThreadId>(1);

        let handle = {
            let state = state.clone();
            thread::spawn(move || {
                error_span!("worker-thread", thread=?thread::current().id()).in_scope(|| {
                    id_tx.send(thread::current().id()).unwrap();
                    loop {
                        let Ok(recv) = receiver.recv() else {
                            warn!("worker thread disconnected");
                            break;
                        };

                        match recv {
                            WorkerThreadCommand::Stop => {
                                debug!("stop command received");
                                break;
                            }
                            WorkerThreadCommand::Execute(mut runnable) => {
                                debug!("execute command received");
                                state.store(WorkerThreadState::Running);
                                debug!("running runnable...");
                                let result = catch_unwind(AssertUnwindSafe(|| runnable()));
                                debug!("runnable finished. result is okay={}", result.is_ok());
                                match result {
                                    Ok(_) => {
                                        state.store(WorkerThreadState::Idle(Instant::now()));
                                    }
                                    Err(err) => {
                                        state.store(WorkerThreadState::Panicked);
                                        resume_unwind(err);
                                    }
                                }
                            }
                        }
                    }
                })
            })
        };

        Self {
            id: id_rx.recv().unwrap(),
            handle,
            command: sender,
            state,
        }
    }

    fn join(self) -> Result<(), Box<dyn Any + Send>> {
        self.command
            .send(WorkerThreadCommand::Stop)
            .expect("failed to send stop command");
        let result = self.handle.join();

        let inner = self.state.load();

        match inner {
            WorkerThreadState::Running
            | WorkerThreadState::Idle(_)
            | WorkerThreadState::Waiting => Ok(()),
            WorkerThreadState::Panicked => {
                let e = result.err().unwrap();
                Err(e)
            }
        }
    }

    /// gets this state
    fn state(&self) -> WorkerThreadState {
        self.state.load()
    }
}

struct InnerThreadPoolExecutor {
    queue: VecDeque<Box<dyn FnMut() + Send>>,
    core_size: usize,
    max_size: usize,
    keep_alive_timeout: Duration,
    threads: Vec<WorkerThread>,
    run_loop: OnceLock<JoinHandle<()>>,
    stop_token_sender: OnceLock<Sender<()>>,
}

impl Drop for InnerThreadPoolExecutor {
    fn drop(&mut self) {
        if let Some(handle) = self.run_loop.take() {
            let stop_token_sender = self.stop_token_sender.take().unwrap();
            let _ = stop_token_sender.send(());
            handle.join().unwrap();
        }
    }
}

impl InnerThreadPoolExecutor {
    fn new(core_size: usize, max_size: usize, keep_alive_timeout: Duration) -> Self {
        Self {
            queue: Default::default(),
            core_size,
            max_size,
            keep_alive_timeout,
            threads: vec![],
            run_loop: Default::default(),
            stop_token_sender: Default::default(),
        }
    }

    fn submit_callable<T: Send + 'static, C>(&mut self, c: C) -> ThreadPoolPromise<T>
    where
        C: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = bounded::<Result<T, Box<dyn Any + Send>>>(1);
        let mut opt = Some(c);
        let runnable: Box<dyn FnMut() + Send> = Box::new(move || {
            let Some(callable) = opt.take() else {
                panic!("this runnable was already called");
            };
            let ret = panic::catch_unwind(AssertUnwindSafe(|| callable()));
            sender.send(ret).expect("failed to send result");
        });

        trace!("submitting runnable...");
        self.queue.push_back(runnable);

        ThreadPoolPromise {
            receiver,
            _marker: Default::default(),
        }
    }

    fn idle(&self) -> usize {
        self.threads
            .iter()
            .filter(|w| matches!(w.state(), WorkerThreadState::Idle(_) ))
            .count()
    }

    fn take_next_idle(&mut self) -> Option<WorkerThread> {
        let position = self.threads.iter().position(|w| match w.state() {
            WorkerThreadState::Idle(idle_start) => idle_start.elapsed() > self.keep_alive_timeout,
            _ => false,
        })?;
        Some(self.threads.swap_remove(position))
    }

    fn get_next_idle(&mut self) -> Option<&WorkerThread> {
        match self
            .threads
            .iter()
            .position(|w| matches!(w.state(), WorkerThreadState::Idle(_)))
        {
            None => {}
            Some(idle) => return Some(&self.threads[idle]),
        }
        if self.workers() < self.max_size {
            let i = self.threads.len();
            self.threads.push(WorkerThread::spawn());
            Some(&self.threads[i])
        } else {
            None
        }
    }

    fn running(&self) -> usize {
        self.threads
            .iter()
            .filter(|w| matches!(w.state(), WorkerThreadState::Running| WorkerThreadState::Waiting))
            .count()
    }

    fn workers(&self) -> usize {
        self.threads.len()
    }

    /// Steps this executor
    #[instrument(skip_all, fields(threads=%self.threads.len(), running=%self.running(), idle=%self.idle()
    ))]
    fn step(&mut self) {
        self.stop_non_core_idle();
        while self.running() < self.max_size && !self.queue.is_empty() {
            let runnable = self.queue.pop_front().unwrap();
            if let Some(worker) = self.get_next_idle() {
                worker.state.store(WorkerThreadState::Waiting);
                worker
                    .command
                    .send(WorkerThreadCommand::Execute(runnable))
                    .expect("failed to send command to worker");
                debug!("Started runnable");
            } else {
                self.queue.push_back(runnable);
            }
        }
    }

    /// stops non-core idle threads that have been running for longer than timeout
    fn stop_non_core_idle(&mut self) {
        // trace!("stopping non-core idle threads");
        while self.idle() > 0 && self.workers() > self.core_size {
            let Some(next) = self.take_next_idle() else {
                break;
            };
            trace!("joining worker thread: {next:}");
            next.join().expect("failed to join worker thread");
        }
    }
}

/// A thread pool executor. Can be cloned freely to get extra handles to this executor.
#[derive(Clone)]
pub struct ThreadPool {
    core_size: usize,
    max_size: usize,
    keep_alive_timeout: Duration,
    inner: Arc<Mutex<InnerThreadPoolExecutor>>,
}

impl ThreadPool {
    /// Creates a new thread pool executor
    pub fn new(core_size: usize, max_size: usize, keep_alive_timeout: Duration) -> Self {
        let inner = InnerThreadPoolExecutor::new(core_size, max_size, keep_alive_timeout);
        let inner = Arc::new(Mutex::new(inner));
        let mut executor = Self { core_size, max_size, keep_alive_timeout, inner };
        Self::start(&mut executor);
        executor
    }

    fn start(this: &mut Self) {
        let inner = this.inner.clone();
        trace!(core=this.core_size, max=this.max_size, keep_alive=%this.keep_alive_timeout.as_secs_f32() ,"starting thread pool");
        let (sender, receiver) = bounded::<()>(0);
        let handle = {
            let inner = inner.clone();
            thread::spawn(move || {
                loop {
                    match receiver.try_recv() {
                        Ok(()) => {
                            break;
                        }
                        Err(TryRecvError::Disconnected) => {
                            break;
                        }
                        _ => {}
                    }
                    let mut locked = inner.lock();
                    locked.step();
                }
            })
        };
        let lock = inner.lock();
        lock.run_loop.set(handle).unwrap();
        lock.stop_token_sender.set(sender).unwrap()
    }
}

impl Default for ThreadPool {
    fn default() -> Self {
        Self::new(0, num_cpus::get(), Duration::from_millis(1000))
    }
}

impl Debug for ThreadPool {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadPool")
            .field("core", &self.core_size)
            .field("max", &self.max_size)
            .field("keep_alive", &self.keep_alive_timeout.as_secs_f32())
            .finish()
    }
}

impl WorkerPool for ThreadPool {
    fn max_size(&self) -> usize {
        self.inner.lock().max_size
    }

    fn submit<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
        &self,
        f: F,
    ) -> impl Promise<Output=T> + 'static {
        let mut inner = self.inner.lock();
        inner.submit_callable(f)
    }
}

/// A thread pool promise
pub struct ThreadPoolPromise<T: Send> {
    receiver: Receiver<Result<T, Box<dyn Any + Send>>>,
    _marker: PhantomData<T>,
}

/// A thread pool promise
pub struct ScopedThreadPoolPromise<T: Send> {
    receiver: Receiver<Result<T, Box<dyn Any + Send>>>,
    _marker: PhantomData<T>,
}

impl<T: Send> Promise for ThreadPoolPromise<T> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self.receiver.try_recv() {
            Ok(Ok(t)) => PollPromise::Ready(t),
            Ok(Err(e)) => resume_unwind(e),
            Err(TryRecvError::Disconnected) => {
                panic!("receiver channel is disconnected")
            }
            Err(TryRecvError::Empty) => PollPromise::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promise::PromiseSet;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn test_thread_pool_executor() {
        let pool = ThreadPool::default();
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
        let mut pool = ThreadPool::new(4, 4, Duration::ZERO);
        let count = Arc::new(AtomicUsize::new(0));
        let mut promises = vec![];

        for idx in 0..128 {
            let count = count.clone();
            let promise = pool.submit(move || {
                let i = count.fetch_add(1, Ordering::SeqCst);
                println!("job #{:3}: {} -> {}", idx, i, i + 1);
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
    fn test_thread_pool_return_value() {
        let mut pool = ThreadPool::default();
        let result: Result<_, Infallible> = pool.submit(|| Ok(42)).get();
        assert_eq!(result.unwrap(), 42);
    }
}
