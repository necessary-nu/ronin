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
mod enclosing;
mod interrupts;
mod layout;
mod order;
mod parallel;
use parallel::{ChildUnit, ReadsAhead, evaluate_unit, read_ahead};
mod recipe;
mod report;
mod sink;

#[cfg(test)]
mod equivalence;

#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod shuffle_tests;

pub use kati::shuffle::Shuffle;
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

use crate::frontend::{BuildGraph, Edge, FrontendError, Node, Scope};
use crate::htab::{RapidHashMap, RapidHashSet};
use crate::make::sink::{ChildGroup, UnitOutput, UnitSubgraph};
use kati::build_sink::RecipeExpansion;
use kati::evaluate::{Evaluated, evaluate};
use kati::ninja::emit_populated;
use kati::session::Session;
use std::collections::HashSet;
use std::error;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

/// Kati observes the process working directory while evaluating a source unit.
static COMPILATION_DIRECTORY: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn compilation_directory_guard() -> std::sync::RwLockWriteGuard<'static, ()> {
    COMPILATION_DIRECTORY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keep the process directory stable while a non-Make frontend uses it.
pub(crate) fn stable_process_directory_guard() -> std::sync::RwLockReadGuard<'static, ()> {
    COMPILATION_DIRECTORY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    /// A recursive invocation the compiler composed points at a directory that
    /// holds no makefile to compile.
    ///
    /// Its own variant rather than an [`Self::Evaluate`] string because the two
    /// callers want different things from it. A build cannot go on — the child
    /// graph does not exist and the line that would have started a Make was
    /// lifted out of the recipe — while a report can, and says so at the
    /// invocation instead. The rendering is the same either way.
    MissingChildMakefile {
        /// The directory the invocation selected, which exists and holds none
        /// of the names a Make reads.
        directory: PathBuf,
    },
    /// The user stopped the read before it could finish.
    ///
    /// Its own variant because it is not a rejection: the Makefile said nothing
    /// wrong, and nothing is to be reported about it. What it decides is the
    /// status the invocation leaves with, which is the interrupt's rather than
    /// the 2 every refusal shares.
    Interrupted,
}

impl fmt::Display for MakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evaluate(diagnostic) => formatter.write_str(diagnostic),
            Self::Construct(error) => error.fmt(formatter),
            Self::Interrupted => formatter.write_str("interrupted"),
            Self::MissingChildMakefile { directory } => write!(
                formatter,
                "no makefile found for recursive compilation in '{}'",
                directory.display()
            ),
        }
    }
}

impl error::Error for MakeError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Evaluate(_) | Self::MissingChildMakefile { .. } | Self::Interrupted => None,
            Self::Construct(error) => Some(error),
        }
    }
}

