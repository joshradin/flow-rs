use flow_rs::action::action;
use flow_rs::task_ordering::{GraphTraversalTaskOrderer, TaskOrderer};
use flow_rs::*;
use test_log::test;
use tracing::info;

/// This example flow will first accept a list of integer, get the square of all of the integers,
/// then sum that. Finally, it will return a list of all the integers with the sum added to each.
#[test]
fn test_stepped() -> Result<(), FlowError> {
    let mut flow = FlowBuilder::new().build();

    let test_data = Vec::from_iter(0..32);
    populate_flow(&test_data, &mut flow)?;

    // flow.output().flows_from(final_sums);

    let expected = expected_result(&test_data);
    let result: Vec<i32> = flow
        .apply(test_data)
        .expect("failed to run flow to produce");
    info!("got result: {:?}", &result);
    assert_eq!(result, expected);

    Ok(())
}

/// This example flow will first accept a list of integer, get the square of all of the integers,
/// then sum that. Finally, it will return a list of all the integers with the sum added to each.
#[test]
fn test_graph() -> Result<(), FlowError> {
    let mut flow = FlowBuilder::new()
        .with_task_orderer(GraphTraversalTaskOrderer)
        .build();

    let test_data = Vec::from_iter(0..32);
    populate_flow(&test_data, &mut flow)?;

    // flow.output().flows_from(final_sums);

    let expected = expected_result(&test_data);
    let result: Vec<_> = flow
        .apply(test_data)
        .expect("failed to run flow to produce");

    assert_eq!(result, expected);

    Ok(())
}

fn populate_flow<T: TaskOrderer>(
    test_data: &Vec<i32>,
    flow: &mut Flow<Vec<i32>, Vec<i32>, T>,
) -> Result<(), FlowError> {
    let ref f = flow.create("init", move |i: Vec<i32>| i).reusable()?;
    flow.input().flows_into(f)?;

    let mut squares = vec![];
    for i in 0..test_data.len() {
        let get_nth = f.flows_into(flow.create(format!("get[{i}]"), move |v: Vec<i32>| v[i]))?;
        let step_ref = get_nth.flows_into(flow.create(
            format!("square[{i}]"),
            action(|i| {
                //thread::sleep(Duration::from_millis(1000));
                i * i
            }),
        ))?;

        squares.push(step_ref);
    }

    let ref sum = squares
        .flows_into(
            flow.create("sum", action(|i: Vec<i32>| -> i32 { i.iter().sum() }))
                .funnelled()?,
        )?
        .reusable()?;

    let mut final_sums = vec![];
    for i in 0..test_data.len() {
        let step_ref = flow.create(
            format!("addSum[{i}]"),
            action(move |(vs, sum): (Vec<i32>, i32)| {
                vs[i] + sum
            }),
        );
        let step_ref = (f, sum).flows_into(step_ref)?;
        // step_ref.flows_from((flow.input().nth(i), sum.clone()));
        final_sums.push(step_ref);
    }

    let output = final_sums.flows_into(flow.create("output", |t: Vec<i32>| t).funnelled()?)?;
    output.flows_into(flow.output())?;

    Ok(())
}

#[must_use]
fn expected_result(t: &[i32]) -> Vec<i32> {
    let sum = t.iter().map(|&x| x * x).sum::<i32>();

    t.iter().map(|&x| x + sum).collect()
}
