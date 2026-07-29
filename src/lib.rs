//! Ronin, a Ninja-compatible build tool implemented in Rust.

#![deny(missing_docs, unreachable_pub)]

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
mod os;
mod parse;
mod scan;
mod subprocess;
mod tool;
mod util;

pub use cli::{run, run_os, RunResult, NINJA_COMPAT_VERSION, PRODUCT_NAME};
pub use error::{Error, ErrorKind};
pub use subprocess::{install_signal_handlers, interrupted_signal, reraise_signal};

#[cfg(test)]
mod port_tests;
