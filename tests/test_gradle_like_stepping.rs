use flow_rs::{FlowBuilder, FlowError};
use flow_rs::job_ordering::SteppedTaskOrderer;
use test_log::test;

mod gradle_like;

#[test]
fn stepping() -> Result<(), FlowError> {
    let mut flow = FlowBuilder::new()
        .with_task_orderer(SteppedTaskOrderer)
        .build();

    gradle_like::create_flow(&mut flow)?;
    flow.run()?;

    Ok(())
}