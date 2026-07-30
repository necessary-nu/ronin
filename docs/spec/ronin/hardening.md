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
