//! Version-1 baseline workload shapes shared by the wall-time and
//! allocation-accounting harnesses.
//!
//! Both harnesses must measure byte-identical manifests, so the sizes and
//! generators live here exactly once. The workload catalog is frozen at
//! version 1; changing any shape requires a new workload version and fresh
//! recorded baselines.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

pub(crate) const WORKLOAD_VERSION: u32 = 1;
pub(crate) const COMMAND_EDGES: usize = 4_000;
pub(crate) const DEEP_EDGES: usize = 2_000;
pub(crate) const WIDE_EDGES: usize = 4_000;
pub(crate) const CANONICAL_PATHS: usize = 4_000;
pub(crate) const DEPENDENCY_EDGES: usize = 300;
pub(crate) const SCHEDULER_EDGES: usize = 128;

fn write_manifest(directory: &Path, manifest: &str) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    fs::write(directory.join("build.ninja"), manifest)
}

pub(crate) fn command_evaluation(directory: &Path) -> io::Result<()> {
    let mut manifest =
        String::from("rule cc\n  command = cc -DINDEX=$index -Iinclude $in -o $out\n");
    for index in 0..COMMAND_EDGES {
        let _ = write!(
            manifest,
            "build out/{index}.o: cc src/{index}.c\n  index = {index}\n"
        );
    }
    manifest.push_str("build all: phony");
    for index in 0..COMMAND_EDGES {
        let _ = write!(manifest, " out/{index}.o");
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)
}

pub(crate) fn deep_graph(directory: &Path) -> io::Result<()> {
    let mut manifest = String::from("build node/0: phony\n");
    for index in 1..DEEP_EDGES {
        let _ = writeln!(manifest, "build node/{index}: phony node/{}", index - 1);
    }
    let _ = writeln!(manifest, "default node/{}", DEEP_EDGES - 1);
    write_manifest(directory, &manifest)
}

pub(crate) fn wide_noop(directory: &Path) -> io::Result<()> {
    let mut manifest = String::new();
    for index in 0..WIDE_EDGES {
        let _ = writeln!(manifest, "build leaf/{index}: phony");
    }
    manifest.push_str("build all: phony");
    for index in 0..WIDE_EDGES {
        let _ = write!(manifest, " leaf/{index}");
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)
}

pub(crate) fn path_canonicalization(directory: &Path) -> io::Result<()> {
    let mut manifest = String::new();
    for index in 0..CANONICAL_PATHS {
        let _ = writeln!(
            manifest,
            "build scratch/{index}/../canonical-{index}: phony"
        );
    }
    write_manifest(directory, &manifest)
}

/// Write the dependency-log sources and manifest. The caller primes
/// `.ninja_deps` with the tool under measurement.
pub(crate) fn dependency_log_sources(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory.join("src"))?;
    fs::create_dir_all(directory.join("out"))?;
    fs::create_dir_all(directory.join("include"))?;
    fs::write(directory.join("include/common.h"), b"/* baseline */\n")?;

    let mut manifest = String::from(
        "rule compile\n  command = printf '$out: $in include/common.h\\n' > $out.d && touch $out\n  depfile = $out.d\n  deps = gcc\n",
    );
    for index in 0..DEPENDENCY_EDGES {
        fs::write(directory.join(format!("src/{index}.c")), b"int baseline;\n")?;
        let _ = writeln!(manifest, "build out/{index}.o: compile src/{index}.c");
    }
    manifest.push_str("build all: phony");
    for index in 0..DEPENDENCY_EDGES {
        let _ = write!(manifest, " out/{index}.o");
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)
}

pub(crate) fn scheduler(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory.join("jobs"))?;
    let mut manifest = String::from("rule step\n  command = touch $out\n");
    for index in 0..SCHEDULER_EDGES {
        let _ = writeln!(manifest, "build jobs/{index}: step");
    }
    manifest.push_str("build all: phony");
    for index in 0..SCHEDULER_EDGES {
        let _ = write!(manifest, " jobs/{index}");
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)
}
