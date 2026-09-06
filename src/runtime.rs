//! Dense build-time state kept separate from manifest graph entities.

use crate::graph::{EdgeId, Graph, NodeId};
use std::num::NonZeroU64;
use std::ops::Range;

mod assumed;
mod deferred;
mod edge;

pub(crate) use assumed::{AssertedDates, AssumedNodes};
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
    /// Present, and older than anything a filesystem can answer, which is what
    /// GNU Make's `-o` writes over the date of the file it names: `OLD_MTIME`
    /// is 2 and sits one below `ORDINARY_MTIME_MIN` and one above
    /// `NONEXISTENT_MTIME` (filedef.h), so the name reads as there and as older
    /// than every real file — and what depends on it therefore does not rebuild.
    ///
    /// One rather than two because the two encodings differ by where absence
    /// sits: GNU Make spends 0 on "not yet asked" and 1 on "not there", and
    /// here the sentinel for "not yet asked" is negative, so absence is 0 and
    /// the smallest present moment is 1.
    pub(crate) const OLDEST: Self = Self(1);
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
        if !self.is_observed()
            || self.is_missing()
            || self.0 == Self::NEWEST.0
            || self.0 == Self::OLDEST.0
        {
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
}

/// The answers a graph walk asks of a name, held apart from the dates.
///
/// [`crate::graph`]'s descent through the prerequisites reads one of these per
/// prerequisite and reads nothing else about it, and a no-op over a composed
/// Linux kernel reads 1.1 billion of them. Among the dates they would be
/// thirty-two bytes apart and the graph's worth of them would be 1.6 MB, which
/// is larger than the cache that walk runs out of and is re-read in a
/// different order on every one of its several thousand passes. Here they are
/// one byte apart and the whole graph's worth is fifty kilobytes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NodeFlags(u8);

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
}

impl NodeFlags {
    const DIRTY: u8 = 1 << 0;
    const DYNDEP_PENDING: u8 = 1 << 1;
    const ABSENT_ON_DISK: u8 = 1 << 2;
    const INTERMEDIATE_PENDING: u8 = 1 << 3;

    const fn set(&mut self, flag: u8, value: bool) {
        if value {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }

    pub(crate) const fn dirty(self) -> bool {
        self.0 & Self::DIRTY != 0
    }

    pub(crate) const fn set_dirty(&mut self, dirty: bool) {
        self.set(Self::DIRTY, dirty);
    }

    /// Whether the last look at the filesystem found nothing under this name.
    ///
    /// Kept apart from [`NodeRuntime::mtime`] because the scan writes over that
    /// one: a file the graph is allowed not to have stands in the newest
    /// timestamp behind it, and a phony output stands in its inputs'. What was
    /// actually there is still the question GNU Make asks to decide a target
    /// must be made, so it is recorded where the syscall answers it and nowhere
    /// else — [`RuntimeState::observe`] is the only writer.
    pub(crate) const fn absent_on_disk(self) -> bool {
        self.0 & Self::ABSENT_ON_DISK != 0
    }

    pub(crate) const fn dyndep_pending(self) -> bool {
        self.0 & Self::DYNDEP_PENDING != 0
    }

    pub(crate) const fn set_dyndep_pending(&mut self, pending: bool) {
        self.set(Self::DYNDEP_PENDING, pending);
    }

    /// Whether the intermediate file this name stands for has work of its own
    /// left to do.
    ///
    /// The answer the scan reached about the file and then declined to pass on.
    /// `check_dep` (remake.c) asks an intermediate whether it is NEWER than the
    /// file being checked, never whether it is out of date, so an intermediate
    /// that is merely stale leaves its dependent alone — and only once the
    /// dependent has to be made for some other reason does `update_file_1`'s
    /// second loop come back and update it. This is what that second loop reads.
    ///
    /// It is an answer about the file rather than about the edge that makes it,
    /// so it is held on the name, beside the two other reasons a walk descends
    /// past a prerequisite. That is what keeps [`crate::graph`]'s descent to a
    /// single load per prerequisite: the generating edge is reached only for
    /// the prerequisite whose byte says one of the three is true.
    pub(crate) const fn intermediate_pending(self) -> bool {
        self.0 & Self::INTERMEDIATE_PENDING != 0
    }

    pub(crate) const fn set_intermediate_pending(&mut self, pending: bool) {
        self.set(Self::INTERMEDIATE_PENDING, pending);
    }
}

// [spec:ronin:req:runtime.typed-runtime-state]
#[derive(Default)]
pub(crate) struct RuntimeState {
    nodes: Vec<NodeRuntime>,
    node_flags: Vec<NodeFlags>,
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
    pub(crate) assumed_new: AssumedNodes,
    /// The nodes this scan answers about as though the file were older than
    /// everything, and had already been brought up to date, which is GNU
    /// Make's `-o`.
    ///
    /// Beside `assumed_new`, and the two are asked in that order because
    /// `main` stamps them in that order: `-o` writes `OLD_MTIME` first
    /// (main.c:2312) and `-W` writes `NEW_MTIME` over it (main.c:2325), so a
    /// name given to both is new whichever order the words were written in.
    ///
    /// Unlike `assumed_new` this reaches every pass. `-W` is withheld from a
    /// restarted read because an assumed-new makefile prerequisite would send
    /// the read around forever; an assumed-OLD one cannot, and GNU Make's
    /// stamp carries no `restarts` guard to match.
    pub(crate) assumed_old: AssumedNodes,
    /// For each node the graph gave a second place to look for, which of the
    /// two places this build has settled on. See [`crate::graph::searched`];
    /// absent means unsettled, which is every node of every graph but the few a
    /// directory search answered about.
    pub(crate) searched_names: crate::htab::RapidHashMap<NodeId, SearchedName>,
    /// The same question answered by the FIRST of the several edges that
    /// decide one such node's freshness, for the readers that are not among
    /// them.
    ///
    /// Held apart from `searched_names` rather than replacing it, because the
    /// two are different questions about one name and both are asked. See
    /// [`crate::graph::searched`]; absent means no group answered about the
    /// node, and then the one answer above is everybody's.
    pub(crate) head_names: crate::htab::RapidHashMap<NodeId, SearchedName>,
}

impl RuntimeState {
    pub(crate) fn new(graph: &Graph) -> Self {
        let mut state = Self::default();
        state.reset(graph);
        state
    }

