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

### One stored copy per log entry (`ronin-log-span-loading`)

`LogEntry` carried an `output` path that duplicated the map key it was stored
under, so loading `.ninja_log` allocated and copied every output path twice.
The path now lives in the key alone and writers take it alongside the entry.
The deps log's node construction also stopped zero-filling a buffer it
overwrote immediately afterwards.

The saving is exactly one allocation per log line, and the arithmetic confirms
it rather than approximating: dependency-log loading has 301 entries and shed
301 requests; the scheduler barrier has 129 and shed 128.

| Workload | Requests before | Requests after | Change | Bytes change |
| --- | ---: | ---: | ---: | ---: |
| dependency-log-load | 11,065 | 10,764 | −2.7% | −2.1% |
| scheduler-barrier | 6,249 | 6,121 | −2.0% | −2.6% |
| depfile-scan | 23,848 | 23,746 | −0.4% | −0.4% |

These percentages understate the change. The fixtures carry logs of 301 and
129 lines against manifests of thousands of statements, whereas a real tree's
`.ninja_log` holds one line per build output and is read at every invocation,
so the per-line invariant is what matters rather than this ratio.

### Interned binding names (`ronin-name-interning`)

This node was promoted ahead of its pass on the evidence from
`ronin-eval-scratch-buffers`: cutting a third of command evaluation's
allocations had bought only 4% of wall time, which said allocation was no
longer the binding constraint. Every binding still resolved through a
`BTreeMap<String, _>`, so each lookup compared bytes across pointer-chasing
nodes, several times per edge plus once per scope in the parent chain.

Names are now interned to integer symbols, with the fourteen that evaluation
and the build rules refer to by fixed identity interned first so their
constants are compile-time comparisons. Binding tables became sorted
contiguous runs keyed by symbol.

Allocation counts barely move — the interner costs a few dozen allocations
once — but bytes fall sharply as tree nodes give way to contiguous storage,
and wall time moves more than any other node in this programme:

| Workload | Bytes before | Bytes after | Change | Median before | Median after | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| manifest-command-evaluation | 12,373,629 | 10,499,336 | −15.1% | 14.976 ms | 10.366 ms | −30.8% |
| path-canonicalization | 4,311,222 | 4,312,632 | +0.0% | 6.165 ms | 5.562 ms | −9.8% |
| dependency-log-load | 1,278,233 | 1,222,635 | −4.4% | 3.299 ms | 3.186 ms | −3.4% |

Against the C reference, command evaluation went from 1.58x to **1.13x**, and
path canonicalization to **0.93x — faster than C samurai**, the first workload
where Ronin leads it. Peak resident memory on command evaluation fell from
9,784 KiB to 6,884 KiB.

The lesson generalises: allocation count was the right leading indicator while
allocations dominated, and it stopped being one once they did not. The
harness measures what it measures, and the wall-time gate is what caught the
divergence.

### Reused path scratch and in-place canonicalization (`ronin-path-scratch`)

Selected by profiling rather than by the plan's order, and the plan's
dependency on `ronin-memchr-scanner` was dropped as part of it. Phase
decomposition of the 4,000-edge command manifest showed parsing is 93% of that
workload (10.31 ms of 11.12 ms), that the scanner is only about 4% of parsing
— padded comment bytes measure roughly 500 MB/s, so its 222 KiB costs about
0.44 ms — and that per-path work dominates: holding statement count fixed, a
second path per statement costs 0.71 us, so 8,000 paths account for roughly
5.7 ms.

Each path reference allocated about four times: a parts vector while scanning,
the evaluated result, and `canonpath`'s output buffer plus its component
stack, the last two even when the path was already canonical. Paths now
evaluate into one buffer reused for a whole manifest, canonicalize in place
behind a no-allocation check for the already-canonical case, and intern from
bytes so a reference to an existing node allocates nothing at all.

| Workload | Requests before | Requests after | Change | Per statement |
| --- | ---: | ---: | ---: | --- |
| manifest-command-evaluation | 108,099 | 80,097 | −25.9% | 27.0 → 20.0 |
| deep-graph-evaluation | 30,182 | 20,185 | −33.1% | 15.1 → 10.1 |
| wide-noop-build | 44,271 | 24,269 | −45.2% | 11.1 → 6.1 |
| path-canonicalization | 28,133 | 24,135 | −14.2% | 7.0 → 6.0 |
| dependency-log-load | 10,790 | 8,688 | −19.5% | 35.8 → 28.9 |
| depfile-scan | 23,774 | 14,671 | −38.3% | 235.4 → 145.3 |
| multi-target-scan | 46,815 | 26,813 | −42.7% | 234.1 → 134.1 |
| scheduler-barrier | 6,153 | 5,507 | −10.5% | 47.7 → 42.7 |

