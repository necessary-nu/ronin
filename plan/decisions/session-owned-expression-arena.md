---
id [dec:ronin:session-owned-expression-arena]
epitome "A session's expression nodes live in one arena addressed by dense indices; the arena is worth 7.9% of the gated no-op at its ceiling, and unlike an allocator swap it is cheaper in the kernel too."
state @approved
category @property
scope {
    elements ([arch:ronin:make-frontend])
    rules (
        [spec:ronin:req:performance.allocation-accounting]
        [spec:ronin:req:performance.reproducible-baseline]
    )
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Keep one reference-counted allocation per expression node."
        rejected_because "It is 30.3% of everything a Make read allocates, attributed by DHAT to `parse_expr_impl_ext` alone: 14,413 `Arc<Value>` and 5,921 `Vec<Arc<Value>>` for one read of vim's `src` Makefile. None of it has an individual lifetime — every node is built out of makefile text, held by the variable, rule or statement that text describes, and dropped when the compilation is over."
    }
    {
        option "Bump-allocate the nodes and reach them through raw pointers, the way `bumpalo` or a hand-rolled typed arena would."
        rejected_because "A `Session` is MOVED — into a worker thread that composes a recursive unit, and back out of it — so an interior pointer would have to be proved to survive the move, and the proof would be a comment rather than a type. It is also the first non-FFI `unsafe` this repository would carry, in the component the product's correctness rests on: every one of kati's eleven unsafe blocks is a libc call. A dense index survives the move for free and the compiler checks the rest."
    }
    {
        option "Give `Value` a lifetime and borrow the arena, which is the safe pointer form."
        rejected_because "`Session<'a>` cannot own the arena it borrows from, and it is a Session that moves between threads. The pair would have to be split, which puts the arena's ownership somewhere no worker can hold it."
    }
    {
        option "Take the arena as far as it goes: bump every allocation in the process and free none of them."
        rejected_because "Not a shipping candidate — it is unbounded — but it is the measurement that licenses the rest of this decision, and it is recorded here as the CEILING rather than as an option. See below."
    }
    {
        option "Answer the single-fragment case with an inline slot and leave the rest alone."
        rejected_because "Not rejected so much as subsumed: 4,980 of the 6,014 expressions read from vim's `src` Makefile hold exactly one fragment, and the arena's shared fragment stack answers all of them without a second mechanism."
    }
)
consequences {
    accepted (
        "`ValueId` is a four-byte index into a session-owned `ValueArena`; a `Children` range names a list's items or a call's arguments in one shared child table."
        "Expansion never holds a `&Value` across a `&mut Evaluator`: each arm takes the handles, symbols and locations it needs out of the node and lets the borrow go before it recurses. The compiler enforces it; nothing is unsafe."
        "A read accumulates fragments on one stack for the whole session, marking the height on the way in and taking everything above the mark on the way out, and unwinds to the mark when it raises."
        "`Bytes` remains the boundary for anything that outlives an expression: literal text, interned names, recorded commands. The arena holds STRUCTURE, not text — a `Value::Literal` still carries a `Bytes` slice of the makefile buffer, which is already a borrow of something the session owns."
        "`Loc` is `Copy`."
    )
    deferred (
        "The evaluation-time temporaries — the `BytesMut` a `$(...)` expansion writes into, the vectors the dependency builder grows — are a second 28% of the same census and are not in this arena. They have the same lifetime and would fit the same argument."
    )
}
edges {
    requires ([dec:ronin:typed-graph-arenas])
    refines ([dec:ronin:session-owned-evaluation])
}
codifies ([spec:ronin:req:performance.allocation-accounting])
affects ([arch:ronin:make-frontend])
---

## Rationale

`[dec:ronin:allocator-is-not-the-c-librarys]` ends by naming the direction this
decision takes: "Keep the C library's allocator and remove allocations
instead… It is the only direction that removes kernel time as well as user
time: an allocation not made is an arena page not committed." That was an
argument, not a measurement. This is the measurement.

## The ceiling was measured before anything was built

The question an arena has to answer first is what an arena is worth at all,
and it can be answered without writing one: build a Ronin whose global
allocator hands out bytes from a reserved region and never reclaims any of
them. Every malloc is a pointer bump, every free is nothing. Whatever that arm
saves is the most any arena inside the front end could save, because it has
already arena'd everything the process allocates.

Paired and interleaved against the stock binary, medians of 21 to 41
alternations per arm:

| workload        | base      | bump-everything | ratio  |
| ---             | ---:      | ---:            | ---:   |
| vim-noop        | 27.81 ms  | 25.62 ms        | 0.921x |
| recursive-noop  | 87.10 ms  | 84.46 ms        | 0.970x |
| wide-noop       | 619.63 ms | 576.25 ms       | 0.930x |

And split by privilege level, interleaved, 21 alternations on vim-noop:

