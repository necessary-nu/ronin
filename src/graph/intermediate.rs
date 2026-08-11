//! The half of GNU Make's intermediate file that the dirty scan cannot reach.
//!
//! A file the implicit rule search invented is allowed not to be there, and
//! [`super::recompute_edge_dirty_with`] says so where every other timestamp
//! comparison is made: the outputs stand in for the newest thing behind them
//! and the edge is dirty only if an input was. That settles what a consumer
//! sees, which is all the scan can settle, because it evaluates inputs before
//! the edge that reads them.
//!
//! Whether the file is worth creating is the other question, and only the
//! consumer can answer it. So it is asked here, on the way back down.

use super::{DirtyEvaluator, EdgeId, Graph, NodeId};
use crate::runtime::RuntimeState;

impl DirtyEvaluator {
    /// Ask for the intermediates whose absence was excused, wherever something
    /// that reads them has to run anyway.
    ///
    /// Walks down from `target` through the edges that are going to run and
    /// stops at every edge that is not, so an intermediate nothing needs stays
    /// uncreated — which is the whole point of it being intermediate.
    pub(super) fn push_intermediates(
        &mut self,
        graph: &Graph,
        runtime: &mut RuntimeState,
        target: NodeId,
    ) {
        self.pushed.begin(graph.edge_count());
        let mut work = Vec::new();
        if let Some(edge) = graph
            .node(target)
            .generator
            .filter(|_| runtime.node(target).dirty())
        {
            work.push(edge);
        }
        while let Some(edge) = work.pop() {
            if self.pushed.replace(edge.index()) {
                continue;
            }
            let inputs: &[NodeId] = &graph.edge(edge).input;
            for &input in inputs {
                let Some(generator) = graph.node(input).generator else {
                    continue;
                };
                if runtime.edge(generator).absent_intermediate() {
                    ask_for(graph, runtime, generator);
                } else if !runtime.node(input).dirty() {
                    continue;
                }
                work.push(generator);
            }
        }
    }
}

fn ask_for(graph: &Graph, runtime: &mut RuntimeState, edge: EdgeId) {
    let outputs: &[NodeId] = &graph.edge(edge).out;
    for &output in outputs {
        runtime.node_mut(output).set_dirty(true);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Graph, NodeId, mkedge, mknode, nodeuse, recompute_dirty_with};
    use crate::env::mkenv;
    use crate::runtime::RuntimeState;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn generated(graph: &mut Graph, output: &str, input: &str) -> NodeId {
        let root = mkenv(graph, None);
        let output = mknode(graph, output);
        let input = mknode(graph, input);
        let edge = mkedge(graph, root);
        graph.edge_mut(edge).out.push(output);
        graph.edge_mut(edge).input.push(input);
        graph.edge_mut(edge).set_input_partitions(1, 1);
        graph.edge_mut(edge).intermediate = true;
        nodeuse(graph, input, edge);
        graph.node_mut(output).generator = Some(edge);
        output
    }

    /// A chain of them: asking for one intermediate has to ask for the ones
    /// underneath it, which is the part the consumer alone cannot say. Nothing
    /// under `mid2` is dirty on its own account — both files are equally
    /// absent whichever way the timestamps fall.
    #[test]
    fn ronin_graph_asking_for_an_intermediate_asks_for_the_chain_below_it() {
        let mut graph = Graph::default();
        let first = generated(&mut graph, "mid1", "src");
        let second = generated(&mut graph, "mid2", "mid1");
        let out = generated(&mut graph, "out", "mid2");
        graph
            .edge_mut(graph.node(out).generator.unwrap())
            .intermediate = false;

        let settled = |graph: &Graph, source: i64| {
            let mtimes = BTreeMap::from([("src".to_owned(), source), ("out".to_owned(), 2)]);
            let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
            let mut runtime = RuntimeState::new(graph);
            let dirty = recompute_dirty_with(graph, &mut runtime, out, &mut stat).unwrap();
            (dirty, runtime)
        };

        let (dirty, runtime) = settled(&graph, 1);
        assert!(!dirty);
        assert!(!runtime.node(first).dirty());
        assert!(!runtime.node(second).dirty());

        let (dirty, runtime) = settled(&graph, 3);
        assert!(dirty);
        assert!(runtime.node(first).dirty());
        assert!(runtime.node(second).dirty());
    }
}
