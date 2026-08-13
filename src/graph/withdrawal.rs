//! The outputs a failed command must not leave behind.
//!
//! A Makefile's `.DELETE_ON_ERROR` says that a target whose recipe failed is
//! half-made, and that leaving it is worse than not having built it: the next
//! build finds a file with the right name and believes it. The property is
//! stated when the graph is constructed and honoured when a command fails.
//!
//! The eligible names rather than a switch on the edge, because Make's
//! exclusions are per output: `.PRECIOUS` protects one member of a grouped
//! recipe while its peers still go, and a `.PHONY` name stands for no file to
//! take back at all. Which of the eligible outputs actually goes is not a
//! question the graph can answer — it depends on what the recipe managed to
//! write before it failed — so the build decides that from timestamps.
//!
//! Nothing in a Ninja manifest says this, so a graph parsed from one carries
//! none of it. That is the bounded divergence `intermediate` and `disposable`
//! already have.

use super::{EdgeId, Graph, NodeId};
use crate::util::IdVec;

impl Graph {
    /// The outputs of `edge` that a failed command must not leave behind.
    ///
    /// Empty for every edge whose Makefile never said `.DELETE_ON_ERROR`, and
    /// for the outputs it said to keep anyway.
    pub(crate) fn delete_on_error(&self, edge: EdgeId) -> &[NodeId] {
        self.delete_on_error
            .get(&edge)
            .map_or(&[], |outputs| outputs.as_slice())
    }

    pub(crate) fn set_delete_on_error(&mut self, edge: EdgeId, outputs: IdVec<NodeId>) {
        if outputs.is_empty() {
            return;
        }
        self.delete_on_error.insert(edge, outputs);
    }
}
