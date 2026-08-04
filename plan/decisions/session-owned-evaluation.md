---
id [dec:ronin:session-owned-evaluation]
epitome "Own every piece of Make evaluation state in an explicit session; no mutable process globals survive."
state @decided
category @property
scope {
    elements ([arch:ronin:make-frontend])
    rules (
        [spec:ronin:req:make.no-ambient-state]
        [spec:ronin:req:make.scope-separation]
    )
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Leave the globals and run one evaluation per process."
        rejected_because "It makes the process the unit of isolation, which forecloses in-process recursive Make, forces the differential harness to fork for every case, and leaves the direct and emitted graph paths unable to run in one process where comparing them is cheapest."
    }
    {
        option "Keep the globals but guard them with a reentrancy lock."
        rejected_because "A lock around ambient state documents the defect without removing it, and serializes exactly the work that should compose."
    }
    {
        option "Reset the globals between evaluations."
        rejected_because "Correctness would depend on remembering to extend the reset whenever a global is added, which is the failure mode that produced twelve of them."
    }
)
consequences {
    accepted (
        "Evaluation state is threaded explicitly: symbol interning, variable scope, the glob, file, and find caches, command results, shell status, used-variable tracking, statistics, and flags."
        "Symbol interning splits from Make's global variable scope. The symbol table currently stores variable bindings alongside interned names, which is why interning a string and defining a variable are the same operation and neither can be reset without the other."
        "Flags become a parsed value constructed from arguments, not a lazily initialized read of the process command line."
        "Types whose Display and Debug implementations currently reach for the interner gain explicit display forms that take the session."
        "The front end sets no global allocator; allocator choice stays a Ronin-level decision."
        "Immutable dispatch tables are not globals in the sense being removed here and may remain, provided they are genuinely read-only after construction."
    )
    deferred (
        "Evaluating two sessions concurrently is not a goal of this change, only evaluating them in one process. Concurrency inside a session stays out of scope until the session boundary exists."
    )
}
edges {
    requires ([dec:ronin:make-as-graph])
}
codifies (
    [spec:ronin:req:make.no-ambient-state]
    [spec:ronin:req:make.scope-separation]
)
affects ([arch:ronin:make-frontend])
---

## Rationale

The vendored front end carries twelve pieces of mutable process-global state:
the symbol table and six symbols derived from it, a default location symbol,
the command-line flags, shell status, used environment variables, used
undefined variables, the glob cache, the makefile cache, the find emulator and
its node counter, command results, and the statistics registry. The binary also
installs a global allocator.

Each of them is individually defensible in a program that runs once and exits.
Together they make the process the unit of evaluation, and this integration
needs the session to be the unit instead. In-process recursive Make needs it.
The manifest-equivalence property needs it, because the cheapest way to compare
the direct graph against the emitted one is to build both without forking. The
differential corpus needs it, because three hundred cases forking a process
each is a harness that nobody will run often enough to trust.

The symbol table is the substantial one, and not because of its size. It is two
things wearing one name: an interner mapping bytes to indices, and Make's
global variable scope keyed by those indices. Interning a string and creating a
variable binding are the same call. Separating them is a real design change to
the front end, and it is the change that makes the rest of the state tractable
— roughly a hundred and twenty call sites read a symbol's bytes without any
context to read them from, and they can only be given one once the interner is
a value somebody owns.
