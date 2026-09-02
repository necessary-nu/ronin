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
Ninja keeps a large regression from being hidden by an old, slower absolute
baseline.

**What that normalization does NOT do is make the comparison portable across
differently loaded hosts, and reading it as though it did is what made this
gate's drift look like a code regression.** Dividing by Ninja cancels whatever
slows both tools by the same factor. It does not cancel a change in host state
the two tools respond to differently, and on these rows they do.
`scheduler-barrier` was the demonstration: at `-j8` the pinned Ninja achieved
5.46 CPUs utilised against Ronin's 2.63, and Ronin finished that row having
retired 0.71 of Ninja's user instructions, 0.52 of its kernel cycles and 0.50
of its task-clock — wait-bound where Ninja is CPU-bound, and losing wall
anyway. The recorded rows were taken at 99% idle; the same binary that produced
them read 5% to 16% adverse against them on a host at load 12–19.

**That wait has since been found and removed, so the row no longer reads that
way.** It was the `vfork` suspension inside `posix_spawn`, charged to the one
thread that schedules: 266 microseconds a launch, of which only 68 were CPU.
Launching now happens on a bounded pool of spawner threads
(`ProcessSupervisor`'s `SpawnPool`), and the row reads 6.4 CPUs utilised
against Ninja's 5.5, at 0.52 of Ninja's wall. Its Ronin/Ninja ratio also stopped
moving with host state: measured over four windows between load 10.6 and 16.5 it
spans 0.520–0.554, where the same four windows spread the old binary's ratio
across 0.926–1.091.

The lesson about portability outlives the row. A comparison against
`baseline-v1.csv` is only as portable as the host state is similar, and
`load_average_before` is what says whether it was. That is what the `--max-load`
guard is for, and a run that raises it above 4.00 has opted out of the record's
comparability, not merely out of waiting for quiet. The measurement behind all
of this — three interleaved windows, two of them slot-swapped, running the
baseline's own revision beside today's — is in
[`ninja-baseline-drift-2026-09-02.md`](ninja-baseline-drift-2026-09-02.md).

`baseline-v1.csv`'s `scheduler-barrier` row is now slack by a factor of two and
has not been re-recorded: every window available since the fix has been a host
at load 10 or above, and recording a row there is the exact mistake the drift
above documents. Re-record it on a quiet host.

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
