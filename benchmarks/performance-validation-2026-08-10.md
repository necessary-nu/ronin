# Ronin performance baseline — 2026-08-10

This run re-records `baseline-v1.csv` against the pinned Ninja source built
with its intended Release configuration. The previous baseline still encoded
an unoptimised 1,464,216-byte Ninja binary even though the documented reference
recipe specifies `-DCMAKE_BUILD_TYPE=Release`. Reconfiguring and rebuilding the
same pinned checkout produced a 430,528-byte binary with `-O3 -DNDEBUG` and
`-flto=auto`.

## Provenance

- Ronin revision: `3e6b9479835c5b55514b0de70aebc0c8a4f97a4d`
- Ronin dirty: false
- Ninja revision: `b51a1e37c2fb89bbefa600bd155e1ce13983f09d`
- Ninja version: `1.14.0.git`
- Ninja configuration: CMake Release, `-O3 -DNDEBUG`, LTO enabled
- Ninja binary size: 430,528 bytes
- C samurai version: `1.9.0`
- Harness schema: `ronin-performance-baseline-v2`
- Workload version: 1
- Profile: release
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- Platform: Linux `6.12.100+deb13-amd64`, x86_64
- Sampling: two warmups, 15 interleaved samples per tool and workload, median wall time
- Memory: coordinator-process peak RSS sampled from `/proc` every 100 us
- Noise control: output discarded; no CPU affinity or frequency control
- Raw results:
  [`performance-validation-2026-08-10.csv`](performance-validation-2026-08-10.csv)

The live `vmstat` intervals immediately before the primary run showed 99% CPU
idle, no blocked tasks, and no material I/O wait. An immediate second
two-warmup, 15-sample sweep kept every individual tool median within 7.1% of
the primary run. Its Ronin/Ninja ratios led to the same result for all eight
workloads and differed from the primary ratios by no more than 8.6%.

## Recorded comparison

| Workload | Ronin median | Ninja median | Ronin / Ninja | Ronin / C samurai | Ronin / Ninja peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Manifest command evaluation | 6.015 ms | 12.231 ms | 0.492x | 0.623x | 0.646x |
| Deep graph evaluation | 5.495 ms | 7.619 ms | 0.721x | 0.915x | 0.629x |
| Wide no-op build | 9.314 ms | 11.389 ms | 0.818x | 0.907x | 0.841x |
| Path canonicalization | 4.110 ms | 7.024 ms | 0.585x | 0.644x | 0.629x |
| Dependency-log load | 3.012 ms | 4.136 ms | 0.728x | 0.957x | 0.650x |
| Scheduler barrier | 43.505 ms | 42.828 ms | 1.016x | 1.079x | 0.607x |
| Clean-tree no-op | 7.869 ms | 12.180 ms | 0.646x | 0.704x | 0.802x |
| Large-manifest parse | 161.506 ms | 314.521 ms | 0.513x | 0.529x | 0.848x |

Ronin is faster than the pinned Release Ninja and C samurai on seven of eight
workloads. The scheduler-barrier median is 1.6% above Ninja and 7.9% above C
samurai; it remains well inside the gate's absolute limit of 120% of Ninja.
Ronin's peak RSS is below Ninja's on every workload.

`baseline-v1.csv` now records these primary Ronin and Ninja medians. The gate
thresholds are unchanged: a candidate may use at most 120% of the recorded
Ronin/Ninja ratio, at most 120% of current Ninja wall time, and at most 200% of
current Ninja peak RSS. This makes future checks compare against the intended
optimised reference instead of preserving ratios derived from the stale debug
binary.
