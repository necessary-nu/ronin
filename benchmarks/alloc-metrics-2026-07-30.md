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

## Landed improvements

Each `ronin-allocation-discipline` node re-records
[`alloc-metrics-v1.csv`](alloc-metrics-v1.csv) after an accepted improvement.
The recorded file always holds the current values; this table preserves the
history.

### Niche-packed u32 arena identifiers (`ronin-compact-graph-ids`)

Allocation requests were byte-identical before and after, as expected: packing
identifiers shrinks allocations rather than removing them.

| Workload | Bytes before | Bytes after | Change |
| --- | ---: | ---: | ---: |
| manifest-command-evaluation | 16,044,639 | 15,135,623 | −5.7% |
| deep-graph-evaluation | 4,743,715 | 4,312,979 | −9.1% |
| wide-noop-build | 9,552,353 | 8,539,849 | −10.6% |
| path-canonicalization | 5,482,982 | 4,960,358 | −9.5% |
| dependency-log-load | 1,953,788 | 1,788,245 | −8.5% |
| scheduler-barrier | 658,634 | 603,519 | −8.4% |

The three commits between the original recording and this one
(`ronin-phony-console-ids`, `ronin-single-pass-dirty`, `ronin-fast-hashing`)
changed comparison and hashing strategy without changing allocation shape, so
the reduction is attributable to identifier packing.

### Allocation-free node stats (`ronin-stat-no-alloc`)

Each stat previously allocated three times: an interned-path clone for the
error path, a joined `PathBuf`, and the C string `std::fs` builds per call.
Holding the working directory open and calling `statat` with the manifest's own
relative path removes all three, since rustix converts short paths through a
stack buffer.

| Workload | Requests before | Requests after | Change | Bytes change |
| --- | ---: | ---: | ---: | ---: |
| manifest-command-evaluation | 192,172 | 192,172 | — | — |
| deep-graph-evaluation | 40,152 | 34,153 | −14.9% | −9.2% |
| wide-noop-build | 64,246 | 52,244 | −18.7% | −9.1% |
| path-canonicalization | 36,103 | 36,103 | — | — |
| dependency-log-load | 26,378 | 24,573 | −6.8% | −7.2% |
| scheduler-barrier | 8,178 | 7,533 | −7.9% | −8.2% |

The two unchanged workloads run `-t commands` and `-t targets`, which never
stat. Requests per build statement fell from 20.1 to 17.1 on deep-graph
evaluation and from 16.1 to 13.1 on the wide no-op build — the shape of the
most common real invocation. The release binary also shrank slightly despite
enabling rustix's `fs` feature, because the `std::fs::metadata` paths went
away.

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
