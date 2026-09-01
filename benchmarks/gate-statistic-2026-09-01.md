# What both wall-time gates judge on — 2026-09-01

Neither gate was measuring the tree. Both took the median of five interleaved
samples per tool and divided, and on a host with anything else running that
statistic straddles its own refusal threshold: **the median of five refuses an
unmodified tree 19.4% of the time on `dependency-log-load`, 15.6% on
`path-canonicalization` and 9.1% on `vim-noop`**. A gate that red-lights a tree
nobody touched teaches everyone to re-run it until it goes green, which is the
same as having no gate.

This is the measurement that replaced the statistic and set each row's
repetition count. It is not a re-recording: the recorded rows in
[`make-baseline-v1.csv`](make-baseline-v1.csv) and
[`baseline-v1.csv`](baseline-v1.csv) are unchanged, and the last section says
why.

## Provenance

- Ronin revision `4f71437`, kati `4ac0718`, clean outside `plan/`,
  `examples/` and `scripts/`
- Release profile, `crt-static`, `rustc 1.97.1`
- Platform: Linux `6.12.100+deb13-amd64`, x86_64, 32 cores
- Oracles: `GNU Make 4.4.1` (`reference/make-oracle`), pinned Ninja
  `b51a1e37c2fb89bbefa600bd155e1ce13983f09d`, C samurai 1.9.0
- One-minute load average **10.7 to 16.5** throughout, floor held by four
  runaway `nsh` processes belonging to another tenant. `--max-load 20` was
  raised deliberately, which is what the guard's own refusal message asks for.
  Every number below is a RATIO of two tools sampled interleaved into the same
  competition, which is what survives a loaded host; the milliseconds beside
  them do not and are not quoted as results.

## The pools

Both harnesses grew `--samples PATH`, which writes every individual sample
rather than the one number per tool per workload the report carries. A summary
is enough to gate on and not enough to argue about — choosing a statistic or a
repetition count is a question about the SPREAD, and the spread is exactly what
a median throws away.

One pool per gate, **151 interleaved repetitions of every workload**: 4 Make
rows against GNU Make, 8 Ninja rows against pinned Ninja and C samurai. 3,624
tool invocations in all.

## How a false refusal was counted

Synthetic gate runs were drawn out of each pool by **moving-block bootstrap** —
blocks of five consecutive repetitions, 20,000 synthetic runs per (estimator,
count, row). Blocks rather than individual samples, and that is the whole
methodology: a gate run is a contiguous slice of a drifting host, and shuffling
individual samples destroys the autocorrelation that is what makes a gate flap.
Shuffled resampling reports a false-refusal rate far below the truth.

A refusal is the gate's own rule: the run's statistic above **1.20 times** the
pool's own value of the same statistic.

## What the median of five does

| Workload | Gate | Refusals in 20,000 |
| --- | --- | ---: |
| `dependency-log-load` | Ninja | **19.355%** |
| `path-canonicalization` | Ninja | **15.585%** |
| `manifest-command-evaluation` | Ninja | **11.400%** |
| `deep-graph-evaluation` | Ninja | **9.400%** |
| `vim-noop` | Make | **9.075%** |
| `wide-noop-build` | Ninja | **7.560%** |
| `clean-tree-noop` | Ninja | 2.000% |
| `large-manifest-parse` | Ninja | 0.000% |
| `scheduler-barrier` | Ninja | 0.000% |
| `wide-noop` | Make | 0.000% |
| `recursive-noop` | Make | 0.000% |
| `zsh-incremental` | Make | 0.000% |

Six rows out of twelve refuse a tree nobody touched. A Ninja run has to pass all
eight of its rows at once, so a whole run refuses about half the time — which is
the three-out-of-three on an unmodified binary that
`a-sessions-allocations-die-together-and-are-freed-one-by-one` recorded, and not
the host being unusual.

## Six estimators, same pools

Refusals in 20,000 bootstrapped runs on the two worst rows:

| Estimator | `vim-noop` @5 | @15 | @31 | `dependency-log-load` @5 | @15 | @31 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| median | 9.075% | 1.590% | 0.195% | 19.355% | 7.775% | 2.320% |
| 20% trimmed mean | 0.000% | 0.000% | 0.000% | 9.640% | 1.935% | 0.120% |
| median of paired ratios | 2.730% | 0.055% | 0.000% | 14.990% | 3.205% | 0.500% |
| 25th percentile | 2.130% | 0.070% | 0.000% | 9.325% | 0.245% | 0.005% |
| **10th percentile** | 2.895% | 0.075% | **0.000%** | 3.355% | 0.010% | **0.000%** |
| minimum | 2.760% | 0.015% | 0.000% | 1.275% | 0.000% | 0.000% |

A no-op's samples have a hard floor — what the tree actually costs — and a long
right tail that is the rest of the machine, because contention can only ADD
time. The informative part of the sample is the bottom of it, and every central
estimator spends its whole life in the contaminated part. The trimmed mean is
the tightest thing there is on the Make rows and still refuses 0.12% of
`dependency-log-load` at thirty-one repetitions; the median needs 51+
repetitions to reach what a low quantile reaches at 21, which on
`zsh-incremental` is three minutes of gate.

**The minimum is the extreme of the argument and is the one thing this must not
use.** A minimum is not a consistent statistic: its value falls as you take more
samples, so a row recorded from five and checked against thirty-one is two
different numbers, and the repetition count could never be changed again. The
tenth percentile is the same number at any sample count and needs only enough
samples to locate.

## The counts, and why they are per workload