Wall time moved much less. A controlled parse-only comparison, thirty
invocations per sample and three samples per build, gave 9.895/10.241/10.225 ms
before and 9.623/9.518/10.950 ms after: roughly 4% to 6%. Command evaluation
against the C reference improved from 1.13x to 1.06x; the other workloads sat
inside this host's run-to-run spread, which for the wide no-op build reached
11.7 to 17.2 ms in a single gate run, so no claim is made about them from one
sample.

The reading is that allocation was about a tenth of per-path cost. Removing
half the allocations from a phase worth 5.7 ms returned about 0.5 ms, so what
remains is the byte work itself — evaluation copying, canonicalization
scanning, hashing, and index probing — not the allocator calls around it. That
is consistent with `ronin-name-interning`, where the gain came from removing
comparisons rather than allocations.

### Run-skipping scanner, measured and rejected (`ronin-memchr-scanner`)

The premise was that byte-at-a-time lexing with per-byte line and column
bookkeeping was a hot spot. The profiling done for `ronin-path-scratch`
disproved it: padding a manifest with comment bytes measures the scanner at
roughly 500 MB/s, so the 222 KiB command manifest costs about 0.44 ms of an
11 ms parse, a 4% ceiling.

The change was implemented anyway to settle it — literal, identifier, and
comment runs skipped with `bstr` byteset searches, and column tracked as a
subtraction from the line start rather than a per-byte counter. Interleaved
minimum-of-thirty comparisons on a quiet host:

| Round | Before | After |
| --- | ---: | ---: |
| 1 | 10.440 ms | 10.703 ms |
| 2 | 10.365 ms | 10.781 ms |
| 3 | 10.568 ms | 10.376 ms |

No gain, marginally negative, so it was reverted. `bstr` builds a 256-bit
lookup table per `find_byteset` call, which the short runs a manifest lexer
sees never amortize — identifier runs are a handful of bytes. A rewrite using
`memchr2`/`memchr3` for the small literal set while keeping byte loops for
identifiers would be the correct shape, but its ceiling is still 4% against a
measurement resolution of about 2%, which does not justify a new dependency.

Two process notes. Interleaving and taking a minimum, rather than a median of
sequential samples, is what made this legible: an earlier non-interleaved run
on a loaded host showed an apparent threefold regression that was entirely
drift. And the profiling that set the 4% ceiling was worth more than the
implementation it argued against.

### One pass over an edge's control bindings (`ronin-edge-metadata-cache`)

`CommandSpec::evaluate` made ten separate lookups per edge, each allocating
its own result. Only four are kept; `deps`, `restat`, and `generator` are
inspected and discarded, and classifying `deps` additionally moved the value
through an owned `String`. Those three now share one buffer held by the
builder, and `deps` is classified from bytes so the supported values cost no
allocation at all.

| Workload | Requests before | Requests after | Change |
| --- | ---: | ---: | ---: |
| dependency-log-load | 8,688 | 8,389 | −3.4% |

The saving is one allocation per edge that sets such a binding, which the
arithmetic confirms: that workload's 300 edges each declare `deps = gcc` and
it shed 299 requests. Workloads whose edges leave these bindings unset are
unchanged, because an unset binding already resolved to nothing without
allocating.

### No environment per edge (`ronin-edge-scope-inline`)

Every edge was given its own environment holding nothing but a link to the
enclosing scope, because edge-local bindings live on the edge itself. It cost
an arena entry per edge and one extra link to walk on every lookup that missed
the edge and its rule. Edges now name their enclosing scope directly, which is
what the lookup walked to anyway.

