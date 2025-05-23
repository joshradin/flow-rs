# `jobflow`
Do ordered jobs concurrently and easily. Designed to allow for arbitrary ordering of tasks
and sending the output of a task into the input of another.

## Jobs and Flows
A flow is composed of many jobs. Jobs are meant to be a form of atomic operations. The users
can create a job by giving it a name and assigning an `Action` to it. When using
`Flow::create` users can directly use any function of either signature `FnOnce() -> R` or `FnOnce(T) -> R`.
If using the former signature, attempting to set an input for this task will return an error.

## Examples
### No I/O

```rust
use jobflow::Flow;
let mut flow = Flow::new();
flow.create("print something", | | {
println ! ("Hello!")
});
flow.run().expect("failed to run");
```
### Accepts an input, returns an output
```rust
use jobflow::{Flow, FlowsInto};
let mut flow = Flow::new();
let job = flow.input().flows_into(flow.create("square", |i: i32| { i * i }))
 .unwrap().flows_into(flow.output())
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
let funnel = flow.create("sum", |i: Vec<i32>| { i.iter().sum::<i32>()}).funnelled().unwrap();
flow.create("a", || { 25_i32 }).flows_into(&funnel).unwrap();
flow.create("b", || { 50_i32 }).flows_into(&funnel).unwrap();
funnel.flows_into(flow.output()).expect("could set sum as output");
let sum = flow.get().expect("failed to get sum");
assert_eq!(sum, 75);
```