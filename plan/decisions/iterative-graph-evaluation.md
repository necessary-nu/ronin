---
id [dec:ronin:iterative-graph-evaluation]
epitome "Evaluate graph state with iterative worklists, dense side tables, and explicit invalidation."
state @approved
category @executive
scope {
    elements ([arch:ronin:graph-engine])
    rules ([spec:samurai:req:compat.graph-semantics])
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Preserve recursive per-target scans and clone adjacency before traversal."
        rejected_because "The baseline shows a 16.47x Ninja regression on a 2,000-edge chain and the approach repeats work and allocations."
    }
    {
        option "Memoize the current Rc-based recursion without changing ownership."
        rejected_because "It reduces some repeated work but preserves borrow complexity, reference churn, and stack depth."
    }
)
consequences {
    accepted (
        "Dirty, ready, visiting, and completion state live in ID-indexed vectors with generation markers."
        "Traversal uses explicit stacks or queues and reports cycles from stored predecessor state."
        "Ready-edge selection uses a real priority heap with a deterministic tie key."
        "Command completion, restat, depfile changes, and dyndep mutations have explicit cache invalidation paths."
    )
    deferred (
        "Parallel graph evaluation is deferred until single-threaded evaluation is correct, measured, and no longer dominant."
    )
}
edges {
    requires ([dec:ronin:typed-graph-arenas])
}
codifies ([spec:samurai:req:compat.graph-semantics])
establishes ()
---

## Rationale

The first recorded baseline makes graph evaluation the dominant performance
problem: Ronin takes 287.159 ms where Ninja takes 17.432 ms and the C reference
takes 5.055 ms. The literal port recursively revisits graph structure and
clones references in hot paths.

Explicit worklists and dense state turn traversal into predictable linear work,
avoid call-stack growth, and provide one place to reason about invalidation.
The semantic contract remains Ninja's dirty/rebuild behavior; this is a
representation and algorithm change, not permission to change results.
