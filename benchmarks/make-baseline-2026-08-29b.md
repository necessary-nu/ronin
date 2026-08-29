# Ronin Make mode against GNU Make 4.4.1 — 2026-08-29, second recording

The first recording of the day was taken before the reads were served in the
order the composition asks for them. This one is taken after, and the row that
moved is the one that change was aimed at: `recursive-noop` was recorded at
2.07× that morning and records 0.95× now.

## Provenance

- Harness schema: `ronin-make-performance-baseline-v1`
- Workload version: 1
- Ronin revision: `9399d5f0abb42ff337469ecd6652f1719641b2c0`, clean tree
- Oracle: `GNU Make 4.4.1`, `reference/make-oracle/make-4.4.1/make`
- Ronin: `GNU Make compatible: ronin 0.1.0`, release profile, invoked through a
  `make`-named symlink because Make mode is reached by the invoked name
- Platform: Linux `6.12.100+deb13-amd64`, x86_64, 32 cores
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- `-j8` for both tools on every workload
- One warm-up, five interleaved samples per tool per workload
- One-minute load average 2.91 at the start, 13.13 at the end — the rise is this
  gate's own five clean builds a side
- Noise control: interleaved samples, rotating which tool goes first each
  repetition; `MAKEFLAGS`/`MFLAGS`/`MAKELEVEL` cleared; stdout and stderr
  discarded; blocking wait with no sampling thread; median wall time

## Results

| Workload | GNU Make | Ronin | Ronin / GNU | 2026-08-29a |
| --- | ---: | ---: | ---: | ---: |
| `wide-noop` — 4,000 explicit rules, one directory, no recursion | 1027.907 ms | 613.770 ms | **0.60×** | 0.82× |
| `recursive-noop` — 259 Makefiles, fan-out 6, depth 3 | 89.548 ms | 84.761 ms | **0.95×** | 2.07× |
| `vim-noop` — vim 9.2.0957 up to date, from the top | 16.760 ms | 29.158 ms | **1.74×** | 1.72× |
| `zsh-incremental` — zsh 5.9.2 steady state | 1463.741 ms | 2374.419 ms | **1.62×** | 1.53× |
| `vim-clean-build` — vim from empty | 16542.173 ms | 16580.577 ms | **1.00×** | 1.00× |

## What the numbers say

**`recursive-noop` went from 2.07× to 0.95×, and Ronin is now the faster of the
two on it.** GNU Make has not moved — 78.7 ms in the morning's recording, 89.5
ms here, which is the host rather than the tool. Ronin went 162.8 ms to 84.8 ms
over the day's four commits, and the last of them is most of it: serving a
worker the read the composition will ask for soonest, rather than the read that
happened to be started first, took the wall from about 123 ms to about 100 ms on
its own. The composing thread used to spend half of every run blocked on a read
sitting near the back of a queue two hundred deep while eight workers read units
it would not reach for another sixty milliseconds; it now waits nine
milliseconds in eighty-five. See `src/make/parallel.rs`.

**`wide-noop` improved to 0.60× and neither the workload nor Ronin's handling of
it changed today.** 4,000 explicit rules in one directory with no recursion at
all: the read pool is never created and the read order has nothing to order.
Both tools measure slower in absolute terms than they did this morning (GNU 918
→ 1028 ms, Ronin 749 → 614 ms) and the ratio moved further in Ronin's favour;
five samples a side is what this harness spends, and a ratio that moves by a
fifth between two recordings of unchanged code is the precision to read the rest
of this table with.

**`vim-noop` is the row to look at next, and it is the only one that has not
improved across the campaign.** 1.72× in the morning, 1.74× here, and a
`--clean-build`-less run between the two put it at 1.86×. vim's top-level
Makefile recurses once, into `src/`, so there is exactly one child to read and
nothing to overlap it with — the pool is not created for a single recursive
recipe. Whatever is costing 12 ms there is not the read schedule.

**`zsh-incremental` drifted from 1.53× to 1.62×** across three recordings today
(1.56×, 1.62×, 1.62×). zsh's steady state is dominated by what its own Makefiles
do rather than by how they are read, and at 1.4 to 2.4 seconds a sample the
absolute numbers carry the host's afternoon with them.

**`vim-clean-build` is 1.00×, which is the point of recording it.** Sixteen and
a half seconds a side, of which the overwhelming majority is the compiler both
tools spawn. It is recorded rather than gated for that reason and because it
costs three minutes a recording.

## Two things about the measurement, for whoever records the next one

**The clean-build row needs a quiet start or it is not a measurement.** An
earlier attempt today, begun at load 3.45, returned `vim-clean-build` samples
spanning 15.2 to 26.4 seconds and a median ratio of 0.82× — a number that would
have made a future `--clean-build` run fail validation at the true 1.00×,
because the gate allows 120% of whatever is recorded. Five fifteen-second `-j8`
builds a side do not fit inside the five-minute patience the harness waits with:
the load from one is still decaying when the next begins. This recording was
begun at 2.91 and its spread is 16.4 to 16.8 seconds.

**Do not compare two tools across two directories on this filesystem.** The root
filesystem is 98% full, and two trees written from the same generator minutes
apart are not interchangeable: the same binary measured 88.3 ms in one 216-leaf
tree and 170.9 ms in another. `make_baseline` gives each tool its own directory,
which is right for keeping the tools out of each other's way and wrong for any
comparison drawn between them — this table's rows are each tool against its own
recorded history, and any tool-against-tool number should be taken with both
tools rotating inside one tree.

## The six-column record this was distilled from

```
# schema=ronin-make-performance-baseline-v1
# workload_version=1
# ronin_revision=9399d5f0abb42ff337469ecd6652f1719641b2c0
# ronin_dirty=false
# build_profile=release
# platform=Linux n-debian13-dev 6.12.100+deb13-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.12.100-1 (2026-07-30) x86_64 GNU/Linux
# rustc=rustc 1.97.1 (8bab26f4f 2026-07-14)
# jobs=8
# warmups=1
# repetitions=5
# load_average_before=2.91
# max_load=4.00
# noise_control=interleaved tool samples; stdout/stderr discarded; blocking wait, no sampling thread; MAKEFLAGS/MFLAGS/MAKELEVEL cleared; median wall time; no CPU pinning
# validation=Ronin/GNU runtime ratio <= 120% of the recorded ratio; no absolute threshold, because Ronin is slower than GNU Make on these workloads and the recorded numbers say so
# sizes=wide:4000,recursive:259units(fanout 6 depth 3 leaf 8)
# gnu-make=GNU Make 4.4.1
# ronin=GNU Make compatible: ronin 0.1.0
tool,workload,median_ms,min_ms,max_ms,samples
gnu-make,wide-noop,1027.907,967.857,1157.840,5
ronin,wide-noop,613.770,588.139,694.605,5
gnu-make,recursive-noop,89.548,87.276,147.217,5
ronin,recursive-noop,84.761,82.408,95.203,5
gnu-make,vim-noop,16.760,15.397,20.712,5
ronin,vim-noop,29.158,27.444,32.327,5
gnu-make,zsh-incremental,1463.741,1443.469,1472.309,5
ronin,zsh-incremental,2374.419,2269.970,2443.223,5
gnu-make,vim-clean-build,16542.173,16423.759,16733.649,5
ronin,vim-clean-build,16580.577,16483.251,16767.246,5
# load_average_after=13.13 (includes this gate's own workloads)
# ratio wide-noop=0.60x
# ratio recursive-noop=0.95x
# ratio vim-noop=1.74x
# ratio zsh-incremental=1.62x
# ratio vim-clean-build=1.00x
```
