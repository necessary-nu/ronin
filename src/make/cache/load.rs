//! Opening what a previous invocation left, as a pair or not at all.
//!
//! The two files are written one after the other, so an interrupted run can
//! leave a new graph beside an old record. The record names the graph it
//! describes by digest, and nothing here believes one without the other: a
//! reader that finds them apart has no cache, which is the state every
//! invocation was in before the files existed.
//!
//! Refusing is cheap and believing wrongly is not. Every failure on this path
//! — a missing file, a byte that does not fit, a digest that does not match —
//! answers `None`, and the caller composes the graph again.

use super::artifact::Artifact;
use crate::frontend::BuildGraph;
use crate::htab::rapidhashv1;
use std::path::Path;

/// One invocation's artifact, opened and checked against itself.
pub(crate) struct Cached {
    /// What the composition read, as a later run checks and replays it.
    pub(crate) artifact: Artifact,
    /// The graph that composition produced, before the makefile update and
    /// the staged work marked it up. See [`super::record`].
    pub(crate) graph: BuildGraph,
}

/// The pair in `directory`, checked against each other.
pub(crate) fn open(directory: &Path) -> Option<Cached> {
    let record = std::fs::read(directory.join(super::RECORD)).ok()?;
    let artifact = Artifact::decode(&record)?;
    let graph = std::fs::read(directory.join(super::GRAPH)).ok()?;
    // The digest before the parse, because a graph left by another run is a
    // graph that parses perfectly and describes a different composition. What
    // makes this pair a pair is that the record named these bytes.
    if rapidhashv1(graph.as_slice()) != artifact.graph {
        return None;
    }
    let graph = crate::graph::persist::read(&graph)?;
    Some(Cached { artifact, graph })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record and a graph as one settling run would leave them.
    fn written(directory: &Path) {
        let settled = crate::make::Groundwork::default();
        let graph = crate::make::cache::snapshot(&BuildGraph::default())
            .expect("an empty graph is writable");
        super::super::write(directory, &settled, Some(graph));
    }

    #[test]
    fn a_written_pair_opens() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        written(directory.path());
        assert!(open(directory.path()).is_some());
    }

    #[test]
    fn nothing_written_opens_nothing() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        assert!(open(directory.path()).is_none());
    }

    #[test]
    fn a_graph_the_record_never_named_is_refused() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        written(directory.path());
        std::fs::write(directory.path().join(super::super::GRAPH), b"another graph")
            .expect("the graph is replaceable");
        assert!(
            open(directory.path()).is_none(),
            "a graph left by another run is not the graph this record describes"
        );
    }

    #[test]
    fn a_truncated_record_is_refused() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        written(directory.path());
        let path = directory.path().join(super::super::RECORD);
        let record = std::fs::read(&path).expect("the record is there");
        std::fs::write(&path, &record[..record.len() - 1]).expect("the record is replaceable");
        assert!(open(directory.path()).is_none());
    }
}
