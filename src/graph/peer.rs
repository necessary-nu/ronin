//! The outputs a recipe makes on the way to making something else.
//!
//! A GNU Make pattern rule that spells several target patterns — the bison
//! `%.tab.c %.tab.h: %.y` shape — is one recipe that writes all of them. But
//! Make still decides each of those names from that name alone: the search
//! matches one pattern, and the rest are entered as targets of their own and
//! merely marked updated when the recipe runs (`implicit.c`, `also_make`).
//!
//! So a peer nobody asked for is not part of the question "must this run".
//! Its absence is not a reason to run the recipe, and its presence is not a
//! reason to skip it; the outputs that were reached decide, and the peer is
//! whatever the recipe last left. It is not the build's to sweep up either:
//! entering it as a target is exactly what keeps it out of the intermediate
//! set, so a peer survives a build that deletes the intermediate beside it.
//!
//! A name reached in its own right stops being a peer, which the front end
//! settles while it walks the graph — by the time an edge is declared the list
//! holds only the names nothing asked for.
//!
//! Beside the edge arena for the reason `.DELETE_ON_ERROR`'s list is: most
//! graphs have no such edge at all, and nothing in a Ninja manifest can say it,
//! so a graph parsed from one carries none of it.

use super::{EdgeId, Graph, NodeId};
use crate::runtime::{FileTime, RuntimeState};
use crate::util::IdVec;

/// Which of `edge`'s outputs the recipe is being run on behalf of.
///
/// GNU Make reaches a pattern rule once per name it was asked about, and runs
/// the recipe the first time it reaches a name that has to be made — so `$@` is
/// that name, which need not be the first the rule writes. Everything the rule
/// also makes is written by the same run and was never asked about.
///
/// The peers are out of it for the reason they are out of the freshness test:
/// nobody asked for them, so no state of theirs could have sent the recipe to
/// the shell. Among the rest the answer is the first that is not there or is
/// behind what it is made from, in the order the graph reached them, and the
/// first of them when none is — an edge can be run for a reason no timestamp of
/// its own carries, and GNU Make answers that with the first name too.
pub(crate) fn trigger_output(
    graph: &Graph,
    runtime: &RuntimeState,
    edge: EdgeId,
) -> Option<NodeId> {
    let peers = graph.peer_outputs(edge);
    let mut reached = graph
        .edge(edge)
        .out
        .iter()
        .copied()
        .filter(|output| !peers.contains(output))
        .peekable();
    let first = *reached.peek()?;
    let newest_input = graph
        .edge(edge)
        .non_order_only_inputs()
        .iter()
        .map(|input| runtime.node(*input).mtime())
        .max()
        .unwrap_or(FileTime::MISSING);
    let out_of_date = reached.find(|output| {
        let state = runtime.node(*output);
        state.absent_on_disk() || state.mtime() < newest_input
    });
    Some(out_of_date.unwrap_or(first))
}

impl Graph {
    /// The outputs of `edge` that the recipe makes only as a side effect.
    ///
    /// Empty for every edge but one compiled from a pattern rule with more
    /// than one target pattern, and for the peers of one that something later
    /// asked for by name.
    pub(crate) fn peer_outputs(&self, edge: EdgeId) -> &[NodeId] {
        self.peer_outputs
            .get(&edge)
            .map_or(&[], |outputs| outputs.as_slice())
    }

    pub(crate) fn set_peer_outputs(&mut self, edge: EdgeId, outputs: IdVec<NodeId>) {
        if outputs.is_empty() {
            return;
        }
        self.peer_outputs.insert(edge, outputs);
    }

    /// Whether the build throws this output away once it has finished with it.
    pub(crate) fn is_disposable_output(&self, node: NodeId) -> bool {
        self.disposable_outputs.contains(&node)
    }

    /// Say that the build may sweep these outputs up.
    pub(crate) fn set_disposable_outputs(&mut self, outputs: &[NodeId]) {
        self.disposable_outputs.extend(outputs.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Graph, NodeId, mkedge, mknode};
    use super::trigger_output;
    use crate::env::mkenv;
    use crate::runtime::{FileTime, RuntimeState};
    use crate::util::IdVec;

    #[test]
    fn an_edge_without_peers_stores_nothing() {
        let mut graph = Graph::default();
        let scope = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, scope);
        graph.set_peer_outputs(edge, IdVec::new());
        assert!(graph.peer_outputs(edge).is_empty());

        let peer = mknode(&mut graph, b"x.tab.h");
        graph.set_peer_outputs(edge, IdVec::from(vec![peer]));
        assert_eq!(graph.peer_outputs(edge), &[peer]);
    }

    /// Which member the recipe is run on behalf of, from the same edge under
    /// three arrangements of the same files. A parsed manifest carries no such
    /// distinction — Ninja has one name per build statement and no notion of a
    /// rule reached once per output.
    #[test]
    fn ronin_graph_trigger_names_stale_member() {
        let mut graph = Graph::default();
        let scope = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, scope);
        let first = mknode(&mut graph, b"a.c");
        let second = mknode(&mut graph, b"a.h");
        let source = mknode(&mut graph, b"a.in");
        graph.edge_mut(edge).out.push(first);
        graph.edge_mut(edge).out.push(second);
        graph.edge_mut(edge).input.push(source);
        graph.edge_mut(edge).set_input_partitions(1, 1);

        let settled = |graph: &Graph, mtimes: [(NodeId, FileTime); 3]| {
            let mut runtime = RuntimeState::new(graph);
            for (node, mtime) in mtimes {
                runtime.node_mut(node).observe(mtime);
            }
            trigger_output(graph, &runtime, edge).unwrap()
        };

        let old = FileTime::observed(1);
        let new = FileTime::observed(2);

        // The first name is current and the second is not there, so it is the
        // second that reached the rule needing to be made.
        assert_eq!(
            settled(
                &graph,
                [(source, old), (first, new), (second, FileTime::MISSING)]
            ),
            second
        );

        // Nothing distinguishes them, so the run belongs to the first.
        assert_eq!(
            settled(
                &graph,
                [
                    (source, new),
                    (first, FileTime::MISSING),
                    (second, FileTime::MISSING)
                ]
            ),
            first
        );

        // The second is a peer nobody asked for, so no state of its own can
        // have sent the recipe to the shell — the answer is the first even
        // though the first is the current one.
        graph.set_peer_outputs(edge, IdVec::from(vec![second]));
        assert_eq!(
            settled(
                &graph,
                [(source, old), (first, new), (second, FileTime::MISSING)]
            ),
            first
        );
    }
}
