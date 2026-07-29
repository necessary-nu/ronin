const HARNESS: &str = include_str!("../examples/baseline.rs");

// [spec:samurai:req:performance.reproducible-baseline/test]
#[test]
fn baseline_catalog_and_metadata_remain_complete() {
    for workload in [
        "manifest-command-evaluation",
        "deep-graph-evaluation",
        "wide-noop-build",
        "path-canonicalization",
        "dependency-log-load",
        "scheduler-barrier",
    ] {
        assert!(HARNESS.contains(workload), "missing workload {workload}");
    }
    for metadata in [
        "ronin_revision",
        "ninja_revision",
        "build_profile",
        "platform",
        "rustc",
        "noise_control",
        "workload_version",
    ] {
        assert!(HARNESS.contains(metadata), "missing metadata {metadata}");
    }
    assert!(HARNESS.contains("b51a1e37c2fb89bbefa600bd155e1ce13983f09d"));
}