impl MakeError {
    /// kati's failure, rendered the way kati renders it: one cause per line.
    fn evaluate(error: &kati::anyhow::Error) -> Self {
        // Asked of the error rather than of its text: a read that stopped is
        // not a Makefile that would not evaluate, and the two leave with
        // different statuses.
        if kati::interrupt::was_interrupted(error) {
            return Self::Interrupted;
        }
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
/// The returned graph is complete execution input when
/// [`Loaded::regeneration_targets`] is empty. Otherwise those targets are
/// compiler inputs to build through the ordinary Ninja scheduler before
/// evaluating from a fresh session. The engine does not receive Make
/// provenance beside the graph: recipe environments and every other
/// graph-affecting Make construct are compiled before execution, and
/// persistence applies the Ninja controls the compiler placed on that graph.
///
/// # Errors
///
/// Returns [`MakeError::Evaluate`] for a Makefile Make itself rejects — a
/// syntax error, an `$(error)`, a prerequisite with no rule to make it — and
/// [`MakeError::Construct`] for one that evaluates but describes a graph the
/// engine cannot hold, such as two rules generating one output.
// [spec:ronin:req:make.graph-direct]
// [spec:ronin:req:make.compiler-boundary]
// [spec:ronin:req:make.state-outside-the-tree+3]
pub fn load_makefile(session: Session, shuffle: Shuffle) -> Result<Loaded, MakeError> {
    let _directory = compilation_directory_guard();
    let directory = std::env::current_dir().map_err(|error| {
        MakeError::Evaluate(format!(
            "reading current directory for Make compilation: {error}"
        ))
    })?;
    let mut key = directory.as_os_str().as_encoded_bytes().to_vec();
    for makefile in &session.flags.makefiles {
        key.push(0);
        key.extend_from_slice(makefile.as_encoded_bytes());
    }
    let environment = session.invocation_environment.clone().unwrap_or_else(|| {
        std::sync::Arc::new(std::env::vars_os().collect::<Vec<(OsString, OsString)>>())
    });
    let environment_value = |name: &str| {
        environment
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
    };
    let level = environment_value("MAKELEVEL")
        .and_then(|level| level.parse().ok())
        .unwrap_or(0usize);
    let recipe_environment = vec![(
        OsString::from("MAKELEVEL"),
        Some(OsString::from(level.saturating_add(1).to_string())),
    )];
    let compilation = Compilation {
        context: CompilationContext {
            diagnostics: std::sync::Arc::clone(&session.diagnostics),
            interrupts: interrupts::ReadInterrupts::installed(),
            census: std::sync::Arc::clone(&session.census),
            reporting: false,
            root_directory: directory.clone(),
            directory,
            path_prefix: PathBuf::new(),
            enclosing: std::sync::Arc::default(),
            makeflags: environment_value("MAKEFLAGS").unwrap_or_default(),
            // No invocation to have carried the switch: this entry compiles a
            // Makefile for a caller that runs the graph itself.
            always_make: false,
            // One read, so it has not started over.
            restarted: false,
            assumed_new: Vec::new(),
            assumed_old: Vec::new(),
            level,
            jobs: session.flags.num_jobs.max(1),
            // No group above this entry to join: what its makefiles decide is
            // the run's own budget.
            job_group: None,
            // This entry compiles for a caller that runs the graph itself, and
            // the session's job count is the whole of what it was told.
            parallel_reads: session.flags.num_jobs.max(1),
            environment,
            recipe_environment,
        },
        session,
        shuffle,
        cache_key: key,
    };
    // A caller that takes this graph and runs it later needs every command in
    // it, so nothing here is left for a launch that this function will not be
    // present for.
    load_with_subninjas_unlocked(
        compilation,
        cli::compile_subninja,
        // One pass, so nothing was staged, no recipe was carried to its end by
        // an earlier one, nothing is read twice, and nothing has an earlier
        // read's answers to be handed.
        &Groundwork::default(),
        RecipeExpansion::Construction,
    )
}

/// Invocation context retained while one Makefile compilation discovers its
/// semantic subninjas.
#[derive(Clone)]
pub(crate) struct CompilationContext {
    /// Where every session composed under this one writes its warnings.
    ///
    /// Carried with the context rather than reached for, because a recursive
    /// child compiled into this graph is a session of its own and what it says
    /// belongs to the invocation that asked, not to the process it happens to
    /// run in.
    pub(crate) diagnostics: std::sync::Arc<kati::diagnostics::Diagnostics>,
    /// What every session composed under this one is told about being stopped.
    ///
    /// Shared for the reason the diagnostics descriptor is: one Ctrl-C stops
    /// the whole invocation, not the one recursive unit that happened to be
    /// waiting for a `$(shell)`.
    pub(crate) interrupts: std::sync::Arc<dyn kati::interrupt::Interruptible>,
    /// Where every one of them records what it classified about a recursive
    /// invocation, for a caller that asked for a report rather than a build.
    pub(crate) census: std::sync::Arc<kati::census::Census>,
    /// Whether this compilation is being run to report on the build rather
    /// than to make it.
    ///
    /// It decides one thing and is written down rather than inferred because
    /// the thing it decides is a refusal: a composition whose child directory
    /// holds no makefile stops a build, because there is no child graph and the
    /// recipe line that would have started a Make was lifted out of the recipe,
    /// and it does not stop a report, which names the invocation and carries
    /// on. Inferring it from `census.is_recording()` would work today and would
    /// be an accident — a build that ever wanted a census would quietly acquire
    /// a report's tolerance for a graph it cannot build.
    pub(crate) reporting: bool,
    pub(crate) root_directory: PathBuf,
    pub(crate) directory: PathBuf,
    pub(crate) path_prefix: PathBuf,
    /// The files the units enclosing the children of this compilation make,
    /// by canonical path, and the nodes they make them at — what a child's
    /// name for one of them resolves to. Set by [`compile_unit`] for the
    /// children it composes; empty for the root. See
    /// [`GraphSink::begin_subninja`].
    pub(crate) enclosing: std::sync::Arc<sink::Enclosing>,
    pub(crate) makeflags: String,
    /// Whether `-B` is in force: every target with a recipe is out of date.
    ///
    /// Carried into the compilation because one freshness question is asked
    /// here rather than by the build — whether a recursive recipe has to run,
    /// which has to be answered before any child Makefile of it can be read.
    /// GNU Make asks that question with `always_make_flag` in hand, so this
    /// one is asked with the same thing in hand: a `-B` run composes the
    /// children it is about to rebuild instead of deciding it has none.
    ///
    /// Inherited by every child compilation, because `-B` reaches a recursive
    /// child through `MAKEFLAGS` and a child of a forced Make is forced.
    pub(crate) always_make: bool,
    /// Whether the read has already started over, which is what takes `-B` off
    /// the makefile update.
    ///
    /// GNU Make sets `always_make_flag = always_make_set && (restarts == 0)`
    /// ahead of the update and back to `always_make_set` for the goals
    /// (main.c), so a restarted read stops forcing the Makefiles and goes on
    /// forcing the goals. Ronin compiles ONE graph for both phases, and the
    /// phase a recursive recipe belongs to is known right where the question is
    /// asked — `made_for_a_makefile` is that answer — so the rule is applied a
    /// recipe at a time instead of a phase at a time.
    ///
    /// Without it a Makefile made through a forced recursion is remade on every
    /// pass, moves its own stamp, and the read never settles.
    ///
    /// Inherited by every child compilation for the reason `always_make` is:
    /// the restart belongs to the invocation and not to one of its units.
    pub(crate) restarted: bool,
    /// The names `-W` gave this invocation, which are infinitely new.
    ///
    /// Carried here for the reason `always_make` is, and for the same single
    /// question: whether a recursive recipe has to run is decided at compile
    /// time, and a switch that answers about a file has to reach the place
    /// where that file's date is asked for. Without it the recipe of a target
    /// the switch named runs, where GNU Make says the target is up to date.
    ///
    /// NOT inherited by a child compilation, and that is the difference from
    /// `always_make`: GNU Make does not put `-W` in `MAKEFLAGS`, so a recursive
    /// child never hears about it. Probed rather than assumed — a child's
    /// `MAKEFLAGS` under `make -W foo` reads ` --no-print-directory` and
    /// nothing else, where under `-n` it reads `n --no-print-directory`.
    pub(crate) assumed_new: Vec<crate::util::BString>,
    /// What `-o` named, and here for the reason `assumed_new` is here: whether
    /// a recursive recipe has to run is decided at compile time, and a switch
    /// that answers about a file has to reach the place where that file's date
    /// is asked for. Without it the recipe of a target the switch named runs,
    /// where GNU Make says the target is up to date.
    ///
    /// Not inherited by a child compilation either: `-o` shares `-W`'s cleared
    /// `toenv` in GNU Make's switch table (main.c), so neither reaches
    /// `MAKEFLAGS` and neither reaches a recursive child. Measured — a child's
    /// `MAKEFLAGS` under `make -o foo` reads empty.
    pub(crate) assumed_old: Vec<crate::util::BString>,
    pub(crate) level: usize,
    pub(crate) jobs: usize,
    /// The job group this unit reads under: the pool holding the group's edges
    /// and the budget it stands at. `None` is the run's own budget, which the
    /// scheduler bounds directly.
    ///
    /// Inherited, because GNU Make's is: a Make that founded no group of its
    /// own passes down the jobserver it joined, and a Make below it draws on
    /// the same tokens.
    pub(crate) job_group: Option<(Vec<u8>, std::num::NonZeroUsize)>,
    /// How many of this compilation's Makefile reads may overlap.
    ///
    /// Separate from `jobs` because `jobs` is what `MAKEFLAGS` carries, and it
    /// collapses "no `-j` at all" and "`-j` with no number" into one unlimited
    /// value — both mean the same thing to the switch table. They mean
    /// opposite things here: with no `-j`, GNU Make runs one recipe at a time
    /// and reads one child Makefile at a time, and so must this. This is the
    /// number the BUILD would run commands against, which is the number
    /// recursive children are counted against too. See [`read_ahead`].
    pub(crate) parallel_reads: usize,
    /// The environment this unit imports while kati evaluates it.
    ///
    /// Shared with the child sessions compiled from it rather than copied into
    /// each: see [`kati::Session::invocation_environment`]. A unit that exports
    /// nothing — which is every unit of a tree that only dispatches — holds the
    /// same allocation its parent does.
    pub(crate) environment: std::sync::Arc<Vec<(OsString, OsString)>>,
    /// Changes child commands need in addition to the root build environment.
    pub(crate) recipe_environment: Vec<(OsString, Option<OsString>)>,
}

/// One Makefile ready for kati evaluation and graph composition.
pub(crate) struct Compilation {
    pub(crate) session: Session,
    pub(crate) shuffle: Shuffle,
    pub(crate) context: CompilationContext,
    /// Canonical Makefile identity plus every graph-affecting invocation input.
    /// Reusing it reuses the already-composed target nodes rather than defining
    /// the same outputs twice.
    pub(crate) cache_key: Vec<u8>,
}

struct CompiledUnit {
    subgraph: UnitSubgraph,
    makeflags: String,
    complete: bool,
}

struct ComposedUnit {
    subgraph: UnitSubgraph,
    complete: bool,
}

/// A required Makefile the read could not open and no rule can make, and what
/// the run says about it once the Makefiles ahead of it have been brought up to
/// date.
///
/// The complaint travels with the refusal because GNU Make prints the two
/// together, from inside the update rather than from the read: `eval_makefile`
/// records the errno and says nothing, and `show_goal_error` speaks beside the
/// refusal it belongs to.
pub(crate) struct RefusedMakefile {
    /// The located `No such file or directory`, already rendered. `None` for a
    /// Makefile the command line named, which has no `include` line to point
    /// at and which GNU Make reports from the read.
    complaint: Option<String>,
    /// What ends the run.
    error: MakeError,
}

/// What the report writes for a run's refusals: each one's held complaint
/// beside the failure it refuses over.
///
/// GNU Make writes a second list after this one — a `Failed to remake makefile
/// 'X'.` line per refusal, from `main.c`'s `us_failed` pass over `read_files`
/// once the update has returned. Ronin does not: every name in it has already
/// been reported one line above, so the pass is GNU's ceremony rather than a
/// failure that would otherwise go unreported.
// [spec:ronin:req:make.narration+2]
pub(crate) fn refusal_report(refusals: Vec<RefusedMakefile>) -> Vec<(Option<String>, MakeError)> {
    refusals
        .into_iter()
        .map(|refusal| (refusal.complaint, refusal.error))
        .collect()
}

/// One unit's Makefiles: the ones among them whose failure is forgiven, and the
/// required one nothing can make.
struct UnitRemakes {
    all: Vec<Node>,
    forgiven: Vec<Node>,
    /// The ones whose contents the read wanted and did not get. GNU Make gives
    /// those the timestamp of a file that is not there (read.c:409), so the rule
    /// that would make one runs however the name looks on disk.
    unread: Vec<Node>,
    /// The complaint each of those holds until its own rule loses.
    complaints: Vec<(Node, String)>,
    /// GNU Make brings the Makefiles around these up to date and then ends the
    /// run over them, so they are carried alongside rather than raised in their
    /// place. At most one without `-k`.
    refusals: Vec<RefusedMakefile>,
}

/// Where this unit's Makefiles ended up in the shared graph.
fn unit_remakes(
    sink: &mut GraphSink,
    names: &Session,
    regenerations: &RegenerationNames,
    refusals: Vec<RefusedMakefile>,
) -> Result<UnitRemakes, MakeError> {
    let mut looked_up = |symbols: &[kati::symtab::Symbol]| {
        sink.unit_nodes(names, symbols).map_err(|error| {
            sink.construction_failure()
                .map_or_else(|| MakeError::evaluate(&error), MakeError::Construct)
        })
    };
    Ok(UnitRemakes {
        all: looked_up(&regenerations.all)?,
        forgiven: looked_up(&regenerations.forgiven)?,
        unread: looked_up(&regenerations.unread)?,
        complaints: looked_up(
            &regenerations
                .complaints
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
        )?
        .into_iter()
        .zip(
            regenerations
                .complaints
                .iter()
                .map(|(_, text)| text.clone()),
        )
        .collect(),
        refusals,
    })
}

/// What one unit's first read of this compilation was told from outside itself.
///
/// Carried from pass to pass beside the units themselves, because a staging
/// pass re-reads a unit over text that has not moved while the ground under it
/// has: the staged work is what moved it. See
/// [`kati::session::GroundJournal`].
///
/// `Clone` because the record is shared with the workers for the length of a
/// pass and taken back between passes, which is what
/// [`std::sync::Arc::make_mut`] needs to say. Nothing clones one in practice:
/// the pass that would has already put every worker down.
#[derive(Clone)]
pub(crate) struct UnitJournal {
    /// The answers the ground gave, in the order the read asked for them.
    ground: Vec<kati::session::GroundAnswer>,
    /// The bytes that read got for every makefile it read.
    ///
    /// Which files a read opens is not a value handed to an expansion, it is
    /// what the read IS, so it is pinned rather than journalled: a makefile a
    /// staged child has rewritten, or removed, still reads as the text GNU
    /// Make's one read had.
    sources: Vec<(OsString, Vec<u8>)>,
}

/// Every unit's journal, keyed by cache key.
pub(crate) type ReadJournals = RapidHashMap<Vec<u8>, UnitJournal>;

/// Turn one recursive invocation into the child compilation it names.
///
/// A bare function rather than a closure, and named here rather than left to a
/// type parameter, because resolving happens on whichever thread reaches the
/// invocation: a worker that has just read a unit resolves the children that
/// unit names so it can read them too, and a closure over the composition's own
/// state could not cross to it. See [`read_ahead`].
pub(crate) type Resolver =
    fn(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>;

struct CompilationState<'a> {
    cache: RapidHashMap<Vec<u8>, UnitSubgraph>,
    compiling: RapidHashSet<Vec<u8>>,
    /// Every unit's unexpanded recipes, gathered as the units are compiled.
    ///
    /// One collection for the whole compilation, because the graph is one
    /// graph and an edge belongs to exactly one unit. A unit reached from the
    /// cache contributed its recipes the one time it was compiled, and the
    /// edges it produced are the edges the cache hands back.
    pending_recipes: recipe::PendingRecipes,
    regenerations: Vec<Node>,
    /// The staged work belonging to a Makefile's own recipe rather than to a
    /// goal's.
    ///
    /// A subset of `regenerations`. GNU Make turns `-n`, `-t` and `-q` off
    /// across the whole makefile update and back on for the goals, because a
    /// Makefile it only pretended to remake is one it would then have to guess
    /// the contents of. A recipe cut into segments around a composed `$(MAKE)`
    /// has those segments built here rather than by the update, so the split
    /// the update makes has to be made here too.
    makefile_staged: Vec<Node>,
    remakes: Vec<Node>,
    forgiven_remakes: Vec<Node>,
    /// The Makefiles whose contents the read wanted and did not get.
    unread_remakes: Vec<Node>,
    /// What each of those says if its own rule loses.
    remake_complaints: Vec<(Node, String)>,
    /// The required Makefiles the first unit of this compilation that had any
    /// could not make.
    refusals: Vec<RefusedMakefile>,
    settled_boundaries: &'a RapidHashSet<EvaluationBoundary>,
    evaluation_boundaries: RapidHashSet<EvaluationBoundary>,
    /// The widest job budget any unit of this compilation asked for, which is
    /// the run's own until a unit's makefiles ask for more.
    job_budget: usize,
    /// The recursive recipes an earlier pass carried to their end.
    ///
    /// A recipe cut into segments is mid-flight from the moment its first
    /// segment is staged until the wrapper holding the rest of it runs, and
    /// [`Self::recipe_begun`] is what says so. It has to stop saying it. A
    /// recipe still called begun on every later pass leaves its target dirty
    /// for the whole invocation, and a Makefile made FROM that target is then
    /// remade on every pass, moves its own stamp, and starts the read over
    /// again — where GNU Make restarts once and settles, because after its
    /// re-exec nothing survives but the disk.
    finished_recipes: &'a RapidHashSet<RecursiveRecipe>,
    /// The recursive recipes this pass compiled whole, which the pass after it
    /// counts as finished.
    ///
    /// Recorded here and weighed by the caller, because whether the pass ran
    /// them is not a fact about the compilation: a pass that stopped at some
    /// other recipe's boundary builds the staged work alone, so a wrapper it
    /// composed on the way there has not run and its recipe is still in
    /// flight. See [`Loaded::completed_recipes`].
    completed_recipes: RapidHashSet<RecursiveRecipe>,
    /// The outputs of every recursive wrapper this pass staged and did not
    /// finish, whether because the compilation stopped at a boundary before
    /// its children were composed or because the recipe was held for a later
    /// pass for any other reason.
    ///
    /// Their edges are still wearing the freshness probe, whose command is
    /// `false` and which is never allowed to execute, so nothing this pass
    /// builds may reach one of them. Every hold in `compose_subninjas` that
    /// happens after `stage_recursive_wrapper` has returned `Dirty` therefore
    /// owes this list its outputs: [`crate::make::cli::remake::makeable_now`]
    /// keeps a Makefile back only for outputs it has been told about, and the
    /// Linux kernel's `syncconfig` reaches a hold that once told it nothing.
    unfinished: Vec<Node>,
    /// The units an earlier pass of this compilation already read, by cache
    /// key, each with what that read was told by the ground. A unit in here is
    /// being read again over text that has not moved, so what its read does on
    /// the way through was done then and is not done now — see
    /// `kati::flags::Flags::is_repeated_read` — and what it was told then is
    /// what it is told again, because the ground has moved and GNU Make's one
    /// read never saw it move.
    read_units: std::sync::Arc<ReadJournals>,
    /// Every unit this pass read, which is the set the next pass repeats. It
    /// accumulates rather than replacing, because a pass reads everything the
    /// pass before it read and then one child more.
    units_read: ReadJournals,
    /// Workers that read a recursive child's Makefiles ahead of the
    /// composition, or `None` where every read happens on this thread.
    ///
    /// Composition itself stays here: one graph is built by one thread, in the
    /// order it is built in today, which is what keeps the graph the same
    /// whatever the workers do. See [`read_ahead`].
    ///
    /// Started on first use rather than with the state, because most
    /// compilations never read ahead at all — a Makefile with no recursive
    /// recipes, or with one, has nothing to overlap — and threads that would
    /// take no work are worth not starting.
    read_pool: std::cell::OnceCell<Option<std::sync::Arc<parallel::ReadPool>>>,
    /// How many workers to start when one is first wanted.
    read_threads: usize,
    /// What this compilation asks its emissions to evaluate, and when.
    ///
    /// Read off the sink once, because the sink answers it the same way every
    /// time, and carried here because the half of an emission that never sees
    /// the sink runs on a worker and still has to run under the sink's policy.
    evaluation: kati::ninja::BuildEvaluation,
    /// Every unit whose read has been started, by cache key.
    ///
    /// One unit is read once per pass however many recipes name it, because
    /// reading it twice would run its `$(shell)` calls twice. The composition
    /// gets that from its own cache; a worker reading a descendant's Makefiles
    /// ahead of the composition cannot see that cache, so this is what it asks
    /// instead. Shared, because both sides claim into it.
    read_claims: std::sync::Arc<std::sync::Mutex<RapidHashSet<Vec<u8>>>>,
    /// The build root a composed unit last asked its wrappers' dates of, and
    /// the disk that answers for it. See [`freshness_disk`].
    freshness_disk: Option<(PathBuf, crate::os::RealDiskInterface)>,
    /// The child units this pass began composing and could not finish, by cache
    /// key.
    ///
    /// A unit that composes WHOLE is cached in `cache` and handed to the second
    /// recipe naming it; one that stopped at a boundary is not, because what it
    /// left in the sink is half a unit. While a pass ended at the first recipe
    /// it could not carry, there was never a second recipe to reach one of
    /// these. Carrying on gives it one, and composing that child again would
    /// read its Makefile a second time — running its `$(shell)` calls twice —
    /// and emit its edges a second time. So a child already found unfinished
    /// holds the recipe naming it instead, and the pass after the staged work
    /// composes it once.
    unfinished_units: RapidHashSet<Vec<u8>>,
}

/// One unit's unexpanded recipes and everything expanding them will need,
/// carried out of the closure that read the Makefile.
type UnitRecipes = (
    kati::eval::Evaluator,
    kati::ninja::DeferredRecipes,
    sink::CommandLayout,
    Vec<(Edge, kati::build_sink::DeferredRecipeId)>,
);

impl CompilationState<'_> {
    /// Hold one unit's unexpanded recipes for as long as the build may still
    /// start one of them.
    ///
    /// Taken after the unit has compiled rather than inside the directory guard
    /// that compiled it, because what is retained is the session and not the
    /// directory: where the recipes expand is recorded here, and entered again
    /// when one of them is asked for.
    fn retain(&mut self, recipes: Option<UnitRecipes>, directory: &std::path::Path) {
        if let Some((session, deferred, layout, edges)) = recipes {
            self.pending_recipes
                .admit(session, deferred, layout, directory.to_owned(), &edges);
        }
    }

    /// Record one unit's Makefiles among everything this compilation has to
    /// build before the read can be trusted, keeping the order they were
    /// reached in and never naming one twice.
    fn admit(&mut self, remakes: UnitRemakes) {
        for target in remakes.all {
            if !self.regenerations.contains(&target) {
                self.regenerations.push(target);
            }
            if !self.remakes.contains(&target) {
                self.remakes.push(target);
            }
        }
        for target in remakes.forgiven {
            if !self.forgiven_remakes.contains(&target) {
                self.forgiven_remakes.push(target);
            }
        }
        for target in remakes.unread {
            if !self.unread_remakes.contains(&target) {
                self.unread_remakes.push(target);
            }
        }
        for (target, complaint) in remakes.complaints {
            if !self
                .remake_complaints
                .iter()
                .any(|(named, _)| *named == target)
            {
                self.remake_complaints.push((target, complaint));
            }
        }
        if self.refusals.is_empty() {
            self.refusals = remakes.refusals;
        }
    }

    /// Whether this recursive recipe has already run part of itself, and not
    /// yet all of it.
    ///
    /// A settled boundary is work an earlier pass of this same invocation put
    /// on the ground. Two of the three kinds are the recipe's own: the lines
    /// written ahead of an invocation, and a child group an earlier invocation
    /// in the same recipe already started. The third — the wrapper's
    /// prerequisites — is not the recipe, it is what GNU Make settles before
    /// the recipe begins, and building it is no reason to call the recipe
    /// started.
    ///
    /// A recipe an earlier pass carried to its end is not begun but over: what
    /// reads its target from here reads a file, exactly as it does for a
    /// Makefile the update reached and won (`mark_makefiles_settled`). See
    /// [`Self::finished_recipes`] for what happens when it keeps saying begun.
    fn recipe_begun(&self, compilation_key: &[u8], pending_index: usize) -> bool {
        if self.finished_recipes.iter().any(|recipe| {
            recipe.compilation_key == compilation_key && recipe.pending_index == pending_index
        }) {
            return false;
        }
        self.settled_boundaries.iter().any(|boundary| {
            boundary.compilation_key == compilation_key
                && boundary.pending_index == pending_index
                && matches!(
                    boundary.predecessor,
                    EvaluationPredecessor::PrecedingLines(_) | EvaluationPredecessor::ChildGroup(_)
                )
        })
    }

    /// Whether an earlier pass already built what this recipe's own
    /// prerequisites stage.
    ///
    /// The question [`Self::stage`] asks first, asked on its own because the
    /// answer is wanted before the wrapper's freshness is settled: it is a fact
    /// about what has already run, and it holds whether or not the recipe still
    /// has anything left to do.
    fn staged_already(&self, compilation_key: &[u8], pending_index: usize) -> bool {
        self.settled_boundaries.contains(&EvaluationBoundary {
            compilation_key: compilation_key.to_vec(),
            pending_index,
            predecessor: EvaluationPredecessor::ParentPrerequisites,
        })
    }

    /// Whether the compilation may go past one boundary, and what it takes if
    /// it may not.
    ///
    /// A boundary an earlier pass already settled is behind this one, and the
    /// work at it has run. Otherwise `staged` joins what this compilation asks
    /// a provisional build for, the boundary is recorded so the pass after the
    /// build knows it is settled, and the caller leaves its unit incomplete.
    fn stage(
        &mut self,
        compilation_key: &[u8],
        pending_index: usize,
        predecessor: EvaluationPredecessor,
        staged: &[Node],
        for_makefile: bool,
    ) -> bool {
        let boundary = EvaluationBoundary {
            compilation_key: compilation_key.to_vec(),
            pending_index,
            predecessor,
        };
        if self.settled_boundaries.contains(&boundary) {
            return true;
        }
        self.regenerations.extend_from_slice(staged);
        self.regenerations.sort_unstable();
        self.regenerations.dedup();
        if for_makefile {
            self.makefile_staged.extend_from_slice(staged);
            self.makefile_staged.sort_unstable();
            self.makefile_staged.dedup();
        }
        self.evaluation_boundaries.insert(boundary);
        false
    }
}

/// One recursive recipe of one compilation unit, named by where it sits.
///
/// A recipe holding a composed `$(MAKE)` is cut into segments and finished
/// across several passes, so the passes need a name for it that survives them.
/// The unit's cache key and the recipe's place among that unit's recursive
/// recipes are that name — the same pair an [`EvaluationBoundary`] is keyed on,
/// which is how a boundary and the recipe it belongs to find each other.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RecursiveRecipe {
    compilation_key: Vec<u8>,
    pending_index: usize,
}

