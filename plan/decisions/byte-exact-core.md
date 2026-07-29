---
id [dec:ronin:byte-exact-core]
epitome "Represent manifest data and Unix paths as exact bytes until an OS or display boundary."
state @approved
category @property
scope {
    elements ([arch:ronin:core-data])
    rules ([spec:samurai:req:compat.byte-inputs])
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Use String and PathBuf throughout and repair invalid text with lossy conversion."
        rejected_because "Distinct Ninja paths and command bytes can collapse or change under lossy Unicode conversion."
    }
    {
        option "Use OsString for every manifest value."
        rejected_because "Most manifest operations are byte-language operations, while OsString makes parsing and expansion cumbersome and platform-dependent."
    }
)
consequences {
    accepted (
        "Owned values use byte containers and borrowed operations use byte slices."
        "Conversions to OsStr, Path, shell display, or UTF-8 diagnostics occur only at explicit boundaries."
        "Evaluation strings become compact ordered parts rather than boxed C-shaped fragments."
    )
    deferred (
        "Windows native-string representation and quoting receive a platform-specific boundary design when Windows support is implemented."
    )
}
edges {
    requires ()
}
codifies ([spec:samurai:req:compat.byte-inputs])
establishes ([arch:ronin:core-data])
---

## Rationale

Ninja manifests are byte-oriented and Unix permits non-UTF-8 path names. The
literal port repeatedly converts byte vectors to `String`, sometimes through
lossy decoding, and stores evaluation fragments in allocation-heavy shapes.
That is both a correctness risk and a hot-path cost.

The core model will preserve bytes exactly and expose explicit rendering
operations for diagnostics and shells. Borrowing spans and slices avoids
copies; owned byte buffers remain available where manifest lifetime or OS
ownership requires them.
