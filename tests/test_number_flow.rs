use flow_rs::action::action;
use flow_rs::*;
use test_log::test;

/// This example flow will first accept a list of integer, get the square of all of the integers,
/// then sum that. Finally, it will return a list of all the integers with the sum added to each.
#[test]
fn test_concurrency() -> Result<(), FlowError> {
    let mut flow: Flow<Vec<i32>, Vec<i32>> = Flow::new();
    let test_data = Vec::from_iter(0..32);
    let ref f = flow.create("init", move || test_data)
        .reusable()?;

    let mut squares = Funnel::<i32>::new();
    for i in 0..test_data.len() {
        let get_nth = f.flows_into(flow.create(format!("get[{i}]"), |v: Vec<i32>| v[i]))?;
        let step_ref =
            get_nth.flows_into(flow.create(format!("square[{i}]"), action(|i| i * i)))?;

        step_ref.flows_into(&mut squares);
    }




    let sum = squares.flow_into(flow.create(
        "sum",
        action(|i: Vec<i32>| -> i32 { i.iter().sum() }),
    ));

    let mut final_sums = vec![];
    for i in 0..test_data.len() {
        let step_ref = flow.create(
            format!("addSum[{i}]"),
            action(|In((i, sum)): In<(i32, i32)>| Out(i + sum)),
        );
        // step_ref.flows_from((flow.input().nth(i), sum.clone()));
        final_sums.push(step_ref);
    }

    // flow.output().flows_from(final_sums);

    let expected = expected_result(&test_data);
    let result = flow
        .apply(test_data)
        .expect("failed to run flow to produce");

    assert_eq!(result, expected);
}

#[must_use]
fn expected_result(t: &[i32]) -> Vec<i32> {
    let sum = t.iter().map(|&x| x * x).sum::<i32>();

    t.iter().map(|&x| x + sum).collect()
}
