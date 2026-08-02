// [spec:ronin:req:release.compatibility-gate]
const RELEASE_GATE: &str = include_str!("../scripts/check-release.sh");

// [spec:ronin:req:release.compatibility-gate/test]
#[test]
fn release_gate_wires_every_candidate_check() {
    for command in [
        "cargo fmt --all -- --check",
        "cargo check --all-targets",
        "cargo clippy --all-targets",
        "cargo doc --no-deps",
        "cargo test --all-targets --no-fail-fast",
        "nplan port check --wave 4",
        "scripts/check-ninja-conformance.sh",
        "scripts/check-performance.sh",
        "nplan spec uncovered",
        "nplan spec stale",
        "nplan lint",
        "nplan audit",
        "cargo package",
    ] {
        assert!(
            RELEASE_GATE.contains(command),
            "release gate is missing {command}"
        );
    }
}
