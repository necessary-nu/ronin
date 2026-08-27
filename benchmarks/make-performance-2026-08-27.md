# Profiling Make mode against GNU Make 4.4.1 — 2026-08-27

`benchmarks/make-baseline-2026-08-27.md` is the first measurement of Ronin's
Make front end against the tool it replaces. This is what the profile said
about the gap it recorded, and what three fixes did to it.

## The finding

**The gap is not architectural.** The expectation going in was that compiling
the whole Makefile set into a graph on every invocation is what makes Ronin
slower than GNU Make, whose no-op is a read and a stat walk. That is not what
the profile says. GNU Make reads the whole Makefile set on every invocation
too, and the cost is concentrated in **one algorithm both tools have**: the
implicit (pattern and suffix) rule search.

`perf record`, release build with debug info, quiet host:

| | `recursive-noop` | `wide-noop` |
| --- | ---: | ---: |
| `kati::evaluate::evaluate` | 87.96% | 89.39% |
| `kati::dep::make_dep` | 83.61% | 88.94% |
| `DepBuilder::pick_pattern_rule` | 72.87% | 87.53% |
| `DepBuilder::implicit_chain_exists` | 63.11% | 78.55% |
| `DepBuilder::ordered_candidates` | 28.06% | 31.26% |
| `DepBuilder::exists` | 23.89% | 23.89% |
| — of which `std::fs::exists` (the `stat`) | 18.94% | 18.18% |

`recursive-noop` is the isolate that names the unit cost: GNU Make forks 259
processes and finishes in 81 ms, 0.31 ms each; Ronin composed the same 259
units in 1119 ms, 4.3 ms each. **Fourteen times the cost per unit, against a
tool that pays for a `fork` and an `exec` where Ronin pays for neither.**

## What was fixed

### 1. SipHash on interner indices — `make-mode-hashes-a-symbol-index-with-siphash`

A sixth of the run was inside SipHash and the probing around it:
`DefaultHasher::write` 5.36%, `hash_one::<&Symbol>` 4.75%,
`hash_one::<&Bytes>` 1.91%, `hash_one::<&(usize, Symbol)>` 1.29%, plus inserts
and rehashes. Every one of those keys is an interner index — a `NonZeroUsize`
— or a byte string interned from a Makefile on the machine's own disk. A keyed
cryptographic permutation with a per-process random key is the right default
for keys a stranger chooses and buys nothing here.

`kati::fasthash` is rustc's `FxHasher` behind `FastMap`/`FastSet`, applied to
the twenty tables the profile put on the hot path. Swapping a `BuildHasher`
changes iteration order, so every table was audited: all but
`DepBuilder::rules` are membership or lookup only, and that one site sorts its
keys by interned name before use.

### 2. A `stat` per invented candidate — `the-implicit-rule-search-stats-every-candidate-name`

Counted with `strace -c`, same Makefile, both tools:

| syscall | Ronin | GNU Make 4.4.1 |
| --- | ---: | ---: |
| `statx` | 480,137 (476,122 `ENOENT`) | 0 |
| `newfstatat` | 8,002 | 8,010 |
| `getdents64` | 0 | 6 |
| **total** | **488,146** | **8,028** |

Sixty times the filesystem syscalls, and the mtime stats the two share are
8,002 against 8,010 — the entire difference is the implicit-rule search asking
after names the built-in catalogue invented (`src/0.c`, `src/0.web`,
`src/SCCS/s.0.c`, a hundred more per target). GNU Make answers all of them from
a hash of what it read with six `getdents64` (`dir.c`).

`kati::dircache` is that hash, narrower than GNU Make's in one way and wider in
another. It **only ever proves absence** — a name the listing holds is still
`stat`ed, because a symlink whose target is gone is listed and does not exist —
and **a directory that is not there proves absence for everything under it**,
which is where half the syscalls were, all of them under `SCCS/` and `RCS/`.
Kept honest by `Session::filesystem_epoch`, the counter `note_command_ran`
already bumps wherever a makefile can reach the disk.

**12,039 syscalls against GNU's 8,028 — from 60x to 1.5x.**

### 3. The candidate walk's buffers — `the-candidate-walk-allocates-a-vector-per-character`

`RuleTrie::get` allocated a `Vec` per trie level, `candidate_pool` joined two
walks into a third, and the loop cloned each candidate's pattern only to
compare it with `%`. Now one buffer, de-duplicated in place, and the pattern
handed over rather than copied.

Smaller than the profile promised, and worth recording as such: `shared_clone`
fell 6.47% → 4.28% and the allocator 13.3% → 11.2%, but wall time moved only
2–6% where it moved at all. The freed work was not on the critical path.

## Results

Nine interleaved samples per tool, `-j8` both sides, quiet host.

| Workload | GNU | Ronin before | Ronin after | Change | Ratio before | Ratio after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `wide-noop` | 913 ms | 2126.803 ms | 1376.408 ms | **−35.3%** | 2.35× | **1.51×** |
| `recursive-noop` | 81 ms | 1119.070 ms | 780.529 ms | **−30.2%** | 13.78× | **9.69×** |
| `vim-noop` | 15 ms | 35.054 ms | 30.086 ms | **−14.2%** | 2.30× | **1.96×** |
| `zsh-incremental` | 1460 ms | 2369.092 ms | 2297.914 ms | — | 1.75× | **1.57×** |
| `vim-clean-build` | 14.97 s | 16.083 s | 15.105 s | — | 1.00× | **1.01×** |

`zsh-incremental`'s absolute figures are not comparable between runs: GNU Make's
own time on it moved 12% between the first and last measurement, because the
workload relinks and link time varies. The ratio is what the gate holds.

Every correctness gate is where it was, at each of the three commits and at the
end: conformance 354 runs / 326 identical / 28 differing / 276 raw; equivalence
274 makefiles, 276 graphs, 8 respellings, 56 make-rejected, **0 disagreements**;
upstream inventory 18 unclassified / **0 compiler** / 20 interface; make_port
4/4; Ninja 425/425 upstream, 10 differential, 92 build-outcome, 3
invocation-boundary, persistence round trip; vim and zsh both build from their
own Makefiles.

## What is left, with numbers

Ronin is still **9.7× GNU Make on the recursion isolate**, and that is where the
next work is. Two items, both measured rather than guessed.

**`ordered_candidates`, still 37.7% of the run inclusive.** It rebuilds and
re-sorts the candidate list for every name at every depth of the search.
Instrumenting the call shows why a memo is not a one-liner: on
`recursive-noop` it runs **80,000 times over 180 distinct names** — 440 calls
per name, so caching is worth nearly everything — while on `wide-noop` it runs
**160,000 times over 78,056 distinct names**, barely two calls per name, where
an unbounded memo would cost something like 280 MB and buy almost nothing. A
**bounded** cache of the last few dozen names fits both, and whatever it caches
must leave the `rules_in_use` filter and the `specific_rule_matched` decision
outside it: the filter runs ahead of the specificity test, and moving it would
change which rules the match-anything retain step drops.

**`DepBuilder::new` / `install_builtin_rules`, 11.5%**, paid once per composed
unit — 259 times on `recursive-noop`. GNU Make pays it per process too and does
it far more cheaply.

A rough ceiling: taking both would put `recursive-noop` somewhere near half its
current time, which is a ratio around 5× rather than 9.7×. Closing it further
than that would mean matching GNU Make's search algorithm rather than
optimising this one — and it is worth saying that the clean build, the number a
real user actually waits on, is already 1.01×.