/// The work that must finish before one recursive child can be evaluated.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct EvaluationBoundary {
    compilation_key: Vec<u8>,
    pending_index: usize,
    predecessor: EvaluationPredecessor,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum EvaluationPredecessor {
    ParentPrerequisites,
    /// The recipe's own lines written ahead of the invocation at this index.
    PrecedingLines(usize),
    ChildGroup(usize),
}

/// Evaluate a root Makefile and every recursive `$(MAKE)` recipe into one
/// shared graph before returning it to the executor.
// [spec:ronin:req:make.recursive-invocation+3]
// [spec:ronin:req:make.compiler-boundary]
pub(crate) fn load_with_subninjas(
    root: Compilation,
    resolve: Resolver,
    settled: &Groundwork,
    expansion: RecipeExpansion,
) -> Result<Loaded, MakeError> {
    let _directory = compilation_directory_guard();
    load_with_subninjas_unlocked(root, resolve, settled, expansion)
}

/// What the passes before this one settled, which this one starts from.
///
/// Carried as one value because the three answer one question between them —
/// what of this invocation is already done — and because a pass that adds to
/// any of them is adding to the same record.
#[derive(Default)]
pub(crate) struct Groundwork {
    /// The compiler-input boundaries whose staged work is on the ground.
    pub(crate) boundaries: RapidHashSet<EvaluationBoundary>,
    /// The recursive recipes an earlier pass carried to their end.
    pub(crate) recipes: RapidHashSet<RecursiveRecipe>,
    /// The units an earlier pass read, which this one repeats rather than
    /// performs.
    ///
    /// Shared rather than owned because a worker reads it: a unit whose read a
    /// worker prepares is told what an earlier pass told it, and what a session
    /// is told must not depend on which thread ends up reading it. It is
    /// changed only between passes, when no worker exists.
    pub(crate) read_units: std::sync::Arc<ReadJournals>,
}

