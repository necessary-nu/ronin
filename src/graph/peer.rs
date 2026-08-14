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
use crate::util::IdVec;

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
}

#[cfg(test)]
mod tests {
    use super::super::{Graph, mkedge, mknode};
    use crate::env::mkenv;
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
}
