// [spec:ronin:req:release.compatibility-gate]
const RELEASE_GATE: &str = include_str!("../scripts/check-release.sh");

// [spec:ronin:req:release.compatibility-gate/test]
#[test]
fn release_gate_wires_every_candidate_check() {
    for command in [
        "cargo fmt --all -- --check",
        "cargo check --all-targets",
        "cargo clippy --all-targets",
        // The Make front end is a vendored fork that used to deny warnings in
        // its own source. It stopped, because a compiler release should not
        // break everybody's build; this line is where that denial went, so it
        // is not free to disappear.
        "cargo clippy -p kati --all-targets -- -D warnings",
        "cargo doc --no-deps",
        "cargo test --all-targets --no-fail-fast",
        "nplan port check --wave 4",
        "scripts/check-ninja-conformance.sh",
        "scripts/check-performance.sh",
        "nplan spec uncovered",
        "nplan spec stale",
        "nplan lint",
        "nplan audit",
    ] {
        assert!(
            RELEASE_GATE.contains(command),
            "release gate is missing {command}"
        );
    }
}

/// Ronin is a binary tool carrying the Make frontend as a path dependency on a
/// submodule, so it is `publish = false` and the gate builds no registry
/// package. Asserting the absence keeps the check from drifting back in on the
/// assumption that a missing `cargo package` was an oversight.
// [spec:ronin:req:release.compatibility-gate/test]
#[test]
fn release_gate_builds_no_registry_package() {
    assert!(
        !RELEASE_GATE.contains("cargo package"),
        "the gate packages a crate Ronin does not publish"
    );
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest.contains("publish = false"),
        "Cargo.toml no longer declares the crate unpublished"
    );
}

/// Every integration test that links the executable under a name of its own is
/// reaching for the Make front end — that link is the only way in, and no other
/// name is worth choosing. The front end exists only under the `make` feature,
/// so such a file must say so: as a file-level `#![cfg(all(unix, feature =
/// "make"))]` where every test in it reaches the front end, or per test as
/// `tests/cli.rs` does where most do not.
///
/// This lives here, in a file that is itself ungated, because a gate that
/// declines to compile cannot report its own absence. `tests/make_grouped.rs`
/// carried `#![cfg(unix)]` alone and its thirteen tests failed the whole
/// `--no-default-features` build against the front end's own refusal; nothing
/// in the suite noticed, because the suite that would have noticed was the one
/// that had gone missing.
#[test]
fn make_tests_declare_the_make_feature() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut checked = 0_usize;
    for entry in std::fs::read_dir(&tests).expect("the tests directory") {
        let path = entry.expect("a tests directory entry").path();
        if path.extension().is_none_or(|end| end != "rs") {
            continue;
        }
        // This file names both markers in order to look for them, and reading
        // itself would make the check true of itself and of nothing else.
        if path
            .file_name()
            .is_some_and(|name| name == "release_gate.rs")
        {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("an integration test source");
        if !source.contains("CARGO_BIN_EXE_ronin") || !source.contains("symlink") {
            continue;
        }
        checked += 1;
        assert!(
            source.contains(r#"feature = "make""#),
            "{} runs the executable under a name of its own and never declares \
             the make feature; without it the binary has no Make front end and \
             every such test fails",
            path.display()
        );
    }
    assert!(
        checked >= 6,
        "only {checked} integration tests were recognised as reaching the Make \
         front end, so the recognition has drifted away from how they are written"
    );
}