fn load_with_subninjas_unlocked(
    root: Compilation,
    resolve: Resolver,
    settled: &Groundwork,
    expansion: RecipeExpansion,
) -> Result<Loaded, MakeError> {
    let mut sink = GraphSink::new_at(&root.context.root_directory, expansion);
    let run_budget = root.context.jobs;
    let mut state = CompilationState {
        cache: RapidHashMap::default(),
        compiling: RapidHashSet::default(),
        pending_recipes: recipe::PendingRecipes::new(std::sync::Arc::clone(
            &root.context.diagnostics,
        )),
        regenerations: Vec::new(),
        makefile_staged: Vec::new(),
        remakes: Vec::new(),
        forgiven_remakes: Vec::new(),
        unread_remakes: Vec::new(),
        remake_complaints: Vec::new(),
        settled_boundaries: &settled.boundaries,
        refusals: Vec::new(),
        evaluation_boundaries: RapidHashSet::default(),
        job_budget: root.context.jobs,
        finished_recipes: &settled.recipes,
        completed_recipes: RapidHashSet::default(),
        unfinished: Vec::new(),
        read_units: std::sync::Arc::clone(&settled.read_units),
        units_read: ReadJournals::default(),
        // `-j` is what GNU Make counts its recursive children against, and a
        // recursive child of Ronin's is a Makefile read rather than a process,
        // so it is counted against the same number. At `-j1` there is no pool
        // at all and every Makefile is read exactly where and when it was read
        // before, which is what keeps a serial run's behaviour — the order two
        // reads' `$(shell)` commands run in included — untouched.
        read_pool: std::cell::OnceCell::new(),
        read_threads: root.context.parallel_reads,
        evaluation: kati::ninja::BuildEvaluation::of(&sink),
        read_claims: std::sync::Arc::default(),
        freshness_disk: None,
        unfinished_units: RapidHashSet::default(),
    };
    let root = compile_unit(
        ChildUnit::Unread(Box::new(root)),
        &mut sink,
        None,
        &std::sync::Arc::default(),
        resolve,
        &mut state,
    )?;
    let pending_recipes = (!state.pending_recipes.is_empty()).then_some(state.pending_recipes);
    // Only a run some unit widened: everywhere else the run's budget IS the
    // scheduler's, and a pool repeating it would hold nothing back.
    let ungrouped = (state.job_budget > run_budget)
        .then(|| std::num::NonZeroUsize::new(run_budget))
        .flatten();
    let graph = sink.into_graph(ungrouped).map_err(MakeError::Construct)?;
    Ok(Loaded {
        graph,
        pending_recipes,
        regenerations: state.regenerations,
        makefile_staged: state.makefile_staged,
        remakes: state.remakes,
        forgiven_remakes: state.forgiven_remakes,
        unread_remakes: state.unread_remakes,
        remake_complaints: state.remake_complaints,
        refusals: state.refusals,
        evaluation_boundaries: state.evaluation_boundaries,
        completed_recipes: state.completed_recipes,
        unfinished: state.unfinished,
        units_read: state.units_read,
        makeflags: root.makeflags,
        job_budget: state.job_budget,
    })
}

/// What one unit's read settled before anything of it reached the graph.
///
/// The half of a read that touches no [`GraphSink`], carried as one value so
/// that it can be produced on whichever thread performed the read and consumed
/// on the one thread that writes the graph. Every field is a function of the
/// evaluator and the dependency nodes alone — see [`parallel::prepare_read`],
/// which is the whole of what makes them, and which lives beside the read it
/// belongs to rather than beside the emission it feeds.
struct Prepared {
    ev: kati::eval::Evaluator,
    /// The build the dependency nodes describe, walked and expanded as far as
    /// anything can be without a sink to hand it to. See
    /// [`kati::ninja::populate_build`].
    populated: kati::ninja::PopulatedBuild,
    /// Every recursive invocation the population found, or `None` where this
    /// unit is one whose children must not be looked for before it is emitted.
    /// See [`kati::ninja::PopulatedBuild::liftable_recursions`].
    recursions: Option<Vec<kati::ninja::PopulatedRecursion>>,
    refusals: Vec<RefusedMakefile>,
    regeneration_names: RegenerationNames,
    exported: Vec<(OsString, Option<OsString>)>,
    unreadable: Option<kati::export::Unreadable>,
    command_line: Vec<(OsString, Option<OsString>)>,
    makeflags: String,
    flag_environment: [(OsString, Option<OsString>); 2],
}

/// One unit's makefile read, and the build it describes emitted into the sink.
///
/// A function rather than a closure so the thing it produces has a name: the
/// read is what a pass repeats, and every field below is something the read
/// settled and the compilation around it then carries.
///
/// `reaper` is where what the read no longer needs is freed. See
/// [`parallel::Reaper`].
fn read_unit(
    prepared: Prepared,
    sink: &mut GraphSink,
    parent_scope: Option<Scope>,
    enclosing: &std::sync::Arc<sink::Enclosing>,
    context: &CompilationContext,
    reaper: Option<&parallel::Reaper>,
) -> Result<UnitRead, MakeError> {
    let Prepared {
        mut ev,
        populated,
        recursions: _,
        refusals,
        regeneration_names,
        exported,
        unreadable,
        command_line,
        makeflags,
        flag_environment,
    } = prepared;
    if let Some(parent) = parent_scope {
        sink.begin_subninja(
            parent,
            context.path_prefix.clone(),
            context.directory.clone(),
            std::sync::Arc::clone(enclosing),
        );
    }
    // How many of this unit's recipes may run at once, as its own settled
    // `MAKEFLAGS` names it. Every source of a `-j` has landed in that one
    // string by now — the command line that started the unit, the budget it
    // inherited, what its own makefiles wrote — with the precedence GNU Make
    // gives them already applied, because a `-j` a command line typed is
    // protected from a makefile's write and one merely inherited is not. See
    // [`crate::make::cli::interface::switch_table`]. A `MAKEFLAGS` naming no
    // `-j` at all leaves the group's own budget standing.
    // [spec:ronin:req:make.jobserver+3]
    let group_budget = context
        .job_group
        .as_ref()
        .map_or(context.jobs, |(_, budget)| budget.get());
    let budget = cli::makeflags_job_budget(&makeflags).unwrap_or(group_budget);
    let job_group = sink.hold_unit_jobs(
        ev.session.flags.not_parallel,
        context.job_group.clone(),
        (budget != group_budget)
            .then(|| std::num::NonZeroUsize::new(budget))
            .flatten(),
    );
    let mut recipe_environment = context.recipe_environment.clone();
    apply_recipe_environment(&mut recipe_environment, &flag_environment);
    apply_recipe_environment(&mut recipe_environment, &exported);
    sink.set_recipe_environment(
        recipe_environment,
        unreadable.as_ref().map(|held| held.why.clone()),
    );
    let deferred = match emit_populated(populated, &mut ev, sink) {
        Ok(deferred) => deferred,
        Err(error) => {
            if let Some(failure) = sink.construction_failure() {
                return Err(MakeError::Construct(failure));
            }
            return Err(MakeError::evaluate(&error));
        }
    };
    // The layout is read while this unit is still the current one: it is
    // what wraps every command this unit produces, and a recipe expanded
    // later has to be wrapped in exactly the same thing.
    let layout = sink.layout();
    let unit_remakes = unit_remakes(sink, &ev.session, &regeneration_names, refusals)?;
    let unit = sink.take_unit();
    ev.finish().map_err(|error| MakeError::evaluate(&error))?;
    // Taken before the session goes wherever it goes next: the recipes may
    // keep it alive for the build and may not, and either way this read is
    // over and what it was told belongs to the pass.
    let journal = UnitJournal {
        ground: ev.session.ground_journal.close_read(),
        sources: ev.session.read_sources(),
    };
    let (deferred_edges, settled_edges) = sink.take_late_edges();
    // A unit with nothing left to expand has no further use for the session
    // that read it, and that session is the whole of what the read built:
    // its symbol table, its variables, and every expression they parsed to.
    let pending_recipes = if deferred.is_empty() {
        parallel::discard(reaper, (ev, layout));
        None
    } else {
        Some((ev, deferred, layout, deferred_edges))
    };
    Ok(UnitRead {
        unit,
        exported,
        command_line,
        unit_remakes,
        makeflags,
        flag_environment,
        pending_recipes,
        settled_edges,
        journal,
        job_group,
        job_budget: budget,
    })
}

/// What reading one unit settled.
struct UnitRead {
    unit: UnitOutput,
    exported: Vec<(OsString, Option<OsString>)>,
    command_line: Vec<(OsString, Option<OsString>)>,
    unit_remakes: UnitRemakes,
    makeflags: String,
    flag_environment: [(OsString, Option<OsString>); 2],
    pending_recipes: Option<UnitRecipes>,
    /// Edges whose recipe this read expanded for itself and which still run a
    /// process per command line.
    settled_edges: Vec<(Edge, sink::SettledSteps)>,
    /// What this read was told from outside itself, for the read that repeats
    /// it.
    journal: UnitJournal,
    /// The job group this unit ended up in, which its children join.
    job_group: Option<(Vec<u8>, std::num::NonZeroUsize)>,
    /// How many of this unit's own recipes may run at once.
    job_budget: usize,
}

