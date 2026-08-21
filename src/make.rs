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
mod layout;
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
use crate::make::sink::{UnitOutput, UnitSubgraph};
use kati::build_sink::RecipeExpansion;
use kati::evaluate::{Evaluated, evaluate};
use kati::ninja::emit_build;
use kati::session::Session;
use std::collections::{BTreeSet, HashMap, HashSet};
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
}

impl fmt::Display for MakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evaluate(diagnostic) => formatter.write_str(diagnostic),
            Self::Construct(error) => error.fmt(formatter),
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
            Self::Evaluate(_) | Self::MissingChildMakefile { .. } => None,
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
// [spec:ronin:req:make.state-outside-the-tree+2]
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
    let environment = session
        .invocation_environment
        .clone()
        .unwrap_or_else(|| std::env::vars_os().collect::<Vec<_>>());
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
            census: std::sync::Arc::clone(&session.census),
            reporting: false,
            root_directory: directory.clone(),
            directory,
            path_prefix: PathBuf::new(),
            makeflags: environment_value("MAKEFLAGS").unwrap_or_default(),
            // No invocation to have carried the switch: this entry compiles a
            // Makefile for a caller that runs the graph itself.
            always_make: false,
            level,
            jobs: session.flags.num_jobs.max(1),
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
        &HashSet::new(),
        // One pass, so nothing is read twice, nothing is repeating, and
        // nothing has an earlier read's answers to be handed.
        &ReadJournals::new(),
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
    pub(crate) level: usize,
    pub(crate) jobs: usize,
    /// The environment this unit imports while kati evaluates it.
    pub(crate) environment: Vec<(OsString, OsString)>,
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
// [spec:ronin:req:make.narration+1]
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
pub(crate) type ReadJournals = HashMap<Vec<u8>, UnitJournal>;

