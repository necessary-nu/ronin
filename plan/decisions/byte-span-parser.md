---
id [dec:ronin:byte-span-parser]
epitome "Use one byte-oriented manifest frontend with spans and borrowed evaluation parts."
state @approved
category @executive
scope {
    elements ([arch:ronin:manifest-frontend])
    rules ([spec:samurai:req:compat.manifest-semantics])
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Keep separate Vec<char>-oriented scanning and String-oriented parsing layers."
        rejected_because "They rebuild logical lines, allocate token strings, and cannot preserve arbitrary bytes exactly."
    }
    {
        option "Use a parser-generator grammar."
        rejected_because "Ninja's escape, indentation, newline, include-scope, and diagnostic behavior is small but unusually contextual; a direct frontend is easier to match against upstream tests."
    }
)
consequences {
    accepted (
        "The lexer reads byte slices and returns tokens carrying source spans."
        "The parser borrows manifest bytes where lifetime permits and interns only graph-owned values."
        "Includes and subninja files retain source identity for diagnostics while sharing explicit scope IDs."
    )
    deferred (
        "Incremental reparsing is deferred because Ninja compatibility rebuilds a manifest generation as a unit."
    )
}
edges {
    requires (
        [dec:ronin:byte-exact-core]
        [dec:ronin:typed-graph-arenas]
    )
}
codifies ([spec:samurai:req:compat.manifest-semantics])
establishes ([arch:ronin:manifest-frontend])
---

## Rationale

The literal frontend passes through byte vectors, `char` vectors, rebuilt
logical lines, `String` tokens, and boxed evaluation fragments. Besides the
allocation cost, each conversion is another opportunity to diverge from
Ninja's byte-level escape and path behavior.

A single byte-oriented frontend keeps the source of truth intact. Spans make
diagnostics precise without copying lexemes, while typed graph IDs provide the
stable destinations required when parsed values become owned graph state.
