//! Dense build-time state kept separate from manifest graph entities.

use crate::graph::{EdgeId, Graph, NodeId};
use std::num::NonZeroU64;
use std::ops::Range;

/// A filesystem timestamp with the unobserved sentinel hidden behind methods.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub(crate) struct FileTime(i64);

impl FileTime {
    const UNOBSERVED_RAW: i64 = -1;

    pub(crate) const UNOBSERVED: Self = Self(Self::UNOBSERVED_RAW);
    pub(crate) const MISSING: Self = Self(0);

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
}

impl Default for NodeRuntime {
    fn default() -> Self {
        Self {
            mtime: FileTime::UNOBSERVED,
            log_mtime: FileTime::UNOBSERVED,
            logged_command_hash: CommandHash::MISSING,
            dirty: false,
            dyndep_pending: false,
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

    pub(crate) const fn dyndep_pending(self) -> bool {
        self.dyndep_pending
    }

    pub(crate) const fn set_dyndep_pending(&mut self, pending: bool) {
        self.dyndep_pending = pending;
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(transparent)]
struct EdgeRuntimeFlags(u8);

impl EdgeRuntimeFlags {
    const DEPS_LOADED: u8 = 1 << 0;
    const DEPS_MISSING: u8 = 1 << 1;
    const COMMAND_DIRTY: u8 = 1 << 2;
    const RESTAT_CLEAN: u8 = 1 << 3;
    const COMMAND_HASH_VALID: u8 = 1 << 4;

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

    pub(crate) const fn restat_clean(self) -> bool {
        self.flags.contains(EdgeRuntimeFlags::RESTAT_CLEAN)
    }

    pub(crate) const fn set_restat_clean(&mut self, clean: bool) {
        self.flags.set(EdgeRuntimeFlags::RESTAT_CLEAN, clean);
    }
}

// [spec:ronin:req:runtime.typed-runtime-state]
#[derive(Default)]
pub(crate) struct RuntimeState {
    nodes: Vec<NodeRuntime>,
    edges: Vec<EdgeRuntime>,
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
        graph.node_mut(output).gen = Some(edge);
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