| counter        | base    | bump-everything | ratio   |
| ---            | ---:    | ---:            | ---:    |
| instructions:u | 103.38M | 79.11M          | 0.765x  |
| cycles:u       | 75.65M  | 61.73M          | 0.816x  |
| cycles:k       | 44.04M  | 46.41M          | 1.054x  |
| page-faults    | 2,919   | 1,248           | 0.428x  |
| task-clock     | 35.15ms | 33.10ms         | 0.942x  |

That base instruction count reproduces the 103.390M this repository recorded
for the tree, so the arm is measuring the tree it claims to.

**This is the opposite shape to the allocator swap, and that is the finding.**
mimalloc bought 15.2M user instructions and spent 22.0M kernel cycles doing it.
The arena buys 24.3M user instructions and spends 2.4M kernel cycles — and
takes 57% of the page faults away rather than adding them, because a region
bumped through once is touched once, where glibc returns pages and re-faults
them. Total cycles fall 9.7%. The wall agrees with the cycles, which is what
was missing last time.

So: **7.9% of the gated vim no-op is the whole prize for arena work in this
front end.** Every stage spends against that budget, and no stage should be
justified without saying what fraction of it the stage is claiming.

### One thing the probe taught that the design had to absorb

The first version of the bump allocator used a single global bump pointer and
made `recursive-noop` **4.8x slower** — 409 ms against 85 ms. Eight composing
threads on one atomic is a cache line they take turns owning. A thread-local
slab, claimed 4 MB at a time, brought it to 0.970x. A per-session arena has
that property by construction, which is the second reason the arena belongs to
the session rather than to the process.

## Where the allocations are

DHAT over one vim `src` sub-make (74,912 blocks; a dynamically linked build,
since the static one it ships as cannot be profiled this way), attributed to
the innermost frame that is this project's own:

| site                                 | blocks |  share |
| ---                                  | ---:   |   ---: |
| `expr::parse_expr_impl_ext`          | 22,702 |  30.3% |
| `Evaluator::eval_rule`               |  8,087 |  10.8% |
| `DepBuilder::resolved_prerequisites` |  5,368 |   7.2% |
| `rule::glob_word`                    |  3,747 |   5.0% |
| `DirectoryCache::certainly_absent`   |  3,361 |   4.5% |
| `DepBuilder::ordered_candidates`     |  2,492 |   3.3% |
| `DepBuilder::new`                    |  2,291 |   3.1% |
| `rule::file_sequence`                |  1,858 |   2.5% |
| `Value::resolve_folds`               |  1,374 |   1.8% |

There is no hotspot in the profile and there is one in the census. The
expression read is a third of the traffic on its own, and it is a third whose
lifetime is a single session's.

## What the arena is, and why indices

`Value` moves into a `ValueArena` the session owns. A child link is a
`ValueId`: four bytes, `Copy`, no reference count. A list's items and a call's
arguments are a `Children` range into one shared child table rather than a
vector each.

Indices rather than interior pointers, and the reason is specific to this side
rather than inherited from `[dec:ronin:typed-graph-arenas]`: a `Session` is
moved. `src/make/parallel.rs` moves one into a worker that composes a recursive
unit and takes it back out; the reaper is handed one to free. A dense index is
still valid after that move. A pointer into the moved value is a question
nobody should have to ask.

The second reason is that the safe pointer form is not available. `Session<'a>`
holding `&'a ValueArena` cannot own the arena it borrows, and it is a `Session`
that crosses the thread boundary, so the two would have to be separated and the
worker would have nowhere to put the half it does not own.

### The borrow rule, and why the compiler holds it

A `&Value` read out of the arena borrows the evaluator that owns the arena, and
expansion is the evaluator's most mutable operation. So `eval_value` reads the
node once, copies out what it needs — handles, symbols, a location — and lets
the borrow go before it does anything else. The two arms that only write bytes,
which between them are most of every makefile, finish inside the borrow and
never leave it.

This is what makes the index form worth its verbosity. There is no invariant
here for a reader to remember: the thing that would go wrong is a borrow error,
not a use-after-free that shows up as a wrong graph six months later.

### The fragment stack

A read accumulates the fragments of the expression it is on. That was a `Vec`
per expression; it is now one stack for the whole session, with the height
marked on the way in and everything above the mark taken on the way out. Reads
nest strictly — a `$(` inside a list is read to its close before the list goes
on — so a stack is the right shape.

It also answers the case that dominates. **4,980 of the 6,014 expressions read
from vim's `src` Makefile hold exactly one fragment**: a line with no `$` in
it, which in a real makefile is most lines. Each of those used to allocate a
four-slot vector to hold the one value it then popped back out.

## The `Bytes` boundary