| Workload | Bytes before | Bytes after | Change |
| --- | ---: | ---: | ---: |
| manifest-command-evaluation | 9,976,845 | 9,518,541 | −4.6% |
| deep-graph-evaluation | 3,467,342 | 3,238,414 | −6.6% |
| wide-noop-build | 6,852,358 | 6,394,054 | −6.7% |
| path-canonicalization | 4,164,034 | 3,705,730 | −11.0% |
| dependency-log-load | 1,181,122 | 1,124,226 | −4.8% |
| multi-target-scan | 7,332,930 | 6,874,626 | −6.2% |
| scheduler-barrier | 483,274 | 455,053 | −5.8% |

Path canonicalization shed 458,304 bytes across 4,000 edges, which is the
arena those environments occupied. Lookup results are unchanged: the removed
scope held no bindings, so resolving through it always continued to its
parent.

### No command evaluation for phony edges (`ronin-skip-phony-commands`)

Found by profiling the current build with callgrind rather than by the plan.
On two manifests containing no commands at all, command evaluation was still
about 9% of instructions: `prepare_build_log_for` hashed every edge, and
hashing an edge forces the full ten-binding `CommandSpec::evaluate`. Ninja
does not hash or log phony edges, and the dirty rule never consults a phony
edge's command hash — it is read only on the non-phony branch — so the work
was entirely discarded.

Instruction counts are from callgrind, which is deterministic and immune to
the host load that makes wall-clock comparisons unreliable here.

| Workload | Instructions before | Instructions after | Change | Bytes change |
| --- | ---: | ---: | ---: | ---: |
| wide-noop-build | 36,147,441 | 31,045,789 | −14.1% | −11.0% |
| deep-graph-evaluation | 20,563,620 | 18,008,271 | −12.4% | −10.9% |

Larger than the 9% the profile suggested, because skipping the hash also
skips the evaluation behind it and the command cache it would have populated:
`append_variable` and `edgevar` leave the profile's top entries entirely.
Allocation requests are unchanged, since the skipped lookups mostly resolved
to nothing and never allocated; the bytes are the per-edge command cache no
longer being built.

One behavioural change, in the corrective direction: `--explain` could
previously report "command line changed" for a phony edge, because an absent
log entry made its empty command hash count as dirty. Phony edges have no
command line, so they no longer report one.

### Allocator re-tested and still rejected (`ronin-allocator-revisit`)

The mimalloc measurement that opened this programme was taken at 48 allocation
requests per build statement. The profile has changed completely since — 6.1
on the wide no-op build, 20.0 on command evaluation — and callgrind puts the
glibc allocator at 17% to 19% of instructions, the largest single cluster, so
the question was worth reopening.

mimalloc executes materially fewer instructions: 30,223,104 to 27,373,796 on
the wide no-op build (−9.4%) and 55,784,522 to 45,570,500 on command
evaluation (−18.3%). It also takes materially more page faults:

| Workload | glibc minor faults | mimalloc minor faults | Change |
| --- | ---: | ---: | ---: |
| wide-noop-build | 897 | 1,149 | +28% |
| manifest-command-evaluation | 1,106 | 1,558 | +41% |

Roughly 250 to 450 extra faults per invocation, which at typical fault cost is
about 0.7 ms — the same order as the instruction saving on a 10 ms run. Wall
clock could not separate them on this host, and the mechanism explains why.
This is the original finding with numbers attached rather than a new one:
mimalloc front-loads cost that a short-lived, single-threaded coordinator never
amortizes, and reducing allocation count has not changed that.

The conclusion is not that allocation is cheap. It is that swapping the
allocator cannot win here, so the remaining 17% to 19% has to come out by
allocating less — which points at arena and inline storage for the small
per-entity collections that dominate what is left, not at a different malloc.

### Streamed path scanning, measured and rejected (`ronin-streamed-path-scanning`)

DHAT attributed 49.4% of remaining allocations to two sites: a fragment vector
per path in `scanstring` (8,000 blocks) and a collection vector per run of
paths in `scanpaths` (4,012). Streaming each path into its interned node
through one reusable fragment buffer, byte buffer, and staging vector removed
exactly what the attribution predicted.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Allocation requests (wide no-op) | 24,258 | 12,247 | −49.5% |
| Requested bytes (wide no-op) | 5,689,878 | 4,341,218 | −23.7% |
| Instructions (wide no-op) | 29,895,642 | 30,228,063 | **+1.1%** |
| Instructions (command evaluation) | 55,119,624 | 55,784,474 | **+1.2%** |
| Peak RSS | 11,532 KiB | 11,532 KiB | — |
| Minor faults | 896 | 897 | — |
| Wall time | 11.058 ms | 11.052 ms | — |

