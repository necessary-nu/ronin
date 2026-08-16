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
use crate::make::report::{
    abandoned, answered, complained_of, discard_intermediates, finished, refused_makefile,
};
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
        /// The recipes that graph still holds unexpanded, which the build the
        /// goals run has to be given.
        recipes: Option<Box<crate::make::recipe::PendingRecipes>>,
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
    /// The targets it lost over come with it, because every other target of
    /// the pass was still made and the read has to decide about each of them.
    Lost(Vec<Node>),
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
    /// Every Makefile is already what it will be, so this read stands.
    Settled(SettledMakefiles),
}

/// What one settled makefile update leaves the goals to work with.
///
/// Two lists rather than one, because the update has three answers and not two.
/// A Makefile it reached and won is prebuilt: its recipe must not run again and
/// the file it left is what everything reads. A Makefile it reached and lost —
/// which only a forgiven one can be, since a required one ends the run — is
/// neither: GNU Make leaves it `updated` with a failing status, so a goal that
/// asks for it is refused rather than served, and its recipe is spent. A
/// Makefile in neither list is one the update never had to touch.
#[derive(Default)]
struct SettledMakefiles {
    /// The ones the update reached and won.
    remade: Vec<Node>,
    /// The ones whose own recipe ran and lost, and was forgiven.
    unmade: Vec<Node>,
}

