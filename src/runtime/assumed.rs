//! The nodes a scan answers about as though the file had just been written.
//!
//! GNU Make's `-W` does not touch the file it names: `main` stamps
//! `NEW_MTIME` on it (main.c:2325), so the name reads as present and newer
//! than everything downstream of it, and its own rule sees a target newer than
//! its prerequisites. This is that stamp, kept per scan because the makefile
//! pass and the goal pass are two scans over one graph and GNU Make answers
//! them differently.

use crate::graph::NodeId;

/// Which nodes of one scan carry the assumed-new stamp.
///
/// A bitmap over node indices rather than a set, because every stat asks and
/// almost none of them is one of these.
#[derive(Clone, Debug, Default)]
pub(crate) struct AssumedNew(Vec<bool>);

impl AssumedNew {
    /// Stamp these nodes, sizing for the graph as it stands.
    ///
    /// Sized here rather than where the scan's other state is cleared, because
    /// the graph may have grown since the ask — a dyndep or a discovered
    /// dependency adds nodes — and a node added afterwards was never one of the
    /// names.
    pub(crate) fn mark(&mut self, nodes: &[NodeId], node_count: usize) {
        if nodes.is_empty() {
            return;
        }
        self.0.resize(node_count, false);
        for node in nodes {
            if let Some(slot) = self.0.get_mut(node.index()) {
                *slot = true;
            }
        }
    }

    /// Whether this scan was told to answer about `node` that way.
    pub(crate) fn contains(&self, node: NodeId) -> bool {
        self.0.get(node.index()).copied().unwrap_or(false)
    }
}
