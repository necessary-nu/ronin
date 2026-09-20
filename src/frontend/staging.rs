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

use super::{BuildGraph, Edge, Node, PrebuiltMarks, Rule};

impl Node {
    /// Where this node sits in the graph's own arena.
    ///
    /// The one name every node has. A path does not serve: a recursive front
    /// end gives two units' identically spelt targets nodes of their own
    /// ([`BuildGraph::isolated_node`]), and such a node answers to no lookup
    /// at all. A record written beside one graph and refused unless the two
    /// still match can name a node by where it is.
    pub(crate) const fn at(self) -> usize {
        self.0.index()
    }
}

impl Edge {
    /// Where this edge sits in the graph's own arena. See [`Node::at`].
    pub(crate) const fn at(self) -> usize {
        self.0.index()
    }
}

impl BuildGraph {
    /// The node a record names, or `None` for a record naming a node this
    /// graph does not have.
    ///
    /// Bounds-checked rather than trusted: the record and the graph are one
    /// pair and are refused unless they still match, and a caller that reads
    /// a number out of a file gets a refusal rather than a panic.
    pub(crate) fn node_at(&self, at: usize) -> Option<Node> {
        self.arenas.node_at(at).map(Node)
    }

    /// The edge a record names. See [`Self::node_at`].
    pub(crate) fn edge_at(&self, at: usize) -> Option<Edge> {
        self.arenas.edge_at(at).map(Edge)
    }

    /// An edge's primary output, which is the name a record holds it under.
    pub(crate) fn output_of(&self, edge: Edge) -> Option<Node> {
        self.arenas.edge(edge.id()).out.first().copied().map(Node)
    }

    /// The first output of every edge this graph carries a prebuilt mark for.
    ///
    /// What a run that LOADED the graph has to build before the marks are
    /// true again. Taken off the marks rather than from a list of names,
    /// because the marks are the composition's own record of what it believed
    /// was already done and cover the whole closure of it, while a name is
    /// something a graph may hold twice or not index at all.
    pub(crate) fn prebuilt_outputs(&self) -> Vec<Node> {
        self.arenas
            .prebuilt_edges()
            .map(|edge| Node(self.arenas.edge(edge).out[0]))
            .collect()
    }

    /// Take off every prebuilt mark a graph read from a file was written with.
    ///
    /// The mark says the work behind an edge was done by THIS invocation, and
    /// a run that read the graph rather than composing it has done none of it.
    /// It comes off before anything is planned, so the work is reached and
    /// decided about against the disk as it stands, and goes back on with
    /// [`Self::remark_prebuilt`] once this run has built it.
    pub(crate) fn unmark_prebuilt(&mut self) -> PrebuiltMarks {
        PrebuiltMarks(self.arenas.unmark_prebuilt())
    }

    /// Put back what [`Self::unmark_prebuilt`] took off, for a run that has
    /// since built that work itself.
    pub(crate) fn remark_prebuilt(&mut self, marks: PrebuiltMarks) {
        self.arenas.remark_prebuilt(marks.0);
    }

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
