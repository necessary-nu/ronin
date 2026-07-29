# Ronin performance validation — 2026-07-29

This clean-revision run validates the completed structural work against both
the recorded Ronin v1 baseline and the pinned Ninja oracle. It was produced by
`scripts/check-performance.sh` with two warmups and 15 interleaved samples per
tool and workload.

## Provenance

- Ronin revision: `1b4518c6b4a0fd0490d63005d00ca72becb3c73c`
- Ronin dirty: false
- Ninja revision: `b51a1e37c2fb89bbefa600bd155e1ce13983f09d`
- Harness schema: `ronin-performance-baseline-v2`
- Workload version: 1
- Profile: release
- Rust: `rustc 1.92.0 (ded5c06cf 2025-12-08)`
- Platform: Linux `6.12.57+deb13-amd64`, x86_64
- Sampling: two warmups, 15 interleaved samples, median wall time
- Memory: coordinator-process `VmHWM` sampled from `/proc` every 100 µs
- Noise control: output discarded; no CPU affinity or frequency control
- Raw results:
  [`performance-validation-2026-07-29.csv`](performance-validation-2026-07-29.csv)

## Gate results

The runtime comparison is normalized to Ninja on each run because the host was
visibly noisy. This avoids treating system-wide load as a Ronin regression.
Every current Ronin median is also below the current Ninja median.

| Workload | Ronin / Ninja | Recorded ratio | Ratio change | Ronin / Ninja peak RSS |
| --- | ---: | ---: | ---: | ---: |
| Manifest command evaluation | 0.572× | 0.900× | −36.5% | 1.31× |
| Deep graph evaluation | 0.588× | 16.473× | −96.4% | 0.86× |
| Wide no-op build | 0.584× | 0.671× | −13.0% | 1.23× |
| Path canonicalization | 0.645× | 0.620× | +3.9% | 0.93× |
| Dependency-log load | 0.591× | 0.644× | −8.3% | 0.76× |
| Scheduler barrier | 0.724× | 1.004× | −27.9% | 0.76× |

The gate permits at most 120% of the recorded Ronin/Ninja runtime ratio, 120%
of current Ninja runtime, and 200% of current Ninja peak RSS. All checks pass.
The 3.9% canonicalization ratio movement is below the material-regression
threshold and is consistent with the wide min/max spread on this unpinned host.
No workload has an unexplained material regression.

The original baseline did not record allocation counts, and no allocation
profiler is present in this environment. The v2 gate therefore uses peak RSS as
its reproducible memory signal. Allocation-specific claims are intentionally
not made from this run.
