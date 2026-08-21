use crate::graph;
use crate::scratch_directory::Scratch;
use std::fs;

pub(super) struct Fixture {
    pub(super) directory: Scratch,
    pub(super) graph: graph::Graph,
}

impl Fixture {
    pub(super) fn parse(label: &str, manifest: &str) -> Self {
        let directory = Scratch::named(&format!("ronin-tool-{label}-"));
        let path = directory.join("build.ninja");
        fs::write(&path, manifest).unwrap();
        let graph = crate::parse::load_manifest_in(
            &path,
            crate::os::WorkingDirectory::default(),
            crate::frontend::ManifestOptions::default(),
        )
        .unwrap()
        .graph
        .into_arenas();
        Self { directory, graph }
    }
}
