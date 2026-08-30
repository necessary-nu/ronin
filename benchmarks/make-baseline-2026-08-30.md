# Ronin Make mode against GNU Make 4.4.1 — 2026-08-30

The recorded baseline had been stale since 29 August. `zsh-incremental` stood
at 1.62× while the tree measured 1.02×, which meant the gate would have
accepted a silent regression all the way back to 1.94× before complaining.
This recording closes that hole and re-records the other four rows with it.

## Provenance

- Harness schema: `ronin-make-performance-baseline-v1`
- Workload version: 1
- Ronin revision: `659ac7601ac4e8dee5983a88da96f13513a093ee`, clean tree
- Oracle: `GNU Make 4.4.1`, `reference/make-oracle/make-4.4.1/make`
- Ronin: `GNU Make compatible: ronin 0.1.0`, release profile, invoked through a
  `make`-named symlink because Make mode is reached by the invoked name
- Platform: Linux `6.12.100+deb13-amd64`, x86_64, 32 cores
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- `-j8` for both tools on every workload
- One warm-up, five interleaved samples per tool per workload
- One-minute load average **2.02** at the start, 16.73 at the end — the rise is
  this gate's own ten clean builds of vim
- Noise control: interleaved samples, rotating which tool goes first each
  repetition; `MAKEFLAGS`/`MFLAGS`/`MAKELEVEL` cleared; stdout and stderr
  discarded; blocking wait with no sampling thread; median wall time
- One tree per workload with the tools rotating inside it, never a tree each —
  see the warning at the foot of `make-baseline-2026-08-29b.md`

The host was busy for most of the day at load 12–25 with another tenant's work.
This is the first window under 4.0 that appeared; it opened at 23:29 and the
whole recording ran inside it. Nothing here was measured on a loaded machine.

## Results

| Workload | GNU Make | Ronin | Ronin / GNU | 2026-08-29b |
| --- | ---: | ---: | ---: | ---: |
| `wide-noop` — 4,000 explicit rules, one directory, no recursion | 961.601 ms | 594.508 ms | **0.62×** | 0.60× |
| `recursive-noop` — 259 Makefiles, fan-out 6, depth 3 | 86.848 ms | 87.671 ms | **1.01×** | 0.95× |
| `vim-noop` — vim 9.2.0957 up to date, from the top | 15.458 ms | 26.440 ms | **1.71×** | 1.74× |
| `zsh-incremental` — zsh 5.9.2 steady state | 1433.399 ms | 1480.547 ms | **1.03×** | 1.62× |
| `vim-clean-build` — vim from empty | 20871.454 ms | 20956.317 ms | **1.00×** | 1.00× |

The run validated against the outgoing baseline before writing this one: every
row inside the 120% band, exit 0.

## What the numbers say

**`zsh-incremental` is 1.03×, and that is the reason this recording exists.**
The recorded 1.62× was measured before `the-composition-stops-where-gnu-make-stops`
landed; the row has since been verified three ways at 1.02×. A gate whose
recorded ratio is 1.62× accepts anything up to 1.94×, so the stale row was not
merely out of date — it was a hole big enough to drive the whole of that node's
improvement back through without a single gate failing. It is now 1.03× and the
ceiling is 1.24×.

**`vim-noop` is 1.71×, down from 1.74×, and it remains the only row above
parity.** Do not read the 0.03 as progress: five samples a side at these
absolutes cannot resolve it, and the one change that landed against this row
today is worth 3.2% of the sub-make's *instructions* and nothing the clock can
see. What the row costs is now understood in detail and written up on
`the-symbol-maps-hash-an-index-with-siphash`: the workload is recursive rather
than the single-Makefile read the campaign has been assuming, 80% of the gap
lives inside the `src` sub-make, and 13.5% of that sub-make is Ronin running as
its own `/bin/sh`, nearly half of which is `nsh::builder::Builder::build`
constructing the shell before it parses anything.

**`recursive-noop` records 1.01× where it recorded 0.95×, and this LOOSENS that
row's gate from a 1.14× ceiling to 1.21×.** Stated plainly because it is the
one thing in this table that makes the gate weaker. It is not a regression: the
two tools are at parity and the difference is five-sample spread. Ronin's
samples ran 80.6 / 87.7 / 116.1 ms (min / median / max) against GNU's 82.2 /
86.8 / 93.2 — one long Ronin sample moves the median by more than the whole
delta being recorded. Whoever next has cause to touch this row should consider
raising `--repetitions` for it rather than reading either 0.95× or 1.01× as a
fact about the tools.

**`wide-noop` is 0.62×, and both tools measured faster than on 29 August**
(GNU 1027.9 → 961.6 ms, Ronin 613.8 → 594.5). The workload and Ronin's handling
of it are unchanged; this is the host being quieter than it was.

**`vim-clean-build` is 1.00×, and both tools got 26% slower in absolute terms**
— 16.5 s each on 29 August, 20.9 s each here. Both moved together, by the same
proportion, so it is the machine and not either tool: this row is overwhelmingly
the compiler both tools spawn, and the host has another tenant on it. The ratio
is what is recorded and what is gated, and it did not move. The absolutes are
worth nothing except as a reminder that this file's columns are only comparable
against each other within a single recording.

## The six-column record this was distilled from

`benchmarks/make-baseline-v1.csv` carries the three-column distillation, because
the harness `include_str!`s it and once had three tests broken by a raw
six-column `--output`.

```
# schema=ronin-make-performance-baseline-v1
# workload_version=1
# ronin_revision=659ac7601ac4e8dee5983a88da96f13513a093ee
# ronin_dirty=false
# build_profile=release
# platform=Linux n-debian13-dev 6.12.100+deb13-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.12.100-1 (2026-07-30) x86_64 GNU/Linux
# rustc=rustc 1.97.1 (8bab26f4f 2026-07-14)
# jobs=8
# warmups=1
# repetitions=5
# load_average_before=2.02
# max_load=4.00
# noise_control=interleaved tool samples; stdout/stderr discarded; blocking wait, no sampling thread; MAKEFLAGS/MFLAGS/MAKELEVEL cleared; median wall time; no CPU pinning
# validation=Ronin/GNU runtime ratio <= 120% of the recorded ratio; no absolute threshold, because Ronin is slower than GNU Make on these workloads and the recorded numbers say so
# sizes=wide:4000,recursive:259units(fanout 6 depth 3 leaf 8)
# gnu-make=GNU Make 4.4.1
# ronin=GNU Make compatible: ronin 0.1.0
tool,workload,median_ms,min_ms,max_ms,samples
gnu-make,wide-noop,961.601,917.826,999.722,5
ronin,wide-noop,594.508,557.884,724.503,5
gnu-make,recursive-noop,86.848,82.174,93.230,5
ronin,recursive-noop,87.671,80.646,116.069,5
gnu-make,vim-noop,15.458,14.862,16.004,5
ronin,vim-noop,26.440,25.885,27.281,5
gnu-make,zsh-incremental,1433.399,1381.883,1834.069,5
ronin,zsh-incremental,1480.547,1418.757,1500.930,5
gnu-make,vim-clean-build,20871.454,18264.971,22029.391,5
ronin,vim-clean-build,20956.317,19989.574,22636.681,5
# load_average_after=16.73 (includes this gate's own workloads)
# ratio wide-noop=0.62x
# ratio recursive-noop=1.01x
# ratio vim-noop=1.71x
# ratio zsh-incremental=1.03x
# ratio vim-clean-build=1.00x
```
