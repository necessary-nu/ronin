# Allocation-accounting baseline — 2026-07-30

This is the initial allocations-per-build-statement baseline for the
`ronin-allocation-discipline` optimization tree. Allocation counts are the
programme's leading regression indicator: at 15–40 ms wall times they
discriminate more reliably than time, and they are deterministic, so a single
run per workload suffices.

## Provenance

- Harness: `examples/alloc_metrics.rs` (`ronin-alloc-metrics-v1` schema)
- Method: in-process `ronin::Runner` runs of the version-1 baseline workload
  shapes under a counting global allocator that forwards to the system
  allocator; requests and requested bytes are snapshotted around each run.
  Minor page faults are sampled from `/proc/self/stat` and reported as
  informational only.
- Ronin revision: `9018388422e92a9c3f577f384378c25d278cbbd8`
- Profile: release; Rust: `rustc 1.92.0 (ded5c06cf 2025-12-08)`
- Platform: Linux 6.12.57, x86-64

## Results

| Workload | Build statements | Requests | Bytes | Requests/stmt | Bytes/stmt |
| --- | ---: | ---: | ---: | ---: | ---: |
| manifest-command-evaluation | 4,001 | 192,172 | 16,044,639 | 48.0 | 4,010 |
| deep-graph-evaluation | 2,000 | 40,152 | 4,743,715 | 20.1 | 2,372 |
| wide-noop-build | 4,001 | 64,246 | 9,552,353 | 16.1 | 2,388 |
| path-canonicalization | 4,000 | 36,103 | 5,482,982 | 9.0 | 1,371 |
| dependency-log-load | 301 | 26,378 | 1,953,788 | 87.6 | 6,491 |
| scheduler-barrier | 129 | 8,178 | 658,634 | 63.4 | 5,106 |

Repeated runs reproduce requests and bytes exactly for the parse-dominated
workloads. The scheduler workload wobbles by ±1 request with subprocess
completion order and the dependency-log workload's bytes vary ~0.5% with
timestamp digit widths; the `--check` tolerance of 10% absorbs both.

The in-process request count for manifest-command-evaluation (192,172)
independently confirms the whole-binary jemalloc measurement of 192,838
allocator requests in [`span-frontend-2026-07-30.md`](span-frontend-2026-07-30.md)
for the same workload.

## Interpretation

~48 allocation requests per build statement on the command-evaluation
workload is the quantitative form of the mimalloc finding: the allocator is
not the problem; the request count is. The `ronin-allocation-discipline`
nodes (scratch-buffer evaluation, name interning, memchr scanning, path
interning arena, span-based log loading, allocation-free stats) each land
with a before/after against this table, re-recording
[`alloc-metrics-v1.csv`](alloc-metrics-v1.csv) as counts drop. The
target end state is single-digit requests per build statement.

## Reproduction

```sh
cargo build --release --example alloc_metrics
./target/release/examples/alloc_metrics --check benchmarks/alloc-metrics-v1.csv
```

Re-record after an accepted improvement with `--record`.
