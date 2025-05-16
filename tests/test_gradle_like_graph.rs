//! Tests a gradle-like scenario

use jobflow::listener::PrintTaskListener;
use jobflow::job_ordering::GraphTraversalTaskOrderer;
use jobflow::{FlowBuilder, FlowError, FlowThreadPool};
use std::time::Duration;
use test_log::test;

mod gradle_like;

#[test]
fn graph() -> Result<(), FlowError> {
    let mut flow = FlowBuilder::new()
        .with_task_orderer(GraphTraversalTaskOrderer)
        .with_thread_pool(FlowThreadPool::new(1, 7, Duration::ZERO))
        .build();
    flow.add_listener(PrintTaskListener);

    gradle_like::create_flow(&mut flow)?;
    flow.run()?;

    Ok(())
}