/// The graph and the scheduler attachments every pass over it shares.
struct Passes<'a, 'out, 'diagnostics> {
    graph: &'a mut BuildGraph,
    persistence: &'a mut Persistence,
    /// The recipes this graph left for the build to expand as it runs them.
    recipes: Option<&'a mut crate::make::recipe::PendingRecipes>,
    output: &'a mut Option<&'out mut dyn Write>,
    diagnostics: &'a mut Option<&'diagnostics mut dyn Write>,
    /// What the passes have narrated so far, which the result carries out.
    reported: &'a mut String,
    /// What each unread Makefile says if its own rule loses, which is the only
    /// occasion GNU Make ever says it.
    complaints: &'a [(Node, String)],
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
        if let Some(recipes) = self.recipes.as_deref_mut() {
            build = build.late_commands(recipes);
        }
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
                // A required `include` whose own rule lost is a read that did
                // not happen, and this is where GNU Make finally says so:
                // `child_error` prints the complaint it has been holding since
                // the open failed, one line ahead of naming the failure. A rule
                // that wins starts the read over instead, and the complaint is
                // never made at all.
                let complaints = self.complaints_for(targets, &outcome);
                return Pass::Finished(complained_of(
                    reported,
                    false,
                    &outcome,
                    self.silent,
                    &complaints,
                ));
            }
            // The recipe really ran and really lost, and Ninja's account of
            // that is honest narration to keep. What is dropped is the failure
            // itself: the read asked for this file with `-include`.
            let unmade = outcome.unmade().to_vec();
            self.narrate(&outcome);
            return Pass::Lost(unmade);
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

    /// The complaint the Makefile this pass stopped at has been holding.
    ///
    /// One, and it belongs to the goal rather than to the recipe. GNU Make's
    /// `show_goal_error` reads `goal_dep`, the goaldep `update_goal_chain` is
    /// working on, so what it names is the makefile that was being brought up to
    /// date and not whichever file in its subtree the failed child was making —
    /// `include gen.mk` whose `gen.mk: dep` loses over `dep` still complains
    /// about `gen.mk`. And `goal->error = 0` after printing makes it once.
    ///
    /// The one it stopped at is the first of the pass whose own recipe lost, and
    /// failing that the first the pass did not regenerate: the makefile update
    /// walks them in the order the read reached them and ends on the first it
    /// could not bring up to date, so everything before that one succeeded and
    /// everything after it was never attempted. The second reading is what
    /// catches a makefile whose own recipe never ran because something it needed
    /// lost first — the goal is still the goal.
    fn complaints_for(&self, targets: &[Node], outcome: &Outcome) -> Vec<String> {
        targets
            .iter()
            .find(|target| outcome.unmade().contains(target))
            .or_else(|| {
                targets
                    .iter()
                    .find(|target| !outcome.regenerated().contains(target))
            })
            .and_then(|stopped| {
                self.complaints
                    .iter()
                    .find(|(target, _)| target == stopped)
                    .map(|(_, complaint)| complaint.clone())
            })
            .into_iter()
            .collect()
    }

    fn abandon(&mut self, failure: Error, forgive: bool) -> Pass {
        if forgive {
            // Nothing ran, so no Makefile of the pass can have moved and there
            // is nothing to name as lost.
            return Pass::Lost(Vec::new());
        }
        Pass::Finished(abandoned(std::mem::take(self.reported), failure))
    }

    /// Bring every Makefile this read consulted up to date, whatever was asked
    /// of the goals, and say whether that changed one of them.
    ///
    /// A recipe that ran without changing the file it names leaves the read
    /// saying exactly what it said before, so GNU Make does not start over for
    /// it — that is the difference between a Makefile that is remade and one
    /// whose rule merely fired. A Makefile whose own update lost does not start
    /// it over either, however the recipe left the file: `any_remade` is raised
    /// for a successful update alone (main.c).
    ///
    /// GNU Make asks that of each Makefile and not of the batch. A pass where
    /// one `-include` is remade and another's rule loses is a pass in which
    /// the read must start over, for the one that won — the loser is forgiven
    /// and says nothing, rather than speaking for its neighbours. So the
    /// stamps are compared file by file, and a file whose own recipe lost is
    /// left out of the comparison however it left the disk behind it: a rule
    /// that writes its target and then exits non-zero starts nothing over.
    fn remake_makefiles(&mut self, makefiles: &Makefiles<'_>) -> Remaking {
        let mut restart = false;
        let mut settled = SettledMakefiles::default();
        for mut subset in makefiles.subsets(self.graph) {
            let paths = paths_of(self.graph, &subset.targets);
            let before = makefile_stamps(&paths, makefiles.directory);
            let unmade = match self.run(&subset.targets, subset.options, subset.forgive) {
                Pass::Ran(outcome) => {
                    self.narrate(&outcome);
                    Vec::new()
                }
                Pass::Lost(unmade) => unmade,
                Pass::Finished(result) => return Remaking::Finished(result),
                Pass::Answered(question) => {
                    let reported = std::mem::take(self.reported);
                    return Remaking::Finished(answered(reported, question));
                }
                Pass::Current => Vec::new(),
            };
            let after = makefile_stamps(&paths, makefiles.directory);
            restart |= subset
                .targets
                .iter()
                .zip(before.iter().zip(&after))
                .any(|(target, (before, after))| before != after && !unmade.contains(target));
            subset.targets.retain(|target| !unmade.contains(target));
            settled.remade.append(&mut subset.targets);
            settled.unmade.extend(unmade);
        }
        if restart {
            Remaking::Restart
        } else {
            Remaking::Settled(settled)
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
    /// Two independent questions, both GNU Make's own, and therefore four
    /// groups rather than three.
    ///
    /// Whether a Makefile's failure is forgiven is the read's answer: one
    /// reached only through `-include` is one whose absence the read said it
    /// could live with, and GNU Make carries that all the way into the rule —
    /// the recipe runs, it loses, and nothing is reported. That is
    /// `file->dontcare = ANY_SET (g->flags, RM_DONTCARE)` in `update_goal_chain`
    /// (remake.c), taken from the goaldep the `include` line made.
    ///
    /// Which switches it is made under is the command line's answer: a Makefile
    /// the invocation also named keeps `-t`, `-q` and `-n`, where every other
    /// one is made for real whatever was asked of the goals. That is the
    /// `file->cmd_target` arm two lines further down, and it restores those
    /// three flags and nothing else.
    ///
    /// So a Makefile that is both keeps the invocation's switches AND is
    /// forgiven. Reading `cmd_target` as an answer to the first question is
    /// what made an `-include`d file the command line named end the run over
    /// its own recipe, where GNU Make forgives it and then refuses the goal.
    fn subsets(&self, graph: &BuildGraph) -> Vec<Subset> {
        let named = |target: &Node| {
            let path = graph.path(*target);
            self.goals.iter().any(|goal| goal.as_slice() == path)
        };
        let group = |on_command_line: bool, forgive: bool| {
            self.remakes
                .iter()
                .copied()
                .filter(|target| {
                    named(target) == on_command_line && self.forgiven.contains(target) == forgive
                })
                .collect::<Vec<_>>()
        };
        // Every forgiven one is attempted, because GNU Make considers every
        // Makefile it read before it decides anything, and one it does not need
        // cannot stand in another's way.
        let forgivingly = |options: BuildOptions| BuildOptions {
            maxfail: usize::MAX,
            ..options
        };
        // Under `-q` a Makefile the command line named is asked about and left
        // alone: GNU Make records the question and carries on with the read
        // rather than stopping the way it would for a goal.
        let asked_about = |targets: Vec<Node>| {
            if self.invocation.questioning() {
                Vec::new()
            } else {
                targets
            }
        };
        vec![
            Subset {
                targets: group(false, false),
                options: remaking_options(self.options),
                forgive: false,
            },
            Subset {
                targets: group(false, true),
                options: forgivingly(remaking_options(self.options)),
                forgive: true,
            },
            Subset {
                targets: asked_about(group(true, false)),
                options: self.options.clone(),
                forgive: false,
            },
            Subset {
                targets: asked_about(group(true, true)),
                options: forgivingly(self.options.clone()),
                forgive: true,
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
    let mut loaded = loaded;
    // GNU Make refuses over a required Makefile nothing can make from inside
    // the update this function is: the Makefiles the read reached before that
    // one are brought up to date, and then the run ends. It ends on a restart
    // as well as on a settlement, because `complain()` gets there before
    // `main.c` reaches the test that would have sent the read around again.
    let refusal = loaded.take_refusal();
    let mut recipes = loaded.take_pending_recipes().map(Box::new);
    let remakes = loaded.remake_targets().to_vec();
    let forgiven = loaded.forgiven_remake_targets().to_vec();
    let unread = loaded.unread_remake_targets().to_vec();
    let complaints = loaded.take_remake_complaints();
    let staged = loaded.staged_targets();
    let boundaries = loaded.evaluation_boundaries().clone();
    let mut graph = loaded.graph;
    // Before anything is planned: a Makefile the read could not read is a file
    // that is not there for every question that follows, which is what makes its
    // own rule out of date and therefore what makes it run.
    graph.mark_makefiles_unread(&unread);
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
        recipes: recipes.as_deref_mut(),
        output,
        diagnostics,
        reported,
        complaints: &complaints,
        silent: invocation.given(Switch::Silent),
    };
    match passes.remake_makefiles(&makefiles) {
        Remaking::Finished(result) => {
            let _ = persistence.finish();
            return Ok(Settlement::Finished(result));
        }
        Remaking::Restart => {
            persistence.finish()?;
            return Ok(refusal.map_or(Settlement::Restart, |refusal| {
                let (complaint, error) = refusal.into_parts();
                Settlement::Finished(refused_makefile(std::mem::take(reported), complaint, error))
            }));
        }
        // Bringing a Makefile up to date is work the rest of the run has
        // already had done for it, and GNU Make does the whole of it inside
        // one process. Ronin reads again in place, so the goals reach the same
        // graph and have to be told the same thing.
        Remaking::Settled(settled) => {
            if let Some(refusal) = refusal {
                persistence.finish()?;
                let (complaint, error) = refusal.into_parts();
                return Ok(Settlement::Finished(refused_makefile(
                    std::mem::take(reported),
                    complaint,
                    error,
                )));
            }
            graph.mark_makefiles_settled(&settled.remade, &settled.unmade);
        }
    }

    let mut passes = Passes {
        graph: &mut graph,
        persistence: &mut persistence,
        recipes: recipes.as_deref_mut(),
        output,
        diagnostics,
        reported,
        complaints: &complaints,
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
            recipes,
        }),
        // Staged work is never forgiven, so `Lost` cannot arrive here; a
        // recursive child whose parent's prerequisites did not build has
        // nothing to be evaluated from.
        Pass::Current | Pass::Ran(_) | Pass::Lost(_) => {
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
