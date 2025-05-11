use crate::backend::task::BackendTask;
use crate::task_ordering::{FlowGraph, TaskOrderer, TaskOrdering, TaskOrderingError};
use crate::TaskId;
use petgraph::adj;
use petgraph::adj::{IndexType, UnweightedList};
use petgraph::algo::tred::dag_to_toposorted_adjacency_list;
use petgraph::algo::{kosaraju_scc, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use tracing::debug;

#[derive(Default)]
pub struct SteppedTaskOrderer;



impl TaskOrderer for SteppedTaskOrderer {
    type TaskOrdering = SteppedTaskOrdering;

    fn create_ordering<G: FlowGraph>(&self, graph: G, max_jobs: usize) -> Result<Self::TaskOrdering, TaskOrderingError> {
        SteppedTaskOrdering::new(graph, max_jobs)
    }
}

#[derive(Debug)]
pub struct SteppedTaskOrdering {
    order: Vec<Vec<(TaskId, bool)>>,
}

impl TaskOrdering for SteppedTaskOrdering {

    fn poll(&mut self) -> Result<Vec<TaskId>, TaskOrderingError> {
        match self.order.last_mut() {
            None => {
                Ok(vec![])
            }
            Some(tasks) => {
                let mut out = vec![];
                for (task, offered) in tasks {
                    if !*offered {
                        out.push(*task);
                        *offered = true;
                    }
                }
                Ok(out)
            }
        }
    }

    fn offer(&mut self, task: TaskId) -> Result<(), TaskOrderingError> {
        debug!("{task} finished");
        for step in &mut self.order {
            if let Some(idx) = step.iter().position(|(idx, _)| *idx ==task) {
                step.remove(idx);
                break;
            }
        }
        self.order.retain(|step| !step.is_empty());
        Ok(())
    }

    fn empty(&self) -> bool {
        self.order.is_empty()
    }
}

impl SteppedTaskOrdering {
    /// Attempts to create a new task ordering
    fn new<'a, G>(flow_graph: G, w: usize) -> Result<Self, TaskOrderingError>
    where
        G:  FlowGraph,
    {
        let mut graph: DiGraph<TaskId, ()> = DiGraph::new();
        let tasks: HashSet<_> = flow_graph.tasks().into_iter().collect();

        for id in &tasks {
            graph.add_node(*id);
        }
        for id  in &tasks {
            let node_id = graph.node_indices().find(|idx| graph[*idx] == *id).unwrap();
            for dependency in flow_graph.dependencies(id) {
                let dependency_id = graph
                    .node_indices()
                    .find(|idx| graph[*idx] == dependency)
                    .unwrap();
                graph.add_edge(node_id, dependency_id, ());
            }
        }


        let toposort = toposort(&graph, None).map_err(|cycle| {
            let cycle = get_cycle(&graph, cycle.node_id()).expect("failed to get a cycle");

            TaskOrderingError::CyclicTasks {
                cycle: cycle.iter().map(|idx| graph[*idx]).collect(),
            }
        })?;
        let (res, _revmap) = dag_to_toposorted_adjacency_list::<_, NodeIndex>(&graph, &toposort);
        let (reduction, _cls) = petgraph::algo::tred::dag_transitive_reduction_closure(&res);

        let ordering = lexico_topological_sort(&reduction);

        let mut levels: BTreeMap<usize, HashSet<NodeIndex>> = BTreeMap::new();
        for idx in ordering.into_iter().rev() {
            let outgoing = reduction.edge_indices_from(idx);
            let max_level = outgoing
                .into_iter()
                .map(|i| {
                    let (_, b) = reduction.edge_endpoints(i).unwrap();
                    b
                })
                .flat_map(|a| {
                    levels
                        .iter()
                        .find_map(|(lvl, nodes)| if nodes.contains(&a) { Some(*lvl) } else { None })
                })
                .max();
            let mut level = match max_level {
                Some(max_level) => max_level + 1,
                None => levels
                    .iter()
                    .filter_map(|(level, idxs)| -> Option<usize> {
                        if idxs.len() < w { Some(*level) } else { None }
                    })
                    .min()
                    .unwrap_or(0),
            };
            while levels.get(&level).map(|s| s.len()) == Some(w) {
                level += 1
            }

            levels.entry(level).or_default().insert(idx);
        }

        let mut steps: Vec<_> = levels
            .into_iter()
            .map(|(_, set)| {
                set.into_iter()
                   .map(|i| (graph[toposort[i.index()]], false))
                   .collect()
            })
            .collect();
        steps.reverse();
        Ok(Self { order: steps })
    }


}



/// Constructs a topological ordering of G in which the vertices are ordered lexicographically by
/// the set of positions of their incoming neighbors.
///
/// add the vertices one at a time to the ordering, at each step choosing a vertex v to add such
/// that the incoming neighbors of v are all already part of the partial ordering, and such that
/// the most recently added incoming neighbor of v is earlier than the most recently added incoming
/// neighbor of any other vertex that could be added in place of v. If two vertices have the same
/// most recently added incoming neighbor, the algorithm breaks the tie in favor of the one whose
/// second most recently added incoming neighbor is earlier, etc
fn lexico_topological_sort<Ix: IndexType>(
    list: &UnweightedList<NodeIndex<Ix>>,
) -> Vec<NodeIndex<Ix>> {
    let mut ordering: Vec<NodeIndex<Ix>> = Vec::new();
    let mut closed_set: HashSet<NodeIndex<Ix>> = HashSet::new();
    let mut open_set: HashSet<NodeIndex<Ix>> = HashSet::from_iter(list.node_indices());
    while !open_set.is_empty() {
        let mut contenders = vec![];
        for node in &open_set {
            let incoming = incoming_edges(*node, list);
            let mut all_in_closed_set = true;
            for incoming_edge in incoming {
                let (a, _) = list.edge_endpoints(incoming_edge).unwrap();
                if !closed_set.contains(&a) {
                    all_in_closed_set = false;
                    break;
                }
            }
            if all_in_closed_set {
                contenders.push(*node);
            }
        }

        contenders.sort_by_cached_key(|contender| {
            let incoming = incoming_edges(*contender, list);
            let mut ages = vec![];
            for incoming_edge in incoming {
                let (a, _) = list.edge_endpoints(incoming_edge).unwrap();
                let index = ordering.iter().position(|&n| a == n).unwrap();
                ages.push(index);
            }
            ages.sort();
            Reverse(ages)
        });
        let first = contenders.first().expect("No contenders found");
        open_set.remove(first);
        closed_set.insert(*first);
        ordering.push(*first);
    }

    ordering
}

fn map_graph(
    g: &DiGraph<TaskId, ()>,
    tasks: &HashMap<TaskId, &BackendTask>,
) -> DiGraph<String, ()> {
    g.map(|_, w| tasks[w].nickname().to_string(), |_, e| ())
}

fn incoming_edges<Ix: IndexType>(
    n: NodeIndex<Ix>,
    list: &UnweightedList<NodeIndex<Ix>>,
) -> Vec<adj::EdgeIndex<NodeIndex<Ix>>> {
    list.edge_indices()
        .filter(|idx| match list.edge_endpoints(*idx) {
            None => false,
            Some((_, b)) => b == n,
        })
        .collect()
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




#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::action;
    use crate::backend::flow_backend::BackendFlowGraph;
    use crate::backend::task::{BackendTask, InputFlavor, ReusableOutput, SingleOutput, TaskError};

    fn quick_task(name: &str) -> BackendTask {
        BackendTask::new(
            name,
            InputFlavor::Single,
            ReusableOutput::new(),
            action(|_: ()| {}),
        )
    }

    fn create_order(width: usize, tasks: &[BackendTask]) -> SteppedTaskOrdering {
        SteppedTaskOrderer::default()
            .create_ordering(
                BackendFlowGraph::new(tasks),
                width,
            )
            .expect("failed to create order")
    }

    #[test]
    fn test_make_reusable() {
        let mut b = BackendTask::new(
            "test",
            InputFlavor::None,
            SingleOutput::new(),
            action(|_: ()| "hello"),
        );
        assert!(matches!(
            b.output_mut().make_reusable::<isize>(),
            Err(TaskError::UnexpectedType { .. })
        ));
        assert!(matches!(b.output_mut().make_reusable::<&str>(), Ok(())));
    }

    #[test]
    fn test_ordering() {
        let a = quick_task("a");
        let mut b = quick_task("b");
        b.input_mut().depends_on(a.id());
        let mut c = quick_task("c");
        c.input_mut().depends_on(a.id());
        let mut d = quick_task("d");
        d.input_mut().depends_on(a.id());
        d.input_mut().depends_on(b.id());
        d.input_mut().depends_on(c.id());
        let mut e = quick_task("e");
        e.input_mut().depends_on(a.id());
        e.input_mut().depends_on(c.id());
        e.input_mut().depends_on(d.id());
        let tasks = vec![a, b, c, d, e];
        // let ordering = SteppedTaskOrdering::new(&tasks, 2).expect("failed to create order");
    }

    #[test]
    fn test_multi_path_ordering() {
        let a = quick_task("a");
        let mut b = quick_task("b");
        b.input_mut().depends_on(a.id());
        let mut c = quick_task("c");
        c.input_mut().depends_on(a.id());
        let mut d = quick_task("d");
        d.input_mut().depends_on(a.id());
        d.input_mut().depends_on(b.id());
        d.input_mut().depends_on(c.id());
        let mut e = quick_task("e");
        e.input_mut().depends_on(a.id());
        e.input_mut().depends_on(c.id());
        e.input_mut().depends_on(d.id());
        let f = quick_task("f");
        let mut g = quick_task("g");
        let mut h = quick_task("h");
        g.input_mut().depends_on(f.id());
        h.input_mut().depends_on(g.id());
        let i = quick_task("i");
        let j = quick_task("j");
        let tasks = vec![a, b, c, d, e, f, g, h, i, j];
        let ordering = create_order(3, &tasks);
        println!("ordering: {:#?}", ordering);
        assert_eq!(ordering.order.len(), 4);
        // let ordering = SteppedTaskOrdering::new(&tasks, 3).expect("failed to create order");
    }

    #[test]
    fn test_cycle_detection() {
        let mut a = quick_task("a");
        let mut b = quick_task("b");
        b.input_mut().depends_on(a.id());
        let mut c = quick_task("c");
        c.input_mut().depends_on(b.id());
        a.input_mut().depends_on(c.id());
        let mut d = quick_task("d");
        d.input_mut().depends_on(c.id());

        let tasks = vec![a, b, c];
        // let ordering = SteppedTaskOrdering::new(&tasks, 2).expect_err("should fail to create order");
        // assert!(matches!(ordering,  TaskOrderingError::CyclicTasks { cycle } if cycle.len() == 3));
    }
}
