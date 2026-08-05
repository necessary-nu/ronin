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
