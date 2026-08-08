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
mod report;
mod sink;

#[cfg(test)]
mod equivalence;

#[cfg(test)]
mod shuffle_tests;

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

use crate::frontend::{BuildGraph, FrontendError, Node, Scope, StatePlacement, UnrecordedOutput};
use kati::evaluate::{evaluate, Evaluated};
use kati::ninja::emit_build;
use kati::session::Session;
use std::collections::{HashMap, HashSet};
use std::error;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

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
/// The graph says what that state's silence means for the same reason: an
/// output the build log does not name is one Make judges on its timestamps
/// alone, because Make has no build log for it to have been absent from.
///
/// # Errors
///
/// Returns [`MakeError::Evaluate`] for a Makefile Make itself rejects — a
/// syntax error, an `$(error)`, a prerequisite with no rule to make it — and
/// [`MakeError::Construct`] for one that evaluates but describes a graph the
/// engine cannot hold, such as two rules generating one output.
// [spec:ronin:req:make.graph-direct]
// [spec:ronin:req:make.state-outside-the-tree]
pub fn load_makefile(session: Session, shuffle: Shuffle) -> Result<Loaded, MakeError> {
    let directory = std::env::current_dir().map_err(|error| {
        MakeError::Evaluate(format!(
            "reading current directory for Make compilation: {error}"
        ))
    })?;
    let makefile = session
        .flags
        .makefile
        .as_ref()
        .map(|makefile| makefile.as_encoded_bytes())
        .unwrap_or_default();
    let mut key = directory.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(makefile);
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
    let compilation = Compilation {
        context: CompilationContext {
            root_directory: directory.clone(),
            directory,
            path_prefix: PathBuf::new(),
            makeflags: environment_value("MAKEFLAGS").unwrap_or_default(),
            level: environment_value("MAKELEVEL")
                .and_then(|level| level.parse().ok())
                .unwrap_or(0),
            jobs: session.flags.num_jobs.max(1),
            environment,
            recipe_environment: Vec::new(),
        },
        session,
        shuffle,
        cache_key: key,
    };
    load_with_subninjas(compilation, cli::compile_subninja)
}

