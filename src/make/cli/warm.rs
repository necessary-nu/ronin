//! Starting a run from the composition the last one left, or not at all.
//!
//! Every way out of here that is not a build is `Ok(None)`, and `Ok(None)` is
//! a run that reads its makefiles — which is what every run did before the
//! cache existed. Nothing is printed and nothing is explained: a user whose
//! tree moved wants a build, not a report about a file they never asked for.
//!
//! The order matters and is the cheap thing first. The pair is opened and
//! checked against itself; every unit's makefiles are held to their digests,
//! which is the common miss and costs a read this run would have made anyway;
//! then the evaluators are rebuilt, which is the expensive part and the part
//! that runs a `$(shell)`. Only then is anything built.

use super::{
    Invocation, PreparedGraph, RootCompilation, carry_command_line_evals, compilation_key,
    evaluated_build_options, evaluated_invocation, remake,
};
use crate::build::BuildOptions;
use crate::error::Error;
use crate::frontend::{BuildGraph, Node};
use crate::make::cache::artifact::BuildArtifact;
use crate::make::warm::RebuiltUnit;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Everything about this command line, beside the directory it runs in
/// and its variables and goals, that makes one composition a different
/// composition.
///
/// What a cache entry is named by, and it is deliberately WIDER than the
/// set that provably shapes a read. A name that is too narrow is two
/// compilations sharing one graph, which is a wrong build; a name that is
/// too wide is a miss, which costs what every run cost before the cache
/// existed. So every switch bit is in it, given and negated, rather than
/// the handful anybody has checked.
///
/// The ones that are known to matter, so the next reader does not have to
/// rediscover them: `-f`, `-I` and `--eval` decide which text is read at
/// all; `--shuffle` reorders what that text says; `-r` and `-R` take away
/// the built-in rules and variables it is read against; `-B`, `-W` and
/// `-o` answer the one freshness question a COMPILATION asks, which is
/// whether a recursive recipe has to run, and a run that answers it
/// differently composes different children; and `-j` settles the job
/// groups the graph itself carries, so a graph composed at `-j8` is not
/// the graph `-j1` composes.
///
/// The jobserver's own address is NOT in it, and must not be: it names a
/// FIFO this process created and carries its process id, so it is
/// different on every run and no two runs would ever share a name.
pub(in crate::make) fn composition_key(invocation: &Invocation) -> Vec<u8> {
    use crate::make::cache::push_field;
    let mut key = Vec::new();
    for paths in [&invocation.makefiles, &invocation.include_dirs] {
        key.extend_from_slice(&(paths.len() as u64).to_le_bytes());
        for path in paths {
            push_field(&mut key, path.as_os_str().as_encoded_bytes());
        }
    }
    key.extend_from_slice(&(invocation.evals.len() as u64).to_le_bytes());
    for eval in &invocation.evals {
        push_field(&mut key, eval);
    }
    for words in [
        &invocation.debug,
        &invocation.assumed_new,
        &invocation.assumed_old,
    ] {
        key.extend_from_slice(&(words.len() as u64).to_le_bytes());
        for word in words {
            push_field(&mut key, word);
        }
    }
    push_field(
        &mut key,
        invocation
            .shuffle_spelling
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    key.extend_from_slice(&invocation.switches.to_le_bytes());
    key.extend_from_slice(&invocation.negated.to_le_bytes());
    for jobs in [invocation.jobs, invocation.inherited_jobs] {
        push_field(&mut key, format!("{jobs:?}").as_bytes());
    }
    key
}

/// Build this invocation from the composition it left last time, or answer
/// `None` for a run that has to read its makefiles.
pub(super) fn warm_start(
    root: &RootCompilation<'_>,
    scripts: &Arc<kati::scripts::Scripts>,
    reported: &mut String,
    output: &mut Option<&mut dyn Write>,
    diagnostics: &mut Option<&mut dyn Write>,
) -> Result<Option<PreparedGraph>, Error> {
    let Some(cached) = crate::make::cache::load::cached(root.directory, root.invocation) else {
        return Ok(None);
    };
    let crate::make::cache::load::Cached { artifact, graph } = cached;
    // A composition that never settled through the compiler-input build left
    // nothing to build from, only a graph and the reads behind it.
    let Some(build) = artifact.build.clone() else {
        return Ok(None);
    };
    let Ok(units) = crate::make::warm::rebuild(root_compilation(root, scripts), &artifact) else {
        return Ok(None);
    };
    if !wrappers_still_clean(&graph, &build) {
        return Ok(None);
    }
    let Some(recipes) = assemble_recipes(&graph, &build, units, root.diagnostics) else {
        return Ok(None);
    };
    // The invocation the makefiles settled on, which is what the run that
    // composed this graph built under. Reading it off the record rather than
    // off a read is the whole of what a loaded composition has instead.
    let invocation = evaluated_invocation(&build.makeflags)?;
    let options = evaluated_build_options(root.options, &invocation, build.job_budget);
    let Some(settlement) = remake::build_loaded_composition(
        remake::LoadedComposition {
            graph,
            build: &build,
            recipes: Some(Box::new(recipes)),
            invocation: &invocation,
            options: options.clone(),
            directory: root.directory,
            goals: &root.invocation.goals,
        },
        reported,
        output,
        diagnostics,
    ) else {
        return Ok(None);
    };
    let settlement = settlement?;
    // The staged work has run by now, and what it wrote can be a text one of
    // these reads was made of.
    if !crate::make::warm::sources_unmoved(&artifact) {
        return Ok(None);
    }
    let prepared = prepared(settlement, invocation, options);
    Ok(prepared)
}

/// What the settled staging leaves the invocation to do.
///
/// A loaded composition that does not settle is one whose makefile update
/// remade something, and the composition it remade is not this one: the run
/// reads its makefiles, which is what GNU Make's own restart amounts to.
fn prepared(
    settlement: remake::Settlement,
    invocation: Invocation,
    options: BuildOptions,
) -> Option<PreparedGraph> {
    match settlement {
        remake::Settlement::Finished(result) => Some(PreparedGraph::Finished(result)),
        remake::Settlement::Settled(settled) => {
            let remake::SettledGraph {
                graph,
                persistence,
                recipes,
                ..
            } = *settled;
            Some(PreparedGraph::Ready {
                graph: Box::new(graph),
                recipes,
                persistence: Some(persistence),
                invocation: Box::new(invocation),
                options: Box::new(options),
            })
        }
        remake::Settlement::Restart | remake::Settlement::Staged => None,
    }
}

/// The root unit's compilation, as the first pass of a cold read builds it.
fn root_compilation(
    root: &RootCompilation<'_>,
    scripts: &Arc<kati::scripts::Scripts>,
) -> crate::make::Compilation {
    let (mut session, context) = super::pass::pass_session(root, 0, scripts);
    carry_command_line_evals(&mut session, &root.invocation.evals);
    let makefiles: Vec<PathBuf> = session.flags.makefiles.iter().map(PathBuf::from).collect();
    let cache_key = compilation_key(&context.directory, &makefiles, &context.makeflags);
    crate::make::Compilation {
        session,
        shuffle: root.invocation.shuffle,
        context,
        cache_key,
    }
}

/// Whether every recursive wrapper the composition settled clean is still
/// clean against the disk as it stands.
///
/// A wrapper found current at compile time has NO CHILD COMPOSED for it: the
/// graph holds a commandless edge where a whole child subgraph would be. That
/// verdict was taken against the disk the composing run had. One that is
/// dirty now needs a child this graph does not hold, so the record is refused
/// and the makefiles are read, which composes it.
fn wrappers_still_clean(graph: &BuildGraph, build: &BuildArtifact) -> bool {
    let mut runtime = crate::runtime::RuntimeState::default();
    let mut scratch = crate::graph::TraversalScratch::default();
    let disk = crate::os::RealDiskInterface::default();
    for (wrapper, staged) in &build.clean_wrappers {
        let Some(edge) = graph.edge_at(*wrapper) else {
            return false;
        };
        let Some(staged) = staged
            .iter()
            .map(|at| graph.node_at(*at))
            .collect::<Option<Vec<Node>>>()
        else {
            return false;
        };
        let mut stat = |path: &std::path::Path| disk.stat(path);
        let settled = graph.staged_wrapper_freshness(
            edge,
            &staged,
            &mut stat,
            crate::runtime::AssertedDates { new: &[], old: &[] },
            crate::frontend::RepeatedScan {
                // Nothing has evaluated since this graph was loaded, and the
                // scan is the first thing to ask the ground at all, so every
                // question goes to it.
                ground_as_of: None,
                runtime: &mut runtime,
                scratch: &mut scratch,
            },
        );
        if !matches!(settled, Ok(settled) if !settled.dirty) {
            return false;
        }
    }
    true
}

/// The graph path a unit's declared name reaches, which is the rule
/// [`crate::make::sink::GraphSink`] applies to the same name as it builds the
/// graph: qualified by the unit's own prefix unless there is none or the name
/// is absolute, then canonicalised the way every path the graph holds is.
fn qualified(prefix: &std::path::Path, name: &[u8]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(name));
    let mut qualified = if prefix.as_os_str().is_empty() || path.is_absolute() {
        name.to_vec()
    } else {
        prefix.join(path).as_os_str().as_bytes().to_vec()
    };
    crate::util::canonpath(&mut qualified);
    qualified
}

