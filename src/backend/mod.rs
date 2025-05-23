//! The backend implementation

pub mod flow_backend;
pub mod flow_listener_shim;
pub mod funnel;
pub mod disjointed;
pub mod job;
pub(crate) mod recv_promise;
pub mod reusable;
mod overlap_checking;