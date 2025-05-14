//! # `flow-rs`
//!
//! Do concurrent tasks safely and easily.

pub mod task_ordering;
pub mod action;
pub(crate) mod backend;
mod flow;
mod pool;
pub mod promise;
pub mod listener;

pub use flow::*;
pub use pool::FlowThreadPool;
pub use backend::task::{TaskId, TaskError};