fn compile_unit(
    child: ChildUnit,
    sink: &mut GraphSink,
    parent_scope: Option<Scope>,
    enclosing: &std::sync::Arc<sink::Enclosing>,
    resolve: Resolver,
    state: &mut CompilationState<'_>,
) -> Result<CompiledUnit, MakeError> {
    // Where the composition stands, which is what the reads this unit starts
    // for its own children are ordered by. See [`parallel::ReadOrder`].
    let order = child.read_order();
    let (compilation_key, read_ahead) = child.claim(state)?;
    if !state.compiling.insert(compilation_key.clone()) {
        return Err(MakeError::Evaluate(
            "recursive Make compilation includes itself".to_owned(),
        ));
    }
    // `None` until the first unit that read ahead started the pool, and for the
    // whole of a run that never starts one — a serial compilation frees exactly
    // where it froze before the pool existed.
    // The handle is taken out of `state` rather than borrowed from it, because
    // what it frees is handed over after the composition has finished with the
    // unit and `state` is wanted mutably in between.
    let pool = state
        .read_pool
        .get()
        .and_then(Option::as_ref)
        .map(std::sync::Arc::clone);
    let reaper = pool.as_deref().and_then(parallel::ReadPool::reaper);
    // What a worker read ahead for the children of THIS unit while it was
    // reading this unit, which the composition below adopts rather than
    // resolving and reading again. Empty where nothing read ahead.
    let mut chained = Vec::new();
    let (context, evaluated) = match read_ahead {
        Ok(read) => {
            let (context, read) = read.collect(reaper);
            (
                context,
                read.map(|read| {
                    chained = read.chained;
                    read.prepared
                }),
            )
        }
        Err((session, context)) => {
            let evaluated = evaluate_unit(
                session,
                &context.directory,
                state.evaluation,
                state.read_threads >= 2,
            );
            (context, evaluated)
        }
    };
    let read = evaluated.and_then(|evaluated| {
        in_directory(&context.directory, || {
            read_unit(evaluated, sink, parent_scope, enclosing, &context, reaper)
        })
    });
    let UnitRead {
        unit,
        exported,
        command_line,
        unit_remakes,
        makeflags,
        flag_environment,
        pending_recipes,
        settled_edges,
        journal,
        job_group,
        job_budget,
    } = match read {
        Ok(read) => read,
        Err(error) => {
            state.compiling.remove(&compilation_key);
            return Err(error);
        }
    };
    // The widest budget any unit asked for is what the one scheduler has to be
    // opened to, because a unit held to its own pool can still only run as many
    // commands at once as the scheduler will start.
    state.job_budget = state.job_budget.max(job_budget);
    state.units_read.insert(compilation_key.clone(), journal);
    state.retain(pending_recipes, &context.directory);
    state.pending_recipes.admit_settled(settled_edges);
    state.admit(unit_remakes);

    let mut unit = unit;
    let mut descendant_context = context;
    // What this unit's children see made around them: what encloses this
    // unit, and the files this unit itself makes.
    descendant_context.enclosing =
        enclosing::for_children(enclosing, std::mem::take(&mut unit.generated));
    descendant_context.makeflags.clone_from(&makeflags);
    descendant_context.job_group = job_group;
    apply_exported_environment(&mut descendant_context.environment, &command_line);
    apply_exported_environment(&mut descendant_context.environment, &exported);
    apply_recipe_environment(
        &mut descendant_context.recipe_environment,
        &flag_environment,
    );
    apply_recipe_environment(&mut descendant_context.recipe_environment, &exported);
    let composed = compose_subninjas(
        unit,
        &compilation_key,
        sink,
        resolve,
        &descendant_context,
        state,
        ReadsAhead { chained, order },
    );
    state.compiling.remove(&compilation_key);
    // Three shared handles, three paths, a string and four vectors, and the
    // children that wanted them have been composed. Freed on the reaper's
    // thread for the reason everything else a read leaves behind is: this one
    // is walked once per unit on the one thread every unit passes through.
    parallel::discard(reaper, (descendant_context, compilation_key));
    let composed = composed?;
    // A recursive recipe's own lines reach their edges while the children are
    // composed, which is after this unit's edges were claimed.
    state
        .pending_recipes
        .admit_settled(sink.take_settled_edges());
    Ok(CompiledUnit {
        subgraph: composed.subgraph,
        makeflags,
        complete: composed.complete,
    })
}

// [spec:ronin:req:make.compiler-input-staging+1]
fn compose_subninjas(
    unit: UnitOutput,
    compilation_key: &[u8],
    sink: &mut GraphSink,
    resolve: Resolver,
    descendant_context: &CompilationContext,
    state: &mut CompilationState<'_>,
    ahead: ReadsAhead,
) -> Result<ComposedUnit, MakeError> {
    let UnitOutput {
        targets,
        subninjas,
        edges,
        serial: not_parallel,
        // Taken by `compile_unit`, into the context the children read.
        generated: _,
    } = unit;
    let mut subtree_edges = edges;
    // What this compilation made, as against what it reached: its own edges,
    // then each recipe's wrapper and the children that recipe composed.
    let mut fresh_edges = subtree_edges.clone();
    let disk = freshness_disk(descendant_context, state)?;
    let (ordered, mut holds) = order::dependency_ordered(subninjas, sink);
    let mut read_ahead = read_ahead(&ordered, resolve, descendant_context, state, ahead);
    let mut serial_jobs = order::SerialJobs::default();
    for (pending_index, mut pending) in ordered.into_iter().enumerate() {
        // Held rather than merely skipped, and that is the whole of what makes
        // the hold reach along a chain. A recipe left here has no wrapper in
        // the graph, so its outputs have nothing generating them; the recipe
        // after it reads one of those outputs and would stage it as a boundary
        // — asking the provisional build for a node no edge makes. Recording
        // it holds every reader behind it in turn, which is the invariant
        // [`Holds::reads_a_held_recipe`] is written to state.
        if holds.reads_a_held_recipe(pending_index) {
            holds.hold(pending_index);
            continue;
        }
        // Whose recipe this is decides which switches its segments run under.
        // A recursive recipe the makefile update would run — a Makefile's own,
        // or that of anything a Makefile is made from — is part of the phase
        // GNU Make turns `-n`, `-t` and `-q` off across; a goal's keeps what
        // the command line gave. Asked here because this is the only place
        // where the wrapper's outputs and the read's Makefiles are both in
        // hand: by the time the staged work is built, the wrapper is still
        // wearing the freshness probe and the staged edge is not yet linked to
        // it, so nothing downstream could work it out.
        let for_makefile =
            made_for_a_makefile(sink, &state.remakes, &pending.outputs().collect::<Vec<_>>());
        let parent_inputs = pending.evaluation_inputs();
        // Work an earlier pass put on the ground stays on it: the closure that
        // pass built is replaced with phony so the final graph runs none of it
        // twice. This is the question [`CompilationState::stage`] asks first,
        // asked here on its own because the answer is wanted before the
        // freshness below — it is a fact about what has already run, and holds
        // whether or not this recipe still has anything left to do.
        if !parent_inputs.is_empty() && state.staged_already(compilation_key, pending_index) {
            sink.mark_subgraphs_prebuilt(&parent_inputs);
        }
        let begun = state.recipe_begun(compilation_key, pending_index);
        // `-B` outranks the disk for the COMPOSITION exactly as it does in the
        // build: GNU Make's `always_make_flag` makes every target with a recipe
        // out of date, a recursive recipe is a recipe, and a wrapper the switch
        // is going to force has to reach the graph as a recursion rather than
        // be short-circuited to a phony. In the phase the switch is on for:
        // GNU turns it off across the makefile update of a restarted read and
        // back on for the goals, and `for_makefile` is which phase this recipe
        // belongs to. See [`CompilationContext::restarted`].
        //
        // And it stops at the composition. `recipe_begun` on the finished edge
        // is a fact about work that has already happened, which no switch can
        // make untrue and no phase can turn off; spelling `-B` that way makes
        // the update force what GNU has stopped forcing.
        let forced =
            descendant_context.always_make && !(for_makefile && descendant_context.restarted);
        let wrapper = match stage_recursive_wrapper(
            sink,
            &mut pending,
            &disk,
            begun || forced,
            descendant_context,
        )? {
            RecursiveWrapper::Current(wrapper) => {
                subtree_edges.push(wrapper);
                continue;
            }
            RecursiveWrapper::Dirty(wrapper) => wrapper,
        };
        // Only a recipe that is going to run needs its compiler inputs on the
        // ground, because only a recipe that is going to run has a child to
        // compile, and it is the child that reads them. A wrapper the graph
        // has just answered for has no child: what made it current is that
        // every one of those inputs is already clean, so a provisional build
        // of them would build nothing and the pass that waits for it learns
        // only what this line has learnt already. GNU Make asks in the same
        // order — `Considering target file 'zutil.mdh'` down to `No need to
        // remake target 'zutil.mdh'`, and the `$(MAKE)` on its recipe line is
        // never reached.
        //
        // Staged the other way round it costs a pass each, and the pass reads
        // the whole compilation again: zsh's generated `Src/Modules/Makefile`
        // holds two recursive recipes per module and made 61 passes over its
        // 3,231 lines to compose ten children, where a `modobjs` with no
        // recursion in it reads the same file once.
        if !parent_inputs.is_empty()
            && !state.stage(
                compilation_key,
                pending_index,
                EvaluationPredecessor::ParentPrerequisites,
                &parent_inputs,
                for_makefile,
            )
        {
            state.unfinished.extend(pending.outputs());
            holds.hold(pending_index);
            continue;
        }
        // Staging batches; composing does not. A boundary is a prerequisite of
        // the wrapper that reads it, so it sits BELOW that wrapper and no
        // provisional build of it reaches one — however many of them a pass
        // stages, it stages work the build was going to do. Composing a child
        // is not like that: it puts the child's edges into the graph the next
        // provisional build runs FROM, and two compositions of one makefile
        // are two ISOLATED copies of every rule in it. Both settled against
        // the disk as it was before either ran, both dirty, and a boundary
        // that reaches both runs one recipe twice at once — which for zsh is
        // `signames.c` writing and then deleting `sigtmp.out` while the other
        // copy reads it, `Src/Makefile` reaching it from two of the five goals
        // it hands `Makemod`.
        //
        // `.NOTPARALLEL` does not answer for this and cannot. Its chain is
        // wired onto a composition that COMPLETED, over the edges the build
        // will actually schedule, because threading a wait into the graph a
        // staging pass builds from drags the previous recipe's whole subtree
        // into a provisional build that had no reason to want it — measured on
        // this same tree, twice, both times exit 2. So a pass that has STOPPED
        // INSIDE a composition composes no further child.
        //
        // Stopped inside, and nothing else. A pass composes as many children as
        // it can reach — zsh's tenth pass composes twenty-seven — and a recipe
        // merely HELD, for prerequisites not on the ground or for reading a held
        // recipe, holds itself alone and leaves the ones after it free to
        // compose. Only [`Holds::stopped_inside`] closes the unit, because only
        // a stop leaves a boundary inside a copy for the next build to reach.
        if !holds.composing() {
            state.unfinished.extend(pending.outputs());
            holds.hold(pending_index);
            continue;
        }

        let Some(child_groups) = compose_child_groups(
            &pending,
            RecipeSite {
                at: (compilation_key, pending_index),
                for_makefile,
                read_ahead: read_ahead.get_mut(pending_index).and_then(Option::take),
            },
            sink,
            resolve,
            descendant_context,
            state,
        )?
        else {
            // The wrapper's edge exists and still carries the probe, because
            // only completing it replaces the rule. Whatever asks for these
            // outputs before the next pass would run `false`.
            state.unfinished.extend(pending.outputs());
            holds.stopped_inside();
            holds.hold(pending_index);
            continue;
        };
        // Read before the wrapper is completed, because completing it consumes
        // the recipe. This is the job GNU Make would now be blocked in, so it
        // is what the next recursive recipe of a `.NOTPARALLEL` unit waits for.
        let completion = pending.outputs().collect::<Vec<_>>();
        let wrapper = sink
            .complete_subninja(wrapper, pending, &child_groups, begun)
            .map_err(MakeError::Construct)?;
        if not_parallel && holds.sorted(pending_index) {
            serial_jobs.push(wrapper, &child_groups, completion);
        }
        // Compiled whole: every segment of this recipe is in the graph, so the
        // pass that builds this graph is the one that runs the rest of it.
        state.completed_recipes.insert(RecursiveRecipe {
            compilation_key: compilation_key.to_vec(),
            pending_index,
        });
        order::adopt_child_groups(child_groups, &mut subtree_edges, &mut fresh_edges);
        subtree_edges.push(wrapper);
        fresh_edges.push(wrapper);
    }
    Ok(settled_unit(
        targets,
        subtree_edges,
        fresh_edges,
        &holds,
        &serial_jobs,
        sink,
    ))
}