Every outcome metric refused to move, and instructions moved the wrong way:
the buffer management costs more than the allocator calls it removes. Wall
time deserves a note — seven interleaved minimum-of-25 rounds gave medians of
11.393 against 11.187 ms, which looks like a 2% win, but the minimums converge
at 11.058 against 11.052, so the median gap was the host settling. Reverted.

The finding that matters is about method rather than about paths.
Allocations per build statement was the right leading indicator while
allocations dominated, and the early nodes in this programme moved wall time
accordingly. At 6.1 requests per statement it has saturated: the survivors are
cheap thread-cache hits, and removing half of them buys nothing. Node
selection should now be driven by callgrind instruction counts and wall time,
with this harness retained as a regression guard rather than used as a target.
The same reasoning undercuts a planned SmallVec change, which would have
targeted a further third of the same saturated metric.

### Byte-keyed names — `ronin-byte-keyed-names`

The first node selected under the new rule: chosen from a callgrind profile,
with no allocation change expected or delivered.

Profiling the whole-program picture to answer whether SIMD was worth pursuing
put `core::str::converts::from_utf8` at 614,479 instructions, 2.06% of the
4,000-edge no-op build, across 8,003 calls from `scan::name` alone. The
scanner accepts a name only through `isvar`, which admits ASCII alphanumerics
plus `_`, `-` and `.`, so every byte of the slice is ASCII by construction and
the validation cannot fail — the call sites even said so, in an
`.expect("variable names are ASCII")`. It existed only so `Lexeme.text` could
be `&str` and `Names` could key on `str`. The manifest buffer itself cannot be
`&str`, because Ninja manifests may hold non-UTF-8 paths
[`compat.byte-inputs`], so names are an ASCII subset of a buffer that is not
valid UTF-8 as a whole, and safe Rust cannot reinterpret that subslice for
free.

The fix uses the byte-string type the crate already re-exports rather than
adding unsafe: `Lexeme.text` and `ScannedEvalPart::Variable` became `&BStr`,
`Names` keys on `BString` with borrowed `&BStr` probes, and `env::envrule`,
`env::poolget` and `env::envvar_named` follow, which carries `Environment.rules`,
`EnvState.pools`, `Rule.name` and `Pool.name` with them. `BStr` supplies the
`Display` and `Debug` that `&str` was providing for diagnostics, so nothing was
given up. A `String::from_utf8_lossy` round trip in `parseedge`'s pool lookup
fell out as well. `from_utf8_unchecked` was rejected: the crate has three
unsafe blocks, both files FFI-adjacent, and a 2% win does not justify a fourth
in the lexer.

| Metric | before | after | change |
| --- | ---: | ---: | ---: |
| Instructions, wide no-op | 29,892,101 | 29,226,333 | −2.23% |
| Instructions, command evaluation | 55,119,624 | 54,187,522 | −1.69% |
| `from_utf8` self cost | 614,479 | absent | — |
| `scan::name` self cost | 1,224,489 | 1,128,453 | −7.8% |
| Allocation requests, wide no-op | 24,258 | 24,258 | none |

Allocations were untouched by design, which is the point: the harness now
guards rather than steers.

Wall time was not resolvable. The host sat at load average 22 throughout, and
six interleaved minimum-of-40 rounds gave deltas of −0.435, −0.976, −1.391,
−0.825, +0.657 and +0.256 ms against an 11–15 ms base — four rounds favouring
the change, two against, with the sign flipping and the minimums never
converging. Per the lesson recorded above, that is drift, not signal, and the
deterministic instruction counts carry the result on their own.

Against C samurai on the identical no-op fixture, both built for the same host
(`cc -O2`), the instruction gap narrowed from 29,892,101/26,506,104 = 1.128× to
29,226,333/26,506,104 = 1.103×.

### Inline capacity for small collections — `ronin-smallvec-collections`

The largest single result in this programme, and a direct reversal of the
conclusion recorded two entries above.

`RawVec::grow_one` was 15.52% of the no-op build inclusive across 20,115
events. Every large caller turned out to be a *first* push onto an empty
`Vec` — one output per build statement, one use per leaf node, one literal
part per path — not repeated doubling. Inline capacity removes those outright.

