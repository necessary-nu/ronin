---
id [dec:ronin:allocator-is-not-the-c-librarys]
epitome "Replacing the C library's allocator buys instructions and pays for them in peak RSS the Ninja gate refuses."
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
        rejected_because "Measured, landed, and refuted by scripts/check-performance.sh in the same session. Peak RSS on the Ninja clean-tree no-op went 4,596 KiB to 23,640 KiB — 402% of Ninja's, against a gate ceiling of 200% — and the wall on that row went 0.675x Ninja to 0.84x against a recorded 0.65x. It buys 14.6% of the Make front end's instructions and charges the whole binary five times its resident footprint."
    }
    {
        option "Install jemalloc, which the kati fork already vendors."
        rejected_because "Same shape, less of it, and still over the line: 11,996 KiB peak RSS on the same row, 212% of Ninja's. Tuned to a build tool's lifetime with `narenas:2,dirty_decay_ms:0,muzzy_decay_ms:0` it falls to 8,180 KiB on that row but sits at 11,868 KiB — 200.3% — on the wide no-op. No margin, and its Ninja-mode wall is at parity with the C library's rather than ahead of it."
    }
    {
        option "Keep the C library's allocator and remove allocations instead."
        rejected_because "Not rejected — this is what stands. It is slower than swapping the allocator and it has no hotspot to aim at, but it costs nothing in footprint. `ronin-allocation-discipline` and `examples/alloc_metrics` are where it lives."
    }
)
consequences {
    accepted (
        "The C library's allocator remains, and 18.9% of a Make-mode evaluation's user cycles remain inside it."
        "The measurement stands as the argument for a representation change rather than an allocator swap: the traffic is one Arc per expression fragment, one Bytes handle per name, one node per target."
    )
    deferred (
        "Reopening this with a footprint budget the operator has set deliberately. Ronin currently uses LESS peak RSS than Ninja on every recorded workload, and .cargo/config.toml names that as a virtue the crt-static trade was made to keep; whether 8 MiB against Ninja's 5.7 MiB is worth 15% of the front end's instructions is a product judgement, not a measurement."
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

That is the shape glibc's allocator handles worst, and the measurement said so.
On the gated vim no-op, `perf stat -e instructions:u -r 20`, against GNU Make
4.4.1 at 48.658M instructions:

| build            | instructions | against GNU Make |
| ---              | ---:         | ---:             |
| glibc            | 103.205M     | 2.120x           |
| jemalloc         | 93.986M      | 1.932x           |
| mimalloc         | 87.938M      | 1.806x           |

## Why it is rejected anyway

`scripts/check-performance.sh` — the Ninja-mode gate, which this campaign had
not been running because the campaign is about Make mode — refuses both. It
measures each coordinator's peak RSS from `/proc` and rejects above 200% of
Ninja's, and it rejects a Ronin/Ninja wall ratio above 120% of the recorded
one. On `clean-tree-noop`:

| build              | wall     | Ronin/Ninja | peak RSS   | of Ninja |
| ---                | ---:     | ---:        | ---:       | ---:     |
| glibc (recorded)   | 8.747 ms | 0.675x      | 4,596 KiB  | 81%      |
| jemalloc, tuned    | 9.536 ms | 0.75x       | 8,180 KiB  | 145%     |
| jemalloc, default  | 8.954 ms | 0.70x       | 11,996 KiB | 212%     |
| mimalloc           | 10.538 ms| 0.84x       | 23,640 KiB | 402%     |

Two independent refusals, and the wall one is a consequence of the RSS one:
these workloads run for eight milliseconds, so the pages an allocator commits
before it has done any work are a measurable fraction of the whole run.

The gate is right and the change was wrong. Ronin uses less peak RSS than Ninja
on every recorded Ninja workload today, and `.cargo/config.toml` names that as
something the static-link trade was made to keep — "Peak RSS goes *down*, and
for a developer tool run thousands of times a day the trade is the right way
round." An allocator that reverses it is not a free 15%; it is a footprint
decision charged to every mode of the binary, including the shell it runs as
once per recipe line.

## What the measurement is still worth

It is the argument for the representation change rather than against it. After
the two cuts that landed beside this one, `bytes` reference counting —
`shared_drop` 4.42% and `shared_clone` 3.13% — is the largest category in the
profile that a change to this repository can reach. Removing the allocations
removes the allocator's share of them too, and costs nothing in footprint.
