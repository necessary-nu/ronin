//! What a compilation says about an edge it put in the graph before it knew
//! everything about it.
//!
//! A Makefile holding a recursive `$(MAKE)` is not compiled in one pass. The
//! wrapper edge for such a recipe exists before its children have been read,
//! because whether the recipe has to run at all is a question about that edge
//! and has to be answered first; what the edge finally carries is settled once
//! the children are composed. Between those two moments the compilation learns
//! things about the edge that no declaration could have carried, and this is
//! where it says them.
//!
//! Nothing in a Ninja manifest reaches here: a manifest states an edge whole,
//! and a graph parsed from one is never in the middle of being decided.

use super::{BuildGraph, Edge, Rule};

impl BuildGraph {
    /// Replace the command rule of an edge whose structure was staged first.
    pub(crate) fn set_edge_rule(&mut self, edge: Edge, rule: Rule) {
        self.arenas.edge_mut(edge.0).rule = Some(rule.0);
    }

    /// Record that part of this edge's recipe has already run.
    ///
    /// See [`crate::graph::Edge::recipe_begun`]: the lines of a recursive
    /// recipe written ahead of its `$(MAKE)` run at a compilation boundary,
    /// and one of them may write the very target this edge makes.
    pub(crate) fn mark_recipe_begun(&mut self, edge: Edge) {
        self.arenas.edge_mut(edge.0).recipe_begun = true;
    }
}
