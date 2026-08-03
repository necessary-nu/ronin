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
mod graph;
mod htab;
mod jobserver;
mod log;
mod missing_deps;
mod msvc;
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

pub use cli::{run, run_os, RunResult, Runner, NINJA_COMPAT_VERSION, PRODUCT_NAME};
pub use error::{Error, ErrorKind};
pub use signal::{install_signal_handlers, Signal, SignalHandlers};
pub use subprocess::INTERRUPTED_EXIT_CODE;

#[cfg(test)]
mod port_tests;
