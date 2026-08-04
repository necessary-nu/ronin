---
id [dec:ronin:frontend-graph-boundary]
epitome "Expose graph construction as a front-end-agnostic capability and keep manifest parsing behind it."
state @decided
category @executive
scope {
    elements ([arch:ronin:graph-construction] [arch:ronin:manifest-frontend] [arch:ronin:graph-engine])
    rules ([spec:ronin:req:frontend.graph-construction])
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Let the Make front end reach into the graph arenas directly."
        rejected_because "The arenas enforce their invariants through the functions that mutate them: node use lists, edge input partitions, validation side tables, and default target collection. A second writer bypassing those is a defect waiting for a corpus large enough to find it."
    }
    {
        option "Have the Make front end call the Ninja manifest parser on generated text."
        rejected_because "That is the serialized round trip this architecture exists to delete."
    }
)
consequences {
    accepted (
        "Parsing a manifest and building a graph become separately invocable. The fused path in the command-line entry point splits."
        "The graph-construction API is the supported extension point for any front end, and its invariants are enforced there rather than trusted at each call site."
        "The Ninja front end is rebuilt on the same API it exposes, so the API is exercised by the existing conformance corpus rather than only by its new consumer."
        "Tool modes and the manifest rebuild path stop reaching through the fused entry point to get at a graph."
    )
    deferred ()
}
edges {
    requires ([dec:ronin:typed-graph-arenas])
    refines ([dec:ronin:byte-span-parser])
}
codifies ([spec:ronin:req:frontend.graph-construction])
establishes ([arch:ronin:graph-construction])
---

## Rationale

Ronin's library surface exposes only the command-line entry points, and the
entry point fuses two separable things: turning a manifest into a graph, and
building a graph. A second front end needs the second half without the first.

Splitting them is worth doing on its own terms. The tool modes, the manifest
rebuild path, and the differential harnesses all currently obtain a graph by
going through argument parsing, which is why each of them carries a little
argument-shaped scaffolding it does not want.

Making the Ninja front end a consumer of the new API rather than a privileged
insider is the part that keeps it honest. If manifest parsing can express
everything through the boundary, the boundary is sufficient for Make; if it
cannot, that is discovered by the conformance corpus rather than by the Make
corpus later.
