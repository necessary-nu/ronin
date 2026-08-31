---
id [dec:ronin:allocator-is-not-the-c-librarys]
epitome "Replacing the C library's allocator buys user instructions and spends more than it buys in kernel time; the 14.6% was never wall, and the footprint was never the binding refusal."
state @rejected
category @property
scope {
    elements ([arch:ronin:cli])
    rules ([spec:ronin:req:performance.reproducible-baseline])
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Install mimalloc as the global allocator."
        rejected_because "Measured three times now — landed and reverted 2026-08-31, then re-measured paired and interleaved on 2026-08-31 after the operator granted the footprint budget. It removes 14.7% of the Make front end's USER instructions and adds 50% to its KERNEL cycles, and the kernel side is larger. On the gated vim no-op, paired and interleaved over 31 alternations, it costs +15.7% wall untuned and +10.2% tuned. The row it was supposed to improve moves from 1.712x GNU Make to 1.917x. In Ninja mode it regresses four of the five recorded workloads on both targets. Peak RSS is 334-417% of Ninja's untuned and 146-275% tuned."
    }
    {
        option "Install jemalloc, which the kati fork already vendors."
        rejected_because "Same shape, more of it. Paired on the gated vim no-op: +20.3% wall default, +28.3% tuned with `narenas:2,dirty_decay_ms:0,muzzy_decay_ms:0`. The tuning that helps its footprint hurts its wall, so there is no setting at which it is merely expensive rather than slower."
    }
    {
        option "Tune mimalloc rather than take it as it comes."
        rejected_because "Tried, and it is worth doing before anyone concludes otherwise: `purge_delay=-1,arena_eager_commit=0` is the pair that matters, taking peak RSS from 21,136 KiB to 14,532 KiB and wall from +15.7% to +10.2% on the gated vim no-op. `arena_reserve` moves RSS a little more; `eager_commit`, `purge_decommits` and `reserve_huge_os_pages` move nothing here. Tuned is better than untuned and still slower than the C library's, in both modes, on both targets."
    }
    {
        option "Ship mimalloc only for musl, since musl-static is what NOS ships."
        rejected_because "The one arm with a real case, and it still does not survive Ninja mode. musl's own allocator is genuinely bad — musl-static is 29.5% slower than glibc-static on the gated vim no-op with nothing else changed — and tuned mimalloc recovers most of that, 36.25 ms to 30.36 ms, a 16.2% Make-mode win on the configuration that actually ships. But in Ninja mode the same binary regresses clean-tree-noop, wide-noop and path-canonicalization, and costs 259% of Ninja's peak RSS on the first of them. A target-conditional allocator would also put the shipped configuration on a code path no gate in this repository measures, which is the exact condition that produced this decision's first, wrong version."
    }
    {
        option "Keep the C library's allocator and remove allocations instead."
        rejected_because "Not rejected — this is what stands, and it now stands on stronger evidence than before. It is the only direction that removes kernel time as well as user time: an allocation not made is an arena page not committed. `ronin-allocation-discipline`, `examples/alloc_metrics`, and the three leads filed as children of `the-allocator-swap-was-refused-by-the-ninja-gate`."
    }
)
consequences {
    accepted (
        "The C library's allocator remains, and 18.9% of a Make-mode evaluation's user cycles remain inside it — but that 18.9% is not 18.9% of the wall, and this decision is the record of the difference."
        "`instructions:u` is not a result on any workload short enough for allocator setup to matter. A Make no-op spends a third of its cycles in the kernel and the counter cannot see them. Paired, interleaved wall is the criterion; an instruction count is a hypothesis."
        "The measurement stands as the argument for a representation change rather than an allocator swap: the traffic is one Arc per expression fragment, one Bytes handle per name, one node per target."
    )
    resolved (
        "The footprint budget this decision deferred to the operator has now been set, and it did not admit the change. Brendan, 2026-08-31, verbatim: \"wat so unfuck it.\" That is consent to 2-4x peak RSS on a tool that peaks under 40 MB, and it was asked for on the stated premise that 14.6% of vim-noop wall came back in exchange. The premise was wrong: the 14.6% was an `instructions:u` reduction, and the paired wall moves the other way by about the same amount. With the budget granted and spent, mimalloc is still a 10-16% wall regression on the row it was bought for. Nothing was relaxed, because relaxing the RSS ceiling would have admitted a change that is slower on the axis the ruling exists to protect."
    )
}
edges {
    requires ([dec:ronin:multicall-identity])
}
codifies ([spec:ronin:req:performance.reproducible-baseline])
---

