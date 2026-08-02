---
id [dec:ronin:completion-driven-execution]
epitome "Drive scheduling from child completion events in one explicit supervisor."
state @approved
category @executive
scope {
    elements ([arch:ronin:execution])
    rules (
        [spec:ronin:req:compat.scheduling]
        [spec:ronin:req:compat.process-integration]
        [spec:ronin:req:compat.command-runtime]
    )
}
author "brendan@bbqsrc.net"
alternatives (
    {
        option "Retain batch barriers that wait for a group before scheduling newly ready work."
        rejected_because "Barriers leave capacity idle and entangle graph progress with output collection."
    }
    {
        option "Adopt a general asynchronous runtime."
        rejected_because "The required event sources are child processes, pipes, signals, jobserver tokens, and console ownership; a small explicit supervisor is easier to audit and adds no runtime dependency."
    }
)
consequences {
    accepted (
        "One supervisor owns running children, ready queues, pool depth, console ownership, jobserver tokens, and signal state."
        "Each completion immediately updates graph state and may release newly ready edges."
        "Evaluated command, response-file, and status data are cached once per edge execution."
        "Child output is drained while running and delivered in Ninja-compatible edge order."
    )
    deferred (
        "Multiple scheduler threads are deferred; process parallelism provides the intended build concurrency."
    )
}
edges {
    requires (
        [dec:ronin:typed-graph-arenas]
        [dec:ronin:iterative-graph-evaluation]
        [dec:ronin:ninja-persistence-boundary]
    )
}
codifies (
    [spec:ronin:req:compat.scheduling]
    [spec:ronin:req:compat.process-integration]
    [spec:ronin:req:compat.command-runtime]
)
establishes ([arch:ronin:execution])
---

## Rationale

The current scheduler reaches Ninja parity on the initial barrier workload, so
the redesign is not justified by that median alone. It is justified by the
control-flow and scalability problems in the literal structure: graph
evaluation, process waiting, pool accounting, jobserver tokens, and output
delivery are split across compatibility paths and synchronized in batches.

An explicit completion-driven supervisor preserves a single source of truth.
It can keep all permitted slots busy, release resources exactly once, and
centralize interrupt and console behavior without introducing an async runtime.
