//! Task ordering algorithms and structures.
//!
//! Uses the [Coffman–Graham algorithm](https://en.wikipedia.org/wiki/Coffman%E2%80%93Graham_algorithm).

use crate::backend::task::{BackendTask, TaskId};
use petgraph::adj;
use petgraph::adj::{IndexType, UnweightedList};
use petgraph::algo::tred::dag_to_toposorted_adjacency_list;
use petgraph::algo::{kosaraju_scc, toposort};
use petgraph::prelude::*;
use petgraph::visit::{NodeRef, Walker};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Display;
use thiserror::Error;

#[derive(Debug)]
pub struct TaskOrdering {
    order: Vec<Vec<TaskId>>,
}

impl TaskOrdering {
    /// Attempts to create a new task ordering
    pub fn new<'a, I>(iter: I, w: usize) -> Result<Self, TaskOrderingError>
    where
        I: IntoIterator<Item = &'a BackendTask>,
    {
        let map: HashMap<_, _> = iter.into_iter().map(|t| (t.id(), t)).collect();
        let mut graph: DiGraph<TaskId, ()> = DiGraph::new();
        for (id, _) in &map {
            graph.add_node(*id);
        }
        for (id, task) in &map {
            let node_id = graph.node_indices().find(|idx| graph[*idx] == *id).unwrap();
            for &dependency in task.dependencies() {
                let dependency_id = graph
                    .node_indices()
                    .find(|idx| graph[*idx] == dependency)
                    .unwrap();
                graph.add_edge(dependency_id, node_id, ());
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
                    .sum::<usize>(),
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
                    .map(|i| graph[toposort[i.index()]])
                    .collect()
            })
            .collect();
        steps.reverse();
        Ok(Self { order: steps })
    }

    /// Gets an iterator over the steps of this
    pub fn iter(&self) -> Iter {
        Iter {
            order: self.order.iter(),
        }
    }
}

impl IntoIterator for TaskOrdering {
    type Item = Vec<TaskId>;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            order: VecDeque::from(self.order),
        }
    }
}

impl<'a> IntoIterator for &'a TaskOrdering {
    type Item = &'a Vec<TaskId>;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug)]
pub struct Iter<'a> {
    order: std::slice::Iter<'a, Vec<TaskId>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Vec<TaskId>;

    fn next(&mut self) -> Option<Self::Item> {
        self.order.next()
    }
}

#[derive(Debug)]
pub struct IntoIter {
    order: VecDeque<Vec<TaskId>>,
}

impl Iterator for IntoIter {
    type Item = Vec<TaskId>;

    fn next(&mut self) -> Option<Self::Item> {
        self.order.pop_front()
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

#[derive(Debug, Error)]
pub enum TaskOrderingError {
    #[error("A cycle was detected. {}", format_cycle(cycle))]
    CyclicTasks { cycle: Vec<TaskId> },
}

fn format_cycle<T: Display>(cycle: &Vec<T>) -> String {
    cycle
        .iter()
        .map(|id| format!("{}", id))
        .collect::<Vec<String>>()
        .join(" -> ")
}

#[cfg(test)]
mod tests {
    use crate::action::action;
    use crate::backend::task::{BackendTask, InputFlavor, ReusableOutput, SingleOutput, TaskError};
    use crate::backend::task_ordering::{TaskOrdering, TaskOrderingError};

    fn quick_task(name: &str) -> BackendTask {
        BackendTask::new(
            name,
            InputFlavor::Single,
            ReusableOutput::new(),
            action(|_: ()| {}),
        )
    }

    #[test]
    fn test_make_reusable() {
        let mut b = BackendTask::new(
            "test",
            InputFlavor::None,
            SingleOutput::new(),
            action(|_: ()| "hello"),
        );
        assert!(matches!(b.output().make_reusable::<isize>(), Err(TaskError::UnexpectedType { .. })));
        assert!(matches!(b.output().make_reusable::<&str>(), Ok(())));
        
    }

    #[test]
    fn test_ordering() {
        let a = quick_task("a");
        let mut b = quick_task("b");
        b.input().depends_on(a.id());
        let mut c = quick_task("c");
        c.input().depends_on(a.id());
        let mut d = quick_task("d");
        d.input().depends_on(a.id());
        d.input().depends_on(b.id());
        d.input().depends_on(c.id());
        let mut e = quick_task("e");
        e.input().depends_on(a.id());
        e.input().depends_on(c.id());
        e.input().depends_on(d.id());
        let tasks = vec![a, b, c, d, e];
        let ordering = TaskOrdering::new(&tasks, 2).expect("failed to create order");
        for (step, task_ids) in ordering.iter().enumerate() {
            print!("step {}: ", step);
            let names = task_ids
                .iter()
                .map(|id| tasks.iter().find(|task| task.id() == *id).unwrap())
                .collect::<Vec<_>>();
            println!("{names:?}");
        }
        assert_eq!(ordering.iter().count(), 4);
    }
    #[test]
    fn test_cycle_detection() {
        let mut a = quick_task("a");
        let mut b = quick_task("b");
        b.input().depends_on(a.id());
        let mut c = quick_task("c");
        c.input().depends_on(b.id());
        a.input().depends_on(c.id());
        let mut d = quick_task("d");
        d.input().depends_on(c.id());

        let tasks = vec![a, b, c];
        let ordering = TaskOrdering::new(&tasks, 2).expect_err("should fail to create order");
        assert!(matches!(ordering, TaskOrderingError::CyclicTasks { cycle } if cycle.len() == 3));
    }
}
