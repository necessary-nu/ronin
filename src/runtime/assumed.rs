//! The nodes a scan answers about from a date the invocation asserted.
//!
//! GNU Make's `-W` and `-o` do not touch the file they name: `main` stamps a
//! date on it and the filesystem is never asked (main.c:2312, main.c:2325).
//! `-W` writes `NEW_MTIME`, so the name reads as present and newer than
//! everything downstream of it and its own rule sees a target newer than its
//! prerequisites; `-o` writes `OLD_MTIME`, so the name reads as present and
//! older than every real file and what depends on it stays as it is. This is
//! that stamp, one set per kind, kept per scan because the makefile pass and
//! the goal pass are two scans over one graph and GNU Make can answer them
//! differently.

use crate::graph::{Graph, NodeId};
use crate::util::BString;

/// Which nodes of one scan carry one of those stamps.
///
/// A bitmap over node indices rather than a set, because every stat asks and
/// almost none of them is one of these.
#[derive(Clone, Debug, Default)]
pub(crate) struct AssumedNodes(Vec<bool>);

impl AssumedNodes {
    /// Stamp these nodes, sizing for the graph as it stands.
    ///
    /// Sized here rather than where the scan's other state is cleared, because
    /// the graph may have grown since the ask — a dyndep or a discovered
    /// dependency adds nodes — and a node added afterwards was never one of the
    /// names.
    pub(crate) fn mark(&mut self, nodes: &[NodeId], node_count: usize) {
        if nodes.is_empty() {
            return;
        }
        self.0.resize(node_count, false);
        for node in nodes {
            if let Some(slot) = self.0.get_mut(node.index()) {
                *slot = true;
            }
        }
    }

    /// Whether this scan was told to answer about `node` that way.
    pub(crate) fn contains(&self, node: NodeId) -> bool {
        self.0.get(node.index()).copied().unwrap_or(false)
    }

    /// Every node stamped, in node order.
    ///
    /// Walked through the graph's own identifiers rather than minted from the
    /// bitmap's indices, because holding a `NodeId` is evidence that its slot
    /// exists and this side table is not the arena.
    pub(crate) fn marked<'a>(&'a self, graph: &'a Graph) -> impl Iterator<Item = NodeId> + 'a {
        graph.node_ids().filter(move |node| self.contains(*node))
    }
}

/// The dates one invocation asserted, by the switch that asserted them.
///
/// The two lists travel together because every question that reads one reads
/// the other, and because they are the same shape: two `&[BString]` parameters
/// side by side would swap silently and answer every cell backwards.
///
/// Names rather than nodes, because the graph a scan reads is not the graph
/// this was asked of: one Make run scans the same graph more than once and a
/// name is looked up in each. A name the graph does not hold is nothing, which
/// is GNU Make's answer too — `enter_file` makes a file nothing depends on.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AssertedDates<'a> {
    /// What `-W` named: read as present and newer than everything.
    pub(crate) new: &'a [BString],
    /// What `-o` named: read as present, older than everything, and already
    /// brought up to date.
    pub(crate) old: &'a [BString],
}

impl AssertedDates<'_> {
    /// Resolve both lists against the graph this scan reads, and stamp them.
    ///
    /// Looked up as the switch stored the name and no further. GNU Make has no
    /// path canonicalisation: `expand_command_line_file` strips a LEADING `./`
    /// and stops, so `-o ./d/in` names the file `d/in` and `-o d/./in` names a
    /// file no rule mentions. Canonicalising here would make the second one
    /// work, which is a file GNU Make's `enter_file` creates and nothing
    /// depends on.
    ///
    /// A name is matched against the file database as the makefiles left it,
    /// which is where the two spellings of a `GPATH` target come apart. `main`
    /// stamps the switches after the read and before the update (main.c:2312,
    /// main.c:2325), and the rename that moves a found target into its `GPATH`
    /// directory happens inside `f_mtime`, during the update — so the name as
    /// WRITTEN is the one the stamp lands on, and the file object it lands on
    /// is the one that then gets renamed. Here the node is already the found
    /// path, so the written name reaches it through
    /// [`Graph::moved_from_written`](crate::graph::Graph::moved_from_written).
    pub(crate) fn mark_on(self, graph: &Graph, runtime: &mut super::RuntimeState) {
        let count = graph.node_ids().len();
        if !self.new.is_empty() {
            let nodes = names_in(graph, self.new, Moved::Reached).collect::<Vec<_>>();
            runtime.assumed_new.mark(&nodes, count);
        }
        if !self.old.is_empty() {
            // A double-colon target is stamped by neither switch, because GNU
            // Make's `enter_file` hands one back a file object it has just
            // made: it returns the entry it found only when that entry is not
            // a double-colon target (file.c), so the date goes onto something
            // no scan consults and the switch is inert. Measured — `-o` over a
            // `::` target leaves the build exactly as it found it, twice, with
            // the dependent older than the target and with it newer.
            //
            // Only `-o` declines here. `-W` reaches the same fresh entry and
            // GNU Make then refuses the build over it — `No rule to make
            // target 'out'`, because the entry it made carries a date and no
            // recipe — and declining the stamp would replace that refusal with
            // a quiet success, which hides the gap rather than closing it. It
            // is owned by make-a-what-if-file-that-is-double-colon-refuses.
            let nodes = names_in(graph, self.old, Moved::Declined)
                .filter(|node| !graph.is_written_undeclared(*node))
                .collect::<Vec<_>>();
            runtime.assumed_old.mark(&nodes, count);
        }
    }
}

/// What a switch's name means when it is the path a `GPATH` search moved a
/// target to, rather than the name the Makefile wrote for it.
///
/// The two switches answer differently, and GNU Make's own comment says why.
/// Such a spelling is not a target when the switch is read, so the stamp lands
/// on a file object of its own; the rename then merges the two, and
/// `rehash_file` (file.c:313) keeps the LATER of the two dates — `if
/// (from_file->last_mtime > to_file->last_mtime)`, under `%%% Kludge so -W wins
/// on a file that gets vpathized`. `-W`'s date is newer than any real file's
/// and wins the merge; `-o`'s is older than any real file's and loses to the
/// date the search has just taken, so the switch is inert. Measured on 4.4.1
/// both ways.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Moved {
    /// The stamp survives the merge, so the name reaches the moved node.
    Reached,
    /// The date the search found outranks it, so the name reaches nothing.
    Declined,
}

/// The nodes these names stand for, dropping any the graph does not hold.
fn names_in<'a>(
    graph: &'a Graph,
    names: &'a [BString],
    moved: Moved,
) -> impl Iterator<Item = NodeId> + 'a {
    names.iter().filter_map(
        move |name| match crate::graph::nodeget(graph, name.as_slice()) {
            Some(node) if moved == Moved::Declined && graph.was_moved_by_search(node) => None,
            Some(node) => Some(node),
            // The name the Makefile wrote for a target the search moved. The
            // node is the found path and this name is nowhere else in the
            // graph, which is why the lookup goes on rather than stopping at
            // the arena.
            None => graph.moved_from_written(name.as_slice()),
        },
    )
}
