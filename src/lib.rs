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
pub use subprocess::INTERRUPTED_EXIT_CODE;

#[cfg(test)]
mod port_tests;
