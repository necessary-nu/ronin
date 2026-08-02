---
id [dec:ronin:typed-graph-arenas]
epitome "Own graph entities in dense arenas and connect them with non-interchangeable typed IDs."
state @approved
category @property
scope {
    elements ([arch:ronin:graph-engine])
    rules ([spec:ronin:req:compat.graph-semantics])
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Retain Rc<RefCell<T>> and Weak<T> links from the literal port."
        rejected_because "Dynamic borrow checks, reference-count traffic, weak upgrades, and pointer-identity scans dominate graph code and obscure mutation boundaries."
    }
    {
        option "Adopt a general-purpose slot-map dependency."
        rejected_because "Graph entities live for one manifest generation and are not reused after removal, so dense Vec-backed arenas are simpler and more cache-local."
    }
)
consequences {
    accepted (
        "NodeId, EdgeId, RuleId, PoolId, and EnvironmentId are distinct newtypes over dense indices."
        "A graph or manifest context owns all arenas and performs mutation through explicit methods."
        "Adjacency stores IDs; iteration order is defined independently of allocator addresses."
    )
    deferred (
        "Generational reuse is unnecessary until a measured workload requires deletion and slot reuse within one manifest generation."
    )
}
edges {
    requires ([dec:ronin:byte-exact-core])
}
codifies ([spec:ronin:req:compat.graph-semantics])
establishes ([arch:ronin:graph-engine])
---

## Rationale

The current Rust graph mirrors C pointers with `Rc<RefCell<_>>`, `Weak`, and
pointer identity. This is safe but neither idiomatic nor cheap. Most entities
are created while parsing a manifest, remain alive for that generation, and
are discarded together on manifest reload. Dense arenas match that lifetime.

Typed IDs make ownership and mutation explicit without paying for reference
counting on every adjacency traversal. They also enable dense side tables for
dirty-state, visitation, queue membership, and cached evaluation.
