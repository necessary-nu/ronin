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

    /// Whether a `::` record declares this node at all.
    pub(crate) fn is_double_colon_target(&self, node: NodeId) -> bool {
        self.double_colon_targets.contains_key(&node)
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
    let freshness = graph.deferred_freshness(edge);
    let deferred = freshness.map_or::<&[NodeId], _>(&[], |freshness| &freshness.outputs);
    let heads_the_group = freshness.is_some_and(|freshness| freshness.heads_the_group);
    for output in &graph.edge(edge).out {
        if graph.searched_at(*output).is_some() {
            settle(runtime, *output, settled);
        }
    }
    for output in deferred {
        if graph.searched_at(*output).is_some() {
            settle_shared(graph, runtime, *output, settled, heads_the_group);
        }
    }
}

/// Settle a target more than one edge answers about, which is a `::` chain's.
///
/// The chain is one name and many entries, walked in order, and GNU Make's
/// `update_file_1` ends an entry it does not have to remake by renaming that
/// entry and every entry after it — `while (file) { file->name = file->hname;
/// file = file->prev; }`, where `prev` chains forward through the record.
///
/// So the found path, once an entry is current, is the answer for the rest of
/// the chain — a LATER entry with work to do remakes the file where the earlier
/// one left it, and only an entry reached before any of them was current writes
/// the name as written. Nothing takes the rename back, which is what makes this
/// a latch rather than the last scan's opinion: an entry running here would
/// otherwise be answered by whichever of its neighbours was scanned last.
///
/// Said only of a `::` target, because it is the only name whose freshness more
/// than one edge decides. Every other deferred output has one edge behind it,
/// where a later scan is a better answer about the same question rather than a
/// different entry's answer to a different one.
fn settle_shared(
    graph: &Graph,
    runtime: &mut RuntimeState,
    node: NodeId,
    settled: SearchedName,
    heads_the_group: bool,
) {
    // The record's own second answer, which the latch above cannot be: the
    // rename reaches FORWARD, so what a dependent holds is the entry the record
    // was filed under and no later entry's rename can reach it. Written by the
    // head entry alone, and read by everything that is not an entry of this
    // record.
    if heads_the_group {
        settle_head(runtime, node, settled);
    }
    if matches!(settled, SearchedName::Unsettled)
        && graph.is_double_colon_target(node)
        && found_name_stands(runtime, node)
    {
        return;
    }
    settle(runtime, node, settled);
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

/// Settle what the FIRST of the edges deciding one node's freshness answered,
/// under the same terms as [`settle`].
///
/// No [`SearchedName::Written`] reaches this one, and it needs none. The other
/// map has one because a scan after the work reads the file the work wrote and
/// answers the other way; a scan cannot reach the head that way, because the
/// rescan a finished edge triggers walks downstream and the head of a record is
/// the one entry nothing in the record points back at. Measured: a `Written`
/// clause here fails not one case of the ported corpus.
fn settle_head(runtime: &mut RuntimeState, node: NodeId, settled: SearchedName) {
    let held = runtime
        .head_names
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

/// Which of a searched-for node's two names one edge reads.
///
/// One name, two answers, and which one an edge gets depends on whether it is
/// among the edges that decided the name. Told apart by whether the node is
/// one of this edge's own deferred outputs, which is what being an entry of
/// that record means here: every other edge — a dependent, and any edge with
/// no deferred outputs at all — is outside the record and takes the head's.
pub(crate) fn settled_name_stands(
    graph: &Graph,
    runtime: &RuntimeState,
    edge: EdgeId,
    node: NodeId,
) -> bool {
    if graph
        .deferred_freshness(edge)
        .is_some_and(|freshness| freshness.outputs.contains(&node))
    {
        return found_name_stands(runtime, node);
    }
    head_name_stands(runtime, node)
}

/// The same question as [`found_name_stands`], asked by a reader that is not
/// one of the edges deciding the node.
///
/// A `::` record is one name and many entries, and GNU Make gives the two
/// kinds of reader two different answers. An entry reads the latch — the
/// rename reaches forward, so each entry sees whatever the entries before it
/// settled. A DEPENDENT holds the `struct file` the hash table answers with,
/// which is the entry the record was filed under, so it reads the head's
/// verdict alone: a first entry that is remade keeps the name as written for
/// every dependent, however many entries after it renamed themselves.
///
/// Nothing recorded means no group answered about this node — a lone `::`
/// record, an ordinary deferred rule, a plain searched-for target — and then
/// the one answer is everybody's.
pub(crate) fn head_name_stands(runtime: &RuntimeState, node: NodeId) -> bool {
    runtime.head_names.get(&node).map_or_else(
        || found_name_stands(runtime, node),
        |head| matches!(head, SearchedName::Found),
    )
}
