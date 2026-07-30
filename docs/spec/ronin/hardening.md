# Ronin Rust hardening contract

These requirements define internal safety and correctness properties for
Ronin's Rust implementation. They refine the observable Ninja compatibility
contract without assigning ownership of the existing compatibility rules to a
second work item.

## Jobserver resource safety

> [spec:samurai:req:runtime.jobserver-resource-safety]
> Ronin MUST preserve the inherited GNU Make jobserver's open-file-description
> flags, model its implicit and explicit slots as single-owner guards, and
> return every acquired token exactly once on normal return or unwinding.
> Waiting for an explicit token MUST enter the completion supervisor as an
> event rather than repeatedly polling the descriptor. Acquisition MUST
> distinguish temporary unavailability from interruption, EOF, and transport
> errors. Exact-token release is attempted by an infallible guard destructor;
> because destructor failures cannot be reported reliably during unwinding,
> release I/O errors are non-reportable and MUST NOT trigger a second release.

## Output byte boundaries

> [spec:samurai:req:runtime.output-byte-boundaries]
> Ronin MUST preserve untouched valid UTF-8 slices when expanding status
> formats and MUST NOT widen individual UTF-8 bytes into Unicode scalar
> values. Ninja-compatible tool serializers MUST preserve path and command
> bytes exactly, escaping only bytes that are significant to the output
> syntax, and target-taking tools MUST resolve native byte arguments without
> requiring UTF-8. Human-oriented diagnostics MAY replace invalid UTF-8 at
> their display boundary. Tool rendering MUST propagate failures while
> acquiring process or filesystem context and MUST append records directly to
> the output buffer without staging a separately allocated string per record.

## MSVC include byte parsing

> [spec:samurai:req:runtime.msvc-byte-parsing]
> Ronin MUST filter MSVC `/showIncludes` output as bytes, preserve non-UTF-8
> include paths and visible compiler output, and perform only explicitly
> ASCII-insensitive filename-extension and system-directory comparisons.

## Transactional persistent state

> [spec:samurai:req:runtime.persistence-transactions]
> Ronin MUST stage build-log entries, dependency-log entries, and dependency
> node IDs without mutating live state. A log rewrite MUST use a unique
> temporary file in the destination directory, completely write and flush it,
> synchronize its contents, acquire the replacement append handle, and
> atomically replace the destination before committing staged memory or graph
> state. Any failure before replacement MUST leave the original file, writer,
> entries, and graph IDs usable and unchanged. Log opening MUST distinguish a
> missing file from every other filesystem error, and records whose durability
> boundary is shared MAY be emitted and flushed as one batch.

## Semantic error boundaries

> [spec:samurai:req:runtime.semantic-errors]
> Ronin MUST represent CLI, manifest, graph, build, persistence, process, and
> tool failures with semantic variants rather than unrestricted diagnostic
> strings. Variants MUST retain applicable byte-exact paths, source locations,
> typed node or edge identities, process exit statuses, and underlying source
> errors. The public error classifier MUST be derived from the semantic error
> chain, including errors propagated across subsystem boundaries. Production
> code MUST NOT use blanket conversions from strings or `io::Error` into
> subsystem errors, parse rendered diagnostics to recover structure, or expose
> diagnostic-string equality as error semantics. Final stdout and stderr
> writes MUST report real failures while treating `BrokenPipe` as an
> intentional downstream close.

## Borrowed manifest frontend

> [spec:samurai:req:runtime.borrowed-span-frontend]
> Ronin MUST retain each manifest source as exact bytes with stable source
> identity, and every returned lexical token MUST identify its byte span.
> Lexemes and evaluation fragments MUST borrow from retained sources until a
> value crosses into graph-owned state, where it is materialized exactly once.
> Scanner results MUST be returned directly rather than transferred through
> scratch side channels, and dependency separators and their allowed grammar
> sets MUST be typed. Allocation count, manifest command-evaluation runtime,
> and peak RSS MUST be measured against the pre-change implementation while
> the pinned Ninja suite remains the compatibility oracle.

## Transactional dynamic dependencies

> [spec:samurai:req:runtime.dyndep-transaction]
> Ronin MUST parse dynamic-dependency files into a source-aware staged
> representation without interning paths or otherwise mutating graph state.
> It MUST validate expected entries, edge ownership, duplicate outputs,
> bindings, and every dynamic path before a single infallible commit phase.
> Any parse or validation failure MUST leave graph nodes, edges, bindings, and
> pending state unchanged. Clean-tool discovery MUST reuse the byte-exact
> dynamic-dependency parser, preserve Ninja escapes and non-UTF-8 paths,
> tolerate a missing file, and propagate other read or parse failures.

## Typed runtime state

> [spec:samurai:req:runtime.typed-runtime-state]
> Ronin MUST keep manifest graph entities free of per-build node observation,
> log comparison, dependency-loading, command-dirtiness, restat, pool
> occupancy, and critical-path scheduling state. Those facts MUST live in
> reusable dense runtime or plan side tables indexed by typed arena IDs.
> Timestamp, dependency-ID, command-hash, partition, concurrency-limit, and
> traversal-mark sentinels MUST be hidden behind zero-cost semantic types or
> explicit enums. Reinitialization and failure paths MUST release pool
> occupancy and clear transient edge state without rebuilding graph topology.

## Scalable process supervision

> [spec:samurai:req:runtime.process-supervisor-scalability]
> On Unix, Ronin MUST supervise captured child output and external jobserver
> notifications with one readiness-driven event loop rather than one thread
> per child, periodic descriptor polling, or a general asynchronous runtime.
> The supervisor MUST preserve combined stdout/stderr order, process-group
> interruption, console exclusivity, completion-driven dependency release,
> and exact child reaping on success, failure, cancellation, and unwinding.
> Load-average reads MUST be cached for a bounded interval, and output and
> diagnostic sinks MUST be flushed once per complete semantic output batch
> while preserving explanation, status, failure-context, and child-output
> order. Runtime selection MUST be supported by reproducible high-concurrency
> measurements of throughput, startup, binary size, idle wakeups, peak thread
> count, peak RSS, signal behavior, console ownership, output ordering,
> cancellation, child reaping, and jobserver integration.

## Guarded signal boundary

> [spec:samurai:req:runtime.guarded-signal-boundary]
> On Unix, Ronin MUST represent handled signals with a closed typed set and
> install their handlers through one owned, process-global boundary before
> worker threads start. The boundary and its registrations MUST have explicit
> executable lifetime. Installation MUST publish the boundary only after every
> flag and readiness-wake registration succeeds, and MUST unregister every
> known partial registration on failure. Signal arrival MUST wake the existing
> process supervisor rather than depend on a periodic scheduler timer.
> Forwarding MUST target the intended child or process group, treat a vanished
> target (`ESRCH`) as benign, and retain every other delivery failure as a
> semantic process error. Ronin MUST re-raise the observed signal with its
> default disposition so shells observe signal termination. Production and
> test code MUST NOT duplicate raw signal numbers or handwritten signal FFI.
