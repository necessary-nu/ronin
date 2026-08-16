//! The waits that order an edge without dirtying it, and the ones whose
//! failure the waiter outlives.
//!
//! An order-only input is Ninja's answer to "start after this, but do not let
//! its timestamp decide whether I am stale". Both of the graph's wait kinds
//! also mean "after it *succeeded*", which is the right reading for a
//! manifest and the wrong one for a double-colon chain under `-k`: GNU Make
//! runs a target's later entries after an earlier entry failed. Forgiving a
//! wait narrows what it means without adding one — see [`crate::graph`]'s
//! `forgiven` module for where the narrowing is kept, and [`crate::build`]'s
//! `release` module for what acts on it.

use super::{BuildGraph, Edge, Node, nodeuse};

impl BuildGraph {
    /// Make `edge` wait for additional order-only inputs.
    ///
    /// Subninja composition uses this to preserve the parent recipe boundary:
    /// every prerequisite of the wrapper target finishes before any edge in
    /// the requested child subtree starts, without making that prerequisite
    /// part of the child's own timestamp dirtiness calculation.
    pub(crate) fn add_order_only_inputs(&mut self, edge: Edge, inputs: &[Node]) {
        for input in inputs {
            if self.arenas.edge(edge.0).input.contains(&input.0) {
                continue;
            }
            nodeuse(&mut self.arenas, input.0, edge.0);
            self.arenas.edge_mut(edge.0).input.push(input.0);
        }
    }

    /// Say that `edge` waits for these inputs only to be sequenced behind
    /// them, so a failure of what produced one does not block it.
    ///
    /// The inputs must already be inputs of the edge; this narrows what the
    /// wait means rather than adding one.
    pub(crate) fn forgive_order_inputs(&mut self, edge: Edge, inputs: &[Node]) {
        for input in inputs {
            self.arenas.forgive_order(edge.0, input.0);
        }
    }
}