## Rationale

Ronin's Make front end retires about twice the instructions GNU Make does for
the same job, and the excess has no hotspot. The largest single category in a
flat profile of the vim `src` sub-make is not any function this repository
wrote: `_int_malloc`, `free`, `malloc_consolidate`, `_int_free_chunk`,
`unlink_chunk` and `realloc` together are 18.9% of user cycles, and a
call-graph profile spreads them across dozens of callers with none above 1%.
There is no site to fix. The traffic is what evaluating a Makefile is: an
expression tree per line, a dependency node per target, a `Bytes` per name, all
built and dropped inside a process that lives for milliseconds.

That is the shape glibc's allocator handles worst, and on the counter that was
used, the measurement said so. On the gated vim no-op, `perf stat -e
instructions:u -r 20`, against GNU Make 4.4.1 at 48.622M instructions:

| build            | instructions | against GNU Make |
| ---              | ---:         | ---:             |
| glibc            | 103.390M     | 2.126x           |
| jemalloc         | 94.196M      | 1.937x           |
| jemalloc, tuned  | 95.285M      | 1.960x           |
| mimalloc         | 88.169M      | 1.813x           |

Those figures reproduce the ones this decision was first written from to within
0.3%, on a different day at a different host load. They are not in question.
What is in question is what they mean.

## The counter was the wrong one

`instructions:u` counts retired user-space instructions. It cannot see a page
fault, an `mmap`, an `madvise` or a `munmap`, and acquiring and releasing
arenas is most of what a general-purpose allocator does that the C library's
does not. Splitting the same run by privilege level, `-r 20`:

| build            | ins:u    | cycles:u | cycles:k | total    | task-clock |
| ---              | ---:     | ---:     | ---:     | ---:     | ---:       |
| glibc            | 103.390M | 71.737M  | 43.866M  | 115.603M | 34.87 ms   |
| mimalloc         | 88.169M  | 68.547M  | 65.823M  | 134.370M | 40.48 ms   |
| jemalloc, tuned  | 95.284M  | 74.015M  | 83.497M  | 157.512M | 45.51 ms   |

mimalloc saves 15.2M user instructions and spends 22.0M kernel cycles to do
it. The 14.7% instruction reduction is real and is 4.4% of user cycles, because
mimalloc's IPC on this workload is worse than glibc's. Then the kernel side
takes that back and more.

## What the wall actually does

Paired and interleaved, 31 alternations per arm, `make -j8` on the gated vim
no-op tree, median of wall and of user+system CPU:

| arm                     | cpu_ms | wall_ms | peak RSS  | wall vs base |
| ---                     | ---:   | ---:    | ---:      | ---:         |
| glibc-static (base)     | 31.13  | 27.98   | 8,112 KiB | 1.000x       |
| glibc + mimalloc        | 37.18  | 32.38   | 21,136    | 1.157x       |
| glibc + mimalloc, tuned | 35.08  | 30.83   | 14,532    | 1.102x       |
| musl-static             | 38.18  | 36.25   | 8,112     | 1.295x       |
| musl + mimalloc         | 36.04  | 32.12   | 20,120    | 1.148x       |
| musl + mimalloc, tuned  | 33.30  | 30.36   | 15,428    | 1.085x       |

Against the oracle, paired the same way: GNU Make 16.36 ms, Ronin 28.01 ms —
**1.712x**, against a recorded row of 1.71x. The tree measures exactly where it
is recorded to be, which is also what licenses every other figure here. Tuned
mimalloc puts the same row at 31.36 ms, **1.917x**. The change does not take
vim-noop down; it puts it up by twelve percent.

`zsh-incremental`, 11 alternations: GNU 1349.02 ms, Ronin 1419.09 ms (1.052x,
recorded 1.03x), tuned mimalloc 1440.61 ms (1.068x).

## The Ninja mode reading was the allocator, not the host

