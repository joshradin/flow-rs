use flow_rs::action::action;
use flow_rs::*;
use test_log::test;

/// This example flow will first accept a list of integer, get the square of all of the integers,
/// then sum that. Finally, it will return a list of all the integers with the sum added to each.
#[test]
fn test_concurrency() {
    // let mut flow: Flow<Vec<i32>, Vec<i32>> = Flow::new();
    // 
    // let test_data = Vec::from_iter(0..32);
    // 
    // let mut squares = vec![];
    // for i in 0..test_data.len() {
    //     let mut step_ref = flow.add(format!("square[{i}]"), action(|i: i32| i * i));
    //     step_ref.flows_from(flow.input().nth(i));
    //     squares.push(step_ref);
    // }
    // 
    // let sum = flow.add("sum", action(|i: Vec<i32>| -> i32 { i.iter().sum() }));
    // 
    // let mut final_sums = vec![];
    // for i in 0..test_data.len() {
    //     let mut step_ref = flow.add(
    //         format!("addSum[{i}]"),
    //         action(|(i, sum): (i32, i32)| i + sum),
    //     );
    //     step_ref.flows_from((flow.input().nth(i), sum.clone()));
    //     final_sums.push(step_ref);
    // }
    // 
    // flow.output().flows_from(final_sums);
    // 
    // let expected = expected_result(&test_data);
    // let result = flow
    //     .apply(test_data)
    //     .expect("failed to run flow to produce");
    // 
    // assert_eq!(result, expected);
}

#[must_use]
fn expected_result(t: &[i32]) -> Vec<i32> {
    let sum = t.iter().map(|&x| x * x).sum::<i32>();

    t.iter().map(|&x| x + sum).collect()
}