/// The unit `compose_subninjas` hands back once its recipes have been walked:
/// incomplete while any recipe is held for a later pass, complete — with the
/// `.NOTPARALLEL` chain wired over what will actually run — once none is.
fn settled_unit(
    targets: Vec<Node>,
    edges: Vec<Edge>,
    fresh_edges: Vec<Edge>,
    holds: &order::Holds,
    serial_jobs: &order::SerialJobs,
    sink: &mut GraphSink,
) -> ComposedUnit {
    if holds.any() {
        return ComposedUnit {
            subgraph: UnitSubgraph {
                targets,
                edges,
                fresh_edges,
            },
            complete: false,
        };
    }
    // [spec:ronin:req:make.notparallel-domain]
    serial_jobs.chain(sink);
    ComposedUnit {
        subgraph: UnitSubgraph {
            targets,
            edges,
            fresh_edges,
        },
        complete: true,
    }
}

/// Where one recursive recipe sits in the compilation, and what has already
/// been done for it.
struct RecipeSite<'a> {
    /// The unit's cache key, and which pending recursive recipe of that unit
    /// this is. The same pair an [`EvaluationBoundary`] is keyed on.
    at: (&'a [u8], usize),
    /// Whether this recipe belongs to the makefile update rather than to a
    /// goal, which decides the switches its segments run under.
    for_makefile: bool,
    /// The child's Makefiles, where a worker read them ahead of the
    /// composition. See [`read_ahead`].
    read_ahead: Option<ChildUnit>,
}

/// Compile every child one recursive recipe starts, in the order the recipe
/// wrote them, with what each of them is read off staged first.
///
/// `None` where the compilation stopped at a boundary: something a child is
/// read off is not on the ground yet, and the unit it belongs to is left
/// incomplete for the pass that follows the build of it.
///
/// `site` is where in the compilation this recipe sits, which phase it belongs
/// to, and whatever a worker has already read for it.
// [spec:ronin:req:make.compiler-input-staging+1]
fn compose_child_groups(
    pending: &sink::PendingSubninja,
    site: RecipeSite<'_>,
    sink: &mut GraphSink,
    resolve: Resolver,
    descendant_context: &CompilationContext,
    state: &mut CompilationState<'_>,
) -> Result<Option<Vec<ChildGroup>>, MakeError> {
    let RecipeSite {
        at: (compilation_key, pending_index),
        for_makefile,
        mut read_ahead,
    } = site;
    let mut child_groups = Vec::with_capacity(pending.invocations.len());
    for (group_index, invocation) in pending.invocations.iter().enumerate() {
        // The recipe's own lines ahead of this invocation are compiler input
        // for it, for the same reason its prerequisites are: the child Makefile
        // is read off the disk those lines write to.
        if let Some(staged) = sink
            .stage_preceding_lines(pending, group_index)
            .map_err(MakeError::Construct)?
            && !stage_preceding(
                sink,
                state,
                (compilation_key, pending_index, group_index),
                staged,
                for_makefile,
            )
        {
            return Ok(None);
        }
        // A recipe whose Makefiles a worker already read arrives resolved. Only
        // the first invocation can have been read ahead, because the ones after
        // it are read off what the ones before them staged.
        let resolved = read_ahead.take().unwrap_or_else(|| {
            match resolve(
                &invocation.command,
                &invocation.make,
                &invocation.shell,
                &invocation.shell_flags,
                descendant_context,
            ) {
                Ok(child) => ChildUnit::Unread(Box::new(child)),
                Err(refusal) => ChildUnit::Refused(refusal),
            }
        });
        let child = match resolved {
            // A report names the invocation and reads the rest of the build. A
            // build cannot: there is no child graph to compose and the recipe
            // line that would have started a Make of its own was lifted out of
            // the recipe, so the work would simply not happen.
            ChildUnit::Refused(MakeError::MissingChildMakefile { directory })
                if descendant_context.reporting =>
            {
                child_groups.push(unreadable_child(descendant_context, invocation, &directory));
                continue;
            }
            ChildUnit::Refused(refusal) => return Err(refusal),
            resolved => resolved,
        };
        let child_key = child.cache_key().to_vec();
        let group = if let Some(subgraph) = state.cache.get(&child_key) {
            ChildGroup {
                subgraph: subgraph.clone(),
                fresh: false,
            }
        } else if state.unfinished_units.contains(&child_key) {
            // An earlier recipe of this pass stopped inside this very unit, so
            // there is nothing to hand over and nothing to be gained by reading
            // it again — reading it again would run its `$(shell)` calls a
            // second time and emit a second copy of its edges. This recipe
            // waits for the same staged work that one is waiting for.
            // See [`CompilationState::unfinished_units`].
            return Ok(None);
        } else {
            let child_scope = pending.scope;
            let enclosing = enclosing::for_the_child_of(&descendant_context.enclosing, pending);
            let child = compile_unit(child, sink, Some(child_scope), &enclosing, resolve, state)?;
            if !child.complete {
                state.unfinished_units.insert(child_key);
                return Ok(None);
            }
            state.cache.insert(child_key, child.subgraph.clone());
            ChildGroup {
                subgraph: child.subgraph,
                fresh: true,
            }
        };
        child_groups.push(group);

        if group_index + 1 < pending.invocations.len()
            && !stage_completed_group(
                sink,
                state,
                (compilation_key, pending_index, group_index),
                &child_groups[group_index].subgraph.targets,
                for_makefile,
            )
        {
            return Ok(None);
        }
    }
    Ok(Some(child_groups))
}

/// Whether the makefile update would run this recursive recipe.
///
/// GNU Make brings a Makefile up to date by updating everything it is MADE
/// FROM, with `-n`, `-t` and `-q` off across the whole of that phase. So a
/// recipe reached from a Makefile this read consulted — its own, or that of
/// anything the Makefile is made from — belongs to the update; one nothing in
/// the read reaches belongs to the goals and keeps the switches the command
/// line gave. Probed against 4.4.1 rather than reasoned from: a `gen.mk:
/// helper` whose `helper` holds the recursion is remade for real under all
/// three switches exactly as one whose own recipe holds it is.
///
/// Walked through `prerequisites_of` for the reason `dependency_ordered` walks
/// it: held recursive edges are not in the graph while this decision is being
/// made, so everything between one wrapper and a Makefile is an ordinary
/// target.
fn made_for_a_makefile(sink: &GraphSink, makefiles: &[Node], outputs: &[Node]) -> bool {
    let mut walked = HashSet::new();
    let mut frontier = makefiles.to_vec();
    while let Some(node) = frontier.pop() {
        if !walked.insert(node) {
            continue;
        }
        if outputs.contains(&node) {
            return true;
        }
        frontier.extend(sink.prerequisites_of(node));
    }
    false
}

/// Stage the recipe's own lines written ahead of one invocation, and say
/// whether the staging settled. See [`stage_completed_group`] for `at`.
fn stage_preceding(
    sink: &mut GraphSink,
    state: &mut CompilationState<'_>,
    at: (&[u8], usize, usize),
    staged: Node,
    for_makefile: bool,
) -> bool {
    let (compilation_key, pending_index, group_index) = at;
    if !state.stage(
        compilation_key,
        pending_index,
        EvaluationPredecessor::PrecedingLines(group_index),
        &[staged],
        for_makefile,
    ) {
        return false;
    }
    sink.mark_subgraphs_prebuilt(&[staged]);
    true
}

/// Stage a finished child group as what the next invocation in the same recipe
/// waits on, and say whether the staging settled.
///
/// GNU Make runs a recipe's lines in the order they were written, so the Make a
/// later invocation starts reads whatever an earlier one left behind.
///
/// `at` is where in the compilation this group sits: the unit's key, which
/// pending recursive recipe it belongs to, and which invocation of that recipe
/// it is.
fn stage_completed_group(
    sink: &mut GraphSink,
    state: &mut CompilationState<'_>,
    at: (&[u8], usize, usize),
    completed: &[Node],
    for_makefile: bool,
) -> bool {
    let (compilation_key, pending_index, group_index) = at;
    if !state.stage(
        compilation_key,
        pending_index,
        EvaluationPredecessor::ChildGroup(group_index),
        completed,
        for_makefile,
    ) {
        return false;
    }
    sink.mark_subgraphs_prebuilt(completed);
    true
}

/// Write a composition whose child could not be read into the census, and give
/// back the nothing it composed.
///
/// The entry follows the `Composed` one the classifier already made for the
/// same line rather than replacing it: composing is what the compiler decided
/// about the recipe, and this is what happened when it acted on the decision.
/// The directory is named as a reader would write it, relative to the build's
/// root, because a report is read beside the tree it is about.
///
/// An empty subgraph rather than no subgraph, because the recursive wrapper is
/// completed against one child group per invocation.
fn unreadable_child(
    context: &CompilationContext,
    invocation: &crate::make::sink::SubninjaInvocation,
    directory: &std::path::Path,
) -> ChildGroup {
    context.census.record(kati::census::Invocation {
        location: invocation.location.clone(),
        disposition: kati::census::Disposition::MissingMakefile {
            directory: directory
                .strip_prefix(&context.root_directory)
                .unwrap_or(directory)
                .display()
                .to_string(),
        },
    });
    ChildGroup {
        subgraph: UnitSubgraph {
            targets: Vec::new(),
            edges: Vec::new(),
            fresh_edges: Vec::new(),
        },
        fresh: true,
    }
}