Sizing decided the design. Under smallvec's `union` layout a value costs eight
bytes plus the larger of the inline array and a pointer/length pair, so four
four-byte arena identifiers occupy exactly the twenty-four bytes a `Vec`
already spends. `Node.uses`, `Node.validation_uses`, `Edge.out`, `Edge.input`
and `Edge.validation` therefore gained four inline slots at zero footprint
cost; `util::tests::id_vec_matches_vec_footprint` pins that, asserting both the
size equality and that the fifth element is the one that spills.
`ScannedEvalString.parts` is different — `ScannedEvalPart` is 24 bytes, so one
inline slot widens the value from 24 to 32 — and was measured separately rather
than assumed.

| Workload | before | graph only | + scanner | total |
| --- | ---: | ---: | ---: | ---: |
| Instructions, wide no-op | 29,226,333 | 26,547,330 | 24,102,023 | **−17.5%** |
| Instructions, command evaluation | 54,187,522 | 48,039,297 | 42,784,164 | **−21.0%** |
| Requests, wide no-op | 24,258 | 16,256 | 8,254 | −66.0% |
| Requests, command evaluation | 80,087 | 64,085 | 48,084 | −40.0% |
| Peak RSS, wide no-op | 11,532 KiB | 11,264 KiB | 11,288 KiB | −2.1% |

Requests per build statement on the no-op workload fell from 6.1 to 2.1,
reaching the single-digit end state this document set as the target.
Peak RSS fell rather than rose: inline storage replaced heap blocks whose
per-block allocator overhead is no longer paid.

Wall time moved, and unambiguously — the first node in this programme where it
did. Interleaved minimum-of-30 rounds against the preceding commit, on a host
at load average 8:

| Workload | round 0 | round 1 | round 2 | round 3 |
| --- | ---: | ---: | ---: | ---: |
| command-evaluation | −11.1% | −11.8% | −11.3% | −11.9% |
| wide-noop | −5.5% | −4.0% | −5.2% | −4.2% |

Consistent sign, consistent magnitude, four rounds each — the pattern the
rejected nodes never produced. Wide no-op gains less than its instruction count
suggests because process startup and stat syscalls do not scale with
instructions.

Against C samurai, interleaved minimum-of-30 on the same fixtures:
command-evaluation 0.894x, 0.938x, 0.928x — **faster than the C reference** —
and wide-noop 1.031x, 1.055x, 1.066x. Command evaluation stood at 1.65x in
[`baseline-2026-07-29.md`](baseline-2026-07-29.md).

The methodological point is sharper than the numbers. `ronin-streamed-path-scanning`
attacked `scanstring` and `scanpaths`, the same two sites, and *cost* 1.1%.
Inline capacity on those same sites *saves* 9.2%. The target was never wrong;
the mechanism was. Reusable buffers pay staging, clearing and index bookkeeping
on every path to avoid an allocation, while inline capacity pays nothing at
all — the storage is simply already there. A rejected node is evidence about a
mechanism, not a verdict on a target, and the earlier entry's inference from
"removing allocations bought nothing" to "SmallVec will buy nothing" was
unsound: it treated allocation count as the mechanism when it was only ever a
proxy. Allocation *count* had indeed saturated; allocation *growth events*,
which cost malloc plus memcpy plus free apiece, had not.

### Hoisting the inline discriminant — `ronin-inline-slice-hoisting`

Kept, but it came in at a third of the predicted size, and the shortfall is
the interesting part.

Inline capacity charges a discriminant check per access, and six traversal
loops paid it per element rather than per edge — several also re-resolved the
arena entry itself, `graph.edge(edge).out[index]` doing an arena bounds check,
an inline-vs-heap branch and a slice bounds check for every element. Binding
`let outputs: &[NodeId] = &graph.edge(edge).out;` once resolves all three. Every
hoist compiled unchanged, which confirms the per-element form was gratuitous
rather than borrow-driven.

| Metric | before | after | change |
| --- | ---: | ---: | ---: |
| Instructions, wide no-op | 24,102,023 | 23,905,714 | −0.81% |
| Instructions, command evaluation | 42,784,164 | 42,784,009 | −0.0004% |
| `DirtyEvaluator::evaluate` | 1,348,434 | 1,220,413 | −9.5% |
| `Builder::add_target` | 1,772,899 | 1,752,928 | −1.1% |

