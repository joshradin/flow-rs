//! The executor used for executing tasks

use crate::pool::{ThreadPool, WorkerPool};

/// The executor used for performing tasks
pub struct Executor<P: WorkerPool = ThreadPool> {
    pool: P,
}
