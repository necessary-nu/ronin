//! What `--shuffle` reorders, read off the graph rather than off a build.
//!
//! The order the edges are minted in is the order the scheduler takes equally
//! ready ones in, so an edge's position is the build order this Makefile would
//! run in.

use super::{Shuffle, load_makefile};
use crate::util::ByteSlice;
use kati::session::Session;
use std::ffi::OsString;

/// Every edge's first output, in the order the edges were minted.
fn minted(makefile: &str, shuffle: Shuffle) -> Vec<String> {
    let directory = tempfile::tempdir().expect("a scratch directory");
    let path = directory.path().join("Makefile");
    std::fs::write(&path, makefile).expect("the scratch directory is writable");
    let session = Session::from_args(vec![
        OsString::from("make"),
        OsString::from("-f"),
        path.into_os_string(),
    ])
    .expect("a taken argv");
    let graph = load_makefile(session, shuffle)
        .expect("the makefile describes a graph")
        .graph
        .into_arenas();
    graph
        .edge_ids()
        .filter_map(|edge| graph.edge(edge).out.first().copied())
        .map(|output| graph.node_path(output).to_str_lossy().into_owned())
        .filter(|path| path.ends_with('_'))
        .collect()
}

const PREREQUISITES: &str = "all: a_ b_ c_\na_: ; @:\nb_: ; @:\nc_: ; @:\n";

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn make_shuffle_reverses_the_prerequisites_a_makefile_wrote() {
    assert_eq!(minted(PREREQUISITES, Shuffle::None), ["a_", "b_", "c_"]);
    assert_eq!(minted(PREREQUISITES, Shuffle::Identity), ["a_", "b_", "c_"]);
    assert_eq!(minted(PREREQUISITES, Shuffle::Reverse), ["c_", "b_", "a_"]);
}

/// The seed settles the permutation completely, which is what lets a run that
/// found an unstated dependency be repeated.
// [spec:ronin:req:make.graph-direct/test]
#[test]
fn make_shuffle_answers_one_seed_with_one_order() {
    let seeded = minted(PREREQUISITES, Shuffle::Seed(12345));
    assert_eq!(seeded, minted(PREREQUISITES, Shuffle::Seed(12345)));
    assert_eq!(
        seeded
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "{seeded:?}"
    );
}

/// A Makefile that says its own recipes cannot overlap is describing an order,
/// and GNU Make does not reorder it.
// [spec:ronin:req:make.graph-direct/test]
#[test]
fn make_shuffle_leaves_a_notparallel_makefile_in_the_order_it_wrote() {
    let serial = format!(".NOTPARALLEL:\n{PREREQUISITES}");
    assert_eq!(minted(&serial, Shuffle::Reverse), ["a_", "b_", "c_"]);
}
