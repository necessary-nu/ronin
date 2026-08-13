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
    /// A command lost, and the read said it did not need what it was making.
    Lost,
    /// The run is over, with this result.
    Finished(RunResult),
    /// The run is over having answered `-q` rather than built anything.
    Answered(Result<bool, Error>),
}

/// The Makefiles one read consulted, and what deciding about them takes.
struct Makefiles<'a> {
    /// Every Makefile a rule says how to remake, in the order the read reached
    /// them.
    remakes: &'a [Node],
    /// The ones among those the read said it did not need.
    forgiven: &'a [Node],
    invocation: &'a Invocation,
    options: &'a BuildOptions,
    /// What this invocation's own command line named, which the switches a
    /// Makefile assigned cannot tell us: the reparsed `MAKEFLAGS` invocation
    /// carries switches alone.
    goals: &'a [BString],
    directory: &'a Path,
}

/// One subset of the Makefiles, and how it is brought up to date.
struct Subset {
    targets: Vec<Node>,
    options: BuildOptions,
    /// Whether a recipe that lost here ends the run.
    forgive: bool,
}

/// What bringing the Makefiles up to date came to.
enum Remaking {
    /// The run is over, with this result.
    Finished(RunResult),
    /// A Makefile moved, so the read has to happen again on the new text.
    Restart,
    /// Every Makefile is already what it will be, so this read stands. These
    /// are the ones the update reached and won, which the goals are not to
    /// consider again.
    Settled(Vec<Node>),
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
    fn run(&mut self, targets: &[Node], options: BuildOptions, forgive: bool) -> Pass {
        if targets.is_empty() {
            return Pass::Current;
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
            Err(failure) => return self.abandon(failure, forgive),
        };
        if planned.already_up_to_date() {
            return Pass::Current;
        }
        let disposable = planned.disposable();
        let outcome = match planned.run() {
            Ok(outcome) => outcome,
            Err(failure) => return self.abandon(failure, forgive),
        };
        discard_intermediates(&disposable, dryrun);
        if outcome.exit_code() != 0 {
            if !forgive {
                let reported = std::mem::take(self.reported);
                return Pass::Finished(finished(reported, false, &outcome, self.silent));
            }
            // The recipe really ran and really lost, and Ninja's account of
            // that is honest narration to keep. What is dropped is the failure
            // itself: the read asked for this file with `-include`.
            self.narrate(&outcome);
            return Pass::Lost;
        }
        Pass::Ran(outcome)
    }

    /// Answer `-q` about one subset instead of building it.
    fn ask(&mut self, targets: &[Node], options: BuildOptions) -> Pass {
        let mut build = Build::with_options(self.graph, self.persistence, options);
        if let Some(sink) = self.diagnostics.as_deref_mut() {
            build = build.diagnostics(sink);
        }
        let question = build
            .plan(targets)
            .map(|planned| planned.already_up_to_date());
        if matches!(question, Ok(true)) {
            return Pass::Current;
        }
        Pass::Answered(question)
    }

    fn abandon(&mut self, failure: Error, forgive: bool) -> Pass {
        if forgive {
            return Pass::Lost;
        }
        Pass::Finished(abandoned(std::mem::take(self.reported), failure))
    }

    /// Bring every Makefile this read consulted up to date, whatever was asked
    /// of the goals, and say whether that changed one of them.
    ///
    /// A recipe that ran without changing the file it names leaves the read
    /// saying exactly what it said before, so GNU Make does not start over for
    /// it — that is the difference between a Makefile that is remade and one
    /// whose rule merely fired. A Makefile whose update lost does not start it
    /// over either, however the recipe left the file: `any_remade` is raised
    /// for a successful update alone (main.c), which is why each subset is
    /// stamped around its own pass rather than all of them around the phase.
    fn remake_makefiles(&mut self, makefiles: &Makefiles<'_>) -> Remaking {
        let mut restart = false;
        let mut remade = Vec::new();
        for mut subset in makefiles.subsets(self.graph) {
            let paths = paths_of(self.graph, &subset.targets);
            let before = makefile_stamps(&paths, makefiles.directory);
            match self.run(&subset.targets, subset.options, subset.forgive) {
                Pass::Ran(outcome) => self.narrate(&outcome),
                Pass::Lost => continue,
                Pass::Finished(result) => return Remaking::Finished(result),
                Pass::Answered(question) => {
                    let reported = std::mem::take(self.reported);
                    return Remaking::Finished(answered(reported, question));
                }
                Pass::Current => {}
            }
            restart |= makefile_stamps(&paths, makefiles.directory) != before;
            remade.append(&mut subset.targets);
        }
        if restart {
            Remaking::Restart
        } else {
            Remaking::Settled(remade)
        }
    }

