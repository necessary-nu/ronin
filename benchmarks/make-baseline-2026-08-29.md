# Ronin Make mode against GNU Make 4.4.1 — 2026-08-29

The first re-recording since the parallel-Makefile-read campaign began. The
file it replaces held figures taken before the read pool existed, and it held
them for four commits — not because nobody looked, but because `make_baseline`
refuses to sample above a one-minute load average of 4.00 and this host sat
between 7 and 182 for three consecutive nodes. It went to 0.74.

## Provenance

- Harness schema: `ronin-make-performance-baseline-v1`
- Workload version: 1
- Ronin revision: `551d5741be0a5bc4f848c031a6697d093e0ff0e7`, clean tree
- Oracle: `GNU Make 4.4.1`, `reference/make-oracle/make-4.4.1/make`
- Ronin: `GNU Make compatible: ronin 0.1.0`, release profile, invoked through a
  `make`-named symlink because Make mode is reached by the invoked name
- Platform: Linux `6.12.100+deb13-amd64`, x86_64, 32 cores
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- `-j8` for both tools on every workload
- One warm-up, five interleaved samples per tool per workload
- One-minute load average 0.74 at the start, 8.02 at the end — the rise is this
  gate's own clean builds
- Noise control: interleaved samples, rotating which tool goes first each
  repetition; `MAKEFLAGS`/`MFLAGS`/`MAKELEVEL` cleared; stdout and stderr
  discarded; blocking wait with no sampling thread; median wall time

## Results

| Workload | GNU Make | Ronin | Ronin / GNU | 2026-08-27 |
| --- | ---: | ---: | ---: | ---: |
| `wide-noop` — 4,000 explicit rules, one directory, no recursion | 918.201 ms | 748.759 ms | **0.82×** | 2.35× |
| `recursive-noop` — 259 Makefiles, fan-out 6, depth 3 | 78.654 ms | 162.805 ms | **2.07×** | 13.78× |
| `vim-noop` — vim 9.2.0957 up to date, from the top | 15.834 ms | 27.269 ms | **1.72×** | 2.30× |
| `zsh-incremental` — zsh 5.9.2 steady state | 1360.252 ms | 2077.537 ms | **1.53×** | 1.75× |
| `vim-clean-build` — vim from empty | 15185.080 ms | 15125.885 ms | **1.00×** | 1.00× |

## What the numbers say

**`recursive-noop` went from 13.78× to 2.07×.** 1119 ms to 163 ms against a GNU
Make figure that has not moved — 81 ms then, 79 ms now. That is what the
parallel-read campaign has been worth, measured end to end for the first time.
Per unit it is 4.3 ms to 0.63 ms, against GNU Make's 0.30 ms for a unit it pays
a `fork` and an `exec` for.

**`wide-noop` is now a win.** 749 ms against 918 — Ronin is 18% faster than GNU
Make on the one workload in the set with no recursion in it at all. It was 2.35×
slower in August. Graph construction is no longer the problem; composition is.

**The clean build is still a wash**, 1.00×, and still the row that matters most
to somebody actually using this as their `make`: on a real build the front end's
cost disappears into the compiler's.

**Both remaining losses are recursion-shaped.** `zsh-incremental` should be read
with the same correction the 2026-08-27 record applies to it — roughly 1.3 s of
that GNU figure is `gcc` and `ld`, identical work for both tools, so the front
end's own share is about 60 ms for GNU against about 780 ms for Ronin. That is
the same finding `recursive-noop` isolates, in a tree somebody really uses.

**Where the remaining `recursive-noop` gap is.** Of the 84 ms between 163 and
79, roughly 32 ms is the composing thread waiting for reads it could have
started earlier — measured by running the same 259 units as a flat tree, where
every read is dispatched in one burst and the run finishes in 160 ms while the
fan-out-6 tree takes 192. Most of the rest is resolving child invocations on the
composing thread, which is 55% of it and which costs nothing today precisely
because it happens inside that wait.

## Two traps in the recorder

**Record with `--clean-build`; validate without it.** Without the flag the
recorder writes a four-row file and silently omits `vim-clean-build`, because
that workload sits outside `GATED_WORKLOADS` on purpose. `--validate` passes on
the remaining four, so nothing complains and the row is simply gone.

**`--output` is not the recorded baseline.** It writes the full six-column
measurement record with provenance comments, which is what belongs in a file
like this one. `benchmarks/make-baseline-v1.csv` is the distilled three-column
reference the harness compiles in with `include_str!`, and it must begin
`workload,gnu_median_ms,ronin_median_ms`. Pointing `--output` at it replaces the
reference with the raw record, and the harness's own tests are what say so.
