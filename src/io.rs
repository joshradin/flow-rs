//! Special input and output types for jobs
//!
//! Input:
//!
//! Output:
//!  - [`Disjointed`] - used for splitting the output of a task to be used as the input of multiple tasks

pub mod disjoint;

#[doc(inline)]
pub use disjoint::Disjointed;
