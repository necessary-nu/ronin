---
id [dec:ronin:allocator-is-not-the-c-librarys]
epitome "The binary installs mimalloc as its global allocator, because glibc's malloc is a sixth of a Make-mode evaluation."
state @approved
category @property
scope {
    elements ([arch:ronin:make-frontend] [arch:ronin:graph-engine])
    rules ([spec:ronin:req:performance.reproducible-baseline])
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Keep the C library's allocator and remove allocations instead."
        rejected_because "Tried first and still worth doing, but the traffic has no hotspot to remove: profiling the vim `src` sub-make put 18.9% of user cycles inside glibc malloc and free with no single caller above 1%. `ronin-allocation-discipline` exists for the sites that can be removed; it cannot reach the ones that are the shape of the work."
    }
    {
        option "Use jemalloc, which the kati fork already vendors behind an off-by-default feature."
        rejected_because "Measured against mimalloc on the same tree and the same host: vim-noop 93.99M instructions against 87.92M, the vim `src` sub-make 75.40M against 70.95M. It is better than glibc on both and worse than mimalloc on both, and its per-process startup is cheaper by only a third of what it gives up."
    }
    {
        option "Install the allocator only for the Make front end, leaving the shell and Ninja mode on the C library's."
        rejected_because "There is one binary and one global allocator in it. Choosing per multicall identity means a branch on every allocation and a decision taken before `main` runs, and the branch costs a share of what the change is for."
    }
)
consequences {
    accepted (
        "The binary carries a C dependency built from source: libmimalloc-sys, whose build script is `cc` and whose closure holds no proc-macro, so the crt-static constraint in .cargo/config.toml still holds."
        "Every process the binary starts pays mimalloc's option scan, which reads the environment once per option. Measured at 216k instructions for `sh -c true` — a 49% regression on the shortest-lived thing this binary does."
        "Allocation addresses change, so anything that had come to depend on their order would change behaviour rather than fail."
    )
    deferred (
        "Reducing the per-process constant. It is mimalloc's own `_mi_options_init` walking about fifty options and reading the environment for each, and there is no compile-time switch for it in the vendored sources."
    )
}
edges {
    requires ([dec:ronin:multicall-identity])
}
codifies ([spec:ronin:req:performance.reproducible-baseline])
---

## Rationale

Ronin's Make front end retires about twice the instructions GNU Make does for
the same job, and the excess has never had a hotspot. The largest single
category in a flat profile of the vim `src` sub-make is not any function this
repository wrote: it is `_int_malloc`, `free`, `malloc_consolidate`,
`_int_free_chunk`, `unlink_chunk` and `realloc`, which together are 18.9% of
user cycles. A call-graph profile attributes them to dozens of callers with
none above 1%. There is no site to fix. The traffic is what evaluating a
Makefile is: an expression tree per line, a dependency node per target, a
`Bytes` per name, all built and dropped inside one short process.

That shape — many small allocations, freed in bulk, in a process that lives
for milliseconds — is the one glibc's allocator handles worst and a modern
thread-caching allocator handles best. `malloc_consolidate` alone, which is
glibc merging free chunks back into its bins, was 3.0% of the run.

Measured on the vim-noop workload, which is the gated one, at 20 repetitions
per arm on the same host, counting instructions rather than time because this
host is not quiet enough for a 26 ms wall to mean anything:

| build                       | instructions | against GNU Make 4.4.1 |
| ---                         | ---:         | ---:                   |
| GNU Make 4.4.1              | 48.658M      | 1.000x                 |
| Ronin, glibc                | 103.141M     | 2.120x                 |
| Ronin, jemalloc             | 93.986M      | 1.932x                 |
| Ronin, mimalloc             | 87.922M      | 1.807x                 |

The vim `src` sub-make, which is 80% of that row's gap, moves 81.211M to
70.952M — 12.6% of everything the sub-make does, for a four-line change.

## What it costs, stated plainly

`sh -c 'true'` goes from 437.7k instructions to 653.8k. The whole of that is
mimalloc's `_mi_options_init`, which walks its option table on process load and
calls `getenv` once or twice per option; each call scans the environment with a
case-insensitive compare, so the cost rises with the size of the environment
the recipe inherited. It is a per-process constant and nothing in the shell's
own work got slower.

This matters because Ronin runs as its own `/bin/sh`, once per recipe line. It
is still the right trade at every scale measured: vim-noop starts seven shells
and the row improves by 15.2M instructions net of them, and a build whose
recipes are real work is dominated by the compiler either way. But a workload
of very many trivial recipes is where this decision is worst, and whoever finds
one should measure it rather than assume this record covered it.

The alternative of choosing the allocator per multicall identity was rejected
above on the branch cost. It stays available if that workload turns up.
