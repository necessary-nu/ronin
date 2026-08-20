//! Every dependency cycle a graph holds, found without building it.
//!
//! The engine finds a cycle only where it walks, and it walks only what a
//! build's targets asked for — so a cycle in a corner of a manifest nothing
//! requested is invisible to every tool that loads one. This is the sweep that
//! sees all of them, which is a report's question rather than a build's.

use super::{Graph, NodeId, cycle_through};
use crate::error::GraphError;

/// Every dependency cycle the graph holds, wherever in it it sits.
///
/// A build walks only what its targets asked for, so a cycle in a corner of
/// the manifest nothing requested is invisible to it — and that is exactly
/// what a report about the manifest itself exists to find. One cycle per node
/// the walk closes on, in node order, so the same manifest reports the same
/// cycles in the same order every time.
// [spec:ronin:req:runtime.iterative-tool-traversals]
pub(crate) fn dependency_cycles(graph: &Graph) -> Vec<GraphError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Colour {
        New,
        /// On the walk's current path, so reaching it again closes a cycle.
        Active,
        Done,
    }
    enum Step {
        Enter(NodeId),
        Leave,
    }

    let mut colour = vec![Colour::New; graph.node_ids().len()];
    let mut closed = std::collections::HashSet::new();
    let mut cycles = Vec::new();
    let mut path: Vec<NodeId> = Vec::new();
    let mut work = Vec::new();
    for root in graph.node_ids() {
        if colour[root.index()] != Colour::New {
            continue;
        }
        work.push(Step::Enter(root));
        while let Some(step) = work.pop() {
            match step {
                Step::Leave => {
                    let left = path.pop().expect("every leave follows its own enter");
                    colour[left.index()] = Colour::Done;
                }
                Step::Enter(node) => match colour[node.index()] {
                    Colour::Done => {}
                    Colour::Active => {
                        if closed.insert(node) {
                            cycles.push(cycle_through(graph, &path, node));
                        }
                    }
                    Colour::New => {
                        colour[node.index()] = Colour::Active;
                        path.push(node);
                        work.push(Step::Leave);
                        if let Some(edge) = graph.node(node).generator {
                            for input in graph.edge(edge).input.iter().rev() {
                                work.push(Step::Enter(*input));
                            }
                        }
                    }
                },
            }
        }
    }
    cycles
}