struct CompilationState<'a> {
    cache: HashMap<Vec<u8>, UnitSubgraph>,
    compiling: HashSet<Vec<u8>>,
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
    settled_boundaries: &'a HashSet<EvaluationBoundary>,
    evaluation_boundaries: HashSet<EvaluationBoundary>,
    /// The outputs of every recursive wrapper this pass staged and did not
    /// finish, because the compilation stopped at a boundary before its
    /// children were composed.
    ///
    /// Their edges are still wearing the freshness probe, whose command is
    /// `false` and which is never allowed to execute, so nothing this pass
    /// builds may reach one of them.
    unfinished: Vec<Node>,
    /// The units an earlier pass of this compilation already read, by cache
    /// key, each with what that read was told by the ground. A unit in here is
    /// being read again over text that has not moved, so what its read does on
    /// the way through was done then and is not done now — see
    /// `kati::flags::Flags::is_repeated_read` — and what it was told then is
    /// what it is told again, because the ground has moved and GNU Make's one
    /// read never saw it move.
    read_units: &'a ReadJournals,
    /// Every unit this pass read, which is the set the next pass repeats. It
    /// accumulates rather than replacing, because a pass reads everything the
    /// pass before it read and then one child more.
    units_read: ReadJournals,
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

    /// Whether this recursive recipe has already run part of itself.
    ///
    /// A settled boundary is work an earlier pass of this same invocation put
    /// on the ground. Two of the three kinds are the recipe's own: the lines
    /// written ahead of an invocation, and a child group an earlier invocation
    /// in the same recipe already started. The third — the wrapper's
    /// prerequisites — is not the recipe, it is what GNU Make settles before
    /// the recipe begins, and building it is no reason to call the recipe
    /// started.
    fn recipe_begun(&self, compilation_key: &[u8], pending_index: usize) -> bool {
        self.settled_boundaries.iter().any(|boundary| {
            boundary.compilation_key == compilation_key
                && boundary.pending_index == pending_index
                && matches!(
                    boundary.predecessor,
                    EvaluationPredecessor::PrecedingLines(_) | EvaluationPredecessor::ChildGroup(_)
                )
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
// [spec:ronin:req:make.recursive-invocation+2]
// [spec:ronin:req:make.compiler-boundary]
pub(crate) fn load_with_subninjas<F>(
    root: Compilation,
    resolve: F,
    settled_boundaries: &HashSet<EvaluationBoundary>,
    read_units: &ReadJournals,
    expansion: RecipeExpansion,
) -> Result<Loaded, MakeError>
where
    F: FnMut(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let _directory = compilation_directory_guard();
    load_with_subninjas_unlocked(root, resolve, settled_boundaries, read_units, expansion)
}

fn load_with_subninjas_unlocked<F>(
    root: Compilation,
    mut resolve: F,
    settled_boundaries: &HashSet<EvaluationBoundary>,
    read_units: &ReadJournals,
    expansion: RecipeExpansion,
) -> Result<Loaded, MakeError>
where
    F: FnMut(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let mut sink = GraphSink::new_at(&root.context.root_directory, expansion);
    let mut state = CompilationState {
        cache: HashMap::new(),
        compiling: HashSet::new(),
        pending_recipes: recipe::PendingRecipes::new(std::sync::Arc::clone(
            &root.context.diagnostics,
        )),
        regenerations: Vec::new(),
        makefile_staged: Vec::new(),
        remakes: Vec::new(),
        forgiven_remakes: Vec::new(),
        unread_remakes: Vec::new(),
        remake_complaints: Vec::new(),
        settled_boundaries,
        refusals: Vec::new(),
        evaluation_boundaries: HashSet::new(),
        unfinished: Vec::new(),
        read_units,
        units_read: ReadJournals::new(),
    };
    let root = compile_unit(root, &mut sink, None, &mut resolve, &mut state)?;
    let pending_recipes = (!state.pending_recipes.is_empty()).then_some(state.pending_recipes);
    let graph = sink.into_graph().map_err(MakeError::Construct)?;
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
        unfinished: state.unfinished,
        units_read: state.units_read,
        makeflags: root.makeflags,
    })
}

/// One unit's makefile read, and the build it describes emitted into the sink.
///
/// A function rather than a closure so the thing it produces has a name: the
/// read is what a pass repeats, and every field below is something the read
/// settled and the compilation around it then carries.
fn read_unit(
    session: Session,
    sink: &mut GraphSink,
    parent_scope: Option<Scope>,
    context: &CompilationContext,
) -> Result<UnitRead, MakeError> {
    let Evaluated {
        mut ev,
        mut nodes,
        regeneration_nodes,
        refusals,
    } = evaluate(session).map_err(|error| MakeError::evaluate(&error))?;
    let refusals = refused_makefiles(refusals);
    let regeneration_names = admit_regeneration_roots(&mut nodes, regeneration_nodes);
    let exported = exported_environment(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
    let command_line =
        command_line_environment(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
    // A Makefile may replace MAKEOVERRIDES (and therefore the recursive
    // MAKEFLAGS value) before naming a child. That evaluated compiler
    // variable, not the invocation's pre-evaluation seed, is what the
    // semantic subninja parses.
    let (makeflags, mflags) =
        evaluated_flag_variables(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
    let flag_environment = flag_recipe_environment(&makeflags, mflags);
    if let Some(parent) = parent_scope {
        sink.begin_subninja(
            parent,
            context.path_prefix.clone(),
            context.directory.clone(),
        );
    }
    sink.serialise_unit(ev.session.flags.not_parallel);
    let mut recipe_environment = context.recipe_environment.clone();
    apply_recipe_environment(&mut recipe_environment, &flag_environment);
    apply_recipe_environment(&mut recipe_environment, &exported);
    sink.set_recipe_environment(recipe_environment);
    let deferred = match emit_build(&nodes, &mut ev, sink) {
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
    let pending_recipes = (!deferred.is_empty()).then_some((ev, deferred, layout, deferred_edges));
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
    settled_edges: Vec<(Edge, Vec<crate::build::LateStep>)>,
    /// What this read was told from outside itself, for the read that repeats
    /// it.
    journal: UnitJournal,
}

fn compile_unit<F>(
    compilation: Compilation,
    sink: &mut GraphSink,
    parent_scope: Option<Scope>,
    resolve: &mut F,
    state: &mut CompilationState<'_>,
) -> Result<CompiledUnit, MakeError>
where
    F: FnMut(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let compilation_key = compilation.cache_key.clone();
    if !state.compiling.insert(compilation_key.clone()) {
        return Err(MakeError::Evaluate(
            "recursive Make compilation includes itself".to_owned(),
        ));
    }
    let context = compilation.context.clone();
    let mut session = compilation.session;
    // `--shuffle` is Make's own reordering rather than this frontend's: the
    // walk that drops circular prerequisites reads the order it chose, so the
    // evaluator has to be told before it plans.
    session.flags.shuffle = compilation.shuffle;
    // A `$(shell)` in this unit's makefile and a recipe line in the graph it
    // compiles to are the same language, so the read uses the shell the build
    // will use. Per unit, because a recursive child reads with its own session
    // and must not read with a different shell than its parent.
    // [spec:ronin:req:product.builtin-shell]
    session.flags.default_shell_program =
        crate::subprocess::builtin_shell().map(std::path::Path::to_path_buf);
    // Per unit rather than per pass, because a staging pass reads units it has
    // read before AND one it has not: the parent and the children behind the
    // settled boundaries are repeating themselves, while the child the pass
    // was taken for is speaking for the first time.
    let replayed = state.read_units.get(&compilation_key);
    session.flags.is_repeated_read = replayed.is_some();
    // And what that first read was TOLD, for the calls that cannot be held
    // back because the expansion that asked has to be handed a value. A unit
    // reading for the first time — the child this pass was taken for — gets
    // nothing and asks the ground itself.
    session.ground_journal.replay(
        replayed
            .map(|journal| journal.ground.clone())
            .unwrap_or_default(),
    );
    // And the text it READ, which is the other half of the same premise. A
    // makefile a staged child has since written was not part of the first read
    // and must not become part of this one; one it has rewritten is still the
    // text the first read had.
    for (name, contents) in replayed.map_or(&[][..], |journal| &journal.sources) {
        session.supply_makefile(name.clone(), contents.clone());
    }
    let read = in_directory(&context.directory, || {
        read_unit(session, sink, parent_scope, &context)
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
    } = match read {
        Ok(read) => read,
        Err(error) => {
            state.compiling.remove(&compilation_key);
            return Err(error);
        }
    };
    state.units_read.insert(compilation_key.clone(), journal);
    state.retain(pending_recipes, &context.directory);
    state.pending_recipes.admit_settled(settled_edges);
    state.admit(unit_remakes);

    let mut descendant_context = context;
    descendant_context.makeflags.clone_from(&makeflags);
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
    );
    state.compiling.remove(&compilation_key);
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

fn compose_subninjas<F>(
    unit: UnitOutput,
    compilation_key: &[u8],
    sink: &mut GraphSink,
    resolve: &mut F,
    descendant_context: &CompilationContext,
    state: &mut CompilationState<'_>,
) -> Result<ComposedUnit, MakeError>
where
    F: FnMut(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let UnitOutput {
        targets,
        subninjas,
        edges,
    } = unit;
    let mut subtree_edges = edges;
    let disk = freshness_disk(descendant_context)?;
    for (pending_index, mut pending) in dependency_ordered(subninjas, sink).into_iter().enumerate()
    {
        // Whose recipe this is decides which switches its segments run under.
        // A recursive recipe the makefile update would run — a Makefile's own,
        // or that of anything a Makefile is made from — is part of the phase
        // GNU Make turns `-n`, `-t` and `-q` off across; a goal's keeps what
        // the command line gave. Asked here because this is the only place
        // where the wrapper's outputs and the read's Makefiles are both in
        // hand: by the time the staged work is built, the wrapper is still
        // wearing the freshness probe and the staged edge is not yet linked to
        // it, so nothing downstream could work it out.
        let outputs = pending.outputs().collect::<Vec<_>>();
        let for_makefile = made_for_a_makefile(sink, &state.remakes, &outputs);
        let parent_inputs = pending.evaluation_inputs();
        if !parent_inputs.is_empty() {
            if !state.stage(
                compilation_key,
                pending_index,
                EvaluationPredecessor::ParentPrerequisites,
                &parent_inputs,
                for_makefile,
            ) {
                return Ok(incomplete_unit(targets, subtree_edges));
            }
            sink.mark_subgraphs_prebuilt(&parent_inputs);
        }

        // `-B` outranks the disk here exactly as it does in the build: GNU
        // Make's `always_make_flag` makes every target with a recipe out of
        // date, and a recursive recipe is a recipe.
        let begun =
            state.recipe_begun(compilation_key, pending_index) || descendant_context.always_make;
        let wrapper = match stage_recursive_wrapper(sink, &mut pending, &disk, begun)? {
            RecursiveWrapper::Current(wrapper) => {
                subtree_edges.push(wrapper);
                continue;
            }
            RecursiveWrapper::Dirty(wrapper) => wrapper,
        };

        let Some(child_groups) = compose_child_groups(
            &pending,
            (compilation_key, pending_index),
            sink,
            resolve,
            descendant_context,
            state,
            for_makefile,
        )?
        else {
            // The wrapper's edge exists and still carries the probe, because
            // only completing it replaces the rule. Whatever asks for these
            // outputs before the next pass would run `false`.
            state.unfinished.extend(pending.outputs());
            return Ok(incomplete_unit(targets, subtree_edges));
        };
        let wrapper = sink
            .complete_subninja(wrapper, pending, &child_groups, begun)
            .map_err(MakeError::Construct)?;
        for edge in child_groups.into_iter().flat_map(|child| child.edges) {
            if !subtree_edges.contains(&edge) {
                subtree_edges.push(edge);
            }
        }
        subtree_edges.push(wrapper);
    }
    Ok(ComposedUnit {
        subgraph: UnitSubgraph {
            targets,
            edges: subtree_edges,
        },
        complete: true,
    })
}

/// Compile every child one recursive recipe starts, in the order the recipe
/// wrote them, with what each of them is read off staged first.
///
/// `None` where the compilation stopped at a boundary: something a child is
/// read off is not on the ground yet, and the unit it belongs to is left
/// incomplete for the pass that follows the build of it.
///
/// `at` is where in the compilation this recipe sits: the unit's key and which
/// pending recursive recipe of that unit it is.
fn compose_child_groups<F>(
    pending: &sink::PendingSubninja,
    at: (&[u8], usize),
    sink: &mut GraphSink,
    resolve: &mut F,
    descendant_context: &CompilationContext,
    state: &mut CompilationState<'_>,
    for_makefile: bool,
) -> Result<Option<Vec<UnitSubgraph>>, MakeError>
where
    F: FnMut(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let (compilation_key, pending_index) = at;
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
        let child = match resolve(
            &invocation.command,
            &invocation.make,
            &invocation.shell,
            &invocation.shell_flags,
            descendant_context,
        ) {
            Ok(child) => child,
            // A report names the invocation and reads the rest of the build. A
            // build cannot: there is no child graph to compose and the recipe
            // line that would have started a Make of its own was lifted out of
            // the recipe, so the work would simply not happen.
            Err(MakeError::MissingChildMakefile { directory }) if descendant_context.reporting => {
                child_groups.push(unreadable_child(descendant_context, invocation, &directory));
                continue;
            }
            Err(refusal) => return Err(refusal),
        };
        let child_key = child.cache_key.clone();
        let child_subgraph = if let Some(subgraph) = state.cache.get(&child_key) {
            subgraph.clone()
        } else {
            let child_scope = pending.scope;
            let child = compile_unit(child, sink, Some(child_scope), resolve, state)?;
            if !child.complete {
                return Ok(None);
            }
            state.cache.insert(child_key, child.subgraph.clone());
            child.subgraph
        };
        child_groups.push(child_subgraph);

        if group_index + 1 < pending.invocations.len()
            && !stage_completed_group(
                sink,
                state,
                (compilation_key, pending_index, group_index),
                &child_groups[group_index].targets,
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
) -> UnitSubgraph {
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
    UnitSubgraph {
        targets: Vec::new(),
        edges: Vec::new(),
    }
}

const fn incomplete_unit(targets: Vec<Node>, edges: Vec<Edge>) -> ComposedUnit {
    ComposedUnit {
        subgraph: UnitSubgraph { targets, edges },
        complete: false,
    }
}

fn freshness_disk(context: &CompilationContext) -> Result<crate::os::RealDiskInterface, MakeError> {
    let working_directory = if context.root_directory.as_os_str().is_empty() {
        crate::os::WorkingDirectory::default()
    } else {
        crate::os::WorkingDirectory::new(&context.root_directory).map_err(|error| {
            MakeError::Evaluate(format!(
                "opening Make build directory for freshness: {error}"
            ))
        })?
    };
    Ok(crate::os::RealDiskInterface::new(working_directory))
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
) -> Result<RecursiveWrapper, MakeError> {
    let edge = sink.probe_subninja(pending).map_err(MakeError::Construct)?;
    let mut stat = |path: &std::path::Path| disk.stat(path);
    let dirty = begun
        || sink
            .settle_subninja_freshness(edge, &mut stat)
            .map_err(|error| MakeError::Evaluate(error.to_string()))?;
    Ok(if dirty {
        RecursiveWrapper::Dirty(edge)
    } else {
        RecursiveWrapper::Current(edge)
    })
}

/// Put held recursive edges before any held edge that needs their outputs.
///
/// Kati emits recursive edges in target walk order, which is not necessarily
/// prerequisite order. A provisional compiler graph must nevertheless be able
/// to build a recursive target used as another recursive target's evaluation
/// input. Stable topological order makes that producer available first; Make's
/// ordinary cycle diagnostics remain responsible for a cyclic remainder.
///
/// A wrapper's prerequisite is not necessarily another wrapper's output, so
/// the producer is searched for through whatever ordinary targets stand
/// between the two. zsh's generated `Src/Makemod` is the shape that shows it:
/// `X.mdh` re-invokes the makefile and needs `X.mdhi`, which has an ordinary
/// recipe and needs `X.mdhs`, which re-invokes the makefile too. Comparing
/// only what each wrapper directly reads finds `X.mdh` no producer at all and
/// composes it first, against a provisional graph that has not been given the
/// edge which makes what it asks for.
fn dependency_ordered(
    subninjas: Vec<sink::PendingSubninja>,
    sink: &GraphSink,
) -> Vec<sink::PendingSubninja> {
    let mut producers = HashMap::new();
    for (index, pending) in subninjas.iter().enumerate() {
        for output in pending.outputs() {
            producers.insert(output, index);
        }
    }

    let mut predecessor_counts = vec![0usize; subninjas.len()];
    let mut successors = vec![Vec::new(); subninjas.len()];
    for (consumer, pending) in subninjas.iter().enumerate() {
        let mut predecessors = HashSet::new();
        let mut walked = HashSet::new();
        let mut frontier = pending.evaluation_inputs();
        while let Some(input) = frontier.pop() {
            if !walked.insert(input) {
                continue;
            }
            let Some(&producer) = producers.get(&input) else {
                // Nothing held makes this one, so what makes it is an
                // ordinary edge and the wrapper being looked for is behind
                // that edge rather than at it.
                frontier.extend(sink.prerequisites_of(input));
                continue;
            };
            if producer != consumer && predecessors.insert(producer) {
                predecessor_counts[consumer] += 1;
                successors[producer].push(consumer);
            }
        }
    }

    let mut ready = predecessor_counts
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<BTreeSet<_>>();
    let mut pending = subninjas.into_iter().map(Some).collect::<Vec<_>>();
    let mut ordered = Vec::with_capacity(pending.len());
    while let Some(index) = ready.pop_first() {
        ordered.push(pending[index].take().expect("ready pending edge exists"));
        for successor in &successors[index] {
            predecessor_counts[*successor] -= 1;
            if predecessor_counts[*successor] == 0 {
                ready.insert(*successor);
            }
        }
    }
    ordered.extend(pending.into_iter().flatten());
    ordered
}

/// Apply Make's `export`/`unexport` result to the environment imported by a
/// semantic child compiler session.
fn apply_exported_environment(
    environment: &mut Vec<(OsString, OsString)>,
    changes: &[(OsString, Option<OsString>)],
) {
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
    evaluation_boundaries: HashSet<EvaluationBoundary>,
    /// The targets of every recursive recipe this read staged and did not
    /// finish compiling, whose edges still hold the freshness probe.
    unfinished: Vec<Node>,
    /// The units this read covered, by cache key, each with what the ground
    /// told it.
    units_read: ReadJournals,
    /// The root unit's canonical, fully evaluated `MAKEFLAGS`.
    makeflags: String,
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
    pub(crate) const fn evaluation_boundaries(&self) -> &HashSet<EvaluationBoundary> {
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

    /// The units this pass read, which a later pass over the same text is
    /// repeating rather than performing.
    #[must_use]
    pub(crate) fn take_units_read(&mut self) -> ReadJournals {
        std::mem::take(&mut self.units_read)
    }

    /// The switch state the Makefile left for its own build and its children.
    #[must_use]
    pub(crate) fn makeflags(&self) -> &str {
        &self.makeflags
    }
}

/// The environment changes Make's export set makes to a child's, as bytes the
/// front end can put in an `env` prefix or hand to a semantic child compiler.
///
/// The decision itself is Make semantics and lives in kati beside the variable
/// store it reads; this is the translation of its answer into the front end's
/// string type.
fn exported_environment(
    ev: &mut kati::eval::Evaluator,
) -> Result<Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>, kati::anyhow::Error> {
    Ok(as_environment(&kati::export::exported_environment(
        ev,
        None,
        kati::export::ChildKind::Recipe,
    )?))
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
fn command_line_environment(
    ev: &mut kati::eval::Evaluator,
) -> Result<Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>, kati::anyhow::Error> {
    use std::os::unix::ffi::OsStringExt;
    let variables = ev
        .session
        .globals
        .matching(|variable| variable.read().origin() == kati::var::VarOrigin::CommandLine);
    let mut environment = Vec::with_capacity(variables.len());
    for (name, _) in variables {
        let value = ev.eval_var(name)?;
        environment.push((
            std::ffi::OsString::from_vec(name.as_bytes(&ev.session).to_vec()),
            Some(std::ffi::OsString::from_vec(value.to_vec())),
        ));
    }
    Ok(environment)
}
