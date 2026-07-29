---
id [dec:ronin:ninja-persistence-boundary]
epitome "Treat Ninja logs, depfiles, and dyndeps as byte-exact interoperability boundaries."
state @approved
category @property
scope {
    elements ([arch:ronin:persistence])
    rules ([spec:samurai:req:compat.persistent-state])
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Replace Ninja state files with a Rust-native serialization format."
        rejected_because "Cross-tool readability and existing build-directory reuse are explicit compatibility requirements."
    }
    {
        option "Deserialize whole files into allocation-heavy object graphs."
        rejected_because "Streaming or reusable-buffer processing is simpler, more bounded, and better suited to large dependency logs."
    }
)
consequences {
    accepted (
        "Ninja signatures, versions, endianness, truncation recovery, and recompact behavior remain exact."
        "Parsing is linear and uses typed graph IDs plus reusable buffers where ownership permits."
        "Depfile deduplication preserves first-seen order without quadratic scans."
    )
    deferred (
        "Memory mapping is an optional measured optimization, not a required representation."
    )
}
edges {
    requires (
        [dec:ronin:byte-exact-core]
        [dec:ronin:typed-graph-arenas]
    )
}
codifies ([spec:samurai:req:compat.persistent-state])
establishes ([arch:ronin:persistence])
---

## Rationale

`.ninja_log` and `.ninja_deps` are shared state, not private implementation
details. Ronin must consume state written by Ninja and leave state Ninja can
consume. The same applies to depfiles and dyndeps at their respective
boundaries.

Typed IDs remove whole-graph identity scans, while byte-exact parsing protects
paths and signatures. Any optimization must retain recovery from partial
writes and the established on-disk versions.
