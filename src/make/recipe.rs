//! Recipes expanded when their edge is about to run.
//!
//! GNU Make expands a recipe in `new_job`, immediately before running it and
//! only for a target it decided to remake. Everything the expansion does
//! happens there: a `$(shell)` runs then, an `$(info)` prints then, an
//! `$(error)` stops the build then, and a `$(wildcard)` sees the files every
//! earlier recipe left behind. A target that turns out to be up to date has
//! none of it happen at all.
//!
//! kati compiles a whole graph before anything runs, which is what a manifest
//! needs and what this front end kept while the graph was the only product.
//! Now that the same process runs the graph, the recipes it can hold
//! unexpanded stay unexpanded — the compiler still reads the ones whose text
//! shapes the graph — and the engine asks for one as it launches its edge.

use super::layout::Script;
use super::sink::{CommandLayout, SettledSteps};
use crate::build::{LateBinding, LateCommand, LateCommands, LateStep};
use crate::graph::EdgeId;
use crate::htab::RapidHashMap;
use crate::util::BString;
use kati::build_sink::DeferredRecipeId;
use kati::eval::Evaluator;
use kati::ninja::DeferredRecipes as KatiRecipes;
use std::path::{Path, PathBuf};

/// One compilation unit's unexpanded recipes, and everything expanding one of
/// them needs that the recipe itself does not carry.
struct RecipeUnit {
    /// The evaluation session that read this unit's Makefile.
    ///
    /// A recipe is expanded against the variables that session holds, which is
    /// the whole reason the session outlives compilation: an expansion against
    /// anything else would be a different expansion. A recursive `$(MAKE)`
    /// child has a session of its own, read from its own Makefile with its own
    /// `MAKEFLAGS` and its own exports, so it is retained beside the root's
    /// rather than folded into it.
    // [spec:ronin:req:make.no-ambient-state]
    session: Evaluator,
    recipes: KatiRecipes,
    layout: CommandLayout,
    /// Where this unit's Makefile was read, and so where its recipes expand.
    ///
    /// Absolute, because `-C` is canonicalised when the child invocation is
    /// resolved. The root's is the process's own directory and costs nothing;
    /// a child's is entered for the length of the expansion.
    directory: PathBuf,
}

/// Every unexpanded recipe a build may still have to expand, and the sessions
/// that own them.
///
/// Held by the destination for as long as the build may still start one of
/// them. One collection covers the whole compilation rather than one per unit,
/// because the graph is one graph: an edge is looked up by its own identity
/// and finds the unit whose Makefile wrote it.
pub(crate) struct PendingRecipes {
    /// Where the sessions below write what they raise while expanding.
    ///
    /// The same descriptor the compilation read through, because expanding a
    /// recipe is the last of the read: a `$(warning)` in recipe position is
    /// raised here, by the session that owns the variables it names.
    diagnostics: std::sync::Arc<kati::diagnostics::Diagnostics>,
    units: Vec<RecipeUnit>,
    edges: RapidHashMap<EdgeId, (usize, DeferredRecipeId)>,
    /// Edges whose recipe the compiler had to read for itself, with the
    /// launches that recipe's lines became.
    ///
    /// Four shapes force a recipe to be read while the graph is built — a
    /// recursive `$(MAKE)`, a depfile, a grouped double-colon action, and a
    /// `$?` the scheduler binds — and none of them is a reason for the recipe
    /// to reach ONE shell. The text was settled early; the launches were
    /// settled with it, and they are carried here so the edge still runs a
    /// process per command line. There is no expansion to do and no session to
    /// do it in, which is why this is a map of its own rather than a second
    /// kind of unit.
    settled: RapidHashMap<EdgeId, SettledSteps>,
    /// Whether the build now running is the makefile update.
    ///
    /// GNU Make recomputes `MAKEFLAGS` without `-n`, `-t` and `-q` for the
    /// length of that phase and computes it again with them for the goals
    /// (`define_makeflags`, main.c), so a `$(MAKE)` inside the recipe that
    /// remakes a Makefile starts a child that is not pretending either. It is
    /// the same reason the update itself is not pretended: a Makefile only
    /// half-made is one whose contents the read would then have to guess, and
    /// a child that pretends on the update's behalf leaves exactly that.
    ///
    /// Held here rather than read off the build, because the phase is the
    /// front end's own idea — the engine schedules a graph and has no notion
    /// of a makefile update — and this is the front end's side of the launch.
    remaking_makefiles: bool,
}