The arena holds structure, not text. A `Value::Literal` still carries a
`Bytes`, and that is correct rather than unfinished: it is a slice of the
makefile buffer the session already owns, so it is a borrow wearing a refcount
rather than a copy. What crosses out of the session — an interned symbol
reaching the graph, a recorded command — crosses as `Bytes` exactly as before,
and no `ValueId` is ever handed to anything outside the session that minted it.

The two sibling leads are on the other side of that boundary and stay their own
nodes. `file-sequence-clones-a-word-the-interner-only-borrows` and
`symtab-name-hands-out-an-owner-where-a-borrow-would-do` are both about a
`Bytes` handed out where a borrow would do; neither becomes easier or harder
because expression structure moved, and together they are another 7.5% of the
census.

## What it measured

One vim `src` read, counted in process with a counting global allocator:

| | before | after |
| --- | ---: | ---: |
| allocation requests | 67,198 | 46,258 |
| the 88-byte class (`Arc<Value>`) | 14,510 | 91 |
| the 32-byte class (parse vectors and their kin) | 12,467 | 6,253 |
| requested bytes | 7,434,953 | 8,262,917 |

20,940 fewer allocations, 31.2% of the total, against a predicted 30.3%. Bytes
go UP 11%, which is the arena reserving capacity ahead of use and rounding up
as it doubles; it is arithmetic on a Vec, not retained memory, and peak RSS is
reported with the wall.

## What it cost on the wall

Paired and interleaved against the same GNU Make the gate uses, medians of 101
alternations on vim-noop and 21 to 61 elsewhere, on a host whose one-minute
load was 6.5 to 19 throughout — so the ratios are what carry, not the absolute
milliseconds:

| row             | base/GNU | arena/GNU | arena/base |
| ---             | ---:     | ---:      | ---:       |
| vim-noop        | 1.773x   | 1.702x    | 0.960x     |
| wide-noop       | 0.579x   | 0.565x    | 0.976x     |
| zsh-incremental | 1.074x   | 1.061x    | 0.988x     |
| recursive-noop  | 0.997x   | 0.998x    | 1.001x     |

Split by privilege level on vim-noop, interleaved, 31 alternations:
`instructions:u` 0.921x, `cycles:u` 0.938x, `cycles:k` 1.018x, page faults
1.002x, task-clock 0.968x. The kernel side does not move, which is the whole
difference from the allocator arms: this change removes user work without
buying it with kernel work.

Peak RSS over the whole process tree, worst of seven runs: vim-noop 21,620 →
20,224 KiB, recursive-noop 62,924 → 62,200, wide-noop 39,176 → 39,880. The
arena reserves capacity, and gets it back from the sixteen-byte `Arc` header
and the allocator metadata it stopped paying per node.

**4.0% of vim-noop against a 7.9% ceiling: this stage spends half the budget.**

## The recursion is not the second beneficiary, and the Reaper was the wrong tell

The expectation going in was that `-j8` recursion would gain most: 259 sessions
composed on worker threads and freed by a reaper thread, cross-thread frees
being glibc's worst case. It gains nothing — `recursive-noop` reads 1.001x. The
Reaper's own CPU on that row does halve, 20 ms to 10 ms, and the row is 87 ms
long, so halving it is worth 11% of a number that was 23% of the run — except
the run did not move, because that time was already off the composing thread,
which is what the Reaper is for.

The mechanism is simpler than the thread story. The recursive workload's 259
units are **4,103 makefile lines between them, fewer than vim's single
`src/Makefile` at 4,962**. Session count is not expression volume. The rows
that moved are the ones that read a lot of makefile — one big unit, single
threaded, with no reaper running at all.

## Reproducing the ceiling without re-deriving it

The bump-everything arm is not carried in the tree, for the reason
`[dec:ronin:allocator-is-not-the-c-librarys]` gives about the allocator arms.
To rebuild it: a `src/bin/` binary that `include!`s `../main.rs`, a
`#[global_allocator]` bumping from a static `[u8; 1 << 30]` region with
`dealloc` a no-op for pointers inside it and a fall-through to `System` for
everything else. **Give each thread its own slab** — claim 4 MiB at a time from
one atomic — or the recursive workload measures 4.8x slower than the base and
the whole reading is wrong.

The census arm is a second binary of the same shape whose allocator counts
requests and size classes and reports from a `.fini_array` constructor, so a
run that ends in `exit()` still says what it allocated. The per-site
attribution is DHAT over a `RUSTFLAGS="-C debuginfo=1"` build without
`crt-static`: DHAT cannot profile the static binary that ships, but a dynamic
one makes the same 74,912 blocks and attributes every one of them.

## What this does not claim

It does not claim the ceiling. The remaining 68.8% of the census is where the
other 3.9% lives, and the largest single piece of it — `Evaluator::eval_rule`
at 10.8%, which is the `BytesMut` every expansion writes into — has the same
lifetime and would fit the same argument.