/// The disk a unit's recursive wrappers ask about their targets' dates.
///
/// Held from one unit to the next in `state`. Standing one up walks every
/// component of the build root with the kernel — `realpath` and a `stat` on top
/// of it — and then opens the directory to hold it for relative stats, and the
/// build root is one directory for the whole compilation: every unit below the
/// first was asking the same question of the same path and paying the same
/// syscalls for the same answer. The interface shares its open descriptor
/// across clones, so the compilation now opens the root once rather than once
/// per unit as well.
///
/// Keyed by the root the context names rather than assumed constant, so a unit
/// that arrives with a different root gets its own answer. What the memo does
/// change is that the build root's existence is read at the first unit that
/// asks and not again: a root removed midway through a composition is no longer
/// noticed by the unit that trips over it, which is the same answer the
/// composition would have given had that unit been composed first.
fn freshness_disk(
    context: &CompilationContext,
    state: &mut CompilationState<'_>,
) -> Result<crate::os::RealDiskInterface, MakeError> {
    if let Some((root, disk)) = &state.freshness_disk
        && root == &context.root_directory
    {
        return Ok(disk.clone());
    }
    let working_directory = if context.root_directory.as_os_str().is_empty() {
        crate::os::WorkingDirectory::default()
    } else {
        crate::os::WorkingDirectory::new(&context.root_directory).map_err(|error| {
            MakeError::Evaluate(format!(
                "opening Make build directory for freshness: {error}"
            ))
        })?
    };
    let disk = crate::os::RealDiskInterface::new(working_directory);
    state.freshness_disk = Some((context.root_directory.clone(), disk.clone()));
    Ok(disk)
}

enum RecursiveWrapper {
    Current(Edge),
    Dirty(Edge),
}

/// Decide whether one recursive recipe has to run, before any child Makefile
/// of it has been read.
///
/// `begun` outranks the disk. A recipe whose earlier lines have already run at
/// a compilation boundary is a recipe GNU Make is in the middle of, and one of
/// those lines may have written the target this wrapper makes: asking the disk
/// again would read the recipe's own work as evidence that the recipe need not
/// run. GNU Make asks once, before the first line, and then runs all of it.
fn stage_recursive_wrapper(
    sink: &mut GraphSink,
    pending: &mut sink::PendingSubninja,
    disk: &crate::os::RealDiskInterface,
    begun: bool,
    context: &CompilationContext,
) -> Result<RecursiveWrapper, MakeError> {
    let asserted = crate::runtime::AssertedDates {
        new: &context.assumed_new,
        old: &context.assumed_old,
    };
    let edge = sink.probe_subninja(pending).map_err(MakeError::Construct)?;
    let mut stat = |path: &std::path::Path| disk.stat(path);
    let dirty = begun
        || sink
            .settle_subninja_freshness(edge, &mut stat, asserted)
            .map_err(|error| MakeError::Evaluate(error.to_string()))?;
    Ok(if dirty {
        RecursiveWrapper::Dirty(edge)
    } else {
        RecursiveWrapper::Current(edge)
    })
}

/// Apply Make's `export`/`unexport` result to the environment imported by a
/// semantic child compiler session.
fn apply_exported_environment(
    environment: &mut std::sync::Arc<Vec<(OsString, OsString)>>,
    changes: &[(OsString, Option<OsString>)],
) {
    // A unit that exports nothing keeps its parent's environment rather than
    // copying it to sort it back into the order it was already in. This is the
    // common case and the whole point of sharing the snapshot: on a tree that
    // only dispatches, no unit changes the environment its parent settled, so
    // nothing below the root ever copies it.
    if changes.is_empty() {
        return;
    }
    // Only now is a copy owed, and `make_mut` is what makes it: the snapshot
    // this unit shares with its parent becomes this unit's own.
    let environment = std::sync::Arc::make_mut(environment);
    // The environment reaches here having just been cloned out of a parent's
    // context, so its capacity is exactly its length and the first name pushed
    // into it moves the whole of it. Asked for once, up front, rather than a
    // growth per name.
    environment.reserve(changes.len());
    for (name, value) in changes {
        environment.retain(|(candidate, _)| candidate != name);
        if let Some(value) = value {
            environment.push((name.clone(), value.clone()));
        }
    }
    environment.sort_unstable_by(|left, right| left.0.cmp(&right.0));
}

/// Keep only the last change for each recipe variable while carrying a nested
/// unit's overrides into its descendants.
fn apply_recipe_environment(
    environment: &mut Vec<(OsString, Option<OsString>)>,
    changes: &[(OsString, Option<OsString>)],
) {
    // Reserved for the reason the exported environment is: see above.
    environment.reserve(changes.len());
    for (name, value) in changes {
        environment.retain(|(candidate, _)| candidate != name);
        environment.push((name.clone(), value.clone()));
    }
}