    /// Build the work staged so a recursive unit can be evaluated at all, under
    /// the invocation's own switches.
    ///
    /// GNU Make has no such phase, so there is nothing here it would have
    /// disabled: `-n` describes this work and does none of it, which leaves the
    /// child that was waiting for it uncompilable and the run over.
    fn stage(&mut self, staged: &[Node], invocation: &Invocation, options: BuildOptions) -> Pass {
        if invocation.questioning() {
            return self.ask(staged, options);
        }
        let dryrun = options.dryrun;
        let pass = self.run(staged, options, false);
        let Pass::Ran(outcome) = pass else {
            return pass;
        };
        if dryrun {
            let reported = std::mem::take(self.reported);
            return Pass::Finished(finished(reported, false, &outcome, self.silent));
        }
        self.narrate(&outcome);
        Pass::Ran(outcome)
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

/// Where the graph says each of these targets lives.
fn paths_of(graph: &BuildGraph, targets: &[Node]) -> Vec<Vec<u8>> {
    targets
        .iter()
        .map(|target| graph.path(*target).to_vec())
        .collect()
}

impl Makefiles<'_> {
    /// The Makefiles in the groups GNU Make treats differently, in the order it
    /// reaches them.
    ///
    /// Three, and the differences are all GNU Make's own. A Makefile the
    /// command line also named is a goal as well, and keeps the switches the
    /// invocation gave it (`file->cmd_target` in remake.c). A Makefile reached
    /// only through `-include` is one whose absence the read said it could
    /// live with, and GNU Make carries that all the way into the rule: the
    /// recipe runs, it loses, and nothing is reported (`RM_DONTCARE`). Every
    /// other one is made for real and its failure ends the run.
    fn subsets(&self, graph: &BuildGraph) -> Vec<Subset> {
        let asked_about = self
            .remakes
            .iter()
            .copied()
            .filter(|target| {
                let path = graph.path(*target);
                self.goals.iter().any(|goal| goal.as_slice() == path)
            })
            .collect::<Vec<_>>();
        let forgiven = self
            .forgiven
            .iter()
            .copied()
            .filter(|target| !asked_about.contains(target))
            .collect::<Vec<_>>();
        let made = self
            .remakes
            .iter()
            .copied()
            .filter(|target| !asked_about.contains(target) && !forgiven.contains(target))
            .collect::<Vec<_>>();
        vec![
            Subset {
                targets: made,
                options: remaking_options(self.options),
                forgive: false,
            },
            Subset {
                targets: forgiven,
                // Every one of them is attempted, because GNU Make considers
                // every Makefile it read before it decides anything, and one
                // it does not need cannot stand in another's way.
                options: BuildOptions {
                    maxfail: usize::MAX,
                    ..remaking_options(self.options)
                },
                forgive: true,
            },
            Subset {
                // Under `-q` a Makefile the command line named is asked about
                // and left alone: GNU Make records the question and carries on
                // with the read rather than stopping the way it would for a
                // goal.
                targets: if self.invocation.questioning() {
                    Vec::new()
                } else {
                    asked_about
                },
                options: self.options.clone(),
                forgive: false,
            },
        ]
    }
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
    let forgiven = loaded.forgiven_remake_targets().to_vec();
    let staged = loaded.staged_targets();
    let boundaries = loaded.evaluation_boundaries().clone();
    let mut graph = loaded.graph;
    let (mut persistence, warning) = Persistence::open(&mut graph, directory)?;
    reported.push_str(warning.as_deref().unwrap_or_default());

    let makefiles = Makefiles {
        remakes: &remakes,
        forgiven: &forgiven,
        invocation,
        options: &options,
        goals,
        directory,
    };
    let mut passes = Passes {
        graph: &mut graph,
        persistence: &mut persistence,
        output,
        diagnostics,
        reported,
        silent: invocation.given(Switch::Silent),
    };
    match passes.remake_makefiles(&makefiles) {
        Remaking::Finished(result) => {
            let _ = persistence.finish();
            return Ok(Settlement::Finished(result));
        }
        Remaking::Restart => {
            persistence.finish()?;
            return Ok(Settlement::Restart);
        }
        // Bringing a Makefile up to date is work the rest of the run has
        // already had done for it, and GNU Make does the whole of it inside
        // one process. Ronin reads again in place, so the goals reach the same
        // graph and have to be told the same thing.
        Remaking::Settled(remade) => graph.mark_makefiles_remade(&remade),
    }

    let mut passes = Passes {
        graph: &mut graph,
        persistence: &mut persistence,
        output,
        diagnostics,
        reported,
        silent: invocation.given(Switch::Silent),
    };
    match passes.stage(&staged, invocation, options) {
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
        // Staged work is never forgiven, so `Lost` cannot arrive here; a
        // recursive child whose parent's prerequisites did not build has
        // nothing to be evaluated from.
        Pass::Current | Pass::Ran(_) | Pass::Lost => {
            settled_boundaries.extend(boundaries);
            persistence.finish()?;
            Ok(Settlement::Restart)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Makefiles, remaking_options};
    use crate::build::BuildOptions;
    use crate::frontend::{BuildGraph, Node};
    use crate::make::cli::interface_tests::parsed;
    use crate::util::BString;
    use std::path::Path;

    fn nodes(graph: &mut BuildGraph, paths: &[&str]) -> Vec<Node> {
        paths
            .iter()
            .map(|path| graph.node(path.as_bytes()).expect("a node"))
            .collect()
    }

    fn named(graph: &BuildGraph, targets: &[Node]) -> Vec<String> {
        targets
            .iter()
            .map(|target| String::from_utf8_lossy(graph.path(*target)).into_owned())
            .collect()
    }

    /// A Makefile is made for real however the goals were asked about, and one
    /// the command line also named keeps what the invocation gave it.
    #[test]
    fn dry_run_describes_goals_not_makefiles() {
        let mut graph = BuildGraph::new();
        let remakes = nodes(&mut graph, &["gen.mk", "asked.mk"]);
        let invocation = parsed(&["make", "-n", "asked.mk"]);
        let options = BuildOptions {
            dryrun: true,
            verbose: true,
            ..BuildOptions::default()
        };
        let makefiles = Makefiles {
            remakes: &remakes,
            forgiven: &[],
            invocation: &invocation,
            options: &options,
            goals: &[BString::from("asked.mk")],
            directory: Path::new("."),
        };

        let subsets = makefiles.subsets(&graph);
        assert_eq!(named(&graph, &subsets[0].targets), vec!["gen.mk"]);
        assert!(!subsets[0].options.dryrun);
        assert_eq!(named(&graph, &subsets[2].targets), vec!["asked.mk"]);
        assert!(subsets[2].options.dryrun);
        assert!(subsets[1].targets.is_empty());
    }

    /// `-q` asks about a Makefile the command line named rather than making it,
    /// and leaves every other one to be made.
    #[test]
    fn question_leaves_its_named_makefile_alone() {
        let mut graph = BuildGraph::new();
        let remakes = nodes(&mut graph, &["gen.mk", "asked.mk"]);
        let invocation = parsed(&["make", "-q", "asked.mk"]);
        let options = BuildOptions::default();
        let makefiles = Makefiles {
            remakes: &remakes,
            forgiven: &[],
            invocation: &invocation,
            options: &options,
            goals: &[BString::from("asked.mk")],
            directory: Path::new("."),
        };

        let subsets = makefiles.subsets(&graph);
        assert_eq!(named(&graph, &subsets[0].targets), vec!["gen.mk"]);
        assert!(subsets[2].targets.is_empty());
    }

    /// An optional include is its own subset, attempted to the end and forgiven.
    #[test]
    fn optional_include_is_forgiven_and_attempted() {
        let mut graph = BuildGraph::new();
        let remakes = nodes(&mut graph, &["needed.mk", "spare.mk"]);
        let invocation = parsed(&["make"]);
        let options = BuildOptions::default();
        let makefiles = Makefiles {
            remakes: &remakes,
            forgiven: &remakes[1..],
            invocation: &invocation,
            options: &options,
            goals: &[],
            directory: Path::new("."),
        };

        let subsets = makefiles.subsets(&graph);
        assert_eq!(named(&graph, &subsets[0].targets), vec!["needed.mk"]);
        assert!(!subsets[0].forgive);
        assert_eq!(named(&graph, &subsets[1].targets), vec!["spare.mk"]);
        assert!(subsets[1].forgive);
        assert_eq!(subsets[1].options.maxfail, usize::MAX);
    }

    /// Remaking a Makefile is a build, so nothing about it is described rather
    /// than done, and everything else the invocation settled is kept.
    #[test]
    fn remaking_keeps_settings_but_not_description() {
        let options = BuildOptions {
            dryrun: true,
            verbose: true,
            maxfail: 7,
            quiet: true,
            ..BuildOptions::default()
        };
        let remaking = remaking_options(&options);
        assert!(!remaking.dryrun && !remaking.verbose);
        assert_eq!(remaking.maxfail, 7);
        assert!(remaking.quiet);
    }
}