The prediction was around 2%, recovering the 212,024 and 328,070 instructions
that inline capacity had added to `add_target` and `DirtyEvaluator::evaluate`.
Only the latter gave much back, and command evaluation did not move at all.
The explanation is loop length: that workload's edges have one input and one
output apiece, so there is nothing to amortize a hoist over, and `add_target`
already bound its edge to a local, leaving only a discriminant that the
optimizer was evidently hoisting on its own. Hoisting pays in proportion to how
long the loop is, which the estimate failed to account for. Wall time was not
measured — 0.81% is well inside the noise floor established above.

### The scanner's byte primitive — `ronin-scanner-byte-primitive`

Kept at 1.35%, against an estimate of 6–8%. The miss invalidates the model the
estimate was built on, which matters more than the gain.

`current()` resolved `&Arc<Source>` to `Vec<u8>` to slice on every byte read,
and `next()` re-read the byte it was consuming in order to test it for a
newline, then read *again* to return a value that all nineteen call sites
discard. Three bounds-checked loads per byte where one suffices. Caching the
byte slice in the `Scanner`, dropping `next`'s return type, and adding
`advance_within_line` for the hot loops — all of which match the newline case
in an earlier arm, making the test dead work — addresses every one.

| Metric | before | after | change |
| --- | ---: | ---: | ---: |
| Instructions, wide no-op | 23,905,714 | 23,584,069 | −1.35% |
| Instructions, command evaluation | 42,784,009 | 42,226,722 | −1.30% |
| `scan::space` | 964,293 | 772,233 | −19.9% |
| `scan::scanstring` | 2,376,959 | 2,273,389 | −4.4% |
| `scan::name` | 1,128,453 | 1,152,462 | +2.1% |

`space` gained most because it is almost entirely `singlespace` calling `next`.
`name` went marginally the wrong way, which at 2% on one function is codegen
noise from changed inlining rather than a real cost.

The estimate assumed the lexer cost about 18 instructions per manifest byte and
that removing two loads and a branch would roughly halve it. The arithmetic was
right and the model was wrong. `scanstring` runs 16,005 times over a 129,809-byte
manifest — **8.1 bytes per call** — so its 142 instructions per call are
dominated by per-*call* overhead (allocating and returning the fragment list,
`push_literal`, the `space` call on the path branch), not by the byte loop.
Optimizing the byte loop could only ever reach a fraction of it.

Two consequences. Deriving line and column lazily on error, the remaining half
of the original proposal, is now rejected without being attempted: it would
require stripping line and column from `ByteSpan` and recovering them in
`source_span`, an invasive change across the error surface, to remove one
increment from a loop that runs eight times per call — around 0.5% by the same
arithmetic that just overestimated by five times. And the escape-free fast path
becomes the priority, precisely because it attacks per-call cost: a path with no
`$` needs no fragment list, no evaluation and no copy, and in this fixture that
is every path.

### The escape-free path — `ronin-plain-path-fast-path`

`ScannedEvalString` became an enum: `Plain(&'source [u8])` for a value in which
no `$` ever appeared, `Parts(…)` for one that needs expanding. Almost every
manifest path is `Plain`, and `node_for` can then hash and probe a `Plain` path
that `is_canonical` already accepts **directly against the manifest bytes** —
no evaluation, no copy into scratch, no canonicalization pass, and no
allocation at all unless the node turns out to be new.

| Metric | before | after | change |
| --- | ---: | ---: | ---: |
| Instructions, wide no-op | 23,584,069 | 22,776,535 | −3.42% |
| Instructions, command evaluation | 42,226,722 | 40,892,549 | −3.16% |
| `node_for` + `canonpath` | 1,790,554 | 1,078,356 | −39.8% |
| Requested bytes, wide no-op | 4,987,636 | 4,794,044 | −3.9% |
| Requested bytes, command evaluation | 8,176,630 | 7,726,990 | −5.5% |

`canonpath` disappears from the profile entirely, inlined into a `node_for`
that no longer calls it on the common path. The win is almost exactly that
pair: −712,198 of the −807,534 total.

The estimate was about 10% and the shortfall is instructive in a different way
from the last two. The mechanical part landed as predicted — evaluation,
copying and canonicalization for plain paths are simply gone. What did not move
is `mknode`, unchanged at 1,388,925, which the estimate had counted as partly
addressable. Hashing a path and probing the index is irreducible: interning
requires it however the bytes arrive. Roughly half the estimate was assigned to
work that no fast path can remove.

