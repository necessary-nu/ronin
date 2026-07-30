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

### In-place depfile tokenization (`ronin-depfile-inplace`)

The suite gained a `depfile-scan` workload first, because
`dependency-log-load` ingests `.ninja_deps` and never parses a depfile, so this
change would otherwise have been invisible. Declaring `depfile` without `deps`
keeps every rebuild scan on the depfile parser — the path a real build takes
after each compile. It entered the suite as the worst workload in the table at
401.3 requests per build statement, roughly ten allocations per dependency
path.

Three sources were removed: the tokenizer allocated a fresh `Vec` per token and
two hash maps per rule line, and each ingested path was copied three times on
its way to a node (`mkstr` plus zero-fill, `canonpath`'s output, then
`to_vec`). Rule-local tokens now reuse their buffers across lines, accumulating
path sets allocate only on a path's first appearance, and canonicalization
consumes the parsed buffer and hands the result straight to `mknode`.

| Workload | Requests before | Requests after | Change | Bytes change |
| --- | ---: | ---: | ---: | ---: |
| depfile-scan | 40,534 | 27,734 | −31.6% | −15.8% |

Requests per build statement fell from 401.3 to 274.6. The other workloads are
unchanged, which is the expected signature: none of them parse a depfile.

### Reused traversal scratch (`ronin-traversal-scratch`)

Graph scans allocated and zeroed graph-sized visit arrays per call: four in
`recompute_dirty_with_validations` plus one each in the depfile, build-log, and
restat traversals. That is paid once per target, once per restat completion,
and once per dyndep reload, so it compounds on exactly the builds that are
already slow. The arrays now live for the build and reset by bumping a
generation counter, with stamps packed into a single byte — the same width as
the arrays they replace — so a real clear is needed only every 63 traversals.

A `multi-target-scan` workload was added to measure it, since every existing
workload names one target and so never repeats a traversal. It names 200
targets against the wide manifest:

| Workload | Requests before | Requests after | Change | Bytes before | Bytes after | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| multi-target-scan | 55,982 | 54,787 | −2.1% | 13,018,568 | 8,237,373 | −36.7% |

Single-target workloads move by one or two requests, which is the expected
result: they traverse once, so they pay the setup and skip the repeat. The
4.8 MB removed from the 200-target scan is the shape that matters for large
builds, and restat-heavy and dyndep-heavy builds repeat traversals the same
way without naming extra targets.

`Plan::dependents` was left as a vector of vectors rather than converted to a
compressed adjacency layout: `rebuild_frontier` clears the inner vectors
instead of dropping them, so their capacity is already reused across rebuilds
and a conversion would trade readable code for no measurable allocation.

### Appending command evaluation (`ronin-eval-scratch-buffers`)

`edgevar` returned an owned value from every recursion level, so each lookup
cloned the rule binding's whole `EvalString`, copied every literal part into a
parts vector, copied a `BString` per `$in`/`$out` node into a path vector, and
then merged all of it into a third buffer. The cycle guard allocated a vector
and an owned name per level on top.

Evaluation now appends into one caller-owned buffer, borrows rule bindings
straight out of the graph, appends node paths from the interned bytes, and
tracks the cycle guard with borrowed names. `edgevar_into` exposes the buffer
so a caller reading several bindings for one edge reuses it. Absent and empty
values stay distinct, because Ninja treats a missing `$in` differently from an
empty one.

| Workload | Requests before | Requests after | Change | Bytes change |
| --- | ---: | ---: | ---: | ---: |
| manifest-command-evaluation | 192,172 | 128,072 | −33.4% | −10.9% |
| dependency-log-load | 24,572 | 12,272 | −50.1% | −17.0% |
| depfile-scan | 27,732 | 24,332 | −12.3% | −4.1% |
| scheduler-barrier | 7,535 | 6,509 | −13.6% | −4.6% |

Requests per build statement on command evaluation fell from 48.0 — untouched
by the seven preceding nodes — to 32.0. The phony-only workloads are
unchanged, as expected: they declare no rule bindings to evaluate.

