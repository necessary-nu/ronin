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

```sh
cargo build --release
cc -O2 -std=c99 -o /tmp/ronin-samu-reference \
  build.c deps.c env.c graph.c htab.c log.c os-posix.c parse.c samu.c \
  scan.c tool.c tree.c util.c -lrt
cargo run --release --example baseline -- \
  --ninja /tmp/ninja-build/ninja \
  --ninja-source /tmp/ninja \
  --samurai /tmp/ronin-samu-reference \
  --warmups 1 --repetitions 5
```

The harness refuses a Ninja source checkout other than commit
`b51a1e37c2fb89bbefa600bd155e1ce13983f09d`. Results include the Ronin and
Ninja revisions, tool versions, release profile, platform, Rust compiler,
workload sizes, repetition count, and noise-control method. Runs are
sequential, discard command output, warm each workload, and report median,
minimum, and maximum wall time. On Linux they also sample each coordinator
process's peak RSS from `/proc`. CPU frequency and affinity are not controlled,
so compare large changes and rerun noisy cases before drawing conclusions.

The original Ronin medians are stored as machine-readable input in
[`baseline-v1.csv`](baseline-v1.csv). Run the release gate with:

```sh
scripts/check-performance.sh \
  --ninja /tmp/ninja-build/ninja \
  --ninja-source /tmp/ninja \
  --warmups 1 --repetitions 7
```

The gate interleaves Ronin and Ninja samples to reduce temporal bias. It rejects
a Ronin/Ninja runtime ratio above 120% of the recorded v1 ratio or a Ronin
median above 120% of the current pinned Ninja median. On Linux it also rejects
peak RSS above 200% of Ninja. Normalizing the historical comparison against
Ninja makes it portable across differently loaded hosts while preventing a
large regression from being hidden by an old, slower absolute baseline.
