//! Dense build-time state kept separate from manifest graph entities.

use crate::graph::{EdgeId, Graph, NodeId};
use std::num::NonZeroU64;
use std::ops::Range;

mod assumed;
mod deferred;
mod edge;

pub(crate) use assumed::AssumedNew;
pub(crate) use deferred::DeferredRuntime;
pub(crate) use edge::EdgeRuntime;

/// A filesystem timestamp with the unobserved sentinel hidden behind methods.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub(crate) struct FileTime(i64);

impl FileTime {
    const UNOBSERVED_RAW: i64 = -1;

    pub(crate) const UNOBSERVED: Self = Self(Self::UNOBSERVED_RAW);
    pub(crate) const MISSING: Self = Self(0);
    /// Newer than anything a filesystem can answer, which is what GNU Make's
    /// `-W` writes over the date of the file it names: `NEW_MTIME` is
    /// `INTEGER_TYPE_MAXIMUM (FILE_TIMESTAMP)` (filedef.h) and the switch
    /// stamps it on the file rather than touching it.
    pub(crate) const NEWEST: Self = Self(i64::MAX);
    pub(crate) const fn observed(raw: i64) -> Self {
        debug_assert!(raw >= 0, "observed filesystem timestamps are nonnegative");
        Self(raw)
    }

    pub(crate) const fn raw(self) -> i64 {
        self.0
    }

    pub(crate) const fn is_unobserved(self) -> bool {
        self.0 == Self::UNOBSERVED_RAW
    }

    pub(crate) const fn is_missing(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn is_observed(self) -> bool {
        !self.is_unobserved()
    }

    /// This timestamp read as the newest moment the record it came from is
    /// consistent with.
    ///
    /// An archive index dates its members in whole seconds, so a member filed
    /// from an object written part way through a second reads as older than the
    /// object it is a copy of, and the archive is rewritten forever. GNU Make
    /// marks such a file `low_resolution_time` and rounds it up to the end of
    /// its second — but only where the file is the one being updated
    /// (reference/gnumake/src/remake.c, `update_file_1`: `this_mtime +=
    /// FILE_TIMESTAMPS_PER_S - 1 - ns`), never where it is a prerequisite of
    /// something else. That is what makes this a reading rather than a
    /// timestamp: the same file answers both ways depending on which side of
    /// the comparison it is on.
    ///
    /// Missing and unobserved answer for themselves. Neither is a moment.
    pub(crate) const fn to_end_of_second(self) -> Self {
        if !self.is_observed() || self.is_missing() || self.0 == Self::NEWEST.0 {
            return self;
        }
        Self(self.0 - self.0.rem_euclid(1_000_000_000) + 999_999_999)
    }
}

/// A Ninja command hash with the format's zero/missing value encapsulated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub(crate) struct CommandHash(Option<NonZeroU64>);

impl CommandHash {
    pub(crate) const MISSING: Self = Self(None);

    pub(crate) const fn from_raw(raw: u64) -> Self {
        match NonZeroU64::new(raw) {
            Some(hash) => Self(Some(hash)),
            None => Self::MISSING,
        }
    }

    pub(crate) const fn raw(self) -> u64 {
        match self.0 {
            Some(hash) => hash.get(),
            None => 0,
        }
    }

    pub(crate) const fn is_missing(self) -> bool {
        self.0.is_none()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeRuntime {
    mtime: FileTime,
    log_mtime: FileTime,
    logged_command_hash: CommandHash,
    dirty: bool,
    dyndep_pending: bool,
    absent_on_disk: bool,
}

/// Which of a searched-for node's two names the build has settled on.
///
/// GNU Make's `update_file_1` choosing between `file->name` and `file->hname`
/// once the prerequisites are updated: a target it finds nothing to do for
/// takes the found name and everything reading it reads that path, and one it
/// remakes throws the found name away. Ronin reaches the same fork twice over —
/// the scan can say a target need not be made, and the build can make it — so
/// the two answers are distinguished rather than folded into a flag.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SearchedName {
    /// The build has not settled it, or nothing was searched for here.
    #[default]
    Unsettled,
    /// The build found nothing to do, so the path the search returned stands.
    Found,
    /// The build made the node here, so the name as written stands.
    ///
    /// Final. A scan after the work reads the file the work wrote and would
    /// answer the other way, and GNU Make asks the question once — before the
    /// recipe, not after it.
    Written,
}

impl Default for NodeRuntime {
    fn default() -> Self {
        Self {
            mtime: FileTime::UNOBSERVED,
            log_mtime: FileTime::UNOBSERVED,
            logged_command_hash: CommandHash::MISSING,
            dirty: false,
            dyndep_pending: false,
            absent_on_disk: false,
        }
    }
}

impl NodeRuntime {
    pub(crate) const fn mtime(self) -> FileTime {
        self.mtime
    }

