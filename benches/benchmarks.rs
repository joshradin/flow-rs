use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use jobflow::job_ordering::{GraphTraversalTaskOrderer, JobOrderer, SteppedTaskOrderer};
use jobflow::{Flow, FlowBuilder, FlowError, FlowThreadPool, FlowsInto, JobRef};

fn gradle_flow<TO: JobOrderer>(flow: &mut Flow<(), (), TO>) -> Result<(), FlowError> {
    flow.create("help", || {});
    flow.create("start", || {});
    Ok(())
}

fn bench_gradle_flow(c: &mut Criterion) {
    let mut group = c.benchmark_group("gradle_flow");
    group.bench_function("default", |b| {
        let thread_pool = FlowThreadPool::default();
        b.iter_batched(
            || {
                let mut flow: Flow = FlowBuilder::new()
                    .with_thread_pool(thread_pool.clone())
                    .build();
                gradle_flow(&mut flow).expect("failed to create flow");
                flow
            },
            |flow| {
                let r = flow.run().unwrap();
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("stepped", |b| {
        let thread_pool = FlowThreadPool::default();
        b.iter_batched(
            || {
                let mut flow: Flow = FlowBuilder::new()
                    .with_thread_pool(thread_pool.clone())
                    .with_task_orderer(SteppedTaskOrderer)
                    .build();
                gradle_flow(&mut flow).expect("failed to create flow");
                flow
            },
            |flow| {
                flow.run().unwrap();
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("graphed", |b| {
        let thread_pool = FlowThreadPool::default();
        b.iter_batched(
            || {
                let mut flow = FlowBuilder::new()
                    .with_thread_pool(thread_pool.clone())
                    .with_task_orderer(GraphTraversalTaskOrderer)
                    .build();
                gradle_flow(&mut flow).expect("failed to create flow");
                flow
            },
            |flow| {
                flow.run().unwrap();
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_gradle_flow);
criterion_main!(benches);