Wall time moved much less than allocation count. Against C samurai, command
evaluation went from 1.65x to 1.58x, dependency-log loading from 1.38x to
1.28x, and the scheduler barrier from 1.14x to 1.12x. That gap is the useful
signal: a third of the allocations were real but were not the dominant cost.
What remains in evaluation is lookup cost — every binding still resolves
through `String`-keyed `BTreeMap`s — which is what `ronin-name-interning`
targets, and the per-call output buffer that `ronin-edge-metadata-cache`
removes by batching an edge's bindings into one pass.

`merge` and `pathlist` keep documented `dead_code` allowances. They are ported
C symbols the port specification requires, and nothing in production calls them
now that evaluation appends in place.

### One stored copy per node path (`ronin-path-interning-arena`)

Interning a node stored its path three times: the node's own buffer, a copy as
the lookup map's key, and an eagerly shell-quoted copy that was made even for
the overwhelming majority of paths that need no quoting.

The lookup map is replaced by an open-addressed index holding only node
identifiers, so a probe hashes the candidate bytes and compares against the
path the node already owns — no key copy, and with niche-packed identifiers a
slot costs four bytes. The shell-quoted form becomes optional, present only
when quoting actually changes the path, and both styles render from the plain
buffer otherwise. This is the layout `htab.c` uses, so the ported `htab`
specification rules move onto the index, where they are now literally true
rather than approximated by a standard hash map.

Exactly two allocations disappear per interned node, which the numbers
confirm: path canonicalization interns 4,000 nodes and shed 8,003 requests.

| Workload | Requests before | Requests after | Change | Bytes change |
| --- | ---: | ---: | ---: | ---: |
| manifest-command-evaluation | 128,072 | 112,067 | −12.5% | −8.2% |
| deep-graph-evaluation | 34,152 | 30,149 | −11.7% | −6.9% |
| wide-noop-build | 52,243 | 44,238 | −15.3% | −7.0% |
| path-canonicalization | 36,103 | 28,100 | −22.2% | −13.1% |
| dependency-log-load | 12,272 | 11,065 | −9.8% | −5.1% |
| depfile-scan | 24,332 | 23,848 | −2.0% | −1.7% |
| multi-target-scan | 54,787 | 46,782 | −14.6% | −6.6% |
| scheduler-barrier | 6,509 | 6,249 | −4.0% | −3.1% |

Every workload improved, which no earlier node achieved. Node paths are still
individually owned `BString`s rather than spans into one byte arena: that
further step would touch every `graph.node(id).path` reader for a smaller
marginal gain than the two copies removed here, so it is left for its own
change.

### Wall-time confirmation

Allocation counts are the leading indicator, not the goal, so the release
performance gate ran against pinned Ninja after the identifier-packing and
allocation-free-stat nodes landed (7 repetitions, one warmup, interleaved
samples, revision `867e5ff`). It passed every recorded-ratio, Ninja-runtime,
and peak-RSS threshold. Medians in milliseconds:

| Workload | Ronin | Pinned Ninja | C samurai | Ronin / samurai |
| --- | ---: | ---: | ---: | ---: |
| manifest-command-evaluation | 15.184 | 40.846 | 9.183 | 1.65× |
| deep-graph-evaluation | 7.174 | 16.855 | 5.682 | 1.26× |
| wide-noop-build | 12.507 | 31.000 | 9.829 | 1.27× |
| path-canonicalization | 6.165 | 17.038 | 5.589 | 1.10× |
| dependency-log-load | 3.600 | 7.085 | 2.612 | 1.38× |
| scheduler-barrier | 42.978 | 45.129 | 37.824 | 1.14× |

Ronin is faster than this Ninja build on every workload and now within 10% to
65% of the C reference, against the 1.65× to 16.5× spread recorded in
[`baseline-2026-07-29.md`](baseline-2026-07-29.md). Command evaluation remains
the widest gap, which is what `ronin-eval-scratch-buffers` and
`ronin-edge-metadata-cache` target — consistent with it also being the workload
whose 48 requests per build statement have not yet moved.

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
