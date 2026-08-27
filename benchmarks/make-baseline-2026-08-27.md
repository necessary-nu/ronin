# Ronin Make mode against GNU Make 4.4.1 — 2026-08-27

The first measurement of Ronin's Make front end against the tool it stands in
for. `scripts/check-performance.sh` has compared Ronin's **Ninja** mode with
pinned stock Ninja since Wave 4, and Ronin wins seven of its eight workloads.
Make mode had never been compared with GNU Make at all, so there was no number,
no gate, and no way for a regression in either direction to be noticed.

## Provenance

- Harness schema: `ronin-make-performance-baseline-v1`
- Workload version: 1
- Ronin revision: `e73d5abaf0c41548f3f26dc8ae1946f6e51d8bac` (dirty: the harness itself)
- Oracle: `GNU Make 4.4.1`, `reference/make-oracle/make-4.4.1/make`
- Ronin: `GNU Make compatible: ronin 0.1.0`, release profile, invoked through a
  `make`-named symlink because Make mode is reached by the invoked name
- Platform: Linux `6.12.100+deb13-amd64`, x86_64, 32 cores
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- `-j8` for both tools on every workload
- One-minute load average 1.41 at the start of the gated run; the harness
  refuses to sample above 4.00
- Noise control: interleaved samples, rotating which tool goes first each
  repetition; `MAKEFLAGS`/`MFLAGS`/`MAKELEVEL` cleared; stdout and stderr
  discarded; blocking wait with no sampling thread; median wall time

## Results

Nine interleaved samples per tool per workload, except the clean build (two).

| Workload | GNU Make | Ronin | Ronin / GNU |
| --- | ---: | ---: | ---: |
| `wide-noop` — 4,000 explicit rules, one directory, no recursion | 904.664 ms | 2126.803 ms | **2.35×** |
| `recursive-noop` — 259 Makefiles, fan-out 6, depth 3 | 81.209 ms | 1119.070 ms | **13.78×** |
| `vim-noop` — vim 9.2.0957 up to date, from the top | 15.240 ms | 35.054 ms | **2.30×** |
| `zsh-incremental` — zsh 5.9.2 steady state | 1355.675 ms | 2369.092 ms | **1.75×** |
| `vim-clean-build` — vim from empty | 16063.362 ms | 16082.989 ms | **1.00×** |

## What the numbers say

**The clean build is a wash.** Sixteen seconds either way, 1.00×. On the run
that dominates a real user's day-one experience, the front end's cost is lost
in the compiler's, and nothing here needs fixing.

**The cost is per Makefile read, and it is enormous.** `recursive-noop` exists
to isolate it and does: 259 units of almost no graph at all, where GNU Make
forks 259 processes in 81 ms — 0.31 ms each — and Ronin composes the same 259
units in 1119 ms, 4.3 ms each. **Fourteen times the cost per unit, against a
tool that pays for a `fork` and an `exec` and Ronin does not.** That is the
finding, and it is what the profiling work should be aimed at.

**A no-op is where Ronin is worst, and a no-op is where a developer lives.**
GNU Make's no-op is a read and a stat walk. Ronin's is a read, a full
compilation to a graph, and then a stat walk, and it pays that on every
invocation whether or not a single byte of any Makefile changed.

**`zsh-incremental` flatters Ronin, and should be read carefully.** zsh 5.9.2
does not settle: a whole-tree `make` in an already-built tree recompiles one
object, updates `stamp-modobjs` and relinks `Src/zsh` — every run, under GNU
Make itself, and under Ronin equally. Roughly 1.3 s of that 1.36 s GNU figure
is `gcc` and `ld`, identical work for both tools. Subtracting it, the front
end's own share is about 55 ms for GNU against about 1070 ms for Ronin, which
is the same order as `recursive-noop` says and not the 1.75× the table shows.
The workload is kept because it is the shape a zsh developer actually
experiences; it is labelled `incremental` rather than `noop` because calling it
a no-op would be false.

**`wide-noop`'s GNU figure is high on its own terms** — 905 ms for a 4,000-rule
no-op is GNU Make being slow, not Ronin being fast — so 2.35× understates the
absolute gap less than it looks. The workload is still worth keeping: it is the
one shape in the set with no recursion in it at all, so it separates graph
construction from composition.

## What is gated

`scripts/check-make-performance.sh`, wired into `scripts/check-release.sh`
after `scripts/check-make-projects.sh` (whose builds are what make the two real
trees up to date). The four no-op and incremental workloads are gated; the
clean build is recorded here and measured on request with `--clean-build`,
because sixteen seconds a side is too much for every release pass.

Validation refuses a Ronin/GNU ratio more than 20% worse than the recorded one.
There is deliberately **no absolute threshold**: Ronin does not beat GNU Make on
these workloads, a gate that demanded it would fail the day it was written, and
a gate that fails the day it is written is a gate somebody switches off.
