use crate::{env, graph, parse};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Fixture {
    pub(super) directory: std::path::PathBuf,
    pub(super) graph: graph::Graph,
}

impl Fixture {
    pub(super) fn parse(label: &str, manifest: &str) -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ronin-tool-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("build.ninja");
        fs::write(&path, manifest).unwrap();
        let mut graph = graph::graphinit();
        let mut parser = parse::parseinit();
        let mut state = env::envinit(&mut graph);
        parse::parse(&path, &mut graph, &mut parser, state.root, &mut state).unwrap();
        Self { directory, graph }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
