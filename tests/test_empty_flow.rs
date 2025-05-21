use jobflow::Flow;
use test_log::test;

#[test]
fn empty_flow() {
    // Create a background thread which checks for deadlocks every 10s
    let flow = Flow::new();
    flow.run().expect("failed to run flow");
}
