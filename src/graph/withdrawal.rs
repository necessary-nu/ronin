//! The outputs a stopped command may be made to give back.
//!
//! A recipe that stops partway leaves a file with the right name and the wrong
//! contents, and the next build finds it and believes it. GNU Make takes such a
//! file back for two independent reasons — `job.c` asks
//! `if (exit_sig != 0 || delete_on_error)` — and both reasons reach the same
//! `delete_target`, which is what decides whether a given name may go at all.
//!
//! So the graph carries the two halves separately. The eligible names are the
//! ones `delete_target` would not refuse, and they are stated whatever the
//! Makefile said, because a recipe killed by a signal is cleaned up after
//! without being asked. `.DELETE_ON_ERROR` is the other half: the answer to
//! whether an ordinary non-zero exit is also reason enough.
//!
//! The eligible names rather than a switch, because Make's exclusions are per
//! output: `.PRECIOUS` protects one member of a grouped recipe while its peers
//! still go, and a `.PHONY` name stands for no file to take back at all. Which
//! of the eligible outputs actually goes is not a question the graph can answer
//! — it depends on what the recipe managed to write before it stopped — so the
//! build decides that from timestamps.
//!
//! Nothing in a Ninja manifest says any of this, so a graph parsed from one
//! carries no entry at all, and no entry means no exclusions rather than no
//! outputs: Ninja withdraws everything a cut-short command wrote. That is the
//! bounded divergence `intermediate` and `disposable` already have.

use super::{EdgeId, Graph, NodeId};
use crate::util::IdVec;

/// What one edge's stopped command may give back, and when it must.
#[derive(Clone, Debug, Default)]
pub(crate) struct Withdrawal {
    /// The outputs a stopped recipe may be made to give back.
    pub(crate) outputs: IdVec<NodeId>,
    /// Whether an ordinary failure is reason enough to take them.
    pub(crate) on_error: bool,
}

impl Graph {
    /// What `edge`'s stopped command may give back, or `None` when no front end
    /// narrowed it.
    pub(crate) fn withdrawal(&self, edge: EdgeId) -> Option<&Withdrawal> {
        self.withdrawal.get(&edge)
    }

    /// The outputs of `edge` that an ordinary failure must not leave behind.
    ///
    /// Empty for every edge whose Makefile never said `.DELETE_ON_ERROR`, and
    /// for the outputs it said to keep anyway.
    pub(crate) fn delete_on_error(&self, edge: EdgeId) -> &[NodeId] {
        self.withdrawal(edge)
            .filter(|withdrawal| withdrawal.on_error)
            .map_or(&[], |withdrawal| withdrawal.outputs.as_slice())
    }

    /// State what a stopped command may give back, and whether an ordinary
    /// failure is reason enough.
    ///
    /// Recorded even when the list is empty and the switch is off, because the
    /// absence of an entry is what says a manifest front end left the question
    /// alone; a Makefile that narrows an edge to nothing has answered it.
    pub(crate) fn set_withdrawal(&mut self, edge: EdgeId, outputs: IdVec<NodeId>, on_error: bool) {
        self.withdrawal
            .insert(edge, Withdrawal { outputs, on_error });
    }
}
