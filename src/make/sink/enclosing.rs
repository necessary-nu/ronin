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
//! "Finds it made" is the whole of the premise, and it is a claim about the
//! GROUND, which a rule's text cannot answer: `all: ; @echo RAN` has a recipe
//! and is not `.PHONY`, and writes no file called `all`. GNU Make's child
//! stats `all`, finds nothing there, and runs the recipe a second time. So the
//! ground is asked, at the one moment it can answer — see
//! [`GraphSink::made_and_absent`].
//!
//! The three questions one child's read asks live here: which node a child's
//! name resolves to, whether the edge under it is a rule the child does not
//! read, and what the edge makes for the children this unit will compose. What
//! a child is handed is [`GraphSink::begin_subninja`]'s.

use super::GraphSink;
use crate::frontend::Node;
use kati::build_sink::SinkEdge;
use std::os::unix::ffi::OsStrExt;

impl GraphSink {
    /// The enclosing unit's node for the path this child's node names, where
    /// the child's name is a name for the enclosing unit's file.
    ///
    /// `None` leaves the child with the isolated node it was given, which is a
    /// target of its own: either no enclosing unit makes this path, or one
    /// made it and the file is not there — see [`Self::made_and_absent`].
    pub(super) fn enclosing_node_for(&self, node: Node) -> Option<Node> {
        let enclosing = *self.unit.enclosing.get(self.graph.path(node))?;
        (!self.made_and_absent(enclosing)).then_some(enclosing)
    }

    /// Whether an enclosing unit has already made this node in this invocation
    /// and left no file behind — which is what GNU Make's child process finds
    /// when it stats the path, and its reason to run a rule of its own.
    ///
    /// Both halves are needed and neither alone will do. A path that is merely
    /// absent may be one no pass has reached yet: zsh's `Src/zsh.export` is
    /// absent while a subdirectory that names it is being composed and written
    /// long before that subdirectory's modules link, and a child that took its
    /// own `false` stub for it there would run the stub. A node that is merely
    /// prebuilt is the ordinary case this module exists for, where the file is
    /// on the ground and the child's rule is the stub that must not be read.
    ///
    /// The moment is the one moment the question has an answer. A `$(MAKE)`
    /// boundary is staged: the parent's prerequisites are built and the read
    /// starts again, so by the time a child is composed the work GNU Make ran
    /// before starting that child has run here too, and what it left is on the
    /// disk to be stat'd. `-n` writes nothing, and GNU's child under `-n` finds
    /// nothing made either.
    fn made_and_absent(&self, enclosing: Node) -> bool {
        if !self.prebuilt.contains(&enclosing) {
            return false;
        }
        let path = std::ffi::OsStr::from_bytes(self.graph.path(enclosing));
        !self.root_directory.join(path).exists()
    }

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
    /// something a child's name for the same spelling refers to. Whether the
    /// recipe puts the file on the ground is not a question the text answers,
    /// and it is asked of the ground instead: see [`Self::made_and_absent`].
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