/// Hold every rebuilt unit's evaluator where the build can reach it, keyed by
/// the edge that runs each recipe.
///
/// `None` where the record names an edge this graph does not have, or where
/// this emission says a recorded number is for some OTHER output than the
/// edge the record filed it under. The numbers are minted in walk order, so a
/// walk that disagreed with the recorded one would hand every edge somebody
/// else's recipe — a wrong command rather than a slow run — and nothing about
/// the number itself would look wrong. Counting them catches nothing: a walk
/// can mint a recipe that never reaches an edge the record names, so the two
/// counts differ on a perfectly good record (measured on zsh, six minted
/// against five recorded).
fn assemble_recipes(
    graph: &BuildGraph,
    build: &BuildArtifact,
    units: Vec<RebuiltUnit>,
    diagnostics: &Arc<kati::diagnostics::Diagnostics>,
) -> Option<crate::make::recipe::PendingRecipes> {
    let mut recipes = crate::make::recipe::PendingRecipes::new(Arc::clone(diagnostics));
    for unit in units {
        let Some(recorded) = build.units.iter().find(|held| held.key == unit.key) else {
            continue;
        };
        let minted: crate::htab::RapidHashMap<kati::build_sink::DeferredRecipeId, &[u8]> = unit
            .deferred_outputs
            .iter()
            .map(|(output, recipe)| (*recipe, output.as_slice()))
            .collect();
        let mut edges = Vec::new();
        for (edge, _, recipe) in build.deferred.iter().filter(|(_, key, _)| *key == unit.key) {
            let edge = graph.edge_at(*edge)?;
            // The number is believed only where this emission agrees about
            // what it is for. A walk that minted them in another order hands
            // every edge somebody else's recipe, and no count or bound would
            // notice.
            if qualified(&unit.path_prefix, minted.get(recipe)?)
                != graph.path(graph.output_of(edge)?)
            {
                return None;
            }
            edges.push((edge, *recipe));
        }
        recipes.admit(
            unit.key,
            unit.read,
            unit.deferred,
            recorded.layout.clone(),
            recorded.directory.clone(),
            &edges,
        );
    }
    let settled = build
        .settled
        .iter()
        .map(|(edge, steps)| Some((graph.edge_at(*edge)?, steps.clone())))
        .collect::<Option<Vec<_>>>()?;
    recipes.admit_settled(settled);
    Some(recipes)
}

