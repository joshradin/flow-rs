use flow_rs::action::action;
use flow_rs::*;
use test_log::test;

/// This example flow will first accept a list of integer, get the square of all of the integers,
/// then sum that. Finally, it will return a list of all the integers with the sum added to each.
#[test]
fn test_concurrency() {
    let mut flow: Flow<Vec<i32>, Vec<i32>> = Flow::new();
    let f= flow.create("empty", || {});
    let f= flow.create("empty", || { Out(1_i32) });

    let test_data = Vec::from_iter(0..32);

    let mut squares = vec![];
    for i in 0..test_data.len() {
        let mut step_ref = flow.create(format!("square[{i}]"), action(|In(i): In<i32>| Out(i * i)));
        // step_ref.flows_from(flow.input().nth(i));
        squares.push(step_ref);
    }

    let sum = flow.create(
        "sum",
        action(|In(i): In<Vec<i32>>| -> Out<i32> { Out(i.iter().sum()) }),
    );

    let mut final_sums = vec![];
    for i in 0..test_data.len() {
        let mut step_ref = flow.create(
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
