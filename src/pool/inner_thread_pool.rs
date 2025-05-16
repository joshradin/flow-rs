use crate::pool::settings::ThreadPoolSettings;
use crossbeam::atomic::AtomicCell;
use parking_lot::{Mutex, RwLock};
use petgraph::visit::NodeRef;
use static_assertions::assert_impl_all;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::num::NonZero;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering::{AcqRel, Relaxed};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{io, thread};
use tracing::{error_span, info, trace, Span};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct WorkerThreadId(NonZero<usize>);

impl Debug for WorkerThreadId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.0.get(), f)
    }
}

impl Display for WorkerThreadId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0.get(), f)
    }
}


impl WorkerThreadId {
    fn new(id: usize) -> Self {
        Self(NonZero::new(id).unwrap())
    }
}

pub struct ThreadPoolTask(Box<dyn FnOnce() + Send>);

impl Debug for ThreadPoolTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadPoolTask").finish()
    }
}

assert_impl_all!(ThreadPoolTask: Send);

type Stealer = crossbeam::deque::Stealer<ThreadPoolTask>;
type Steal = crossbeam::deque::Steal<ThreadPoolTask>;
type Injector = crossbeam::deque::Injector<ThreadPoolTask>;
type Worker = crossbeam::deque::Worker<ThreadPoolTask>;

impl ThreadPoolTask {
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        ThreadPoolTask(Box::new(f))
    }
}

pub struct InnerThreadPool {
    parent: Span,
    global: Arc<Injector>,
    settings: ThreadPoolSettings,
    registry: RwLock<WorkerThreadRegistry>,
    handles: Mutex<Vec<WorkerThreadHandle>>,
    next_id: AtomicUsize,
    active_workers: AtomicUsize,
    active_tasks: AtomicUsize,
}

impl Drop for InnerThreadPool {
    fn drop(&mut self) {
        let mut locked = self.handles.lock();
        locked.iter().for_each(|handle| {
            let _ = handle.cont.compare_exchange(true, false, AcqRel, Relaxed);
        })
    }
}

impl InnerThreadPool {
    /// Creates a new [`InnerThreadPool`] with the given settings
    pub fn new(settings: ThreadPoolSettings) -> Arc<Self> {
        let span = Span::current();
        let injector = Arc::new(Injector::new());
        Arc::new(Self {
            parent: span,
            global: injector.clone(),
            settings,
            registry: RwLock::new(WorkerThreadRegistry {
                active: Default::default(),
                stealer_map: Default::default(),
            }),
            next_id: AtomicUsize::new(1),
            handles: Default::default(),
            active_workers: AtomicUsize::new(0),
            active_tasks: AtomicUsize::new(0),
        })
    }

    pub fn active(&self) -> usize {
        self.active_workers.load(Relaxed)
    }

