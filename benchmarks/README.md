# Ronin performance baseline

The dependency-free `baseline` Cargo example compares release builds of Ronin,
the pinned upstream Ninja oracle, and (when present) the original C samurai
reference. It generates deterministic version-1 workloads for:

- manifest parsing and command evaluation;
- deep graph evaluation;
- wide no-op builds;
- path canonicalization;
- `.ninja_deps` population and load;
- completion and barrier scheduling.

Build the three binaries, then run:

Both references live under `reference/` in the checkout, which is gitignored.
They used to live in `/tmp`, where a reboot silently destroyed them and the
gates went on running against whatever was left.

The C samurai reference is built from an upstream checkout; this repository no
longer carries the C sources it was ported from. `baseline` fails when it is
missing rather than dropping the `samurai-c` rows: pass `--without-samurai` to
ask for a two-way comparison deliberately.

```sh
cargo build --release
git clone https://git.sr.ht/~mcf/samurai /tmp/samurai-upstream
cc -O2 -std=c99 -o reference/samurai /tmp/samurai-upstream/*.c -lrt
git clone https://github.com/ninja-build/ninja reference/ninja
git -C reference/ninja checkout b51a1e37c2fb89bbefa600bd155e1ce13983f09d
cmake -S reference/ninja -B reference/ninja-build -DCMAKE_BUILD_TYPE=Release
cmake --build reference/ninja-build
cargo run --release --example baseline -- --warmups 1 --repetitions 5
```

The defaults point at `reference/`, so the paths above only need repeating when
they are somewhere else.

The harness refuses a Ninja source checkout other than commit
`b51a1e37c2fb89bbefa600bd155e1ce13983f09d`. Results include the Ronin and
Ninja revisions, tool versions, release profile, platform, Rust compiler,
workload sizes, repetition count, and noise-control method. Runs interleave
tools, discard command output, warm each workload, and report median, minimum,
and maximum wall time. On Linux they also sample each coordinator process's
peak RSS from `/proc`. CPU frequency and affinity are not controlled, so
compare large changes and rerun noisy cases before drawing conclusions.

The recorded Ronin and pinned Release Ninja medians are stored as
machine-readable input in [`baseline-v1.csv`](baseline-v1.csv). Run the
release gate with:

```sh
scripts/check-performance.sh --warmups 1 --repetitions 7
```

The gate interleaves Ronin and Ninja samples to reduce temporal bias. It rejects
a Ronin/Ninja runtime ratio above 120% of the recorded v1 ratio or a Ronin
median above 120% of the current pinned Ninja median. On Linux it also rejects
peak RSS above 200% of Ninja. Normalizing the historical comparison against
Ninja makes it portable across differently loaded hosts while preventing a
large regression from being hidden by an old, slower absolute baseline.

The current baseline was recorded on a quiet host against a Ninja binary built
from the pinned source revision with CMake's Release profile. Its provenance,
analysis, and confirmation sweep are in
[`performance-validation-2026-08-10.md`](performance-validation-2026-08-10.md),
with the exact machine-readable primary output beside it. The earlier
clean-revision gate comparison remains in
[`performance-validation-2026-07-29.md`](performance-validation-2026-07-29.md).

The dependency-free `alloc_metrics` Cargo example measures deterministic
in-process allocation requests and requested bytes for the same version-1
workloads under a counting global allocator, reporting both per build
statement. The recorded baseline lives in
[`alloc-metrics-v1.csv`](alloc-metrics-v1.csv) with the initial analysis in
[`alloc-metrics-2026-07-30.md`](alloc-metrics-2026-07-30.md). Validate a
candidate against it with:

```sh
cargo build --release --example alloc_metrics
./target/release/examples/alloc_metrics --check benchmarks/alloc-metrics-v1.csv
```

The check fails when any workload exceeds its recorded allocation requests or
requested bytes by more than 10%; re-record with `--record` after an accepted
improvement.

The process-supervision runtime comparison is recorded in
[`runtime-scalability-2026-07-30.md`](runtime-scalability-2026-07-30.md).
Its isolated thread-per-child, current-thread Tokio, busy-polling, and
readiness-driven implementations live in `runtime-probe`, alongside the
high-concurrency whole-tool A/B harness.
