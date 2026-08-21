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

use super::{BuildGraph, Edge, Node, Rule};

impl BuildGraph {
    /// Replace the command rule of an edge whose structure was staged first.
    pub(crate) fn set_edge_rule(&mut self, edge: Edge, rule: Rule) {
        self.arenas.edge_mut(edge.0).rule = Some(rule.0);
    }

    /// Read an edge's outputs as the files they are rather than as names it
    /// stands in for.
    ///
    /// Said about an edge that turns out commandless without ever having been
    /// an alias: a recursive wrapper the compilation found current, whose
    /// outputs are on disk and are what everything reading them compares
    /// against. See [`crate::graph::Edge::outputs_unaliased`] for the two
    /// things a commandless edge can mean.
    pub(crate) fn unalias_outputs(&mut self, edge: Edge) {
        self.arenas.edge_mut(edge.0).outputs_unaliased = true;
    }

    /// Which of `roots` cannot be made through this graph yet, because making
    /// one reaches an edge the compilation has not finished with.
    ///
    /// A recursive wrapper whose children are not composed yet holds the
    /// freshness probe, and the probe's command is `false`. Anything that
    /// reaches such an edge has to wait for the pass that finishes it, so this
    /// is what a caller holds back rather than builds. Walked over the whole
    /// input closure, because a Makefile made from something a recursive
    /// recipe produces is as blocked as one the recipe makes itself.
    pub(crate) fn blocked_targets(&self, roots: &[Node], unfinished: &[Node]) -> Vec<Node> {
        if unfinished.is_empty() {
            return Vec::new();
        }
        let mut blocked = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for root in roots {
            let mut work = vec![root.0];
            seen.clear();
            while let Some(node) = work.pop() {
                if !seen.insert(node) {
                    continue;
                }
                if unfinished.iter().any(|target| target.0 == node) {
                    blocked.push(*root);
                    break;
                }
                let Some(edge) = self.arenas.node(node).generator else {
                    continue;
                };
                work.extend(self.arenas.edge(edge).input.iter().copied());
                work.extend(self.arenas.edge(edge).validation.iter().copied());
            }
        }
        blocked
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
