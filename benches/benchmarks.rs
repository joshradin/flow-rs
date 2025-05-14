use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use flow_rs::{Flow, FlowError, FlowThreadPool, FlowsInto, TaskRef};


fn gradle_flow(pool: &FlowThreadPool) -> Result<Flow, FlowError> {
    let mut flow: Flow = Flow::builder()
        .with_thread_pool(pool.clone())
        .build();


    flow.create("test", || {
        println!("running test task")
    });

    Ok(flow)
}

fn bench_gradle_flow(c: &mut Criterion) {
    let mut group = c.benchmark_group("gradle_flow");
    group.sample_size(10);
    group.bench_function("default", |b| {
        let thread_pool = FlowThreadPool::default();
        b.iter(
            || {
                println!("creating flow");
                let flow = gradle_flow(&thread_pool).unwrap();
                println!("running flow");
                let r = flow.run().unwrap();
                println!("running task result: {:?}", r);
            }
        )
    });
}

criterion_group!(benches, bench_gradle_flow);
criterion_main!(benches);