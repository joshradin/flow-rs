use jobflow::job_ordering::SteppedTaskOrderer;
use jobflow::{FlowBuilder, FlowError};
use test_log::test;

mod gradle_like;

#[test]
fn gradle_like_stepping() -> Result<(), FlowError> {
    let mut flow = FlowBuilder::new()
        .with_task_orderer(SteppedTaskOrderer)
        .build();

    gradle_like::create_flow(&mut flow)?;
    flow.run()?;

    Ok(())
}
