//! What a build learns about one edge while it runs.
//!
//! Separate from the manifest's own [`crate::graph::Edge`] for the same reason
//! [`crate::runtime::NodeRuntime`] is separate from a node: what a Makefile said
//! is fixed for the run, and what this scan concluded is not.

use crate::runtime::CommandHash;

#[derive(Clone, Copy, Debug, Default)]
#[repr(transparent)]
struct EdgeRuntimeFlags(u8);

impl EdgeRuntimeFlags {
    const DEPS_LOADED: u8 = 1 << 0;
    const DEPS_MISSING: u8 = 1 << 1;
    const COMMAND_DIRTY: u8 = 1 << 2;
    const RESTAT_CLEAN: u8 = 1 << 3;
    const COMMAND_HASH_VALID: u8 = 1 << 4;
    const ABSENT_INTERMEDIATE: u8 = 1 << 5;
    const INTERMEDIATE_PENDING: u8 = 1 << 6;

    const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    const fn set(&mut self, flag: u8, value: bool) {
        if value {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EdgeRuntime {
    command_hash: CommandHash,
    depfile_dependencies: usize,
    flags: EdgeRuntimeFlags,
}

impl Default for EdgeRuntime {
    fn default() -> Self {
        Self {
            command_hash: CommandHash::MISSING,
            depfile_dependencies: 0,
            flags: EdgeRuntimeFlags::default(),
        }
    }
}

impl EdgeRuntime {
    pub(crate) const fn command_hash(self) -> Option<CommandHash> {
        if self.flags.contains(EdgeRuntimeFlags::COMMAND_HASH_VALID) {
            Some(self.command_hash)
        } else {
            None
        }
    }

    pub(crate) const fn set_command_hash(&mut self, hash: CommandHash) {
        self.command_hash = hash;
        self.flags.set(EdgeRuntimeFlags::COMMAND_HASH_VALID, true);
    }

    pub(crate) const fn invalidate_command_hash(&mut self) {
        self.command_hash = CommandHash::MISSING;
        self.flags.set(EdgeRuntimeFlags::COMMAND_HASH_VALID, false);
    }

    pub(crate) const fn deps_loaded(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::DEPS_LOADED)
    }

    pub(crate) const fn set_deps_loaded(&mut self, loaded: bool) {
        self.flags.set(EdgeRuntimeFlags::DEPS_LOADED, loaded);
    }

    pub(crate) const fn deps_missing(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::DEPS_MISSING)
    }

    pub(crate) const fn set_deps_missing(&mut self, missing: bool) {
        self.flags.set(EdgeRuntimeFlags::DEPS_MISSING, missing);
    }

    pub(crate) const fn depfile_dependencies(self) -> usize {
        self.depfile_dependencies
    }

    pub(crate) const fn set_depfile_dependencies(&mut self, count: usize) {
        self.depfile_dependencies = count;
    }

    pub(crate) const fn command_dirty(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::COMMAND_DIRTY)
    }

    pub(crate) const fn set_command_dirty(&mut self, dirty: bool) {
        self.flags.set(EdgeRuntimeFlags::COMMAND_DIRTY, dirty);
    }

    /// Whether the last scan excused this edge's outputs for not being there,
    /// because they are intermediate: nothing reading them was called out of
    /// date for their absence, so anything that has to be rebuilt anyway must
    /// ask for them explicitly.
    pub(crate) const fn absent_intermediate(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::ABSENT_INTERMEDIATE)
    }

    pub(crate) const fn set_absent_intermediate(&mut self, absent: bool) {
        self.flags
            .set(EdgeRuntimeFlags::ABSENT_INTERMEDIATE, absent);
    }

    /// Whether this intermediate edge has work of its own left to do.
    ///
    /// The answer the scan reached about the edge and then declined to pass on.
    /// `check_dep` (remake.c) asks an intermediate whether it is NEWER than the
    /// file being checked, never whether it is out of date, so an intermediate
    /// that is merely stale leaves its dependent alone — and only once the
    /// dependent has to be made for some other reason does `update_file_1`'s
    /// second loop come back and update it. This is what that second loop reads.
    pub(crate) const fn intermediate_pending(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::INTERMEDIATE_PENDING)
    }

    pub(crate) const fn set_intermediate_pending(&mut self, pending: bool) {
        self.flags
            .set(EdgeRuntimeFlags::INTERMEDIATE_PENDING, pending);
    }

    pub(crate) const fn restat_clean(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::RESTAT_CLEAN)
    }

    pub(crate) const fn set_restat_clean(&mut self, clean: bool) {
        self.flags.set(EdgeRuntimeFlags::RESTAT_CLEAN, clean);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every flag is its own bit: setting one must not disturb any other, and
    /// clearing one must leave the rest standing. The bit field is the only
    /// reason this could ever go wrong, and it goes wrong silently.
    #[test]
    fn each_edge_flag_is_independent() {
        let mut edge = EdgeRuntime::default();
        assert!(!edge.deps_loaded());
        assert!(!edge.deps_missing());
        assert!(!edge.command_dirty());
        assert!(!edge.restat_clean());
        assert!(!edge.absent_intermediate());
        assert!(!edge.intermediate_pending());

        edge.set_deps_loaded(true);
        edge.set_deps_missing(true);
        edge.set_command_dirty(true);
        edge.set_restat_clean(true);
        edge.set_absent_intermediate(true);
        edge.set_intermediate_pending(true);
        assert!(edge.deps_loaded());
        assert!(edge.deps_missing());
        assert!(edge.command_dirty());
        assert!(edge.restat_clean());
        assert!(edge.absent_intermediate());
        assert!(edge.intermediate_pending());

        edge.set_absent_intermediate(false);
        assert!(!edge.absent_intermediate());
        assert!(edge.intermediate_pending());
        assert!(edge.deps_loaded());
        assert!(edge.command_dirty());
    }

    /// An intermediate that is THERE and stale is pending without ever having
    /// been absent, which is the whole reason the two answers are separate
    /// bits: `check_dep` forgives a stale intermediate the way it forgives an
    /// absent one, and only the second is a file that has to be invented.
    #[test]
    fn pending_work_needs_no_absence() {
        let mut edge = EdgeRuntime::default();
        edge.set_intermediate_pending(true);
        assert!(edge.intermediate_pending());
        assert!(!edge.absent_intermediate());
        edge.set_intermediate_pending(false);
        assert!(!edge.intermediate_pending());
    }

    /// The command hash carries its own validity rather than spending a value
    /// on the missing case, so invalidating it has to be visible.
    #[test]
    fn an_invalidated_hash_is_gone() {
        let mut edge = EdgeRuntime::default();
        assert!(edge.command_hash().is_none());
        edge.set_command_hash(CommandHash::from_raw(9));
        assert_eq!(edge.command_hash(), Some(CommandHash::from_raw(9)));
        edge.invalidate_command_hash();
        assert!(edge.command_hash().is_none());
    }

    /// A depfile's dependency count is read back by the scan that decides
    /// whether the recorded deps still describe the file.
    #[test]
    fn depfile_count_round_trips() {
        let mut edge = EdgeRuntime::default();
        assert_eq!(edge.depfile_dependencies(), 0);
        edge.set_depfile_dependencies(3);
        assert_eq!(edge.depfile_dependencies(), 3);
    }
}
