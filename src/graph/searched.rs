//! A second place to look for a name the build file did not put here.
//!
//! GNU Make's directory search answering about a TARGET rather than about a
//! prerequisite. `f_mtime` searches for a target it cannot find here, hangs the
//! answer off the file object as `hname` beside the written `name`, and takes
//! the found file's date for the target; `update_file_1` then chooses between
//! the two names once the prerequisites have been updated. The choice cannot be
//! folded into the compiler — a target current when the Makefile was read is
//! made stale by a prerequisite's own recipe — so the graph carries the second
//! place and the build settles it.

use super::{EdgeId, Graph, NodeId};
use crate::error::GraphError;
use crate::runtime::{FileTime, RuntimeState, SearchedName};
use crate::util::{BString, ByteSlice as _};
use std::io;
use std::path::Path;

/// Which part of a settled name one reference stands for.
///
/// GNU Make reads a prerequisite in three forms — the name, the directory it
/// carries, and the name with that directory taken off — and a reference is
/// written for the form the recipe asked for, because a reference is one word
/// with no directory in it and halving it would answer about the reference.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SettledView {
    /// The whole name.
    Whole,
    /// The directory the name carries, and `.` for a name that carries none.
    Directory,
    /// The name with that directory taken off.
    Filename,
}

/// One name a front end could not write down, and the name it wrote instead.
pub(crate) struct SettledNameReference {
    /// The name the command reads the spelling from.
    pub(crate) variable: BString,
    /// The node it stands for.
    pub(crate) node: NodeId,
    /// Which part of that node's settled name to substitute.
    pub(crate) view: SettledView,
}

/// Every such reference in one edge's command, and where they are spelt from.
pub(crate) struct SettledNames {
    /// The directory the command runs in, and so the directory the names it
    /// reads are relative to. Empty for a command that runs where the build
    /// does, which is the common case.
    pub(crate) directory: BString,
    pub(crate) references: Vec<SettledNameReference>,
}

impl Graph {
    /// Where else `node` may be found, when it is not where it is named.
    pub(crate) fn searched_at(&self, id: NodeId) -> Option<&BString> {
        self.searched_at.get(&id)
    }

    /// The spellings this edge's command left for the build to fill in.
    pub(crate) fn settled_names(&self, edge: EdgeId) -> Option<&SettledNames> {
        self.settled_names.get(&edge)
    }

    /// Say that `edge`'s command carries references rather than names.
    pub(crate) fn set_settled_names(&mut self, edge: EdgeId, settled: SettledNames) {
        if settled.references.is_empty() {
            return;
        }
        self.settled_names.insert(edge, settled);
    }

    /// Say that `node` was found at `found`, and so is observed there for as
    /// long as nothing has written it here.
    pub(crate) fn set_searched_at(&mut self, id: NodeId, found: BString) {
        self.searched_at.insert(id, found);
    }

    /// Say that `node` is where the search moved a target the build file wrote
    /// as `written`.
    pub(crate) fn set_written_as(&mut self, id: NodeId, written: BString) {
        self.written_as.insert(id, written);
    }

    /// The node the build file wrote as `name`, for a name the search moved.
    ///
    /// `None` for every name that is its own node's, which is every name of
    /// every graph but the few `GPATH` renamed — so the walk is over a table
    /// that is nearly always empty, and the caller has already failed to find
    /// the name in the arena.
    pub(crate) fn moved_from_written(&self, name: &[u8]) -> Option<NodeId> {
        self.written_as
            .iter()
            .find_map(|(node, written)| (written.as_slice() == name).then_some(*node))
    }

    /// Whether the search moved this node from the name the build file wrote.
    pub(crate) fn was_moved_by_search(&self, node: NodeId) -> bool {
        self.written_as.contains_key(&node)
    }

    /// Say that a `::` record filed `node` under `declared`.
    pub(crate) fn set_double_colon_target(&mut self, node: NodeId, declared: BString) {
        self.double_colon_targets.insert(node, declared);
    }