/// Evaluate one unit from its Make working directory and restore the caller's
/// directory before any child unit is entered.
///
/// The frontend needs it too: a child invocation's makefile is named relatively
/// — `Makefile`, not `sub/Makefile` — so anything that reads it before the unit
/// is entered resolves the name against the wrong directory.
pub(in crate::make) fn in_directory<T>(
    directory: &std::path::Path,
    evaluate: impl FnOnce() -> Result<T, MakeError>,
) -> Result<T, MakeError> {
    let previous = std::env::current_dir().map_err(|error| {
        MakeError::Evaluate(format!(
            "reading current directory for Make compilation: {error}"
        ))
    })?;
    if previous != directory {
        std::env::set_current_dir(directory).map_err(|error| {
            MakeError::Evaluate(format!(
                "entering Make compilation directory '{}': {error}",
                directory.display()
            ))
        })?;
    }
    let result = evaluate();
    let restored = if previous == directory {
        Ok(())
    } else {
        std::env::set_current_dir(&previous).map_err(|error| {
            MakeError::Evaluate(format!(
                "restoring Make compilation directory '{}': {error}",
                previous.display()
            ))
        })
    };
    match (result, restored) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

/// The evaluator's refusals in the frontend's own error type.
///
/// More than one only under `-k`, where `complain()` reports instead of dying
/// and the update walks on to the makefile after the one it could not make.
fn refused_makefiles(refusals: Vec<kati::dep::Refusal>) -> Vec<RefusedMakefile> {
    refusals
        .into_iter()
        .map(|refusal| RefusedMakefile {
            complaint: refusal.complaint,
            error: MakeError::evaluate(&refusal.error),
        })
        .collect()
}

/// Add the generated Makefiles to the roots the graph walk has to reach, and
/// answer with the names by which the frontend asks for them back.
///
/// They are roots of the same graph as the goals and their edges have to be in
/// it, but they are not goals: they come after them, so a generated include
/// never displaces the default target, and asking for one is the frontend's
/// separate decision to make.
fn admit_regeneration_roots(
    nodes: &mut Vec<kati::dep::NamedDepNode>,
    regeneration_roots: Vec<kati::dep::RegenerationRoot>,
) -> RegenerationNames {
    let mut names = RegenerationNames::default();
    for root in regeneration_roots {
        names.all.push(root.node.0);
        if !root.required {
            names.forgiven.push(root.node.0);
        }
        if root.unread {
            names.unread.push(root.node.0);
        }
        if let Some(complaint) = root.complaint {
            names.complaints.push((root.node.0, complaint));
        }
        nodes.push(root.node);
    }
    names
}

/// The Makefiles this read consulted that a rule says how to remake, by name.
#[derive(Default)]
struct RegenerationNames {
    all: Vec<kati::symtab::Symbol>,
    /// The ones every `include` of which said the file need not be there.
    forgiven: Vec<kati::symtab::Symbol>,
    /// The ones the read wanted and did not get, whatever is at their name.
    unread: Vec<kati::symtab::Symbol>,
    /// The located complaint each unread required one holds until its own rule
    /// loses, which is when GNU Make says it.
    complaints: Vec<(kati::symtab::Symbol, String)>,
}

/// A Makefile compiled into the complete graph the engine executes.
pub struct Loaded {
    /// What the Makefile builds.
    pub graph: BuildGraph,
    /// Compiler inputs this provisional graph knows how to build.
    regenerations: Vec<Node>,
    /// The staged work among those that belongs to a Makefile's own recipe.
    ///
    /// See [`Self::makefile_staged_targets`]: GNU Make brings the Makefiles up
    /// to date with `-n`, `-t` and `-q` turned off and the goals with them back
    /// on, and a Makefile whose recipe composes a `$(MAKE)` reaches its own
    /// segments through here rather than through the update.
    makefile_staged: Vec<Node>,
    /// The subset of those inputs that are Makefiles this read consulted.
    ///
    /// Building one of the others stages a recursive unit's prerequisites;
    /// building one of these can change what the Makefile says, which is the
    /// only thing that restarts the read.
    remakes: Vec<Node>,
    /// The Makefiles among those whose failure to be remade is forgiven.
    ///
    /// `-include` says the file may be absent, and GNU Make carries that
    /// indifference into the rule that would have made it: the recipe runs, it
    /// fails, nothing is reported and the read goes on without the include.
    forgiven_remakes: Vec<Node>,
    /// The Makefiles among those whose contents the read wanted and did not
    /// get, whatever stands at their name.
    ///
    /// GNU Make says this with a timestamp rather than with a flag:
    /// `eval_makefile` writes `last_mtime = NONEXISTENT_MTIME` on the file it
    /// could not open (read.c:409), so every later question about it is answered
    /// as though nothing were there. That is what makes the rule for an
    /// unopenable makefile run at all — a recipe with no prerequisites is
    /// otherwise up to date the moment its target exists, and an unreadable file
    /// exists.
    unread_remakes: Vec<Node>,
    /// What one of those says if its own rule loses, which is where GNU Make's
    /// second `show_goal_error` caller speaks: `child_error` (job.c:581) prints
    /// the held complaint one line ahead of naming the failure. A rule that wins
    /// starts the read over instead, and the complaint is never made.
    remake_complaints: Vec<(Node, String)>,
    /// The required Makefiles the read could not open and no rule can make.
    ///
    /// GNU Make refuses over one of these from inside the update that brings
    /// the Makefiles up to date: the ones it reached first are remade, and
    /// then the run ends — without restarting the read, however much that
    /// remaking changed. Under `-k` there may be several and the read does start
    /// over, because `complain()` reports rather than dies and `main.c` gets to
    /// ask the question it never reached.
    refusals: Vec<RefusedMakefile>,
    /// Recursive evaluation boundaries satisfied by building those inputs.
    evaluation_boundaries: RapidHashSet<EvaluationBoundary>,
    /// The recursive recipes this read compiled whole.
    ///
    /// Worth carrying out because a recipe cut into segments has to stop being
    /// called begun once it is over — see `CompilationState::finished_recipes`
    /// — and only the caller knows whether this pass was the one that ran it.
    /// Ask [`Self::compilation_ran_to_the_end`] first: a read that stopped at a
    /// boundary builds the staged work alone.
    completed_recipes: RapidHashSet<RecursiveRecipe>,
    /// The targets of every recursive recipe this read staged and did not
    /// finish compiling, whose edges still hold the freshness probe.
    unfinished: Vec<Node>,
    /// The units this read covered, by cache key, each with what the ground
    /// told it.
    units_read: ReadJournals,
    /// The root unit's canonical, fully evaluated `MAKEFLAGS`.
    makeflags: String,
    /// The widest budget any unit of this read asked to run at.
    job_budget: usize,
    /// The recipes this graph's own executor will expand as it launches them,
    /// with the session they belong to.
    pending_recipes: Option<recipe::PendingRecipes>,
}

impl Loaded {
    /// The recipes this graph's executor expands as it launches them.
    ///
    /// Every build over this graph must be given these, including the ones
    /// that build compiler inputs: an edge whose recipe was deferred has no
    /// command until it is asked for.
    pub(crate) const fn take_pending_recipes(&mut self) -> Option<recipe::PendingRecipes> {
        self.pending_recipes.take()
    }

    /// Compiler inputs to build before evaluating this Makefile again.
    ///
    /// Each node is part of [`Self::graph`] and runs through the ordinary Ninja
    /// scheduler. An empty slice means the graph is the final compilation.
    #[must_use]
    pub fn regeneration_targets(&self) -> &[Node] {
        &self.regenerations
    }

    /// The Makefiles this read consulted that a rule says how to remake.
    ///
    /// A subset of [`Self::regeneration_targets`]. Whether building them
    /// changed any of these files is what decides whether the read has to
    /// happen again on the new text.
    #[must_use]
    pub(crate) fn remake_targets(&self) -> &[Node] {
        &self.remakes
    }

    /// The Makefiles among those the read said it did not need.
    ///
    /// A subset of [`Self::remake_targets`]. A rule that fails to make one of
    /// these does not end the run and is not reported, and its file is not
    /// looked at again to decide whether the read has to start over.
    #[must_use]
    pub(crate) fn forgiven_remake_targets(&self) -> &[Node] {
        &self.forgiven_remakes
    }

    /// The Makefiles whose contents this read wanted and did not get.
    ///
    /// A subset of [`Self::remake_targets`]. The build must treat each as a file
    /// that is not there, however its name looks on disk, which is GNU Make's
    /// `last_mtime = NONEXISTENT_MTIME` for a makefile that would not open.
    #[must_use]
    pub(crate) fn unread_remake_targets(&self) -> &[Node] {
        &self.unread_remakes
    }

    /// What each unread Makefile says if its own rule loses.
    ///
    /// Taken rather than read: it is said once, where the update settles that
    /// the file is not coming, and the run ends there.
    pub(crate) fn take_remake_complaints(&mut self) -> Vec<(Node, String)> {
        std::mem::take(&mut self.remake_complaints)
    }

    /// The required Makefile nothing can make, which ends the run.
    ///
    /// Taken rather than read, because raising it is what the caller does with
    /// it and it is raised once. The Makefiles this read reached before it are
    /// in [`Self::remake_targets`] and are brought up to date first: GNU Make
    /// refuses from inside that update, so the work ahead of the refusal is
    /// done and the read never starts over however much of it moved.
    pub(crate) fn take_refusals(&mut self) -> Vec<RefusedMakefile> {
        std::mem::take(&mut self.refusals)
    }

    /// The compiler inputs that are not Makefiles and belong to a goal's
    /// recipe: work staged so a recursive unit can be evaluated at all.
    ///
    /// The other half of [`Self::regeneration_targets`], less what
    /// [`Self::makefile_staged_targets`] claims. GNU Make has no such phase —
    /// it is how a `$(MAKE)` recipe's prerequisites reach the ground before the
    /// child is compiled — so a goal's is built under the invocation's own
    /// switches rather than under the ones a Makefile is remade with.
    #[must_use]
    pub(crate) fn staged_targets(&self) -> Vec<Node> {
        self.regenerations
            .iter()
            .copied()
            .filter(|target| !self.remakes.contains(target))
            .filter(|target| !self.makefile_staged.contains(target))
            .collect()
    }

    /// The staged work that belongs to the makefile update rather than to the
    /// goals.
    ///
    /// A Makefile whose own rule is a recipe holding a composed `$(MAKE)` is
    /// cut into segments, and the segments are staged here instead of being run
    /// by the update — the wrapper is held back from it until the compilation
    /// is finished with. They are still that Makefile's recipe, so they are
    /// built the way the update builds one: `-n`, `-t` and `-q` off, because a
    /// Makefile only pretended to be remade is one whose contents the read
    /// would then have to guess. GNU Make makes the same split in
    /// `update_goal_chain`.
    #[must_use]
    pub(crate) fn makefile_staged_targets(&self) -> Vec<Node> {
        self.makefile_staged
            .iter()
            .copied()
            .filter(|target| !self.remakes.contains(target))
            .collect()
    }

    /// Recursive compiler boundaries satisfied by the provisional build.
    #[must_use]
    pub(crate) const fn evaluation_boundaries(&self) -> &RapidHashSet<EvaluationBoundary> {
        &self.evaluation_boundaries
    }

    /// The targets of every recursive recipe this read staged and did not
    /// finish compiling.
    ///
    /// Their edges hold the freshness probe rather than a recipe, and the
    /// probe's command is `false`. A build made from this graph must not reach
    /// one, which for the Makefiles means holding back whichever of them is
    /// made through one until the pass that finishes the compilation.
    #[must_use]
    pub(crate) fn unfinished_targets(&self) -> &[Node] {
        &self.unfinished
    }

    /// The recursive recipes this pass carried whole, taken out for the passes
    /// that follow.
    ///
    /// EMPTY for a read that stopped at a boundary, however much it composed on
    /// the way there. Such a pass builds the staged work and nothing else, so a
    /// wrapper it assembled has not run and its recipe is still in flight —
    /// which is the difference between a recipe a pass finished and one it
    /// merely put together. A read that reached the end of every unit hands
    /// over a graph holding every recipe whole, and the pass over it is the one
    /// that BUILDS: the makefile update, and then the goals.
    #[must_use]
    pub(crate) fn take_recipes_carried_whole(&mut self) -> RapidHashSet<RecursiveRecipe> {
        if self.evaluation_boundaries.is_empty() {
            std::mem::take(&mut self.completed_recipes)
        } else {
            RapidHashSet::default()
        }
    }

    /// The units this pass read, which a later pass over the same text is
    /// repeating rather than performing.
    #[must_use]
    pub(crate) fn take_units_read(&mut self) -> ReadJournals {
        std::mem::take(&mut self.units_read)
    }

    /// The two things the read settled about the invocation itself: the switch
    /// state the Makefile left for its own build and its children, and how many
    /// recipes the graph must be allowed to run at once.
    ///
    /// The budget is the run's own `-j` unless a unit's makefiles asked for
    /// more, in which case it is the widest of them: every unit not in that
    /// unit's group is held to its own budget by a pool, and a scheduler kept
    /// at the narrower number would hold the widened one back with it.
    #[must_use]
    pub(crate) fn settled_invocation(&self) -> (&str, usize) {
        (&self.makeflags, self.job_budget)
    }
}

/// One unit's settled export set, and the one name it could not read.
///
/// The name comes back beside the changes rather than as a failure because GNU
/// Make only ever reads such a value where it starts something; see
/// [`kati::export::Unreadable`].
type SettledExports = (
    Vec<(OsString, Option<OsString>)>,
    Option<kati::export::Unreadable>,
);

/// The environment changes Make's export set makes to a child's, as bytes the
/// front end can put in an `env` prefix or hand to a semantic child compiler.
///
/// The decision itself is Make semantics and lives in kati beside the variable
/// store it reads; this is the translation of its answer into the front end's
/// string type.
fn exported_environment(
    ev: &mut kati::eval::Evaluator,
) -> Result<SettledExports, kati::anyhow::Error> {
    let (changes, unreadable) =
        kati::export::settled_environment(ev, None, kati::export::ChildKind::Recipe)?;
    Ok((as_environment(&changes), unreadable))
}

/// One of kati's environment deltas as the front end's own strings.
fn as_environment(
    changes: &[kati::export::EnvironmentChange],
) -> Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> {
    use std::os::unix::ffi::OsStringExt;
    changes
        .iter()
        .map(|(name, value)| {
            (
                std::ffi::OsString::from_vec(name.to_vec()),
                value
                    .as_ref()
                    .map(|value| std::ffi::OsString::from_vec(value.to_vec())),
            )
        })
        .collect()
}

fn evaluated_flag_variables(
    ev: &mut kati::eval::Evaluator,
) -> Result<(String, String), kati::anyhow::Error> {
    let makeflags = ev.session.intern("MAKEFLAGS");
    let makeflags = String::from_utf8_lossy(&ev.eval_var(makeflags)?).into_owned();
    let mflags = ev.session.intern("MFLAGS");
    let mflags = String::from_utf8_lossy(&ev.eval_var(mflags)?).into_owned();
    Ok((makeflags, mflags))
}

fn flag_recipe_environment(makeflags: &str, mflags: String) -> [(OsString, Option<OsString>); 2] {
    [
        (OsString::from("MAKEFLAGS"), Some(OsString::from(makeflags))),
        (OsString::from("MFLAGS"), Some(OsString::from(mflags))),
    ]
}

/// Command-line bindings a semantic child receives through its compiler
/// environment in addition to MAKEFLAGS.
///
/// Normally the same bindings also arrive as command-line assignments and win
/// there. A Makefile that clears `MAKEOVERRIDES` removes that half, leaving the
/// environment-origin value GNU Make exposes to its child. Explicit
/// `export`/`unexport` results are applied after this list and can replace it.
///
/// `settled` is what the export pass already expanded, and a name it settled is
/// read from there rather than expanded a second time. A command-line variable
/// is exported without being asked, so nearly every name here is one of those —
/// and settling a value twice is not the same as settling it once when a
/// `$(shell)` in it has an effect. GNU Make expands an exported recursive value
/// once per environment it builds (`target_environment`, variable.c), never
/// twice for the same one. What is left to expand is the command-line name the
/// export pass declined: a name no shell could read back, or one the makefile
/// took back with `unexport`.
///
/// `unreadable` is the name that pass could not read at all, which is left out
/// here for the same reason it was left out there: expanding it would raise the
/// refusal GNU Make raises only at the job that carries the value.
fn command_line_environment(
    ev: &mut kati::eval::Evaluator,
    settled: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
    unreadable: Option<&kati::export::Unreadable>,
) -> Result<Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>, kati::anyhow::Error> {
    use std::os::unix::ffi::OsStringExt;
    let variables = ev
        .session
        .globals
        .matching(|variable| variable.read().origin() == kati::var::VarOrigin::CommandLine);
    let mut environment = Vec::with_capacity(variables.len());
    for (symbol, _) in variables {
        let name = std::ffi::OsString::from_vec(symbol.as_bytes(&ev.session).to_vec());
        if unreadable.is_some_and(|held| held.name == name.as_os_str().as_encoded_bytes()) {
            continue;
        }
        let value = match settled.iter().find(|(settled, _)| *settled == name) {
            Some((_, value)) => value.clone(),
            None => Some(std::ffi::OsString::from_vec(ev.eval_var(symbol)?.to_vec())),
        };
        environment.push((name, value));
    }
    Ok(environment)
}
