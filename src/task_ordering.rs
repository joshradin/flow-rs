use crate::TaskId;
use petgraph::adj::IndexType;
use petgraph::algo::{kosaraju_scc, Cycle};
use petgraph::graph::DiGraph;
use petgraph::prelude::NodeIndex;
use std::fmt::Display;
use thiserror::Error;

mod graph_traversal;
mod stepped;

pub use graph_traversal::GraphTraversalTaskOrderer;
pub use stepped::SteppedTaskOrderer;

pub type DefaultTaskOrderer = stepped::SteppedTaskOrderer;

/// A basic flow graph
pub trait FlowGraph {
    type Tasks: IntoIterator<Item = TaskId>;
    type DependsOn: IntoIterator<Item = TaskId>;
    type Dependents: IntoIterator<Item = TaskId>;

    /// Gets all tasks in this flow graph
    fn tasks(&self) -> Self::Tasks;

    /// Gets the tasks that `task` depends on directly (incoming tasks).
    fn dependencies(&self, task: &TaskId) -> Self::DependsOn;
    /// Gets the tasks that depend on `task` (outgoing tasks).
    fn dependents(&self, task: &TaskId) -> Self::DependsOn;
}

/// The job of the task orderer is to create available tasks for a flow graph
pub trait TaskOrdering {
    /// Gets any available tasks
    fn poll(&mut self) -> Result<Vec<TaskId>, TaskOrderingError>;
    /// Offer a finished task back to the orderer
    fn offer(&mut self, task: TaskId) -> Result<(), TaskOrderingError>;

    /// Offers all the given task ids
    fn offer_all(&mut self, i: impl IntoIterator<Item = TaskId>) -> Result<(), TaskOrderingError> {
        for i in i {
            self.offer(i)?;
        }
        Ok(())
    }

    /// Checks if this task ordering is empty
    fn empty(&self) -> bool;
}

/// Responsible with creating a [`TaskOrdering`]
pub trait TaskOrderer {
    /// The task order to create
    type TaskOrdering: TaskOrdering;

    /// tries to create an task orderer
    fn create_ordering<G: FlowGraph>(
        &self,
        graph: G,
        max_jobs: usize,
    ) -> Result<Self::TaskOrdering, TaskOrderingError>;
}

#[derive(Debug, Clone, Error)]
pub enum TaskOrderingError {
    #[error("A cycle was detected. {}", format_cycle(cycle))]
    CyclicTasks { cycle: Vec<TaskId> },
    #[error("Task {task} is not part of this graph")]
    UnknownTask { task: TaskId },
}

impl TaskOrderingError {
    fn cycle<Ix: IndexType>(err: Cycle<NodeIndex<Ix>>, graph: &DiGraph<TaskId, (), Ix>) -> Self {
        let node_idx = get_cycle(graph, err.node_id()).unwrap();
        let task_ids: Vec<_> = node_idx.into_iter().map(|idx| graph[idx]).collect();
        Self::CyclicTasks { cycle: task_ids }
    }
}

pub fn format_cycle<T: Display>(cycle: &[T]) -> String {
    cycle
        .iter()
        .map(|id| format!("{}", id))
        .collect::<Vec<String>>()
        .join(" -> ")
}

/// Gets a cycle containing this node
fn get_cycle<N, E, Ix: IndexType>(
    graph: &DiGraph<N, E, Ix>,
    node: NodeIndex<Ix>,
) -> Option<Vec<NodeIndex<Ix>>> {
    let scc = kosaraju_scc(graph);
    println!("scc: {:?}", scc);

    scc.iter().find(|nodes| nodes.contains(&node)).cloned()
}