#[cfg(test)]
mod tests {
    use super::qualified;
    use std::path::Path;

    /// The root reads where the build runs, so its names reach the graph as
    /// they were written.
    #[test]
    fn a_name_with_no_prefix_is_the_name() {
        assert_eq!(qualified(Path::new(""), b"init/main.o"), b"init/main.o");
    }

    /// One graph holds one namespace of paths, so a unit read somewhere else
    /// contributes its nodes under where it was read.
    #[test]
    fn a_child_name_is_qualified_by_its_unit() {
        assert_eq!(qualified(Path::new("src"), b"vim.o"), b"src/vim.o");
    }

    /// A name that already says where it is says it once.
    #[test]
    fn an_absolute_name_is_left_alone() {
        assert_eq!(qualified(Path::new("src"), b"/tmp/gen.h"), b"/tmp/gen.h");
    }

    /// Every path the graph holds is canonical, so a name joined onto a
    /// prefix has to be read the same way the graph read it.
    #[test]
    fn a_joined_name_is_canonical() {
        assert_eq!(qualified(Path::new("src"), b"../lib/a.o"), b"lib/a.o");
    }

    /// The comparison this exists for is between a name kati declared and a
    /// path the graph holds, so two different names must not qualify alike.
    #[test]
    fn two_names_under_one_prefix_stay_apart() {
        assert_ne!(
            qualified(Path::new("src"), b"a.o"),
            qualified(Path::new("src"), b"b.o")
        );
    }
}
