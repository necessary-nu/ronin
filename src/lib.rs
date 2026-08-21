//! Ronin, a Ninja-compatible build tool implemented in Rust.

#![deny(missing_docs, unreachable_pub)]
#![allow(
    clippy::redundant_pub_crate,
    reason = "explicit crate visibility documents the supported API boundary and is enforced by unreachable_pub"
)]

mod build;
mod cli;
mod deps;
mod dyndep;
mod env;
mod error;
mod explanations;
pub mod frontend;
mod graph;
mod htab;
mod jobserver;
mod lint;
mod log;
#[cfg(all(unix, feature = "make"))]
pub mod make;
mod missing_deps;
mod msvc;
mod multicall;
mod names;
mod os;
mod parse;
mod persistence;
mod runtime;
mod scan;
mod signal;
mod source;
mod subprocess;
mod tool;
mod util;

pub use cli::{NINJA_COMPAT_VERSION, PRODUCT_NAME, RunResult, Runner, run, run_os};
pub use error::{Error, ErrorKind};
pub use multicall::{run_as_shell, run_process};
pub use signal::{Signal, SignalHandlers, install_signal_handlers};
pub use subprocess::{INTERRUPTED_EXIT_CODE, declare_builtin_shell};

#[cfg(test)]
mod port_tests;

// The one definition of the scratch directory, shared with the integration
// suites by inclusion rather than by a second copy: an integration test is its
// own crate and cannot reach into this one's `cfg(test)` items, and this one
// cannot depend on a `tests/` target, so the file is included on both sides.
#[cfg(test)]
#[allow(
    unreachable_pub,
    reason = "the shared file is `pub` for the integration crates that include it; here it is a private module"
)]
#[path = "../tests/support/scratch.rs"]
mod scratch_directory;