    /// Exactly what [`Self::new`] would have made, in the allocations this one
    /// already holds.
    ///
    /// [`Self::reset`] deliberately keeps what a scan was ASKED — `-B`, `-W`,
    /// `-o` — because one state answers a graph twice under different switches
    /// and the answer belongs to the scan rather than to the graph. A caller
    /// recycling one state across scans that were asked DIFFERENT things wants
    /// none of that carried over, and a fresh state carries none.
    pub(crate) fn reset_asked(&mut self, graph: &Graph) {
        self.always_make = false;
        self.assumed_new = AssumedNodes::default();
        self.assumed_old = AssumedNodes::default();
        self.reset(graph);
    }

    pub(crate) fn reset(&mut self, graph: &Graph) {
        self.nodes
            .resize(graph.node_ids().len(), NodeRuntime::default());
        self.nodes.fill(NodeRuntime::default());
        self.node_flags
            .resize(graph.node_ids().len(), NodeFlags::default());
        self.node_flags.fill(NodeFlags::default());
        self.edges
            .resize(graph.edge_count(), EdgeRuntime::default());
        self.edges.fill(EdgeRuntime::default());
        self.deferred.clear();
        self.searched_names.clear();
        self.head_names.clear();
        // A reset is taken per freshness probe, and a Make composition takes
        // thousands of those against a graph that grows with every unit, so
        // nothing here may be proportional to the graph. The dyndep edges are
        // asked of the list the graph keeps rather than found by walking the
        // arena for them.
        for edge in graph.dyndep_edges() {
            if let Some(dyndep) = graph.edge(*edge).dyndep {
                self.flags_mut(dyndep).set_dyndep_pending(true);
            }
        }
    }