    pub(crate) const fn set_mtime(&mut self, mtime: FileTime) {
        self.mtime = mtime;
    }

    pub(crate) const fn log_mtime(self) -> FileTime {
        self.log_mtime
    }

    pub(crate) const fn set_log_mtime(&mut self, mtime: FileTime) {
        self.log_mtime = mtime;
    }

    pub(crate) const fn logged_command_hash(self) -> CommandHash {
        self.logged_command_hash
    }

    pub(crate) const fn set_logged_command_hash(&mut self, hash: CommandHash) {
        self.logged_command_hash = hash;
    }

    pub(crate) const fn dirty(self) -> bool {
        self.dirty
    }

    pub(crate) const fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    /// Record what the filesystem answered for this name.
    ///
    /// The only way [`Self::absent_on_disk`] is written, which is what makes it
    /// mean the syscall rather than the scan: every other mtime a node acquires
    /// stands in for something and would spoil the answer.
    pub(crate) const fn observe(&mut self, mtime: FileTime) {
        self.mtime = mtime;
        self.absent_on_disk = mtime.is_missing();
    }

    /// Whether the last look at the filesystem found nothing under this name.
    ///
    /// Kept apart from [`Self::mtime`] because the scan writes over that one: a
    /// file the graph is allowed not to have stands in the newest timestamp
    /// behind it, and a phony output stands in its inputs'. What was actually
    /// there is still the question GNU Make asks to decide a target must be
    /// made, so it is recorded where the syscall answers it and nowhere else.
    pub(crate) const fn absent_on_disk(self) -> bool {
        self.absent_on_disk
    }

    pub(crate) const fn dyndep_pending(self) -> bool {
        self.dyndep_pending
    }

    pub(crate) const fn set_dyndep_pending(&mut self, pending: bool) {
        self.dyndep_pending = pending;
    }
}

// [spec:ronin:req:runtime.typed-runtime-state]
#[derive(Default)]
pub(crate) struct RuntimeState {
    nodes: Vec<NodeRuntime>,
    edges: Vec<EdgeRuntime>,
    deferred: crate::htab::RapidHashMap<EdgeId, DeferredRuntime>,
    /// Whether this scan is answering GNU Make's `-B`: every edge that has a
    /// command is out of date and every prerequisite counts as changed,
    /// whatever the dates on disk say.
    ///
    /// It lives here rather than beside the options because the dirty walk is
    /// the only reader and reaches this state already, and because it belongs
    /// to a scan rather than to the graph the scan reads — the same graph is
    /// scanned once for the makefiles and once for the goals, and a Make run
    /// answers the two differently. Left alone by [`Self::reset`], which
    /// clears what a scan learned rather than what it was asked.
    pub(crate) always_make: bool,
    /// The nodes this scan answers about as though the file had just been
    /// written, which is GNU Make's `-W`.
    ///
    /// Beside `always_make` and for the same reason: it belongs to a scan
    /// rather than to the graph, and the makefile pass and the goal pass are
    /// answered differently — GNU Make stamps the `-W` files before the
    /// makefile update on a first read and only after it on a restart
    /// (main.c:2325, main.c:2837). Left alone by [`Self::reset`], which clears
    /// what a scan learned rather than what it was asked.
    pub(crate) assumed_new: AssumedNew,
    /// For each node the graph gave a second place to look for, which of the
    /// two places this build has settled on. See [`crate::graph::searched`];
    /// absent means unsettled, which is every node of every graph but the few a
    /// directory search answered about.
    pub(crate) searched_names: crate::htab::RapidHashMap<NodeId, SearchedName>,
}

impl RuntimeState {
    pub(crate) fn new(graph: &Graph) -> Self {
        let mut state = Self::default();
        state.reset(graph);
        state
    }

