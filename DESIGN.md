# `flow-rs` Design and Implementation

This document will reflect the design and implementation details for `flow-rs`

## Design

This is the general design for `flow-rs`

### Basic Ideas
```psuedocode
let flow := new Flow;

let taskA := flow.create("A", fn(I) -> T)
let taskB := flow.create("B", fn(T) -> O)
taskB.flows_from(taskA)
// or
taskA.flows_into(taskB)

/// set inputs and outputs
flow.output().flows_from(taskB)
flow.input().flows_into(taskA)

let result := flow.apply(I)
```

### I/O
By default, all data inputs and outputs are use-once, however modifiers can be used to allow either multiple
steps to re-use the same input. ~~Array-like data types can be broken down into indices.~~

```psuedocode
# Illegal multiple tasks using same input
let taskA := flow.create("A", fn(I) -> T)
let taskB := flow.create("B", fn(T) -> O)
let taskC := flow.create("C", fn(T) -> U)
taskA.flows_into(taskB)
taskA.flows_into(taskB) # NOT ALLOWED

# Correct usage
let taskA := flow.create("A", fn(I) -> T).reusable()
let taskB := flow.create("B", fn(T) -> O)
let taskC := flow.create("C", fn(T) -> U)
taskA.flows_into(taskB)
taskA.flows_into(taskB) # Ok
```

Tasks should be able to use the output of a task or the input of an entire flow as an input. 

## Implementation Details

Should be implemented with two layers, a frontend and a backend. The backend will be entirely runtime-checked while
the front-end will be compile-time checked.