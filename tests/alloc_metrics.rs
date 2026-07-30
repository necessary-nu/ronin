const HARNESS: &str = include_str!("../examples/alloc_metrics.rs");
const RECORDED_BASELINE: &str = include_str!("../benchmarks/alloc-metrics-v1.csv");

// [spec:samurai:req:performance.allocation-accounting/test]
#[test]
fn allocation_harness_and_recorded_baseline_remain_complete() {
    for workload in [
        "manifest-command-evaluation",
        "deep-graph-evaluation",
        "wide-noop-build",
        "path-canonicalization",
        "dependency-log-load",
        "scheduler-barrier",
    ] {
        assert!(HARNESS.contains(workload), "missing workload {workload}");
        assert_eq!(
            RECORDED_BASELINE
                .lines()
                .filter(|line| line.starts_with(&format!("{workload},")))
                .count(),
            1,
            "recorded baseline must contain {workload} exactly once"
        );
    }
    for required in [
        "#[global_allocator]",
        "ronin-alloc-metrics-v1",
        "workload_version",
        "build_profile",
        "requested_bytes",
        "--record",
        "--check",
    ] {
        assert!(
            HARNESS.contains(required),
            "missing harness element {required}"
        );
    }
    assert!(RECORDED_BASELINE.contains("# schema=ronin-alloc-metrics-v1"));
    assert!(RECORDED_BASELINE.contains("# workload_version=1"));
    assert!(RECORDED_BASELINE.contains("# build_profile=release"));
    assert!(
        RECORDED_BASELINE.contains("workload,build_statements,allocation_requests,requested_bytes")
    );
}