    pub(crate) fn reset(&mut self, graph: &Graph) {
        self.nodes
            .resize(graph.node_ids().len(), NodeRuntime::default());
        self.nodes.fill(NodeRuntime::default());
        self.edges
            .resize(graph.edge_count(), EdgeRuntime::default());
        self.edges.fill(EdgeRuntime::default());
        self.deferred.clear();
        self.searched_names.clear();
        for edge in graph.edge_ids() {
            if let Some(dyndep) = graph.edge(edge).dyndep {
                self.node_mut(dyndep).set_dyndep_pending(true);
            }
        }
    }

    pub(crate) fn synchronize(&mut self, graph: &Graph) -> Range<usize> {
        let old_node_count = self.nodes.len();
        let old_edge_count = self.edges.len();
        self.nodes
            .resize(graph.node_ids().len(), NodeRuntime::default());
        self.edges
            .resize(graph.edge_count(), EdgeRuntime::default());
        for edge in graph.edge_ids().skip(old_edge_count) {
            if let Some(dyndep) = graph.edge(edge).dyndep {
                self.node_mut(dyndep).set_dyndep_pending(true);
            }
        }
        old_node_count..self.nodes.len()
    }

    pub(crate) fn node(&self, node: NodeId) -> NodeRuntime {
        self.nodes[node.index()]
    }

    pub(crate) fn node_mut(&mut self, node: NodeId) -> &mut NodeRuntime {
        &mut self.nodes[node.index()]
    }

    pub(crate) fn edge(&self, edge: EdgeId) -> EdgeRuntime {
        self.edges[edge.index()]
    }

    pub(crate) fn edge_mut(&mut self, edge: EdgeId) -> &mut EdgeRuntime {
        &mut self.edges[edge.index()]
    }

    pub(crate) fn deferred(&self, edge: EdgeId) -> Option<&DeferredRuntime> {
        self.deferred.get(&edge)
    }

    pub(crate) fn deferred_mut(&mut self, edge: EdgeId) -> &mut DeferredRuntime {
        self.deferred.entry(edge).or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::graph::{mkedge, mknode};
    use crate::util::BString;

    // [spec:ronin:req:runtime.typed-runtime-state/test]
    #[test]
    fn runtime_reset_clears_transient_state_without_mutating_the_graph() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        let output = mknode(&mut graph, BString::from("out"));
        let dyndep = mknode(&mut graph, BString::from("out.dd"));
        graph.node_mut(output).generator = Some(edge);
        graph.edge_mut(edge).out.push(output);
        graph.edge_mut(edge).dyndep = Some(dyndep);
        let node_count = graph.node_ids().len();
        let edge_count = graph.edge_count();

        let mut runtime = RuntimeState::new(&graph);
        runtime.node_mut(output).set_mtime(FileTime::observed(42));
        runtime.node_mut(output).set_dirty(true);
        runtime.edge_mut(edge).set_deps_loaded(true);
        runtime.edge_mut(edge).set_command_dirty(true);
        runtime.edge_mut(edge).set_restat_clean(true);
        runtime.reset(&graph);

        assert_eq!(graph.node_ids().len(), node_count);
        assert_eq!(graph.edge_count(), edge_count);
        assert!(runtime.node(output).mtime().is_unobserved());
        assert!(!runtime.node(output).dirty());
        assert!(runtime.node(dyndep).dyndep_pending());
        assert!(!runtime.edge(edge).deps_loaded());
        assert!(!runtime.edge(edge).command_dirty());
        assert!(!runtime.edge(edge).restat_clean());
    }
}
