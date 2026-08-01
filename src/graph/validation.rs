//! Validation edges, held aside from the nodes they name.
//!
//! Ninja's `|@` validation clause is rare — a typical manifest has none at all
//! — but an inline list on every node cost twenty-four bytes each whether the
//! feature was used or not, a third of `Node`, on the largest structure a big
//! manifest builds. A side map charges only the nodes that use it.

use super::{EdgeId, Graph, NodeId};

impl Graph {
    /// Edges validated by `node`, empty for the nodes that have none.
    pub(crate) fn node_validation_uses(&self, node: NodeId) -> &[EdgeId] {
        self.validation_uses.get(&node).map_or(&[], |edges| edges)
    }

    pub(crate) fn add_validation_use(&mut self, node: NodeId, edge: EdgeId) {
        self.validation_uses.entry(node).or_default().push(edge);
    }
}
