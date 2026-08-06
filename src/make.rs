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

use crate::frontend::{BuildGraph, FrontendError, StatePlacement};
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
/// The graph also carries the half of Make's compatibility contract that
/// persistence answers to: a build leaves the tree holding exactly what the
/// Makefile put there, so the state that makes the next build incremental is
/// kept outside it. Ninja's contract says the opposite and a manifest's graph
/// says so too, which is why the graph is what says it rather than the caller.
///
/// # Errors
///
/// Returns [`MakeError::Evaluate`] for a Makefile Make itself rejects — a
/// syntax error, an `$(error)`, a prerequisite with no rule to make it — and
/// [`MakeError::Construct`] for one that evaluates but describes a graph the
/// engine cannot hold, such as two rules generating one output.
// [spec:ronin:req:make.graph-direct]
// [spec:ronin:req:make.state-outside-the-tree]
pub fn load_makefile(session: Session) -> Result<Loaded, MakeError> {
    let Evaluated { mut ev, nodes } =
        evaluate(session).map_err(|error| MakeError::evaluate(&error))?;
    let mut sink = GraphSink::new();
    let emitted = emit_build(&nodes, &mut ev, &mut sink);
    let mut graph = sink.into_graph().map_err(MakeError::Construct)?;
    graph.state_placement = StatePlacement::OutsideTheTree;
    emitted.map_err(|error| MakeError::evaluate(&error))?;
    let exported = exported_environment(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
    ev.finish().map_err(|error| MakeError::evaluate(&error))?;
    Ok(Loaded { graph, exported })
}

/// A Makefile read: the graph it describes, and what it exported.
pub struct Loaded {
    /// What the Makefile builds.
    pub graph: BuildGraph,
    /// What `export` put in every recipe's environment, and what `unexport`
    /// took out of it. Values are evaluated here because the evaluator that
    /// can answer for them does not outlive this call.
    pub exported: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

fn exported_environment(
    ev: &mut kati::eval::Evaluator,
) -> Result<Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>, kati::anyhow::Error> {
    use std::os::unix::ffi::OsStringExt;
    let mut exports = ev.exports.clone();
    // `.EXPORT_ALL_VARIABLES` names nothing, so it is the set of variables the
    // Makefile itself defined. GNU Make leaves the built-in defaults out: with
    // it declared, `CC` is still unset in a recipe.
    if ev.session.flags.export_all_variables {
        for (name, var) in ev
            .session
            .globals
            .matching(|var| var.read().origin() == kati::var::VarOrigin::File)
        {
            let _ = var;
            exports.entry(name).or_insert(true);
        }
    }
    let mut exported = Vec::new();
    let mut names = exports.into_iter().collect::<Vec<_>>();
    // By name, because a map's order is not one and a recipe's environment
    // should not depend on which way the hash fell.
    names.sort_by_cached_key(|(name, _)| name.as_bytes(&ev.session));
    for (name, is_exported) in names {
        let value = if is_exported {
            Some(std::ffi::OsString::from_vec(ev.eval_var(name)?.to_vec()))
        } else {
            None
        };
        exported.push((
            std::ffi::OsString::from_vec(name.as_bytes(&ev.session).to_vec()),
            value,
        ));
    }
    Ok(exported)
}
