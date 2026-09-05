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

use super::{BuildGraph, Edge, Node, NodeId, nodeuse};
use crate::htab::{RapidHashMap, RapidHashSet};

impl BuildGraph {
    /// Everything the edge that makes `node` waits for, of either kind, and
    /// nothing at all for a node this graph does not make.
    ///
    /// One step of the walk that finds which of two recursive recipes has to
    /// be composed first when only ordinary targets stand between them: what
    /// the later one names is not what the earlier one produces, and the
    /// distance between the two is measured here.
    pub(crate) fn prerequisites_of(&self, node: Node) -> Vec<Node> {
        let Some(edge) = self.arenas.node(node.0).generator else {
            return Vec::new();
        };
        self.arenas
            .edge(edge)
            .input
            .iter()
            .copied()
            .map(Node)
            .collect()
    }

    /// Which of `candidates` are made only after one of `marks` has been:
    /// the nodes whose making waits, however far along the chain, for
    /// something in `marks`.
    ///
    /// One walk answers for every candidate at once. The question is asked of
    /// a whole set — every file an enclosing unit makes, against the outputs
    /// of one recursive recipe — and asking it a candidate at a time restarts
    /// the walk for each, which on a graph this size is the composition's cost
    /// rather than a lookup's.
    ///
    /// Order-only prerequisites count: they are a wait, and a wait is the
    /// whole of what is being measured. A node this graph does not make waits
    /// for nothing.
    pub(crate) fn waiting_on(
        &self,
        candidates: impl Iterator<Item = Node>,
        marks: &[Node],
    ) -> RapidHashSet<Node> {
        let mut settled = RapidHashMap::default();
        for mark in marks {
            settled.insert(mark.0, true);
        }
        let mut waiting = RapidHashSet::default();
        let mut walk = Vec::new();
        let mut open = RapidHashSet::default();
        for candidate in candidates {
            if self.settle_wait(candidate, &mut settled, &mut walk, &mut open) {
                waiting.insert(candidate);
            }
        }
        waiting
    }

    /// Whether `start` waits for anything already settled true, filling in
    /// the answer for every node the walk passes through.
    ///
    /// Iterative because the chain between a goal and a leaf source is as deep
    /// as the Makefile made it, and a node reached again while it is still
    /// open is one the walk is already inside: a cycle answers for nothing,
    /// and reading it as no wait leaves the nodes around it decided by their
    /// other prerequisites.
    fn settle_wait(
        &self,
        start: Node,
        settled: &mut RapidHashMap<NodeId, bool>,
        walk: &mut Vec<(NodeId, bool)>,
        open: &mut RapidHashSet<NodeId>,
    ) -> bool {
        walk.clear();
        walk.push((start.0, false));
        while let Some((node, leaving)) = walk.pop() {
            if leaving {
                let waits = self.arenas.node(node).generator.is_some_and(|edge| {
                    self.arenas
                        .edge(edge)
                        .input
                        .iter()
                        .any(|input| settled.get(input).copied().unwrap_or(false))
                });
                settled.insert(node, waits);
                open.remove(&node);
                continue;
            }
            if settled.contains_key(&node) || !open.insert(node) {
                continue;
            }
            walk.push((node, true));
            if let Some(edge) = self.arenas.node(node).generator {
                walk.extend(
                    self.arenas
                        .edge(edge)
                        .input
                        .iter()
                        .map(|input| (*input, false)),
                );
            }
        }
        settled.get(&start.0).copied().unwrap_or(false)
    }

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
