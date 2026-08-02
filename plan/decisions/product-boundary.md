---
id [dec:ronin:product-boundary]
epitome "Ronin owns the product identity while Ninja owns the compatibility vocabulary."
state @approved
category @executive
scope {
    elements ([arch:ronin:cli])
    rules (
        [spec:ronin:req:product.ronin-identity]
        [spec:ronin:req:product.no-samuflags]
        [spec:ronin:req:compat.version-reporting]
        [spec:ronin:req:compat.ninja-owned-names]
        [spec:ronin:req:compat.cli-and-tools]
    )
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Keep the samurai and samu identity throughout the Rust port."
        rejected_because "The operator selected Ronin as the product and executable name."
    }
    {
        option "Retain SAMUFLAGS or add a compatibility alias."
        rejected_because "SAMUFLAGS is not a Ninja interface and was explicitly excluded from Ronin."
    }
)
consequences {
    accepted (
        "Cargo, the executable, diagnostics, help, and product documentation use Ronin."
        "Ninja-owned file names, variables, tool names, and version-token syntax remain stable."
    )
    deferred (
        "A separately packaged ninja executable alias may be evaluated later; it is not required now."
    )
}
edges {
    requires ()
}
codifies (
    [spec:ronin:req:product.ronin-identity]
    [spec:ronin:req:product.no-samuflags]
    [spec:ronin:req:compat.version-reporting]
    [spec:ronin:req:compat.ninja-owned-names]
    [spec:ronin:req:compat.cli-and-tools]
)
establishes ([arch:ronin:cli])
---

## Rationale

Build generators and users need a stable distinction between the tool they are
running and the protocol it implements. Ronin is the former; Ninja is the
latter. Renaming Ninja-owned tokens would break interoperability, while
retaining samurai-only environment behavior would add a compatibility burden
that does not help Ninja users.

`--version` therefore remains a plain Ninja-compatible numeric token describing
the supported compatibility level. Product branding belongs in package
metadata, help, diagnostics, and documentation rather than in that parseable
token.