    fn spawn_worker_if_needed(self: &Arc<Self>) -> io::Result<()> {
        let task_count = self.active_tasks.load(Relaxed);
        let worker_count = self.active_workers.load(Relaxed);

        loop {
            if worker_count < self.settings.max_size() {
                if task_count > worker_count {
                    match self.active_workers.compare_exchange(
                        worker_count,
                        worker_count + 1,
                        AcqRel,
                        Relaxed,
                    ) {
                        Ok(_) => {
                            let worker = self.spawn_worker()?;
                            trace!("spawned worker {:?}", worker.id);
                            let mut lock = self.handles.lock();
                            lock.push(worker);
                            break;
                        }
                        Err(_) => {}
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(())
    }

    fn spawn_worker(self: &Arc<Self>) -> io::Result<WorkerThreadHandle> {
        WorkerThread::spawn(self, self.settings.keep_alive_timeout())
    }

    /// Checks if the given id should stop
    fn should_stop(&self) -> bool {
        let current_workers = self.active_workers.load(Relaxed);
        let current_tasks = self.active_tasks.load(Relaxed);
        current_workers > current_tasks && current_workers > self.settings.core_size()
    }

    /// maybe stop a thread, returning true if it stopped
    fn maybe_stop(self: &Arc<Self>, id: WorkerThreadId) -> thread::Result<bool> {
        let outcome = loop {
            // trace!("checking if {id:?} should be stopped");
            let current_workers = self.active_workers.load(Relaxed);
            if self.should_stop() {
                match self.active_workers.compare_exchange(
                    current_workers,
                    current_workers - 1,
                    AcqRel,
                    Relaxed,
                ) {
                    Ok(_) => {
                        let mut workers = self.handles.lock();
                        let index = workers.iter().position(|p| p.id == id);
                        if let Some(index) = index {
                            let remove = workers.swap_remove(index);
                            let _ = remove.cont.compare_exchange(true, false, AcqRel, Relaxed);
                            trace!("{id:?} is being stopped");
                        }
                        break Ok(true);
                    }
                    Err(_) => {}
                }
            } else {
                break Ok(false);
            }
        };
        outcome
    }

    /// Submit a runnable
    pub fn submit<F>(self: &Arc<Self>, f: F) -> io::Result<()>
    where
        F: FnOnce() + Send + 'static,
    {
        let task = ThreadPoolTask::new(f);
        self.global.push(task);
        self.active_tasks.fetch_add(1, Relaxed);
        self.spawn_worker_if_needed()?;
        Ok(())
    }
}

assert_impl_all!(InnerThreadPool: Sync);

/// The worker thread
struct WorkerThread {
    parent_span: Span,
    parent: Weak<InnerThreadPool>,
    id: WorkerThreadId,
    global: Weak<Injector>,
    worker: Worker,
    state: Arc<AtomicCell<WorkerThreadState>>,
    cont: Arc<AtomicBool>,
    idle_timeout: Duration,
}

impl WorkerThread {
    /// Spawn a worker thread handle
    fn spawn(
        parent: &Arc<InnerThreadPool>,
        idle_timeout: Duration,
    ) -> io::Result<WorkerThreadHandle> {
        let state = Arc::new(AtomicCell::new(WorkerThreadState::idle()));
        let cont = Arc::new(AtomicBool::new(true));

        let id = WorkerThreadId::new(parent.next_id.fetch_add(1, Relaxed));
        let global = parent.global.clone();

        let this = WorkerThread {
            parent_span: parent.parent.clone(),
            parent: Arc::downgrade(parent),
            id,
            global: Arc::downgrade(&global),
            worker: Worker::new_fifo(),
            state: state.clone(),
            cont: cont.clone(),
            idle_timeout,
        };

        let join_handle = thread::Builder::new().spawn(this.worker_loop())?;

        Ok(WorkerThreadHandle {
            id,
            join_handle,
            state,
            cont,
        })
    }

    /// Gets stealers to all of the *other* worker threads
    fn stealers(&self) -> Vec<Stealer> {
        if let Some(parent) = self.parent.upgrade() {
            parent.registry.read().get_stealers(&self.id)
        } else {
            vec![]
        }
    }

    fn idle_duration(&self) -> Option<Duration> {
        if let WorkerThreadState::Idle(idle) = self.state.load() {
            Some(idle.elapsed())
        } else {
            None
        }
    }

    fn timed_out(&self) -> bool {
        match self.idle_duration() {
            None => false,
            Some(s) => s >= self.idle_timeout && self.worker.is_empty(),
        }
    }

    /// Finds a task
    fn find_task(&mut self) -> Option<ThreadPoolTask> {
        self.worker.pop().or_else(|| {
            if let Some(global) = self.global.upgrade() {
                global
                    .steal_batch_and_pop(&self.worker)
                    .success()
                    .or_else(|| {
                        self.stealers()
                            .iter()
                            .map(|s| s.steal_batch_and_pop(&self.worker))
                            .filter(|s| !s.is_retry())
                            .find_map(|s| s.success())
                    })
            } else {
                None
            }
        })
    }

    fn worker_loop(mut self) -> impl FnOnce() {
        move || {
            error_span!(parent: self.parent_span.clone(), "worker", id=?self.id).in_scope(|| {
                if let Some(registry) = self.parent.upgrade() {
                    trace!("adding {:?} to registry", self.id);
                    let mut registry = registry.registry.write();
                    registry.add(&self);
                }
                trace!("Starting {:?} loop", self.id);
                let r: thread::Result<()> = loop {
                    if !self.cont.load(Relaxed) {
                        break Ok(());
                    }
                    if let Some(inner_thread_pool) = self.parent.upgrade() {
                        if self.timed_out() {
                            // trace!("{:?} has timed out", self.id);
                            match inner_thread_pool.maybe_stop(self.id) {
                                Ok(true) => {
                                    break Ok(());
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    break Err(e);
                                }
                            }
                        }
                    }

                    if let Some(task) = self.find_task() {
                        self.state.store(WorkerThreadState::Running);
                        let ThreadPoolTask(f) = task;

                        let result = catch_unwind(AssertUnwindSafe(f));
                        if let Some(inner_thread_pool) = self.parent.upgrade() {
                            inner_thread_pool.active_tasks.fetch_sub(1, Relaxed);
                        }
                        match result {
                            Ok(()) => {}
                            Err(e) => {
                                self.state.store(WorkerThreadState::Panicked);
                                break Err(e);
                            }
                        }
                        self.state.store(WorkerThreadState::idle());
                    } else {
                        // trace!("No task was found")
                    }
                };

                trace!("worker {:?} has finished", self.id);
                if !r.is_err() {
                    self.state.store(WorkerThreadState::Finished)
                }
                if let Some(thread_pool) = self.parent.upgrade() {
                    let mut registry = thread_pool.registry.write();
                    registry.remove(&self);
                }
                if let Err(e) = r {
                    resume_unwind(e);
                }
            })
        }
    }
}

/// A handle to a worker thread
struct WorkerThreadHandle {
    id: WorkerThreadId,
    join_handle: JoinHandle<()>,
    state: Arc<AtomicCell<WorkerThreadState>>,
    cont: Arc<AtomicBool>,
}


#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerThreadState {
    /// Idle since
    Idle(Instant),
    Panicked,
    Running,
    Finished,
}

impl WorkerThreadState {
    fn idle() -> Self {
        Self::Idle(Instant::now())
    }
}

struct WorkerThreadRegistry {
    active: HashSet<WorkerThreadId>,
    stealer_map: HashMap<WorkerThreadId, Stealer>,
}

impl WorkerThreadRegistry {
    fn add(&mut self, thread: &WorkerThread) {
        let stealer = thread.worker.stealer();
        self.active.insert(thread.id);
        self.stealer_map.insert(thread.id, stealer);
    }

    /// Remove this worker thread
    fn remove(&mut self, thread: &WorkerThread) -> bool {
        self.active.remove(&thread.id);
        self.stealer_map.remove(&thread.id).is_some()
    }

    /// Gets all stealers that are not this worker thread
    fn get_stealers(&self, this_id: &WorkerThreadId) -> Vec<Stealer> {
        self.stealer_map
            .iter()
            .filter_map(|(id, stealer)| (id != this_id).then(|| stealer.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use test_log::test;
    use tracing::trace;

    #[test]
    fn test_inner_thread_pool() {
        let inner = InnerThreadPool::new(ThreadPoolSettings::new(
            4,
            num_cpus::get(),
            Duration::from_secs(5),
        ));

        let barrier = Arc::new(Barrier::new(num_cpus::get()));
        for i in 0..num_cpus::get() - 1 {
            inner
                .submit({
                    let barrier = barrier.clone();
                    move || {
                        trace!("thread {i} is waiting");
                        barrier.wait();
                    }
                })
                .expect("could not spawn worker thread");
        }

        barrier.wait();
        trace!("waited 10 seconds");
    }
}
