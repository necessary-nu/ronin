---
id [dec:ronin:multicall-identity]
epitome "Select the front end from the invoked program name, and keep Make mode an explicit Ronin surface rather than an impersonation."
state @decided
category @executive
scope {
    elements ([arch:ronin:cli] [arch:ronin:execution])
    rules (
        [spec:ronin:req:product.make-identity]
        [spec:ronin:req:make.recursive-invocation]
        [spec:ronin:req:make.jobserver]
    )
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Ship a separate executable for Make mode."
        rejected_because "Two binaries sharing one engine is the arrangement the integration exists to remove, and it puts the front-end boundary in the filesystem where it cannot be tested."
    }
    {
        option "Select the front end by sniffing for a Makefile or a build.ninja in the working directory."
        rejected_because "Implicit selection makes the tool's behavior depend on directory contents, so a stray file changes which language is spoken. The invoked name is an explicit statement of intent."
    }
)
consequences {
    accepted (
        "The invoked program name selects the front end, and a command-line override exists in both directions so neither mode is reachable only by symlink."
        "In Make mode the MAKE variable names Ronin's own executable, so recursion re-enters Ronin instead of dispatching to whatever Make is on the path."
        "Make mode serves the jobserver as well as consuming it, so a recursive tree shares one job budget. Ronin already parses the named-pipe form as a client; the server side is new."
        "Make mode reports itself as Ronin in diagnostics. The only place it claims a GNU Make version is the Makefile-visible version variable, where a Makefile may branch on it."
    )
    deferred (
        "Whether to distribute a make-named symlink is a packaging question and is not decided here. The mechanism works whether or not the symlink ships."
    )
}
edges {
    requires ([dec:ronin:make-as-graph])
    refines ([dec:ronin:product-boundary])
}
codifies (
    [spec:ronin:req:product.make-identity]
    [spec:ronin:req:make.recursive-invocation]
    [spec:ronin:req:make.jobserver]
)
affects ([arch:ronin:cli])
---

## Rationale

The existing product boundary says Ronin owns the product identity while Ninja
owns the compatibility vocabulary. Make mode extends that with a second
vocabulary and does not change the rule: Makefile syntax, variable semantics,
and the jobserver protocol are GNU Make's and keep their spelling; the tool
reporting the diagnostics is still Ronin.

Selecting on the invoked name is how every multi-call binary does this, and it
is honest in a way that sniffing the directory is not. A user who invokes
`make` has said which language they are writing.

The jobserver is the part that pays for itself immediately. Ronin already
speaks the client side including the named-pipe form GNU Make 4.4 introduced.
Adding the server side means a recursive Make tree converges on one job budget
instead of one per level, which is the oversubscription every recursive build
currently works around by hand.