    /// Whether a `::` record declares this node under the name `spelt`.
    ///
    /// True for both shapes such a record compiles to, because GNU Make's
    /// `enter_file` makes no distinction between them: a chain of one takes
    /// the fresh appended entry as readily as a chain of three. False for the
    /// path a `GPATH` rename moved the target to, which is the same node under
    /// a name the record never carried.
    pub(crate) fn declares_a_double_colon_record(&self, node: NodeId, spelt: &[u8]) -> bool {
        self.double_colon_targets
            .get(&node)
            .is_some_and(|declared| declared.as_slice() == spelt)
    }
}

/// Record which of a searched-for output's two names this scan settles on.
///
/// Half of GNU Make's fork in `update_file_1`, and the half a scan can reach: a
/// target it finds nothing to do for keeps the path the search returned, and
/// everything that reads the target reads that path. The other half — a target
/// that IS remade, which keeps the name as written — is said by the build as it
/// writes the file, because a scan runs again after the work and would then be
/// reading the very file the work wrote.
///
/// The deferred outputs are settled beside the edge's own, because a deferred
/// edge's real target is not what the edge is spelt as producing.
pub(super) fn settle_searched_outputs(
    graph: &Graph,
    runtime: &mut RuntimeState,
    edge: EdgeId,
    dirty: bool,
) {
    let settled = if dirty {
        SearchedName::Unsettled
    } else {
        SearchedName::Found
    };
    let deferred = graph
        .deferred_freshness(edge)
        .map_or::<&[NodeId], _>(&[], |freshness| &freshness.outputs);
    for output in graph.edge(edge).out.iter().chain(deferred) {
        if graph.searched_at(*output).is_some() {
            settle(runtime, *output, settled);
        }
    }
}

/// The date a node really has, once the second place to look has been
/// consulted.
///
/// GNU Make's `f_mtime`, in its order: the written name is stat'ed first, the
/// directory search runs only when that found nothing, and the found file's
/// date becomes the target's. So a target the build has since made here is read
/// here, and the search's answer is what stands until it does. Every path that
/// observes a node goes through this, because a scan that read one of them
/// through the search and another around it would answer two ways about one
/// file.
pub(crate) fn elsewhere_mtime<F>(
    graph: &Graph,
    node: NodeId,
    mtime: i64,
    stat: &mut F,
) -> Result<i64, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    if !FileTime::observed(mtime).is_missing() {
        return Ok(mtime);
    }
    let Some(found) = graph.searched_at(node) else {
        return Ok(mtime);
    };
    stat(found.to_path().expect("byte paths are valid on Unix")).map_err(|source| {
        GraphError::Stat {
            node,
            path: found.clone(),
            source,
        }
    })
}

/// Say that `edge` is about to write its outputs where they are named, for the
/// ones the front end gave a second place to look for.
///
/// Said as the edge is prepared and not by a later scan: the file is about to
/// be here, and every reading of the name after that finds it. GNU Make's
/// `ignore_vpath`, set once `update_file_1` has decided to remake and never
/// unset.
pub(crate) fn mark_written_here(graph: &Graph, runtime: &mut RuntimeState, edge: EdgeId) {
    for output in &graph.edge(edge).out {
        if graph.searched_at(*output).is_some() {
            settle(runtime, *output, SearchedName::Written);
        }
    }
}

/// Settle which of a searched-for node's names stands, unless the build has
/// already written the node here — which is the one answer nothing later may
/// take back.
fn settle(runtime: &mut RuntimeState, node: NodeId, settled: SearchedName) {
    let held = runtime
        .searched_names
        .entry(node)
        .or_insert(SearchedName::Unsettled);
    if matches!(held, SearchedName::Written) {
        return;
    }
    *held = settled;
}

/// Whether the build settled on the second place to look for `node`.
pub(crate) fn found_name_stands(runtime: &RuntimeState, node: NodeId) -> bool {
    matches!(runtime.searched_names.get(&node), Some(SearchedName::Found))
}
