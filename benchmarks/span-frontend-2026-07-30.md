# Borrowed span frontend measurement — 2026-07-30

This measurement compares the pre-change semantic-error commit
`c608f06ab9d39893cf1b3f8afa1c931ca716c5ec` with the
`ronin-span-frontend` candidate on the deterministic
`manifest-command-evaluation` workload: one rule, 4,000 build edges, 4,000
edge-local bindings, one phony aggregate, and `-t commands all`.

## Allocation A/B

Both release binaries ran the same generated manifest with the system
jemalloc 5.3.0 preloaded. `MALLOC_CONF=stats_print:true,tcache:false` made
jemalloc report every allocation request directly. The request count is the
sum of `nrequests` across small and large size classes; size-class bytes are
the sum of `class_size * nrequests`. They are allocator-facing bytes rather
than exact requested sizes.

| Metric | Before | Borrowed frontend | Change |
| --- | ---: | ---: | ---: |
| Allocation requests | 260,547 | 192,838 | −26.0% |
| Allocator size-class bytes | 21,585,984 | 18,809,224 | −12.9% |

The A/B command shape was:

```sh
env LD_PRELOAD=/usr/lib/x86_64-linux-gnu/libjemalloc.so.2 \
  MALLOC_CONF=stats_print:true,tcache:false \
  ./ronin -t commands all
```

## Runtime and peak RSS

The repository's version-1 baseline harness ran both revisions with two
warmups and 15 interleaved release samples against pinned Ninja
`b51a1e37c2fb89bbefa600bd155e1ce13983f09d`. Both validation runs passed.

| Variant | Median | Range | Median peak RSS |
| --- | ---: | ---: | ---: |
| Ronin before | 17.565 ms | 16.080–19.651 ms | 10,516 KiB |
| Ronin borrowed frontend | 15.364 ms | 14.471–18.611 ms | 10,728 KiB |
| Pinned Ninja, candidate run | 40.702 ms | 39.046–49.379 ms | 7,924 KiB |

The borrowed frontend's raw median was 12.5% lower. Normalized to Ninja in
each run, the Ronin/Ninja ratio moved from 0.419× to 0.377×, a 9.8%
improvement. Median peak RSS increased by 212 KiB (2.0%), which is consistent
with deliberately retaining this workload's 227,649-byte source after parsing.
Ronin remained within the release limits of 1.2× Ninja runtime and 2.0× Ninja
RSS. The allocation A/B above isolates the representation change directly.
