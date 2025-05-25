# `jobflow`
![Crates.io Version](https://img.shields.io/crates/v/jobflow) ![docs.rs](https://img.shields.io/docsrs/jobflow?link=https%3A%2F%2Fdocs.rs%2Fjobflow%2Flatest%2Fjobflow%2F) ![GitHub branch check runs](https://img.shields.io/github/check-runs/joshradin/jobflow/main?label=build)

Do ordered jobs concurrently and easily. Designed to allow for arbitrary ordering of tasks
and sending the output of a task into the input of another.

## Jobs and Flows
A flow is composed of many jobs. Jobs are meant to be a form of atomic operations. The users
can create a job by giving it a name and assigning an [`Action`] to it. When using
[`Flow::create`] users can directly use any function of either signature `FnOnce() -> R` or `FnOnce(T) -> R`.
If using the former signature, attempting to set an input for this task will return an error.


## Examples
### No I/O
```rust
use jobflow::Flow;
let mut flow = Flow::new();
flow.create("print something", || println!("Hello!"));
flow.run().expect("failed to run");
```

### Accepts an input, returns an output
```rust
use jobflow::{Flow, FlowsInto};

let mut flow = Flow::new();
let job = flow
    .input()
    .flows_into(flow.create("square", |i: i32| i * i))
    .unwrap()
    .flows_into(flow.output())
    .unwrap();
let output = flow.apply(10).unwrap();
assert_eq!(output, 100);
```

### Multiple tasks can flow into a single task
Multiple tasks can be used an input into a single task using a funnel. As long as the input
of the action is something that implements `FromIterator<T>` and `IntoIterator<Item=T>`
```rust
use jobflow::{Flow, FlowsInto};
let mut flow = Flow::new();
let funnel = flow
    .create("sum", |i: Vec<i32>| i.iter().sum::<i32>())
    .funnelled()
    .unwrap();

flow.create("a", || 25_i32).flows_into(&funnel).unwrap();
flow.create("b", || 50_i32).flows_into(&funnel).unwrap();
funnel
    .flows_into(flow.output())
    .expect("could set sum as output");
let sum = flow.get().expect("failed to get sum");
assert_eq!(sum, 75);
```

### Task outputs can be disjointed to be consumed by multiple tasks

Multiple tasks can take the output of a single task if the parts are disjoint from each other.
Unlike the [`Reusable`] type, the task's output doesn't need to be clonable. Currently, the output
of a disjointable task *must* be `Vec<T>`. Tasks that take the disjointed input can be any
`FromIterator<Item=T>` type.

```rust
use jobflow::{Flow, FlowsInto};
let mut flow = Flow::new();
let generator = flow
    .create("generator", || (0..32).into_iter().collect::<Vec<i32>>())
    .disjointed()
    .unwrap();
let first_half = flow.create("1/2", |v: Vec<i32>| assert_eq!(v.len(), 16));
let second_half = flow.create("2/2", |v: Vec<i32>| assert_eq!(v.len(), 16));

generator.gets(..16).flows_into(first_half).unwrap();
generator.gets(16..).flows_into(second_half).unwrap();

flow.run().expect("failed to run flow");
```