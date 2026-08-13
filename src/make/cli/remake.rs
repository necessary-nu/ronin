//! Bringing the Makefiles a read consulted up to date, between the reads.
//!
//! GNU Make treats every Makefile it read as a target, updates each one the way
//! it would any other, and starts the read over when that changed one of them.
//! The provisional graph the evaluator handed back knows how to build them, so
//! this is the ordinary Ninja scheduler over a subset of that graph, followed by
//! one question: did any Makefile move.

use super::{Invocation, Switch};
use crate::Error;
use crate::build::BuildOptions;
use crate::cli::RunResult;
use crate::frontend::{Build, BuildGraph, Persistence};
use crate::make::EvaluationBoundary;
use crate::make::report::{abandoned, answered, discard_intermediates, finished};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

/// What building this pass's compiler inputs decided about the next one.
pub(super) enum Settlement {
    /// The run is over — a failure, a `-n` that has said what it would do, or
    /// an answered question.
    Finished(RunResult),
    /// Something the read depends on changed, so the Makefile has to be read
    /// again on the new text.
    Restart,
    /// Nothing changed. This graph is the compilation the goals build from.
    Settled {
        graph: Box<BuildGraph>,
        persistence: Persistence,
    },
}

pub(super) struct CompilerInputBuild<'a> {
    pub(super) loaded: crate::make::Loaded,
    pub(super) invocation: &'a Invocation,
    pub(super) options: BuildOptions,
    pub(super) directory: &'a Path,
}

/// When the file was last written, or nothing when it is not there.
///
/// Absence is a state a Makefile can be in and be brought out of, so it is one
/// of the answers rather than a failure to have one.
fn written_at(directory: &Path, path: &[u8]) -> Option<std::time::SystemTime> {
    use std::os::unix::ffi::OsStrExt;
    let path = directory.join(std::ffi::OsStr::from_bytes(path));
    std::fs::metadata(path).ok()?.modified().ok()
}

/// How the Makefiles this read consulted stand right now.
fn makefile_stamps(paths: &[Vec<u8>], directory: &Path) -> Vec<Option<std::time::SystemTime>> {
    paths
        .iter()
        .map(|path| written_at(directory, path))
        .collect()
}

pub(super) fn build_compiler_inputs(
    request: CompilerInputBuild<'_>,
    reported: &mut String,
    output: &mut Option<&mut dyn Write>,
    diagnostics: &mut Option<&mut dyn Write>,
    settled_boundaries: &mut HashSet<EvaluationBoundary>,
) -> Result<Settlement, Error> {
    let CompilerInputBuild {
        loaded,
        invocation,
        options,
        directory,
    } = request;
    let targets = loaded.regeneration_targets().to_vec();
    let remakes = loaded.remake_targets().to_vec();
    let boundaries = loaded.evaluation_boundaries().clone();
    let mut graph = loaded.graph;
    let makefiles = remakes
        .iter()
        .map(|node| graph.path(*node).to_vec())
        .collect::<Vec<_>>();
    let (mut persistence, warning) = Persistence::open(&mut graph, directory)?;
    reported.push_str(warning.as_deref().unwrap_or_default());
    let before = makefile_stamps(&makefiles, directory);
    let mut build = Build::with_options(&mut graph, &mut persistence, options);
    if let Some(sink) = output.as_deref_mut() {
        build = build.output(sink);
    }
    if let Some(sink) = diagnostics.as_deref_mut() {
        build = build.diagnostics(sink);
    }
    let planned = build.plan(&targets);
    // Nothing to do is the common answer, and it is decisive: no command runs,
    // so no Makefile can have changed under this read. A recursive unit still
    // waiting to be composed is the exception — the graph is not the whole
    // Makefile yet, so the read has to happen again to finish it.
    if matches!(&planned, Ok(planned) if planned.already_up_to_date()) {
        drop(planned);
        if boundaries.is_empty() {
            return Ok(Settlement::Settled {
                graph: Box::new(graph),
                persistence,
            });
        }
        settled_boundaries.extend(boundaries);
        persistence.finish()?;
        return Ok(Settlement::Restart);
    }
    if invocation.questioning() {
        let question = planned.map(|planned| planned.already_up_to_date());
        let flushed = persistence.finish();
        let question = question.and_then(|up_to_date| flushed.map(|()| up_to_date));
        return Ok(Settlement::Finished(answered(
            std::mem::take(reported),
            question,
        )));
    }
    let planned = match planned {
        Ok(planned) => planned,
        Err(failure) => {
            let _ = persistence.finish();
            return Ok(Settlement::Finished(abandoned(
                std::mem::take(reported),
                failure,
            )));
        }
    };
    let disposable = planned.disposable();
    let outcome = planned.run();
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(failure) => {
            let _ = persistence.finish();
            return Ok(Settlement::Finished(abandoned(
                std::mem::take(reported),
                failure,
            )));
        }
    };
    discard_intermediates(&disposable, invocation.given(Switch::DryRun));
    if outcome.exit_code() != 0 || invocation.given(Switch::DryRun) {
        persistence.finish()?;
        return Ok(Settlement::Finished(finished(
            std::mem::take(reported),
            false,
            &outcome,
            invocation.given(Switch::Silent),
        )));
    }
    let composing = !boundaries.is_empty();
    settled_boundaries.extend(boundaries);
    reported.push_str(&String::from_utf8_lossy(outcome.output()));
    // A recipe that ran without changing the file it names leaves the read
    // saying exactly what it said before, so GNU Make does not start over for
    // it — that is the difference between a Makefile that is remade and one
    // whose rule merely fired.
    if !composing && makefile_stamps(&makefiles, directory) == before {
        return Ok(Settlement::Settled {
            graph: Box::new(graph),
            persistence,
        });
    }
    persistence.finish()?;
    Ok(Settlement::Restart)
}