A first cut of this change regressed requested bytes by about 4% while leaving
the request count flat, which is the signature of a widened value rather than a
new allocation. The enum is `max(16, 32)` plus a discriminant — 40 bytes against
the previous struct's 32 — so every `Vec<ScannedEvalString>` grew, and
`scanpaths` builds one per run of paths. The fix was to notice that the inline
slot in `ScannedParts` had been made redundant: it existed to avoid an
allocation for single-fragment values, and `Plain` now takes exactly those.
Reverting `ScannedParts` to a plain `Vec` returns the enum to 32 bytes, and
bytes then land *below* where the node started. Inline capacity is worth paying
for only where it is load-bearing; here a better representation had displaced it.

### Pruning clean subtrees — `ronin-prune-clean-subtrees`, rejected

Attempted, disproved by an existing test, reverted. Two separate reasons, and
the second is the more useful one.

The premise was that a no-op build walks the graph twice: `DirtyEvaluator::evaluate`
computes dirtiness, then `add_node` walks it again to plan. `recompute_edge_dirty_with`
stamps one dirty bit onto *every* output of an edge (graph.rs:298-300), and
folds `input_dirty` in, so a clean edge appears to prove its entire input cone
clean — nothing wanted, no missing input hiding there.

**The induction is wrong, and the test suite produced the counterexample in
seconds.** `input_dirty` is folded over `non_order_only_inputs()` only, so a
clean edge may still carry a *dirty order-only input*: order-only dependencies
are built without dirtying their consumer. That is exactly
`ninja_build_generated_dyndep_now_wants_edge_and_dependent`, whose manifest is
`build $dir/tmp: touch || $dir/dd`. `tmp` exists and is clean, `dd` is missing
and dirty, and `tmp` is reached through the non-order-only input of another
clean edge. Pruning at the first clean edge never reaches `dd`; the build ran
zero commands where three were required. Restricting the prune to descend into
order-only inputs of the pruned edge does not help, because the dirty
order-only input lives further down, behind an edge the prune already skipped.
Finding it requires precisely the traversal the change was trying to avoid.

A sound version exists: fold a per-edge "this subtree contains work" flag during
the dirty pass's `Work::Finish`, which does visit the full input vector, and
prune on that. It is not cheap to get right. Restat-clean edges short-circuit at
graph.rs:240 without reading their inputs at all, so their flag would be
asserted rather than computed, and dyndep adds inputs mid-build, so the flag
needs invalidating on paths that currently need no such care. That is new state
in the correctness-critical part of the planner.

**It would not have shown up on any fixture we have.** Measured with the
unsound version in place, the wide no-op build came to 22,796,553 instructions
against 22,776,535 without it — 20,018 *worse*, because the prune never fires
and only its guard costs. Every edge in that fixture is dirty: `build leaf/N: phony`
has no file on disk, so `missing_without_inputs` holds and
`recompute_edge_dirty_with` returns dirty for all 4,000 leaves and for the
`all` edge above them.

That is worth stating plainly, because the workload is called `wide-noop-build`
and is not a no-op: it is a fully dirty phony graph, and has been serving this
programme as a *scanning* benchmark. **The most common real workload for a build
tool — running it against an up-to-date tree, where the whole graph is clean —
is not represented in this suite at all.** Every improvement recorded above was
selected against parse-and-plan-dominated fixtures. A clean-tree fixture, with
real files on disk and current mtimes, would measure a different part of the
program, and is a prerequisite for judging this node rather than a nicety.

### What the instruments in this document do not measure

Three limits, each found the hard way, each after a measurement had already
been believed.

**Callgrind does not count kernel instructions.** It is a user-space simulator,
so every syscall is a black box. On a 200,000-edge manifest it reports Ronin at
3,074,971,466 against C samurai's 2,729,279,505 — a ratio of 1.13 — while `perf`
reports 5,331,534,241 against 2,939,783,556, a ratio of 1.81. The whole
difference is kernel work Ronin does and samurai does not. Every instruction
count in this document is therefore a *user-space* count, and any conclusion
about syscall-adjacent behaviour drawn from one is unsound.

