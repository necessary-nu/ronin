//! Ronin, a Ninja-compatible build tool implemented in Rust.

mod build;
pub mod cli;
mod deps;
mod dyndep;
mod env;
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
pub mod subprocess;
mod tool;
mod util;

#[cfg(test)]
mod port_tests;
