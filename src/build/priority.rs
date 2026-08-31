//! What the ready queue sorts by.
//!
//! Ninja's scheduling priority, ported whole: `Edge::critical_path_weight`
//! (graph.h), the `EdgeWeightHeuristic` that fills it (build.cc), and the
//! `EdgePriorityLess` that reads it. Kept together and apart from the plan
//! because they are one idea — how long the rest of the build has to wait for
//! this edge — and because the plan is a big enough file without them.

use crate::graph::{EdgeId, Graph};
use std::cmp::Reverse;

/// One entry of the ready queue, ordered so that the greatest is the one to
/// run next: highest priority first, and among equals the lowest edge
/// identifier, which is Ninja's `EdgePriorityLess` (graph.h) exactly.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ReadyEdge {
    priority: SchedulePriority,
    edge: Reverse<EdgeId>,
}

impl ReadyEdge {
    pub(super) const fn new(priority: SchedulePriority, edge: EdgeId) -> Self {
        Self {
            priority,
            edge: Reverse(edge),
        }
    }

    pub(super) const fn edge(self) -> EdgeId {
        self.edge.0
    }
}

/// How much of the graph is still waiting behind an edge, in commands.
///
/// Ninja's `Edge::critical_path_weight`, and computed from the same recurrence:
/// an edge is worth its own cost plus the most any consumer of its outputs is
/// worth, so the number is the longest run of commands between this edge and a
/// final output. Its unit is a command, not a duration — Ninja weighed the path
/// by recorded build-log times while the change was in review and deleted that
/// before merging it (29fe3ef, "Simplify scheduler to not use build
/// log/execution time"), so the shipped heuristic is a count and this matches
/// the shipped one.
///
/// Ninja's numbers exactly, sentinel included, so that a weight read out of
/// this arena can be compared against one printed by the oracle.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub(super) struct CriticalPathWeight(i64);

impl Default for CriticalPathWeight {
    fn default() -> Self {
        Self::UNCOMPUTED
    }
}

impl CriticalPathWeight {
    /// No walk has weighed this edge yet. Ninja's `-1`, and below every real
    /// weight for the same reason: a phony edge with no consumer weighs 0, so
    /// zero is a real answer and cannot double as the absence of one.
    pub(super) const UNCOMPUTED: Self = Self(-1);

    /// What a goal's own consumer would have been worth, had it had one.
    pub(super) const ROOT: Self = Self(0);

    /// This weight as read by an edge that costs `cost` to run.
    pub(super) const fn plus(self, cost: i64) -> Self {
        Self(self.0.saturating_add(cost))
    }

    pub(super) const fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }

    /// The number itself, for the test that holds it against the oracle's.
    #[cfg(test)]
    pub(super) const fn raw(self) -> i64 {
        self.0
    }
}

/// What one edge is worth to the length of a path through it.
///
/// Ninja's `EdgeWeightHeuristic` (build.cc): a phony edge runs no command, so
/// it costs nothing and a chain of them is no longer than the commands at its
/// ends. Everything else costs one.
pub(super) fn edge_cost(graph: &Graph, edge: EdgeId) -> i64 {
    i64::from(!graph.is_phony_rule(graph.edge(edge).rule))
}

/// Which ready edge the plan hands over next.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ScheduleOrder {
    /// Longest chain of commands first, which is Ninja's, at every job count.
    #[default]
    CriticalPath,
    /// The order GNU Make's recursion reaches the recipes: down through each
    /// prerequisite list in turn, left to right, a target after its
    /// prerequisites.
    ///
    /// Nothing computes that order here, because the front end already walked
    /// it. The Make front end composes the graph by descending the goal chain
    /// and each target's prerequisite list, emitting edges as it goes, so an
    /// edge's identifier IS its position in GNU Make's recursion — and taking
    /// the lowest-numbered ready edge is therefore taking the recipe GNU Make
    /// would reach next. Two things follow that a separately computed walk
    /// order would have lost. `--shuffle` is carried for free: GNU Make
    /// permutes the goal chain and each prerequisite list before anything
    /// walks them (`shuffle_goaldeps_recursive`), the front end permutes the
    /// same lists before it composes, and the identifiers come out permuted
    /// with them. And a graph the front end did not compose — a Ninja manifest,
    /// where identifiers are declaration order and mean nothing of the sort —
    /// cannot reach this arm, because only the Make front end asks for it.
    ///
    /// Only ever asked for where one command runs at a time, because that is
    /// the only place GNU Make fixes an order at all — above `-j1` it says
    /// plainly that the order is not defined, and Ninja's is better there.
    Prerequisite,
}

impl ScheduleOrder {
    /// The queue key an edge of this weight gets under this order.
    pub(super) const fn priority(self, weight: CriticalPathWeight) -> SchedulePriority {
        match self {
            Self::CriticalPath => SchedulePriority(weight.0),
            // Flat, so the identifier alone decides and the queue hands back
            // the lowest-numbered edge that is ready. See the variant for why
            // that number is the order GNU Make would reach the recipes in.
            Self::Prerequisite => SchedulePriority::FLAT,
        }
    }
}

/// The ready queue's sort key, greatest first.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub(super) struct SchedulePriority(i64);

impl SchedulePriority {
    /// The same for every edge, leaving the queue's tie key to decide alone.
    pub(super) const FLAT: Self = Self(0);
}
