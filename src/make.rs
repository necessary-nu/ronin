//! Make as a front end.
//!
//! GNU Make's evaluator is Ronin's fork of kati, vendored at `kati/`. Upstream
//! kati turns a Makefile into `build.ninja` and stops; here the same evaluation
//! hands its dependency nodes straight to [`GraphSink`], which builds the graph
//! Ronin's scheduler, persistence, and process supervision already run. No
//! manifest is written, read, or reparsed on the way.
//!
//! Emission is retained, but demoted: `build.ninja` is a debugging artifact and
//! the oracle this module is checked against, not a step in a build. For any
//! Makefile, the graph [`load_makefile`] returns and the graph Ronin gets by
//! parsing that same run's emitted manifest describe the same build.
//!
//! See `plan/decisions/make-as-graph.md`.
// [spec:ronin:req:make.graph-direct]

/// The Make evaluator this front end is built on.
///
/// Re-exported because [`load_makefile`] takes a `kati::session::Session`: a
/// caller has to be able to build one, and building one against a separately
/// declared copy of the crate would produce a different type with the same
/// name.
pub use kati;

pub(crate) mod cli;
mod sink;

#[cfg(test)]
mod equivalence;

pub use sink::GraphSink;

/// The GNU Make release whose vocabulary this front end speaks.
///
/// The one version Make mode names, and it names it in two places: the
/// `MAKE_VERSION` a Makefile can branch on, which the evaluator's bootstrap
/// binds, and `--version`, which reports it as the language rather than as the
/// tool. A test builds a Makefile that reads `MAKE_VERSION` and compares it
/// with this, because the two live in different crates and would otherwise
/// drift apart silently.
// [spec:ronin:req:product.make-identity]
pub const MAKE_VERSION: &str = "4.4.1";

use crate::frontend::{BuildGraph, FrontendError};
use kati::evaluate::{evaluate, Evaluated};
use kati::ninja::emit_build;
use kati::session::Session;
use std::error;
use std::fmt;

/// Why a Makefile did not become a graph.
#[derive(Debug)]
#[non_exhaustive]
pub enum MakeError {
    /// Make evaluation rejected the Makefile. The text is kati's own
    /// diagnostic, with the causes behind it on their own lines, which is how
    /// kati reports them itself.
    Evaluate(String),
    /// The Makefile evaluated, and described a graph that cannot exist.
    Construct(FrontendError),
}

impl fmt::Display for MakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evaluate(diagnostic) => formatter.write_str(diagnostic),
            Self::Construct(error) => error.fmt(formatter),
        }
    }
}

impl error::Error for MakeError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Evaluate(_) => None,
            Self::Construct(error) => Some(error),
        }
    }
}

impl MakeError {
    /// kati's failure, rendered the way kati renders it: one cause per line.
    fn evaluate(error: &kati::anyhow::Error) -> Self {
        let mut diagnostic = String::new();
        for cause in error.chain() {
            if !diagnostic.is_empty() {
                diagnostic.push('\n');
            }
            diagnostic.push_str(&cause.to_string());
        }
        Self::Evaluate(diagnostic)
    }
}

/// Evaluate the Makefile `session` names and build its graph.
///
/// `session` carries the whole Make command line: which makefile, which goals,
/// which command-line assignments, how many jobs. What comes back is a graph
/// [`Build`](crate::frontend::Build) runs and
/// [`Persistence`](crate::frontend::Persistence) makes incremental, with the
/// Makefile's own default goal recorded as the graph's default target.
///
/// # Errors
///
/// Returns [`MakeError::Evaluate`] for a Makefile Make itself rejects — a
/// syntax error, an `$(error)`, a prerequisite with no rule to make it — and
/// [`MakeError::Construct`] for one that evaluates but describes a graph the
/// engine cannot hold, such as two rules generating one output.
// [spec:ronin:req:make.graph-direct]
pub fn load_makefile(session: Session) -> Result<BuildGraph, MakeError> {
    let Evaluated { mut ev, nodes } =
        evaluate(session).map_err(|error| MakeError::evaluate(&error))?;
    let mut sink = GraphSink::new();
    let emitted = emit_build(&nodes, &mut ev, &mut sink);
    let graph = sink.into_graph().map_err(MakeError::Construct)?;
    emitted.map_err(|error| MakeError::evaluate(&error))?;
    ev.finish().map_err(|error| MakeError::evaluate(&error))?;
    Ok(graph)
}
