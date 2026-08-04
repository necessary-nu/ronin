---
id [dec:ronin:make-as-graph]
epitome "Subsume Make by building Ronin's graph from kati's dependency nodes, never through a serialized manifest."
state @decided
category @executive
scope {
    elements ([arch:ronin:make-frontend] [arch:ronin:graph-construction])
    rules (
        [spec:ronin:req:make.graph-direct]
        [spec:ronin:req:make.manifest-equivalence]
    )
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Pipe the two tools: kati --ninja writes build.ninja, Ronin reads it."
        rejected_because "It works today and buys nothing. The manifest is a lossy round trip between two in-memory graphs that already agree, and on a tree the size of Android it serializes and reparses hundreds of megabytes to move data that never had to leave the process."
    }
    {
        option "Keep kati's own executor and make it parallel."
        rejected_because "kati's executor is a 197-line recursive walk with no job scheduling, no build log, no depfile handling, and no pools. Making it competitive means rewriting Ronin's scheduler inside kati."
    }
    {
        option "Translate Makefiles to Ninja syntax and feed the text to Ronin's manifest parser in memory."
        rejected_because "It keeps the serialization cost and the escaping hazards while adding an in-memory pipe. The only thing text buys is debuggability, which the retained emitter already provides."
    }
)
consequences {
    accepted (
        "kati's dependency node is treated as a Ninja edge, because it already is one: output and implicit outputs, inputs partitioned into explicit and order-only, validations, pool, depfile, restat, and commands all map across without loss."
        "Ninja emission is retained but demoted. It is a debugging artifact and the oracle for the direct path, not a step in execution."
        "Command construction stays in the front end. Shell script generation, command translation, depfile extraction, and the response-file threshold are Make concerns and do not move into the graph."
        "Make workloads inherit Ronin's build log, dependency log, restat, pools, and console handling, none of which GNU Make has."
    )
    deferred (
        "Unifying kati's symbol interner with Ronin's path names is deferred until the front end owns its interner, because the two currently disagree about who is allowed to allocate."
    )
}
edges {
    requires (
        [dec:ronin:frontend-graph-boundary]
        [dec:ronin:typed-graph-arenas]
    )
}
codifies (
    [spec:ronin:req:make.graph-direct]
    [spec:ronin:req:make.manifest-equivalence]
)
establishes ([arch:ronin:make-frontend])
---

## Rationale

kati already thinks in Ninja. `DepNode` carries `output`, `implicit_outputs`,
`actual_inputs`, `actual_order_only_inputs`, `actual_validations`, `is_phony`,
`is_restat`, `depfile_var`, and `ninja_pool_var`. Ronin's `Edge` carries `out`
with an explicit-output count, `input` with explicit and order-only
partitions, `validation`, `pool`, `dyndep`, and bindings. The correspondence is
total; the text format in between is a detour.

So the integration is not a translation problem, it is a plumbing problem. The
emission functions already compute everything an edge needs — they simply
write it out as bytes. Redirecting that computation into graph construction
removes the format without touching the semantics.

Retaining the emitter is what makes the change safe to land. For any Makefile,
the graph built directly must equal the graph obtained by parsing the emitted
manifest. That is a differential property between two paths in one process,
checkable over the whole Make corpus, and it is the same verification shape
Ronin already uses against upstream Ninja.
