---
id [dec:ronin:ninja-persistence-boundary]
epitome "Treat Ninja logs, depfiles, and dyndeps as byte-exact interoperability boundaries."
state @approved
category @property
scope {
    elements ([arch:ronin:persistence])
    rules ([spec:ronin:req:compat.persistent-state])
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
        "The boundary is what the files contain, not where they sit: a front end whose own contract forbids leaving state in the tree relocates the same byte-exact files rather than changing or abandoning them."
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
codifies ([spec:ronin:req:compat.persistent-state])
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

Location is not part of the boundary. Ninja mode keeps its logs in the build
directory because Ninja's own contract puts them there and every other reader
looks for them there; Make mode keeps the same two files, in the same formats
and under the same names, outside the tree, because GNU Make's contract is that
a build leaves the tree holding only what its recipes produced. Both are still
files Ninja can read and files written by Ninja can be read from — pointing
either tool at the other's directory is all it takes — which is what this
decision requires. See `[spec:ronin:req:make.state-outside-the-tree]`.
