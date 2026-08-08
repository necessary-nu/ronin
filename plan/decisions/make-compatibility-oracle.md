---
id [dec:ronin:make-compatibility-oracle]
epitome "Verify Make mode against two oracles: GNU Make 4.4.1 for semantics, and the emitted manifest for graph construction."
state @obsolesced
category @executive
scope {
    elements ([arch:ronin:verification] [arch:ronin:make-frontend])
    rules ([spec:ronin:req:make.semantics])
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Keep the vendored Go test harness and run it as-is."
        rejected_because "It adds Go and its own runner to a gate whose value comes from being cheap enough to run on every change, and it reports differences in its own vocabulary rather than the differential form the rest of the verification already speaks."
    }
    {
        option "Pin the oracle at GNU Make 4.2.1 as the vendored harness does."
        rejected_because "4.4 moved the jobserver to named pipes, which is the protocol this integration implements, and changed parallelism directives Make mode must honor. Verifying against a version that predates the features being built tests the wrong thing."
    }
    {
        option "Treat kati's own behavior as the oracle."
        rejected_because "It fixes today's divergences from Make as the specification, including the ones that exist only because kati targets one build system's makefiles."
    }
)
consequences {
    accepted (
        "The corpus is ported to a Rust differential harness in the shape the Ninja corpus already uses: run the case under both tools, compare output and exit status, and record differences rather than assertions."
        "GNU Make 4.4.1 is the pinned semantics oracle. Moving the pin reruns the corpus and reclassifies whatever moves, on the same terms as the Ninja pin."
        "Graph construction is verified against the front end's own emitted manifest, which is why that emitter is retained. This oracle is internal, exact, and needs no external tool."
        "Cases where the front end knowingly departs from GNU Make are classified as such rather than deleted, so the departure is a recorded position instead of a missing test."
    )
    deferred (
        "Verifying against Android's makefiles, the workload the vendored front end was actually built for, is deferred until the corpus passes. It is a validation workload, not an oracle."
    )
}
edges {
    requires ([dec:ronin:make-as-graph])
    refines ([dec:ronin:ninja-compatibility-oracle])
}
codifies ([spec:ronin:req:make.semantics])
affects ([arch:ronin:verification])
---

## Rationale

Two different questions need two different oracles, and conflating them is how
this kind of work goes wrong.

Whether a Makefile evaluates correctly is a question about GNU Make, and only
GNU Make can answer it. That is the vendored corpus of roughly three hundred
cases, run differentially against a pinned real Make, in the same form the
Ninja corpus already takes.

Whether the resulting graph is built correctly is a question with an internal
answer. The front end can already serialize its graph to a manifest that
Ronin's own parser reads, so the two paths can be compared exactly, over every
case, without any external tool and without leaving the process. That is why
the emitter is kept rather than deleted once it stops being on the execution
path: it stops being a feature and becomes the test oracle for its replacement.
