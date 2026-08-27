//! What a front end says about an edge's outputs beyond the fact that it
//! writes them.
//!
//! Ninja's graph knows one thing about an output: some edge produces it. GNU
//! Make knows three more, and none of them has a manifest spelling — which of
//! them a stopped recipe may be made to give back, which of them the recipe
//! writes only on the way to something that was asked for, and which of them
//! the build throws away once it has finished with them. A graph parsed from a
//! manifest carries none of these and keeps Ninja's answers, which is the same
//! bounded divergence `intermediate` already has.
//!
//! Two of the three are said of the OUTPUTS rather than of the edge, and
//! deliberately: GNU Make asks both questions per file, so one action can write
//! a name it keeps beside a name it takes away.

use super::{BuildGraph, Edge, Node};

impl BuildGraph {
    /// Name the outputs of `edge` a stopped command may be made to give back,
    /// and say whether an ordinary failure is reason enough to take them.
    ///
    /// The eligible names rather than a switch: `.PRECIOUS` and `.PHONY` take
    /// individual outputs off the list, so a grouped recipe may have to leave
    /// one member and withdraw the rest. They are named whatever `on_error`
    /// says, because a recipe killed by a signal is cleaned up after without
    /// `.DELETE_ON_ERROR` having asked.
    ///
    /// Nothing in a Ninja manifest says this, so a graph parsed from one never
    /// calls this at all — the same bounded divergence `intermediate` and
    /// `set_disposable_outputs` already have — and an edge nobody answered for keeps
    /// Ninja's answer, which is that a cut-short command gives everything back.
    pub(crate) fn set_withdrawal(&mut self, edge: Edge, outputs: Vec<Node>, on_error: bool) {
        self.arenas.set_withdrawal(
            edge.0,
            outputs.into_iter().map(|node| node.0).collect(),
            on_error,
        );
    }

    /// Declare which of `edge`'s outputs the recipe makes only on the way to
    /// making one that was asked for — GNU Make's `also_make`.
    ///
    /// They stay outputs: the edge is what writes them, and a failed recipe
    /// withdraws them like any other. What they are not is part of the question
    /// the edge answers before it runs, nor part of what the build sweeps up
    /// afterwards. An empty list stores nothing.
    ///
    /// Nothing in a Ninja manifest says this, so a graph parsed from one never
    /// carries it — the same bounded divergence `intermediate` and
    /// `set_disposable_outputs` already have.
    pub(crate) fn set_peer_outputs(&mut self, edge: Edge, outputs: Vec<Node>) {
        self.arenas
            .set_peer_outputs(edge.0, outputs.into_iter().map(|node| node.0).collect());
    }

    /// Declare which outputs the build throws away once it has finished with
    /// them, which is every intermediate but a `.SECONDARY` one and a goal.
    ///
    /// Said of the OUTPUTS rather than of the edge, because that is where GNU
    /// Make decides it: one rule with several target patterns writes several
    /// names and asks `!is_explicit` of each of them (implicit.c), so a name the
    /// implicit search invented and a name the makefile mentioned can come off
    /// one action with different answers.
    ///
    /// Nothing in a Ninja manifest says this, so a graph parsed from one never
    /// carries it — the same bounded divergence `intermediate` already has.
    pub(crate) fn set_disposable_outputs(&mut self, outputs: &[Node]) {
        let outputs: Vec<_> = outputs.iter().map(|node| node.0).collect();
        self.arenas.set_disposable_outputs(&outputs);
    }
}