The first version of this decision recorded a `clean-tree-noop` ratio of 0.84x
against a recorded 0.65x and could not say whether the host or the allocator
caused it, because that session's gates flapped under load. Settled, by running
the Ninja gate against four binaries in one window on one host:

| workload            | glibc base | glibc mimalloc | musl base | musl mimalloc |
| ---                 | ---:       | ---:           | ---:      | ---:          |
| clean-tree-noop     | 0.697x     | 0.794x         | 0.807x    | 0.820x        |
| wide-noop-build     | 0.936x     | 1.028x         | 0.972x    | 1.077x        |
| manifest-command-ev | 0.506x     | 0.760x         | 0.819x    | 0.765x        |
| path-canonicalizatn | 0.667x     | 0.936x         | 0.816x    | 0.957x        |
| large-manifest-parse| 0.478x     | 0.453x         | 0.767x    | 0.554x        |

The base binary passes the gate at 0.649x on `clean-tree-noop` in a validating
run in the same session. The host reproduces the recorded ratio; the allocator
does not. It was the allocator.

The shape is legible in the last row. `large-manifest-parse` runs for 145 ms
and is the one workload mimalloc improves — on both targets, and substantially
on musl. Every workload it regresses runs for four to eleven milliseconds. A
better allocator wins once it has amortised the arenas it reserved, and these
workloads end first. Ronin is a tool whose common case is a few milliseconds,
so the amortising case is the rare one.

## Peak RSS, for the record the ruling was granted against

Ninja mode, KiB, against stock Ninja on the same row, ceiling 200%:

| workload             | ninja | glibc base | mimalloc      | mimalloc, tuned |
| ---                  | ---:  | ---:       | ---:          | ---:            |
| clean-tree-noop      | 5,672 | 5,472 (96%)| 23,676 (417%) | 15,612 (275%)   |
| wide-noop-build      | 5,764 | 5,612 (97%)| 21,484 (373%) | 12,912 (224%)   |
| manifest-command-ev  | 7,664 | 5,488 (72%)| 27,520 (358%) | 19,544 (247%)   |
| path-canonicalization| 5,768 | 3,900 (68%)| 19,452 (353%) |  8,896 (162%)   |

Tuning brings one row inside the ceiling and leaves three outside it. The
operator's budget would have covered all four. It was not needed, and
`MAX_NINJA_RSS_RATIO` in `examples/baseline.rs` — which is where the threshold
lives, not in `scripts/check-performance.sh` — is left at 2.00.

## How to reproduce this without re-deriving it

The arms are not carried in the manifest, deliberately: `.cargo/config.toml`
forbids a proc-macro anywhere in the closure and the standing advice there is
to decline a dependency rather than open the hatch. Both allocator crates are
proc-macro-free and both build clean for `x86_64-unknown-linux-musl`, which was
verified rather than assumed — that is the only part of the experiment worth
not repeating.

To rebuild an arm: add `mimalloc = { version = "0.1.52", optional = true,
features = ["extended"] }` or `tikv-jemallocator = { version = "0.6", optional
= true }` to `[dependencies]`, a feature to select it, and a
`#[global_allocator]` static in `src/main.rs` — the binary, not the library, so
an embedder using Ronin as a Ninja library keeps its own. jemalloc's
configuration symbol is `_rjem_malloc_conf` and not `malloc_conf`; the
unprefixed name links and is never read, and the footprint does not move.
mimalloc's options are settable from the environment as `MIMALLOC_PURGE_DELAY`,
`MIMALLOC_ARENA_EAGER_COMMIT` and the rest, which is how the tuned arms above
were measured without rebuilding.

Measure it paired and interleaved against wall, alternating arms within a
session rather than running each to completion in turn — that is what makes a
loaded host usable, and this host carried a load of 5 to 7 throughout without
disturbing a single ratio reported here.

## What the measurement is still worth

It is the argument for the representation change, and a stronger one than
before. The allocator's 18.9% of user cycles is reachable, but not by an
allocator: every arm that removes the user-space cost adds more kernel cost
than it removes. An allocation that is never made has neither. `bytes`
reference counting — `shared_drop` 4.42% and `shared_clone` 3.13% — is the
largest category in the profile that a change to this repository can reach, and
the three leads filed under `the-allocator-swap-was-refused-by-the-ninja-gate`
are where it goes next.
