---
id [dec:ronin:ninja-compatibility-oracle]
epitome "Use the pinned upstream Ninja implementation and classified full suite as Ronin's compatibility oracle."
state @approved
category @executive
scope {
    elements ([arch:ronin:verification])
    rules (
        [spec:samurai:req:compat.upstream-conformance]
        [spec:samurai:req:performance.reproducible-baseline]
        [spec:samurai:req:performance.no-unexplained-regression]
        [spec:samurai:req:release.compatibility-gate]
    )
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Rely only on the translated C samurai tests."
        rejected_because "They preserve the source port but cannot prove current Ninja compatibility or expose known samurai divergences."
    }
    {
        option "Track Ninja main without a revision pin."
        rejected_because "Moving tests and semantics make failures irreproducible and performance comparisons ambiguous."
    }
)
consequences {
    accepted (
        "The initial oracle is Ninja 1.14.0.git at b51a1e37c2fb89bbefa600bd155e1ce13983f09d."
        "Every upstream test is passed, documented as inapplicable, or tracked as a compatibility failure."
        "Performance records carry both revisions and use versioned workloads."
    )
    deferred (
        "Advancing the Ninja pin is a separate reviewed compatibility change."
    )
}
edges {
    requires (
        [dec:ronin:product-boundary]
        [dec:ronin:byte-exact-core]
        [dec:ronin:typed-graph-arenas]
        [dec:ronin:iterative-graph-evaluation]
        [dec:ronin:byte-span-parser]
        [dec:ronin:ninja-persistence-boundary]
        [dec:ronin:completion-driven-execution]
    )
}
codifies (
    [spec:samurai:req:compat.upstream-conformance]
    [spec:samurai:req:performance.reproducible-baseline]
    [spec:samurai:req:performance.no-unexplained-regression]
    [spec:samurai:req:release.compatibility-gate]
)
establishes ([arch:ronin:verification])
---

## Rationale

The translated C semantics and tests remain the Wave-4 safety net, but the
product target is Ninja compatibility rather than permanent samurai behavior.
The upstream implementation supplies the only comprehensive, evolving oracle
for language, graph, CLI, tool, persistence, process, and output behavior.

Pinning the revision makes test classification and benchmark results
repeatable. A full inventory prevents a small hand-picked subset from being
mistaken for compatibility. Performance claims remain subordinate to semantic
equivalence: doing less work is not an optimization if Ninja requires that
work.
