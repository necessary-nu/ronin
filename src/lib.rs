//! Ronin, a Ninja-compatible build tool implemented in Rust.

pub mod build;
pub mod cli;
pub mod deps;
pub mod dyndep;
pub mod env;
pub mod explanations;
pub mod graph;
pub mod htab;
pub mod jobserver;
pub mod log;
pub mod missing_deps;
pub mod msvc;
pub mod os;
pub mod parse;
pub mod scan;
pub mod subprocess;
pub mod tool;
pub mod tree;
pub mod util;

#[cfg(test)]
mod port_tests;
