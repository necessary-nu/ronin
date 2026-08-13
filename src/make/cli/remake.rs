//! Bringing the Makefiles a read consulted up to date, between the reads.
//!
//! GNU Make treats every Makefile it read as a target, updates each one the way
//! it would any other, and starts the read over when that changed one of them.
//! The provisional graph the evaluator handed back knows how to build them, so
//! this is the ordinary Ninja scheduler over a subset of that graph, followed by
//! one question: did any Makefile move.
//!
//! What the goals were asked is not asked of the Makefiles. `-n` wants to know
//! what building the goals would do and `-q` whether they are up to date, and
//! neither question has an answer until the read is complete — so GNU Make
//! turns `-n`, `-q` and `-t` off while it brings the Makefiles up to date and
//! back on for the goals (`remake.c`, `update_goal_chain`). A Makefile the
//! command line also named is the exception it makes for itself: that one is a
//! goal too, and keeps the switches the invocation gave it.

use super::{Invocation, Switch};
use crate::Error;
use crate::build::BuildOptions;
use crate::cli::RunResult;
use crate::frontend::{Build, BuildGraph, Node, Outcome, Persistence};
use crate::make::EvaluationBoundary;
use crate::make::report::{abandoned, answered, discard_intermediates, finished};
use bstr::BString;
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
    /// What this invocation's own command line named, which the switches a
    /// Makefile assigned cannot tell us: `invocation` is reparsed from the
    /// evaluated `MAKEFLAGS` and carries switches alone.
    pub(super) goals: &'a [BString],
}

/// What one pass over a subset of the compiler inputs came to.
enum Pass {
    /// Nothing in the subset was out of date, so no command ran.
    Current,
    /// Commands ran and every one of them won.
    Ran(Outcome),
    /// The run is over, with this result.
    Finished(RunResult),
    /// The run is over having answered `-q` rather than built anything.
    Answered(Result<bool, Error>),
}

/// The graph and the scheduler attachments every pass over it shares.
struct Passes<'a, 'out, 'diagnostics> {
    graph: &'a mut BuildGraph,
    persistence: &'a mut Persistence,
    output: &'a mut Option<&'out mut dyn Write>,
    diagnostics: &'a mut Option<&'diagnostics mut dyn Write>,
    /// What the passes have narrated so far, which the result carries out.
    reported: &'a mut String,
    silent: bool,
}

impl Passes<'_, '_, '_> {
    /// Build one subset of the compiler inputs and say what that came to.
    ///
    /// Nothing to do is reported rather than run, because for the Makefiles it
    /// is the decisive answer: no command ran, so no Makefile can have changed
    /// under this read.
    fn run(&mut self, targets: &[Node], options: BuildOptions) -> Result<Pass, Error> {
        if targets.is_empty() {
            return Ok(Pass::Current);
        }
        let dryrun = options.dryrun;
        let mut build = Build::with_options(self.graph, self.persistence, options);
        if let Some(sink) = self.output.as_deref_mut() {
            build = build.output(sink);
        }
        if let Some(sink) = self.diagnostics.as_deref_mut() {
            build = build.diagnostics(sink);
        }
        let planned = match build.plan(targets) {
            Ok(planned) => planned,
            Err(failure) => return Ok(self.abandon(failure)),
        };
        if planned.already_up_to_date() {
            return Ok(Pass::Current);
        }
        let disposable = planned.disposable();
        let outcome = match planned.run() {
            Ok(outcome) => outcome,
            Err(failure) => return Ok(self.abandon(failure)),
        };
        discard_intermediates(&disposable, dryrun);
        if outcome.exit_code() != 0 {
            let reported = std::mem::take(self.reported);
            return Ok(Pass::Finished(finished(
                reported,
                false,
                &outcome,
                self.silent,
            )));
        }
        Ok(Pass::Ran(outcome))
    }

    /// Answer `-q` about one subset instead of building it.
    fn ask(&mut self, targets: &[Node], options: BuildOptions) -> Result<Pass, Error> {
        let mut build = Build::with_options(self.graph, self.persistence, options);
        if let Some(sink) = self.diagnostics.as_deref_mut() {
            build = build.diagnostics(sink);
        }
        let question = build
            .plan(targets)
            .map(|planned| planned.already_up_to_date());
        if matches!(question, Ok(true)) {
            return Ok(Pass::Current);
        }
        Ok(Pass::Answered(question))
    }

    fn abandon(&mut self, failure: Error) -> Pass {
        Pass::Finished(abandoned(std::mem::take(self.reported), failure))
    }

    /// Bring every Makefile this read consulted up to date, whatever was asked
    /// of the goals.
    ///
    /// Two subsets, because GNU Make asks them different questions: the ones
    /// the command line named are goals as well and keep the invocation's own
    /// switches, and every other one is made for real.
    fn remake_makefiles(
        &mut self,
        remakes: &[Node],
        invocation: &Invocation,
        options: &BuildOptions,
        goals: &[BString],
    ) -> Result<Option<RunResult>, Error> {
        let asked_about = named_as_goals(self.graph, remakes, goals);
        let made = remakes
            .iter()
            .copied()
            .filter(|target| !asked_about.contains(target))
            .collect::<Vec<_>>();
        // Under `-q` a Makefile the command line named is asked about and left
        // alone: GNU Make records the question and carries on with the read
        // rather than stopping the way it would for a goal.
        let asked_about = if invocation.questioning() {
            Vec::new()
        } else {
            asked_about
        };
        for (targets, options) in [
            (made, remaking_options(options)),
            (asked_about, options.clone()),
        ] {
            match self.run(&targets, options)? {
                Pass::Ran(outcome) => self.narrate(&outcome),
                Pass::Finished(result) => return Ok(Some(result)),
                Pass::Answered(question) => {
                    let reported = std::mem::take(self.reported);
                    return Ok(Some(answered(reported, question)));
                }
                Pass::Current => {}
            }
        }
        Ok(None)
    }

    /// Build the work staged so a recursive unit can be evaluated at all, under
    /// the invocation's own switches.
    ///
    /// GNU Make has no such phase, so there is nothing here it would have
    /// disabled: `-n` describes this work and does none of it, which leaves the
    /// child that was waiting for it uncompilable and the run over.
    fn stage(
        &mut self,
        staged: &[Node],
        invocation: &Invocation,
        options: BuildOptions,
    ) -> Result<Pass, Error> {
        if invocation.questioning() {
            return self.ask(staged, options);
        }
        let dryrun = options.dryrun;
        let pass = self.run(staged, options)?;
        let Pass::Ran(outcome) = pass else {
            return Ok(pass);
        };
        if dryrun {
            let reported = std::mem::take(self.reported);
            return Ok(Pass::Finished(finished(
                reported,
                false,
                &outcome,
                self.silent,
            )));
        }
        self.narrate(&outcome);
        Ok(Pass::Ran(outcome))
    }

    fn narrate(&mut self, outcome: &Outcome) {
        self.reported
            .push_str(&String::from_utf8_lossy(outcome.output()));
    }
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