impl PendingRecipes {
    pub(crate) fn new(diagnostics: std::sync::Arc<kati::diagnostics::Diagnostics>) -> Self {
        Self {
            diagnostics,
            units: Vec::new(),
            edges: RapidHashMap::default(),
            settled: RapidHashMap::default(),
            remaking_makefiles: false,
        }
    }

    /// Say whether the builds that follow are the makefile update.
    ///
    /// Said by the front end at each phase rather than derived, because the
    /// same graph is built over twice and only the caller knows which time it
    /// is looking at.
    pub(crate) const fn remaking_makefiles(&mut self, remaking: bool) {
        self.remaking_makefiles = remaking;
    }

    /// Retain the launches of one unit's recipes that were read while the
    /// graph was built.
    pub(crate) fn admit_settled(&mut self, edges: Vec<(crate::frontend::Edge, SettledSteps)>) {
        self.settled
            .extend(edges.into_iter().map(|(edge, steps)| (edge.id(), steps)));
    }

    /// Retain one unit's session and the recipes it left unexpanded, keyed by
    /// the edge that runs each one.
    pub(crate) fn admit(
        &mut self,
        session: Evaluator,
        recipes: KatiRecipes,
        layout: CommandLayout,
        directory: PathBuf,
        edges: &[(crate::frontend::Edge, DeferredRecipeId)],
    ) {
        let unit = self.units.len();
        self.units.push(RecipeUnit {
            session,
            recipes,
            layout,
            directory,
        });
        self.edges.extend(
            edges
                .iter()
                .map(|(edge, recipe)| (edge.id(), (unit, *recipe))),
        );
    }

    /// Whether this has anything to say about any edge at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.settled.is_empty()
    }
}

/// Run `expand` with `directory` as the process directory.
///
/// GNU Make's recursive child runs in its own `-C` directory, so a
/// `$(wildcard)` or a `$(shell)` in one of its recipes reads that directory
/// rather than the parent's. Ronin runs the whole graph from the root with one
/// process directory that every parallel spawn observes at the moment it
/// forks, which is why `make-process-directory-isolation` ruled out moving the
/// process during execution — and why this uses the very guard that node
/// built. The directory is entered for the length of one expansion, under the
/// exclusive lock a spawn takes the shared side of, so no spawn can observe
/// it; the guard is released before the command this produces is launched.
///
/// A unit already sitting in the process directory pays none of it. That is
/// the root always, and a child invoked without `-C` as well.
fn expanded_in<T>(
    directory: &Path,
    expand: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let entered = std::env::current_dir()
        .map_err(|error| format!("reading the current directory to expand a recipe: {error}"))?;
    if entered == directory {
        return expand();
    }
    let _process = super::compilation_directory_guard();
    std::env::set_current_dir(directory).map_err(|error| {
        format!(
            "entering '{}' to expand a recipe: {error}",
            directory.display()
        )
    })?;
    let expanded = expand();
    // Restored whatever the expansion said, because leaving the process
    // somewhere else would misdirect every command after it, and reported only
    // when there was nothing worse to report.
    match (expanded, std::env::set_current_dir(&entered)) {
        (expanded, Ok(())) => expanded,
        (Err(failure), Err(_)) => Err(failure),
        (Ok(_), Err(error)) => Err(format!(
            "returning to '{}' after expanding a recipe: {error}",
            entered.display()
        )),
    }
}

impl LateCommands for PendingRecipes {
    fn raised(&mut self) -> Vec<u8> {
        self.diagnostics.take()
    }

