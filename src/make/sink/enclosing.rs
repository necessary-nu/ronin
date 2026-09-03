//! A file an enclosing unit makes, as a child sees it.
//!
//! A child compilation's targets are its own — two units may each define
//! `all`, and two isolated nodes keep them apart — but a FILE an enclosing
//! unit makes is one file, and GNU Make's child process finds it made: the
//! parent's phase that made it ran before the recipe that started the child.
//! So a child's name for such a path resolves to the enclosing node and
//! depends on that producer, and a rule of the child's own for it is not
//! read. zsh's every subdirectory writes `$(dir_top)/Src/zsh.mdh: ; false #
//! should only happen with make -n`, a stub for a file `Src` makes; read as a
//! generator of a private node it ran in a clean build, beside the rule that
//! makes the file, and failed.
//!
//! The two halves the emission of one edge needs live here: whether the edge
//! is such a rule, and what the edge makes for the children this unit will
//! compose. Which node a child's name resolves to is `GraphSink::node`'s,
//! and what a child is handed is [`GraphSink::begin_subninja`]'s.

use super::GraphSink;
use crate::frontend::Node;
use kati::build_sink::SinkEdge;

impl GraphSink {
    /// Whether this is a rule of the unit's own for a file an enclosing unit
    /// makes, which is not read: the file is made where it is named, before
    /// this unit's recipe could have started. See [`Self::begin_subninja`].
    /// The rule kati declared for it is taken back, so nothing is left
    /// waiting for an edge that never comes.
    pub(super) fn skips_an_enclosing_files_rule(
        &mut self,
        edge: &SinkEdge<'_>,
        outputs: &[Node],
        implicit_outputs: &[Node],
    ) -> bool {
        let names_one = outputs
            .iter()
            .chain(implicit_outputs)
            .any(|output| self.unit.enclosing_nodes.contains(output));
        if names_one && let Some(id) = edge.rule {
            self.subninja_rules.remove(&id);
            self.rules.remove(&id);
        }
        names_one
    }

    /// Record the files this rule makes, for the children this unit composes.
    ///
    /// Files only: a rule with a recipe, for a target that is not `.PHONY`.
    /// GNU Make's child process finds the parent's FILES made and nothing
    /// else — its `all`, its `FORCE`, its `clean` are its own, however the
    /// parent spelled them — so a phony target, or one with no recipe, is not
    /// something a child's name for the same spelling refers to.
    pub(super) fn record_generated(
        &mut self,
        edge: &SinkEdge<'_>,
        outputs: &[Node],
        implicit_outputs: &[Node],
    ) {
        if edge.rule.is_none() || edge.always_dirty {
            return;
        }
        for output in outputs.iter().chain(implicit_outputs) {
            self.unit
                .generated
                .insert(self.graph.path(*output).to_vec(), *output);
        }
    }
}
