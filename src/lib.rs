//! Rust implementation of the samurai build tool, preserving the original
//! C module boundaries where they remain useful.

pub mod build;
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
pub mod samu;
pub mod scan;
pub mod subprocess;
pub mod tool;
pub mod tree;
pub mod util;

#[cfg(test)]
mod port_tests;
