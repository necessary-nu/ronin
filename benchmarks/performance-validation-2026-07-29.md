# Ronin performance validation — 2026-07-29

This clean-revision run validates the completed Ronin release candidate
against both the recorded Ronin v1 baseline and the pinned Ninja oracle. It
was produced by `scripts/check-performance.sh` with two warmups and 15
interleaved samples per tool and workload.

## Provenance

- Ronin revision: `5cc6ee4aadcace1e66ef0eebe5d50541910883a1`
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
| Manifest command evaluation | 0.425× | 0.900× | −52.8% | 1.32× |
| Deep graph evaluation | 0.447× | 16.473× | −97.3% | 0.86× |
| Wide no-op build | 0.470× | 0.671× | −29.9% | 1.22× |
| Path canonicalization | 0.416× | 0.620× | −32.8% | 0.94× |
| Dependency-log load | 0.535× | 0.644× | −16.9% | 0.72× |
| Scheduler barrier | 0.872× | 1.004× | −13.2% | 0.75× |

The gate permits at most 120% of the recorded Ronin/Ninja runtime ratio, 120%
of current Ninja runtime, and 200% of current Ninja peak RSS. All checks pass.
Every normalized runtime ratio improved from the recorded baseline, and no
workload has an unexplained material regression.

The original baseline did not record allocation counts, and no allocation
profiler is present in this environment. The v2 gate therefore uses peak RSS as
its reproducible memory signal. Allocation-specific claims are intentionally
not made from this run.
