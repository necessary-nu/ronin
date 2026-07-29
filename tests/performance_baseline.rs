const HARNESS: &str = include_str!("../examples/baseline.rs");
const RECORDED_BASELINE: &str = include_str!("../benchmarks/baseline-v1.csv");

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
    assert!(HARNESS.contains("ronin-performance-baseline-v2"));
    assert!(HARNESS.contains("peak_rss_kib"));
    assert!(HARNESS.contains("--validate"));

    let rows = RECORDED_BASELINE.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(rows.len(), 6);
    for workload in [
        "manifest-command-evaluation",
        "deep-graph-evaluation",
        "wide-noop-build",
        "path-canonicalization",
        "dependency-log-load",
        "scheduler-barrier",
    ] {
        assert_eq!(
            rows.iter()
                .filter(|row| row.starts_with(&format!("{workload},")))
                .count(),
            1,
            "recorded baseline must contain {workload} exactly once"
        );
    }
}