**Neither callgrind nor `getrusage` measures threaded work.** Both sum across
threads, so parallelism reads as a regression in each: the batched-stat change
showed system time rising from 6.402 to 7.750 ms under `getrusage` and +4.6%
instructions under callgrind on `wide-noop`, while its wall time fell by a
quarter. Only interleaved wall clock measures latency.

**Wall clock alone hides what a change costs to get it.** The same node was
accepted on wall-clock rounds and only later measured at 3,091 ms of task-clock
across 2.858 CPUs, against 1,395 ms across 0.990 before — 2.2 times the CPU for
23% of the wall time. Record task-clock alongside wall time for anything that
spawns threads.

Using `perf` at all needed `kernel.perf_event_paranoid` lowered from Debian's
default of 3 to 1, plus `kernel.kptr_restrict=0` for kernel symbols.

### Scale is the second blind spot

`ronin-clean-tree-fixture` found that every workload here left the graph dirty.
The same class of gap remains in size: every workload is at most 4,001 build
statements, and Ronin's standing against C samurai is not constant across that
axis. Measured on parse-and-evaluate:

| statements | Ronin | samurai | ratio | Ronin µs/edge | samurai µs/edge |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 4,000 | 7.8 ms | 9.4 ms | 0.83× | 1.95 | 2.35 |
| 25,000 | 61.1 | 68.7 | 0.89× | 2.44 | 2.75 |
| 50,000 | 133.3 | 141.5 | 0.94× | 2.67 | 2.83 |
| 100,000 | 296.0 | 298.5 | 0.99× | 2.96 | 2.99 |
| 200,000 | 630.3 | 593.1 | 1.06× | 3.15 | 2.97 |

Ronin's per-edge cost rises 62% across that range against samurai's 26%, so a
17% lead at fixture size becomes a 6% deficit at 200,000 — and the crossover
falls inside the range real projects occupy. Every claim in this document about
beating the C reference holds at the size the fixtures happen to be.

One warning about measuring this, recorded because it already cost a wrong
conclusion. A first attempt used a fixture whose 200,000 declared sources did
not exist, which makes both tools take an error path they handle completely
differently: samurai reports the missing input after 3 `stat` calls, Ronin after
400,001. That measured one tool short-circuiting against the other doing full
work and produced a confidently reported — and entirely false — constant 1.7×
gap. Scaling probes must use a path both tools complete.

### SIMD, and why the gap is not a vectorization gap

The profile also settled the question that prompted it. C samurai contains one
`strcspn`, in `log.c`, and no other vector code: its lexer reads the manifest
one byte at a time through `getc` on a `FILE*`, pushes back with `ungetc`,
classifies with `ctype`, and appends per byte with `bufadd` — 7.84% of its run
in `getc` and 7.56% in `bufadd`. Ronin's lexer cluster costs 4,945,922
instructions against samurai's 7,266,555, and Ronin's allocator cluster
6,201,847 against 8,294,962. We out-scan a `getc`-per-byte lexer by 32% and
still lose overall, so the deficit is not scanning throughput.

The vector code that does appear is libc's, obtained for free on both sides:
`__memcpy_avx_unaligned_erms` at 2.92% and `__memcmp_avx2_movbe` at 1.78% for
Ronin, against `__strcmp_avx2` at 3.95% for samurai — Ronin wins that
comparison precisely because storing lengths permits `memcmp` where a
NUL-terminated C string forces `strcmp`. Explicit SIMD in the lexer was already
tried and rejected under `ronin-memchr-scanner` at a measured 4% ceiling.

The gap is instead concentrated in scalar bookkeeping: `parse` (+1.47M against
samurai's equivalent), `add_target` (+1.09M), `DirtyEvaluator::evaluate`
(+1.02M), `node_for` (+0.80M) and `mknode` (+0.61M), plus `RawVec::grow_one` at
4,638,289 inclusive — 15.52% of the program across 20,115 growth events, which
alone exceeds the 3,385,997-instruction total gap. Growth is malloc plus memcpy
plus free per event, a different cost from the fresh small allocations whose
removal the streamed-path-scanning node showed to be worthless; the largest
callers are `scanstring` (5.01%), `graph::nodeuse` (3.52%), `parse` (3.42%) and
`scanpaths` (1.83%). `nodeuse` and `parse` are `Node.uses` and the edge
vectors, which restores the case for `ronin-smallvec-collections` on
instruction-count grounds even though the allocation-count grounds recorded
above are gone.

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
