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