/// The switches a Makefile is brought up to date under.
///
/// Not the ones the goals were asked about: `-n` would leave the Makefile
/// described and unmade, and the read would then have to guess what it said.
fn remaking_options(options: &BuildOptions) -> BuildOptions {
    BuildOptions {
        dryrun: false,
        verbose: false,
        ..options.clone()
    }
}

/// The Makefiles this invocation also named on its own command line.
///
/// GNU Make restores `-n`, `-q` and `-t` for these while it remakes them: a
/// Makefile the command line asked about is a goal as well, and what the
/// invocation wanted to know about a goal is exactly what those switches ask.
fn named_as_goals(graph: &BuildGraph, remakes: &[Node], goals: &[BString]) -> Vec<Node> {
    remakes
        .iter()
        .copied()
        .filter(|target| {
            let path = graph.path(*target);
            goals.iter().any(|goal| goal.as_slice() == path)
        })
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
        goals,
    } = request;
    let remakes = loaded.remake_targets().to_vec();
    let staged = loaded.staged_targets();
    let boundaries = loaded.evaluation_boundaries().clone();
    let mut graph = loaded.graph;
    let makefiles = remakes
        .iter()
        .map(|node| graph.path(*node).to_vec())
        .collect::<Vec<_>>();
    let (mut persistence, warning) = Persistence::open(&mut graph, directory)?;
    reported.push_str(warning.as_deref().unwrap_or_default());
    let before = makefile_stamps(&makefiles, directory);

    let mut passes = Passes {
        graph: &mut graph,
        persistence: &mut persistence,
        output,
        diagnostics,
        reported,
        silent: invocation.given(Switch::Silent),
    };
    let remade = passes.remake_makefiles(&remakes, invocation, &options, goals)?;
    if let Some(result) = remade {
        let _ = persistence.finish();
        return Ok(Settlement::Finished(result));
    }
    // A recipe that ran without changing the file it names leaves the read
    // saying exactly what it said before, so GNU Make does not start over for
    // it — that is the difference between a Makefile that is remade and one
    // whose rule merely fired.
    if makefile_stamps(&makefiles, directory) != before {
        persistence.finish()?;
        return Ok(Settlement::Restart);
    }

    let mut passes = Passes {
        graph: &mut graph,
        persistence: &mut persistence,
        output,
        diagnostics,
        reported,
        silent: invocation.given(Switch::Silent),
    };
    match passes.stage(&staged, invocation, options)? {
        Pass::Finished(result) => {
            let _ = persistence.finish();
            Ok(Settlement::Finished(result))
        }
        Pass::Answered(question) => {
            let flushed = persistence.finish();
            let question = question.and_then(|up_to_date| flushed.map(|()| up_to_date));
            Ok(Settlement::Finished(answered(
                std::mem::take(reported),
                question,
            )))
        }
        Pass::Current if boundaries.is_empty() => Ok(Settlement::Settled {
            graph: Box::new(graph),
            persistence,
        }),
        Pass::Current | Pass::Ran(_) => {
            settled_boundaries.extend(boundaries);
            persistence.finish()?;
            Ok(Settlement::Restart)
        }
    }
}
