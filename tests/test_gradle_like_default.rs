use flow_rs::{FlowBuilder, FlowError};
use test_log::test;

// #[path = "gradle_like.rs"]
mod gradle_like;

#[test]
fn default() -> Result<(), FlowError> {
    let mut flow = FlowBuilder::new().build();

    gradle_like::create_flow(&mut flow)?;
    flow.run()?;

    Ok(())
}