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
