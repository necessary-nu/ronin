//! Makefiles the read did not get: the ones it could not read, and the ones it
//! tried to remake and did not.
//!
//! A third state, between the two the graph can already express. A Makefile
//! that was remade is prebuilt: the goals must not run its recipe again, which
//! is what `mark_makefiles_remade` says. A Makefile with no rule is a file like
//! any other: the goals refuse over it if they need it and it is not there. The
//! one in between is a Makefile whose rule really ran, really lost, and was
//! forgiven because `-include` said the file need not be there — and the goals
//! must neither run that rule again nor believe the name means anything.
//!
//! GNU Make expresses it with two flags on the file rather than with a rule.
//! The makefile update sets `updated` and leaves `update_status` non-zero, and
//! `update_file_1` reads that pair back before it looks at anything else
//! (reference/gnumake/src/remake.c:459): a file recently tried and failed
//! returns its old status without reconsidering its commands, and `complain()`
//! then reports it as a target nothing knows how to make. So the diagnostic
//! names the missing prerequisite rather than the recipe that lost, and the
//! recipe runs once.
//!
//! Which is why this cannot be the file on disk. `-include one.mk` whose rule
//! writes the file and then exits non-zero leaves `one.mk` there, and GNU Make
//! still refuses over it when a goal asks: the verdict is about the update, not
//! about the bytes. An entry here therefore outranks the filesystem.
//!
//! Beside the node arena for the reason `validation_uses` and `withdrawal` are
//! beside theirs: no node in a Ninja manifest is ever in it, and almost no node
//! in a Makefile's graph either. A read that restarts clears nothing, because a
//! restart re-reads the Makefile and plans a new graph — which is also GNU
//! Make's answer, and why a forgiven rule that lost is attempted once per pass.

use super::{Graph, NodeId};

impl Graph {
    /// Whether the goals must treat `node` as a target nothing can make.
    pub(crate) fn is_unmade_makefile(&self, node: NodeId) -> bool {
        self.unmade_makefiles.contains(&node)
    }

    /// Record that this read tried to remake `node` and lost.
    ///
    /// Only the frontend that ran that update can say so, and only after it has
    /// settled: a pass that ends in a restart says nothing, because the read
    /// that follows it will try again.
    pub(crate) fn mark_makefile_unmade(&mut self, node: NodeId) {
        self.unmade_makefiles.insert(node);
    }

    /// Whether the read wanted `node`'s contents and did not get them, so every
    /// later question about the file must be answered as though nothing were
    /// there.
    pub(crate) fn is_unread_makefile(&self, node: NodeId) -> bool {
        self.unread_makefiles.contains(&node)
    }

    /// Record that the read could not read `node`.
    ///
    /// Said by the read rather than by the update, and it is the read's whole
    /// answer: `eval_makefile` writes the errno and `last_mtime =
    /// NONEXISTENT_MTIME` together (read.c:409) and returns. The two halves are
    /// separate here because a name nothing is at needs only the first — the
    /// filesystem already agrees — while a name something unreadable is at needs
    /// this one to stop the file it cannot use from passing for the file it
    /// wanted.
    pub(crate) fn mark_makefile_unread(&mut self, node: NodeId) {
        self.unread_makefiles.insert(node);
    }
}
