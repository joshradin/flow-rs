use std::collections::HashSet;
use crate::task_ordering::{FlowGraph, TaskOrderer, TaskOrdering, TaskOrderingError};
use crate::TaskId;
use petgraph::acyclic::Acyclic;
use petgraph::prelude::*;
use tracing::{info, trace};

/// Attempts to create a task order by directly working with a graph
#[derive(Default)]
pub struct GraphTraversalTaskOrderer;

impl TaskOrderer for GraphTraversalTaskOrderer {
    type TaskOrdering = GraphTraversalTaskOrdering;

    fn create_ordering<G: FlowGraph>(
        &self,
        graph: G,
        max_jobs: usize,
    ) -> Result<Self::TaskOrdering, TaskOrderingError> {
        let mut pet_graph = DiGraph::new();
        for t in graph.tasks().into_iter() {
            pet_graph.add_node(t);
        }
        for t in graph.tasks().into_iter() {
            let t_idx = pet_graph
                .node_indices()
                .find(|i| pet_graph[*i] == t)
                .unwrap();
            for dependency in graph.dependencies(&t).into_iter() {
                let d_idx = pet_graph
                    .node_indices()
                    .find(|i| pet_graph[*i] == dependency)
                    .unwrap();
                pet_graph.add_edge(d_idx, t_idx, ());
            }
        }

        let acyclic = Acyclic::try_from_graph(pet_graph.clone())
            .map_err(|e| TaskOrderingError::cycle(e, &pet_graph))?;

        Ok(GraphTraversalTaskOrdering { in_use: HashSet::new(), graph: acyclic })
    }
}

pub struct GraphTraversalTaskOrdering {
    in_use: HashSet<TaskId>,
    graph: Acyclic<DiGraph<TaskId, ()>>,
}

impl TaskOrdering for GraphTraversalTaskOrdering {
    fn poll(&mut self) -> Result<Vec<TaskId>, TaskOrderingError> {
        let result = self
            .graph
            .node_indices()
            .filter(|node_idx| {
                self.graph
                    .neighbors_directed(*node_idx, Direction::Incoming)
                    .count()
                    == 0
            })
            .map(|nidx| self.graph[nidx])
            .filter(|t| !self.in_use.contains(t))
            .collect::<Vec<_>>();
        for task_id in &result {
            self.in_use.insert(*task_id);
        }
        if !result.is_empty() {
            trace!("graph traversal task ordering: {:?}", result);
        }
        Ok(result)
    }

    fn offer(&mut self, task: TaskId) -> Result<(), TaskOrderingError> {
        let node_idx = self.graph.node_indices()
            .find(|i| self.graph[*i] == task).ok_or_else(|| {
            TaskOrderingError::UnknownTask { task }
        })?;

        self.in_use.remove(&task);
        self.graph.remove_node(node_idx);
        Ok(())
    }

    fn empty(&self) -> bool {
        self.graph.node_count() == 0
    }
}
