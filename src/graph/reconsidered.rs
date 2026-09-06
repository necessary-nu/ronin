//! Which edges a scan is allowed to reconsider.
//!
//! A scan started from a set of consumers has to settle those consumers, and
//! everything beneath them is settled already — the scan that settled it ran
//! over a graph nothing has written to since. Naming the edges that can have
//! changed lets the walk stop at the boundary and read the answer standing
//! there, which is the answer a full descent would spend the whole graph
//! reaching.

use super::{EdgeId, Graph, MarkSet, NodeId};
use crate::runtime::RuntimeState;

/// The part of the graph a scan is allowed to reconsider.
///
/// The boundary is only trusted where there is an answer to read. A node no
/// scan has observed has nothing to stand on, and neither has a virtual output
/// no scan will ever stat, so the walk descends into either of them however
/// narrow the restriction is.
pub(crate) struct Reconsidered<'a> {
    edges: &'a MarkSet,
}

impl<'a> Reconsidered<'a> {
    pub(crate) const fn new(edges: &'a MarkSet) -> Self {
        Self { edges }
    }

    pub(super) fn settles(
        &self,
        graph: &Graph,
        runtime: &RuntimeState,
        node: NodeId,
        edge: EdgeId,
    ) -> bool {
        self.edges.contains(edge.index())
            || runtime.node(node).mtime().is_unobserved()
            || graph.is_virtual_output(node)
    }
}

#[cfg(test)]
mod tests {
    use super::Reconsidered;
    use crate::env::mkenv;
    use crate::graph::{
        Graph, MarkSet, NodeId, TraversalScratch, mkedge, mknode, nodeuse,
        recompute_dirty_with_validations,
    };
    use crate::runtime::{FileTime, RuntimeState};
    use std::collections::BTreeMap;
    use std::path::Path;

    /// `src` -> `mid` -> `out`, each by an edge of its own.
    fn chain(graph: &mut Graph) -> (NodeId, NodeId, NodeId) {
        let root = mkenv(graph, None);
        let source = mknode(graph, "src");
        let link = |graph: &mut Graph, input: NodeId, name: &str| {
            let output = mknode(graph, name);
            let edge = mkedge(graph, root);
            graph.edge_mut(edge).out.push(output);
            graph.edge_mut(edge).input.push(input);
            graph.edge_mut(edge).set_input_partitions(1, 1);
            nodeuse(graph, input, edge);
            graph.node_mut(output).generator = Some(edge);
            output
        };
        let middle = link(graph, source, "mid");
        let out = link(graph, middle, "out");
        (source, middle, out)
    }

    fn scan(
        graph: &Graph,
        runtime: &mut RuntimeState,
        target: NodeId,
        cone: Option<NodeId>,
        mtimes: &BTreeMap<String, i64>,
    ) {
        let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut marks = MarkSet::default();
        marks.begin(graph.edge_count());
        if let Some(node) = cone {
            marks.replace(graph.node(node).generator.unwrap().index());
        }
        let restriction = cone.map(|_| Reconsidered::new(&marks));
        recompute_dirty_with_validations(
            graph,
            runtime,
            &mut TraversalScratch::default(),
            std::slice::from_ref(&target),
            restriction.as_ref(),
            &mut stat,
        )
        .unwrap();
    }

    /// A scan told which edges can have changed reads the answer standing at
    /// the boundary rather than deriving it again. That is the whole of what
    /// makes a restat's propagation cost its consumers rather than the graph,
    /// and it is sound only because the caller collected every edge that reads
    /// what moved. `src` moving under a settled scan is the case that cannot
    /// happen while a build holds the graph, staged here so the two answers
    /// differ and the restriction is what decides between them.
    #[test]
    fn ronin_graph_restricted_scan_stands_on_settled_answers() {
        let mut graph = Graph::default();
        let (source, middle, out) = chain(&mut graph);
        let mtimes = BTreeMap::from([
            ("src".to_owned(), 1),
            ("mid".to_owned(), 2),
            ("out".to_owned(), 3),
        ]);
        let settled = |cone: Option<NodeId>| {
            let mut runtime = RuntimeState::new(&graph);
            scan(&graph, &mut runtime, out, None, &mtimes);
            assert!(!runtime.node(out).dirty());
            assert!(!runtime.node(middle).dirty());
            runtime.node_mut(source).observe(FileTime::observed(9));
            scan(&graph, &mut runtime, out, cone, &mtimes);
            runtime
        };

        // With nothing withheld, the scan re-derives `mid` against the date
        // `src` carries and finds it stale.
        let unrestricted = settled(None);
        assert!(unrestricted.node(middle).dirty());
        assert!(unrestricted.node(out).dirty());

        // Told that only `out`'s edge can have changed, it never asks about
        // `mid`'s and reads the answer `mid` already carries.
        let restricted = settled(Some(out));
        assert!(!restricted.node(middle).dirty());
        assert!(!restricted.node(out).dirty());
    }

    /// The boundary is trusted only where there is an answer standing on it. A
    /// node no scan has observed carries nothing, so the walk descends into it
    /// however narrow the restriction is — which is what keeps a consumer the
    /// build never scanned from being settled against a default.
    #[test]
    fn ronin_graph_restricted_scan_descends_into_unobserved() {
        let mut graph = Graph::default();
        let (_, middle, out) = chain(&mut graph);
        let mtimes = BTreeMap::from([("src".to_owned(), 9), ("out".to_owned(), 3)]);
        let mut runtime = RuntimeState::new(&graph);
        scan(&graph, &mut runtime, out, Some(out), &mtimes);
        assert!(
            runtime.node(middle).dirty(),
            "an unobserved node was stood on"
        );
        assert!(runtime.node(out).dirty());
    }
}