    fn command(
        &mut self,
        edge: EdgeId,
        output: &[u8],
        trigger: &[u8],
    ) -> Result<LateBinding, String> {
        let Some((unit, recipe)) = self.edges.get(&edge).copied() else {
            // Nothing to expand, but the edge may still be a recipe whose
            // lines are several processes: the compiler read it early and
            // handed over the launches with the text.
            // Cloned rather than taken: a graph is built over more than
            // once — the makefile remake pass, then the goals — and `-q` asks
            // this same question without running anything.
            return Ok(match self.settled.get(&edge) {
                Some(settled) => match settled.during(self.remaking_makefiles) {
                    [] => LateBinding::Settled,
                    steps => LateBinding::Steps(steps.to_vec()),
                },
                None => LateBinding::Settled,
            });
        };
        let RecipeUnit {
            session,
            recipes,
            layout,
            directory,
        } = &mut self.units[unit];
        let expanded = expanded_in(directory, || {
            recipes
                .expand(session, recipe, trigger)
                .map_err(|failure| super::report::diagnostic_body(&failure))
        })?;
        let Some(expanded) = expanded else {
            return Ok(LateBinding::Settled);
        };
        // Every line of the recipe expanded to nothing. GNU Make walks past
        // them all in `job_next_command`, starts no shell, and counts the
        // target as remade — so the edge is complete without a command, and
        // the run that reaches it says the target is up to date.
        if expanded.runs_nothing {
            return Ok(LateBinding::Nothing);
        }
        // The makefile update hands a recipe's child a `MAKEFLAGS` the
        // pretending switches have been taken out of, which is how GNU Make's
        // own update reaches a `$(MAKE)` a recipe line holds. Composed here
        // rather than when the graph was built, because whether an edge
        // belongs to the update is a fact about the pass and not about the
        // edge: the goals build the same graph with the switches back on.
        let mut environment = expanded.recipe_environment.clone();
        if self.remaking_makefiles {
            environment.extend(layout.while_remaking_makefiles(&environment));
        }
        let launched = layout.launch(
            &expanded.shell,
            &expanded.shell_flags,
            &expanded.script,
            output,
            &environment,
        );
        // GNU Make runs each command line of a recipe as its own process, so
        // that is what the edge is handed. The assembled script stays as the
        // one name the recipe has — a progress line, a log entry and a `-n`
        // all want the whole of it — and as the fallback for a recipe holding
        // a line too long to be an argument, which cannot be launched on its
        // own because the response file it would need is named per edge.
        //
        // That fallback is still a launch rather than a command line: the
        // recipe reaches one shell instead of several, and which shell that is
        // does not change with it.
        let steps = if CommandLayout::launches_line_by_line(&expanded.steps) {
            expanded
                .steps
                .iter()
                .map(|step| LateStep {
                    launch: layout.launch_step(step, &environment),
                    ignore_errors: step.ignore_error,
                    runs_while_pretending: step.recursive_line,
                })
                .collect()
        } else {
            layout
                .launch_script(
                    &expanded.shell,
                    &expanded.shell_flags,
                    match &launched.response_file {
                        Some((path, _)) => Script::File(path),
                        None => Script::Argument(&expanded.script),
                    },
                    &environment,
                )
                .map(|launch| LateStep {
                    launch,
                    // The recipe's own, which is what the assembled script
                    // could be believed about: kati sets it only where every
                    // line of the recipe said so.
                    ignore_errors: expanded.ignore_errors,
                    // Unchanged from what the command line this replaces was
                    // given. A recipe of several lines has no single answer —
                    // GNU Make runs the marked lines of one and skips the rest,
                    // and a script assembled into one process can do neither —
                    // so the substitution says nothing new about it.
                    runs_while_pretending: false,
                })
                .into_iter()
                .collect()
        };
        // [spec:ronin:req:make.narration+1]
        // The same choice the sink makes for a recipe it expanded itself:
        // what the Makefile said, or the recipe's own text — never the shell
        // and environment wrapper needed to run it, and nothing at all for a
        // script too long to be a description.
        let description = match (&expanded.description, &launched.response_file) {
            (Some(text), _) => BString::from(text.to_vec()),
            (None, None) => BString::from(expanded.script.to_vec()),
            (None, Some(_)) => BString::default(),
        };
        let (rspfile, rspfile_content) = match launched.response_file {
            Some((path, content)) => (Some(BString::from(path)), BString::from(content)),
            None => (None, BString::default()),
        };
        Ok(LateBinding::Run(LateCommand {
            command: BString::from(launched.command),
            steps,
            description,
            rspfile,
            rspfile_content,
            ignore_errors: expanded.ignore_errors,
        }))
    }
}