/// Invocation context retained while one Makefile compilation discovers its
/// semantic subninjas.
#[derive(Clone)]
pub(crate) struct CompilationContext {
    pub(crate) root_directory: PathBuf,
    pub(crate) directory: PathBuf,
    pub(crate) path_prefix: PathBuf,
    pub(crate) makeflags: String,
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
    targets: Vec<Node>,
    exported: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

/// Evaluate a root Makefile and every recursive `$(MAKE)` recipe into one
/// shared graph before returning it to the executor.
// [spec:ronin:req:make.recursive-invocation+1]
pub(crate) fn load_with_subninjas<F>(root: Compilation, mut resolve: F) -> Result<Loaded, MakeError>
where
    F: FnMut(&[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let mut sink = GraphSink::new_at(&root.context.root_directory);
    let mut cache = HashMap::new();
    let mut compiling = HashSet::new();
    let root_key = root.cache_key.clone();
    let compiled = compile_unit(
        root,
        &mut sink,
        None,
        &mut resolve,
        &mut cache,
        &mut compiling,
    )?;
    cache.insert(root_key, compiled.targets.clone());
    let mut graph = sink.into_graph().map_err(MakeError::Construct)?;
    graph.state_placement = StatePlacement::OutsideTheTree;
    graph.unrecorded_output = UnrecordedOutput::SaysNothing;
    Ok(Loaded {
        graph,
        exported: compiled.exported,
        // `.NOTPARALLEL` is a per-compilation-unit graph pool. It is never a
        // provenance flag for the one executor running the composed graph.
        serial: false,
    })
}

fn compile_unit<F>(
    compilation: Compilation,
    sink: &mut GraphSink,
    parent_scope: Option<Scope>,
    resolve: &mut F,
    cache: &mut HashMap<Vec<u8>, Vec<Node>>,
    compiling: &mut HashSet<Vec<u8>>,
) -> Result<CompiledUnit, MakeError>
where
    F: FnMut(&[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    let compilation_key = compilation.cache_key.clone();
    if !compiling.insert(compilation_key.clone()) {
        return Err(MakeError::Evaluate(
            "recursive Make compilation includes itself".to_owned(),
        ));
    }
    let context = compilation.context.clone();
    let session = compilation.session;
    let shuffle = compilation.shuffle;
    let is_root = parent_scope.is_none();
    let evaluated = in_directory(&context.directory, || {
        let Evaluated { mut ev, mut nodes } =
            evaluate(session).map_err(|error| MakeError::evaluate(&error))?;
        reorder(shuffle, ev.session.flags.not_parallel, &mut nodes);
        let exported =
            exported_environment(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
        if let Some(parent) = parent_scope {
            sink.begin_subninja(
                parent,
                context.path_prefix.clone(),
                context.directory.clone(),
            );
        }
        sink.serialise_unit(ev.session.flags.not_parallel);
        let mut recipe_environment = context.recipe_environment.clone();
        if !is_root {
            apply_recipe_environment(&mut recipe_environment, &exported);
        }
        sink.set_recipe_environment(recipe_environment);
        if let Err(error) = emit_build(&nodes, &mut ev, sink) {
            if let Some(failure) = sink.construction_failure() {
                return Err(MakeError::Construct(failure));
            }
            return Err(MakeError::evaluate(&error));
        }
        let unit = sink.take_unit();
        ev.finish().map_err(|error| MakeError::evaluate(&error))?;
        Ok((unit, exported))
    });
    let (unit, exported) = match evaluated {
        Ok(evaluated) => evaluated,
        Err(error) => {
            compiling.remove(&compilation_key);
            return Err(error);
        }
    };

    let mut descendant_context = context;
    apply_exported_environment(&mut descendant_context.environment, &exported);
    if !is_root {
        apply_recipe_environment(&mut descendant_context.recipe_environment, &exported);
    }
    for pending in unit.subninjas {
        let mut child_target_groups = Vec::with_capacity(pending.invocations.len());
        for invocation in &pending.invocations {
            let child = resolve(&invocation.command, &invocation.make, &descendant_context)?;
            let child_key = child.cache_key.clone();
            let child_targets = if let Some(targets) = cache.get(&child_key) {
                targets.clone()
            } else {
                let child_scope = pending.scope;
                let child =
                    compile_unit(child, sink, Some(child_scope), resolve, cache, compiling)?;
                cache.insert(child_key, child.targets.clone());
                child.targets
            };
            child_target_groups.push(child_targets);
        }
        sink.complete_subninja(pending, &child_target_groups)
            .map_err(MakeError::Construct)?;
    }
    compiling.remove(&compilation_key);
    Ok(CompiledUnit {
        targets: unit.targets,
        exported,
    })
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
fn in_directory<T>(
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

/// What `--shuffle` reorders the goals and each target's prerequisites by.
///
/// The point of it is that the order a Makefile happens to write is not one it
/// may rely on: a build that only works in written order has a dependency it
/// never stated, and a run in some other order is what finds it. A seed settles
/// the permutation completely, so a run that found something can be repeated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Shuffle {
    /// Build in the order the Makefile wrote.
    #[default]
    None,
    /// Ask for a shuffle and get that same order. GNU Make keeps this apart
    /// from asking for none at all, and so does the value a sub-make inherits.
    Identity,
    /// Back to front.
    Reverse,
    /// The permutation this seed names.
    Seed(u32),
}

impl Shuffle {
    /// What `--shuffle`'s argument asks for, in GNU Make's spellings, which it
    /// compares without regard to case. `None` for a word that is neither a
    /// mode nor a seed.
    ///
    /// `random` is settled here rather than carried as a request, so that what
    /// travels onward names the permutation this run actually used.
    #[must_use]
    pub fn requested(spec: &[u8]) -> Option<Self> {
        Some(match spec.to_ascii_lowercase().as_slice() {
            b"none" => Self::None,
            b"identity" => Self::Identity,
            b"reverse" => Self::Reverse,
            b"random" => {
                use std::hash::{BuildHasher, Hasher};
                let entropy = std::collections::hash_map::RandomState::new()
                    .build_hasher()
                    .finish();
                #[expect(clippy::cast_possible_truncation, reason = "any 32 bits will do")]
                Self::Seed(entropy as u32)
            }
            digits => Self::Seed(
                std::str::from_utf8(digits)
                    .ok()
                    .and_then(|digits| digits.parse().ok())?,
            ),
        })
    }

    /// How a sub-make is told what this one did.
    ///
    /// The seed it settled on rather than the word that asked for one, which is
    /// what makes a tree of makes reproduce a run that failed. `None` for the
    /// mode that reorders nothing, which travels as nothing.
    #[must_use]
    pub fn spelling(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Identity => Some("identity".to_owned()),
            Self::Reverse => Some("reverse".to_owned()),
            Self::Seed(seed) => Some(seed.to_string()),
        }
    }
}

/// The draws one shuffle is made of.
enum Draw {
    Reverse,
    /// `SplitMix64`, whose whole state is the seed: the permutation follows
    /// from it and from the order the graph is walked in, and nothing else.
    Random(u64),
}

impl Draw {
    fn permute<T>(&mut self, items: &mut [T]) {
        match self {
            Self::Reverse => items.reverse(),
            Self::Random(state) => {
                for index in 0..items.len() {
                    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
                    let mut draw = *state;
                    draw = (draw ^ (draw >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                    draw = (draw ^ (draw >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                    draw ^= draw >> 31;
                    let picked = usize::try_from(draw % items.len() as u64)
                        .expect("a remainder of a list length fits a length");
                    items.swap(index, picked);
                }
            }
        }
    }
}

/// Reorder the goals, and each target's prerequisites, before the graph is cut.
///
/// The order the graph is walked in here is the order its edges are minted in,
/// and among edges that are equally ready the scheduler takes the one minted
/// first — so reordering the walk is what reorders the build.
///
/// Done here rather than in the scheduler because it is what the Makefile asked
/// to be built, not how the build runs: the recipes are already expanded by this
/// point, so `$^` and `$<` keep the order the Makefile wrote whatever this does
/// to the order they are built in. Which is GNU Make's rule too.
///
/// `.NOTPARALLEL` takes it back. A Makefile saying its own recipes cannot
/// overlap is describing an order, and reordering it would read past what it
/// said.
///
/// `.WAIT` needs no exception, where GNU Make leaves a list holding one alone:
/// the evaluator has already turned each barrier into order-only prerequisites,
/// so the order it asked for is in the graph rather than in the list, and
/// survives being reordered.
fn reorder(shuffle: Shuffle, not_parallel: bool, nodes: &mut [kati::dep::NamedDepNode]) {
    let mut draw = match shuffle {
        Shuffle::None | Shuffle::Identity => return,
        Shuffle::Reverse => Draw::Reverse,
        Shuffle::Seed(seed) => Draw::Random(u64::from(seed)),
    };
    if not_parallel {
        return;
    }
    draw.permute(nodes);
    let mut seen = std::collections::HashSet::new();
    let mut work = nodes
        .iter()
        .rev()
        .map(|(_, node)| std::sync::Arc::clone(node))
        .collect::<Vec<_>>();
    while let Some(node) = work.pop() {
        let mut node = node.lock();
        if !seen.insert(node.output) {
            continue;
        }
        draw.permute(&mut node.deps);
        draw.permute(&mut node.order_onlys);
        for (_, dep) in node.deps.iter().chain(node.order_onlys.iter()).rev() {
            work.push(std::sync::Arc::clone(dep));
        }
    }
}

/// A Makefile read: the graph it describes, and what it said about running it.
pub struct Loaded {
    /// What the Makefile builds.
    pub graph: BuildGraph,
    /// What `export` put in every recipe's environment, and what `unexport`
    /// took out of it. Values are evaluated here because the evaluator that
    /// can answer for them does not outlive this call.
    pub exported: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
    /// `.NOTPARALLEL`: run this Makefile's own recipes one at a time.
    pub serial: bool,
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
        // A recipe cannot reach a `private` variable, and a recipe's environment
        // is the recipe's: `private export F = g` reaches `$(shell)` and nothing
        // a rule runs.
        if ev
            .session
            .peek_global_var(name)
            .is_some_and(|var| var.read().is_private)
        {
            continue;
        }
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
