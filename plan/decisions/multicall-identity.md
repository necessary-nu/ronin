---
id [dec:ronin:multicall-identity]
epitome "Select the front end from the invoked program name and from nothing else, and keep Make mode an explicit Ronin surface rather than an impersonation."
state @decided
category @executive
scope {
    elements ([arch:ronin:cli] [arch:ronin:execution])
    rules (
        [spec:ronin:req:product.make-identity]
        [spec:ronin:req:make.recursive-invocation+2]
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
    {
        option "Keep --make and --ninja as a second door, so neither mode is reachable only by symlink."
        rejected_because "Tried, and the cost landed on MAKE. A sub-make reached by flag needs the flag carried in MAKE, which makes MAKE more than one word, and software that treats that value as the make program's path then execs a name that does not exist. GNU Make's own suite is the most demanding such consumer and died on it, taking 243 of 261 categories with it before an assertion ran. A second door has to answer the same question the name already answers, and it answered it worse."
    }
)
consequences {
    accepted (
        "The invoked program name selects the front end and nothing else does. The whole name must be make or gmake, so a make.old left by a package upgrade is not a request for Make mode."
        "In Make mode the MAKE variable is one word: the make-named path the invocation arrived through. When kati recognizes its recursive use, switches, assignments, goals, and directory changes become inputs to a child compilation whose graph is composed as subninja."
        "Make mode reports itself as Ronin in diagnostics. The only place it claims a GNU Make version is the Makefile-visible version variable, where a Makefile may branch on it."
    )
    rejected (
        "That the make-named symlink is optional packaging. It was deferred here while a flag could reach Make mode; with the flag gone the symlink is the only way in, so a distribution that omits it ships a Ronin with no Make front end at all."
    )
}
edges {
    requires ([dec:ronin:make-as-graph])
    refines ([dec:ronin:product-boundary])
}
codifies (
    [spec:ronin:req:product.make-identity]
    [spec:ronin:req:make.recursive-invocation+2]
)
affects ([arch:ronin:cli])
---

## Rationale

The existing product boundary says Ronin owns the product identity while Ninja
owns the compatibility vocabulary. Make mode extends that with a second
vocabulary and does not change the rule: Makefile syntax, variable semantics,
and command-line options keep GNU Make's spelling; the graph engine and the
tool reporting diagnostics are still Ronin.

Selecting on the invoked name is how every multi-call binary does this, and it
is honest in a way that sniffing the directory is not. A user who invokes
`make` has said which language they are writing.

The name is the only door, and that is the part worth stating plainly, because
the first version of this decision kept a flag beside it. The argument for the
flag was availability: a mode reachable only by symlink is a mode a packager
can forget to ship. The argument against it is that `MAKE` has to name the way
back in. With a flag in the mechanism, `MAKE` carries the flag, and a value
carrying a flag is a command line rather than a path — which is fine for a
recursive Makefile and fatal for everything else, since a great deal of
software execs `$(MAKE)` directly. Reached by name alone, `MAKE` is a path,
which is all GNU Make ever made it. Availability is the packaging problem it
always was, and it is cheaper to solve there than in the value of `MAKE`.

The path remains useful even though recursion is compiled rather than executed.
It is the Makefile-visible identity that lets kati recognize a recursive
invocation. Once recognized, the invocation's directory, goals, assignments,
and relevant flags describe another compilation unit. Its graph is included as
`subninja`; no child Make process or recursive jobserver transport is needed.