    pub(crate) fn synchronize(&mut self, graph: &Graph) -> Range<usize> {
        let old_node_count = self.nodes.len();
        let old_edge_count = self.edges.len();
        self.nodes
            .resize(graph.node_ids().len(), NodeRuntime::default());
        self.node_flags
            .resize(graph.node_ids().len(), NodeFlags::default());
        self.edges
            .resize(graph.edge_count(), EdgeRuntime::default());
        for edge in graph.edge_ids().skip(old_edge_count) {
            if let Some(dyndep) = graph.edge(edge).dyndep {
                self.flags_mut(dyndep).set_dyndep_pending(true);
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

    pub(crate) fn flags(&self, node: NodeId) -> NodeFlags {
        self.node_flags[node.index()]
    }

    pub(crate) fn flags_mut(&mut self, node: NodeId) -> &mut NodeFlags {
        &mut self.node_flags[node.index()]
    }

    /// Record what the filesystem answered for this name.
    ///
    /// The only way [`NodeFlags::absent_on_disk`] is written, which is what
    /// makes it mean the syscall rather than the scan: every other mtime a node
    /// acquires stands in for something and would spoil the answer. It is here
    /// rather than on either half because it is the one write that touches
    /// both.
    pub(crate) fn observe(&mut self, node: NodeId, mtime: FileTime) {
        self.nodes[node.index()].set_mtime(mtime);
        self.node_flags[node.index()].set(NodeFlags::ABSENT_ON_DISK, mtime.is_missing());
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

    /// Every flag is its own bit, and the walk reads three of them off one
    /// byte, so setting one must not disturb another and clearing one must
    /// leave the rest standing. Staleness and absence in particular have to be
    /// readable apart: an intermediate that is THERE and stale has work
    /// pending without ever having been absent, because `check_dep` forgives a
    /// stale intermediate the way it forgives an absent one and only the
    /// second is a file that has to be invented.
    #[test]
    fn each_node_flag_is_independent() {
        let mut flags = NodeFlags::default();
        assert!(!flags.dirty());
        assert!(!flags.dyndep_pending());
        assert!(!flags.absent_on_disk());
        assert!(!flags.intermediate_pending());

        flags.set_dirty(true);
        flags.set_dyndep_pending(true);
        flags.set(NodeFlags::ABSENT_ON_DISK, true);
        flags.set_intermediate_pending(true);
        assert!(flags.dirty());
        assert!(flags.dyndep_pending());
        assert!(flags.absent_on_disk());
        assert!(flags.intermediate_pending());

        flags.set(NodeFlags::ABSENT_ON_DISK, false);
        assert!(!flags.absent_on_disk());
        assert!(flags.intermediate_pending());
        assert!(flags.dirty());
        assert!(flags.dyndep_pending());
    }

    /// The absence answer means the syscall and not the scan, which is why the
    /// one write that touches both halves is the only way it is written: a
    /// date arriving any other way stands in for something else.
    #[test]
    fn only_a_disk_look_says_absent() {
        let mut graph = Graph::default();
        let node = mknode(&mut graph, BString::from("out"));
        let mut runtime = RuntimeState::new(&graph);

        runtime.observe(node, FileTime::MISSING);
        assert!(runtime.flags(node).absent_on_disk());

        // A date the scan wrote is not an answer about the disk.
        runtime.node_mut(node).set_mtime(FileTime::observed(7));
        assert!(runtime.flags(node).absent_on_disk());

        runtime.observe(node, FileTime::observed(7));
        assert!(!runtime.flags(node).absent_on_disk());
        assert_eq!(runtime.node(node).mtime(), FileTime::observed(7));
    }

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
        graph.set_edge_dyndep(edge, dyndep);
        let node_count = graph.node_ids().len();
        let edge_count = graph.edge_count();

        let mut runtime = RuntimeState::new(&graph);
        runtime.node_mut(output).set_mtime(FileTime::observed(42));
        runtime.flags_mut(output).set_dirty(true);
        runtime.edge_mut(edge).set_deps_loaded(true);
        runtime.edge_mut(edge).set_command_dirty(true);
        runtime.edge_mut(edge).set_restat_clean(true);
        runtime.reset(&graph);

        assert_eq!(graph.node_ids().len(), node_count);
        assert_eq!(graph.edge_count(), edge_count);
        assert!(runtime.node(output).mtime().is_unobserved());
        assert!(!runtime.flags(output).dirty());
        assert!(runtime.flags(dyndep).dyndep_pending());
        assert!(!runtime.edge(edge).deps_loaded());
        assert!(!runtime.edge(edge).command_dirty());
        assert!(!runtime.edge(edge).restat_clean());
    }
}