`vim-noop` is 34 ms and `zsh-incremental` is 1.8 s. One number for the catalog
is the wrong number for at least one row in it, and they need different counts
anyway, because the spread that has to fit inside the margin is a different
spread.

Each count is the smallest on the grid at which no run out of 20,000 refused and
the one-in-a-thousand draw stays inside 1.15 of the row's own centre — so at
least a quarter of the 1.20 margin is left for a regression to occupy rather
than spent on the host.

| Workload | Repetitions | p99.9 draw / centre | Refusals |
| --- | ---: | ---: | ---: |
| `vim-noop` | 31 | 1.113 | 0.000% |
| `recursive-noop` | 31 | 1.054 | 0.000% |
| `wide-noop` | 21 | 1.133 | 0.000% |
| `zsh-incremental` | 15 | 1.084 | 0.000% |
| `vim-clean-build` | 5 | recorded, not gated | — |
| `manifest-command-evaluation` | 21 | 1.085 | 0.000% |
| `deep-graph-evaluation` | 21 | 1.113 | 0.000% |
| `wide-noop-build` | 21 | 1.041 | 0.000% |
| `path-canonicalization` | 21 | 1.118 | 0.000% |
| `dependency-log-load` | 31 | 1.117 | 0.000% |
| `scheduler-barrier` | 21 | 1.097 | 0.000% |
| `clean-tree-noop` | 21 | 1.086 | 0.000% |
| `large-manifest-parse` | 31 | 1.131 | 0.000% |

**The false-refusal rate designed for is below one in ten thousand per row**,
against one in five for the statistic it replaces. The whole gated Make catalog
is about a hundred seconds, which is what nine repetitions of everything already
cost `scripts/check-release.sh`; that script no longer passes a count at all,
because the harness knows which row is a 34 ms no-op and it did not.

## Run against the real gates, before and after

The bootstrap is a model of the gate. This is the gate. Both arms — the
`4f71437` harnesses and these — were run alternately in one window, on a tree
neither of them changes, six rounds each, `--max-load 25` on both arms so that
the comparison is about the statistic and not about one arm declining to
measure.

| Gate | Arm | Refusals | Rows that refused |
| --- | --- | ---: | --- |
| Make | median of 5 | 0 / 6 | — |
| Make | decile, per-row counts | 0 / 6 | — |
| Ninja | median of 5, no load guard | **6 / 6** | `deep-graph-evaluation` ×2, `manifest-command-evaluation` ×2, `path-canonicalization`, `wide-noop-build` |
| Ninja | decile, per-row counts | **2 / 6** | `path-canonicalization`, `wide-noop-build` |

The one-minute load average ran between 12.8 and 21.6 across the window, which
is four to five times the guard both gates now run behind. Six runs is not
enough to see a 9% per-run rate on the Make gate, which is what the 20,000-run
bootstrap above is for; it is more than enough to see a rate near one.

**The two remaining Ninja refusals are the recorded rows, not the statistic**,
and they say the same thing the last section does: both are rows sitting 5% to
13% adverse against `baseline-v1.csv`, so the margin they had left was small
before the host took any of it. At the gate's OWN `--max-load 4.00` neither run
would have happened — it would have waited five minutes and then refused to
measure, which is the correct answer on a host at load 19 and is exactly what
`examples/baseline` could not say before this change.

## Not one threshold moved

Recorded-ratio tolerance 1.20 on both gates, absolute Ronin-versus-Ninja 1.20,
peak RSS 2.00, quiet-host guard 4.00. Peak RSS keeps its median deliberately:
the decile exists to see under a right tail that is contention, and the failure
RSS is gated against IS a high number.

`examples/baseline` now runs behind the same quiet-host guard
`examples/make_baseline` had, which is the other half of this fix: the statistic
answers WITHIN-run spread, the load guard answers BETWEEN-window drift, and they
are different noise sources. Both reports now carry `load_average_before`,
`load_average_after` and `max_load`.

## Why the rows were not re-recorded

Three full recording passes were taken with the new statistic before deciding.

The Make rows are where they are recorded to be:

| Workload | pass 1 | pass 2 | pass 3 | recorded |
| --- | ---: | ---: | ---: | ---: |
| `wide-noop` | 0.56× | 0.61× | 0.58× | 0.62× |
| `recursive-noop` | 0.98× | 0.95× | 0.98× | 1.01× |
| `vim-noop` | 1.70× | 1.66× | 1.69× | 1.71× |
| `zsh-incremental` | 1.05× | 1.09× | 1.06× | 1.03× |

The filing on `the-make-gate-flaps-because-five-repetitions-is-too-few` says it
in one line — *the rows are not drifting, the gate's verdict is* — and this is
that claim measured. Re-recording would have treated a symptom the node
establishes is not the disease.

**The recorded rows are medians and the run's times are deciles, and that costs
an order of magnitude less than the margin.** What is compared is a RATIO, and a
ratio is very nearly independent of which quantile it is taken at, because both
tools' distributions shift together under the same contention. Over the same
pools, the decile ratio and the median ratio agree to within 3% on ten of the
twelve rows and 6.2% on the widest (`large-manifest-parse`).

**Seven of the eight Ninja rows read 5% to 13% ADVERSE against
`baseline-v1.csv`, measured with the same median the record used.** A systematic
move of that size across seven rows at once is a host or a drift to attribute,
not a set of numbers to overwrite — and `scheduler-barrier` is the sharp end of
it: 1.19× against a recorded 1.0158, which is 17% of a row sitting 3% under the
tolerance that would refuse it. That belongs to a node of its own, on a machine
quiet enough for the guard above. Recording it away is exactly the failure this
work exists to stop.
