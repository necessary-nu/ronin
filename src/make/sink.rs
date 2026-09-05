//! kati's [`BuildSink`], implemented against Ronin's graph.
//!
//! The emitter computes an edge and then writes it out as `build.ninja` bytes.
//! This is the same computation with the writing removed: each rule becomes a
//! [`Rule`] and each edge an [`add_edge`](BuildGraph::add_edge), so the graph
//! that reaches the scheduler is the one Make described rather than one
//! recovered from a file.

use crate::frontend::{
    Binding, BuildGraph, Edge, EdgeSpec, FrontendError, Node, Rule, Scope, Template,
};
use kati::anyhow;
use kati::build_sink::{
    BuildSink, DeferredRecipeId, FileEvaluation, NewInputsTiming, OutputEvaluation,
    RecipeExpansion, RuleId, ShellEvaluation, SinkCommand, SinkEdge, SinkPool, SinkRule,
};
use kati::bytes::Bytes;
use kati::strutil::escape_shell;
use kati::symtab::{Interner, Symbol};
use std::collections::BTreeMap;

use crate::htab::{RapidHashMap, RapidHashSet};
use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

mod enclosing;
mod invented;
mod spellings;

/// The binding names an edge kati produced can carry.
///
/// Interned once. Every edge names most of them, and interning is a hash of the
/// name against the graph's own table.
struct Bindings {
    command: Binding,
    description: Binding,
    depfile: Binding,
    deps: Binding,
    generator: Binding,
    rspfile: Binding,
    rspfile_content: Binding,
    pool: Binding,
    ignore_errors: Binding,
    /// `$out`, used to name a response file per edge rather than per rule.
    /// kati mints one rule per edge, so this expands to that edge's own single
    /// output.
    out: Binding,
}

impl Bindings {
    fn intern(graph: &mut BuildGraph) -> Self {
        Self {
            command: graph.binding(b"command"),
            description: graph.binding(b"description"),
            depfile: graph.binding(b"depfile"),
            deps: graph.binding(b"deps"),
            generator: graph.binding(b"generator"),
            rspfile: graph.binding(b"rspfile"),
            rspfile_content: graph.binding(b"rspfile_content"),
            pool: graph.binding(b"pool"),
            ignore_errors: graph.binding(crate::build::IGNORE_ERRORS),
            out: graph.binding(b"out"),
        }
    }
}

pub(crate) use super::layout::{CommandLayout, SettledScript, SettledSegment, SettledSteps};

/// One static recursive invocation within a held recipe.
pub(crate) struct SubninjaInvocation {
    pub(crate) command: Vec<u8>,
    pub(crate) make: Vec<u8>,
    pub(crate) shell: Vec<u8>,
    pub(crate) shell_flags: Vec<u8>,
    /// The recipe's own lines written ahead of this invocation, as a rule that
    /// runs them. `None` when the invocation is the first thing the recipe
    /// does after the one before it.
    pub(crate) preceding_rule: Option<Rule>,
    /// How that segment is launched, once its edge exists: line by line where
    /// its lines can be, and as the assembled script where one cannot.
    preceding_script: Option<SettledSegment>,
    /// The Makefile and line the recipe line was written on, for a report that
    /// has to point at the invocation it is about. `None` for a build, which
    /// has nothing to say about it.
    pub(crate) location: Option<String>,
}

/// What one edge's stopped recipe may give back, and when it must.
///
/// The two travel together because they are one answer in GNU Make: the names
/// `delete_target` would not refuse, and whether an ordinary failure is reason
/// enough to ask for them or only a signal is.
struct PendingWithdrawal {
    outputs: Vec<Node>,
    on_error: bool,
}

/// A recursive recipe held until all its child Makefiles have been compiled.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the lint guards a positional argument list, and this is only ever filled in by name"
)]
pub(crate) struct PendingSubninja {
    pub(crate) invocations: Vec<SubninjaInvocation>,
    pub(crate) scope: Scope,
    residual_rule: Option<Rule>,
    /// How the residual segment is launched, once its edge exists: line by line
    /// where its lines can be, and as the assembled script where one cannot.
    residual_script: Option<SettledSegment>,
    diagnostic_command: Vec<u8>,
    explicit_outputs: Vec<Node>,
    implicit_outputs: Vec<Node>,
    inputs: Vec<Node>,
    order_only_inputs: Vec<Node>,
    /// The subset of `order_only_inputs` this wrapper outlives a failure of.
    forgiven_order_inputs: Vec<Node>,
    /// `.PHONY`, which is what a recursive dispatch rule is.
    ///
    /// Read by the compiler because it settles, without staging anything,
    /// that the wrapper this recipe becomes is out of date and its child will
    /// therefore be composed rather than short-circuited. See
    /// `crate::make::read_ahead`.
    pub(crate) always_dirty: bool,
    deferred: Option<PendingDeferred>,
    completion_output: Option<Node>,
    intermediate: bool,
    disposable_outputs: Vec<Node>,
    low_resolution: bool,
    withdrawal: PendingWithdrawal,
    peer_outputs: Vec<Node>,
    bindings: Vec<(Binding, Vec<u8>)>,
}

impl PendingSubninja {
    /// Target nodes whose held recursive edge this pending unit will produce.
    pub(crate) fn outputs(&self) -> impl Iterator<Item = Node> + '_ {
        self.explicit_outputs
            .iter()
            .chain(&self.implicit_outputs)
            .copied()
    }

    /// Prerequisites GNU Make settles before it starts this recursive recipe.
    pub(crate) fn evaluation_inputs(&self) -> Vec<Node> {
        self.inputs
            .iter()
            .chain(&self.order_only_inputs)
            .copied()
            .collect()
    }
}

/// One recursive recipe of a `.NOTPARALLEL` unit, as a thing to be scheduled.
///
/// GNU Make's job here is the entire sub-make, so the job is the wrapper edge
/// together with every edge of the children this recipe composed, and what
/// finishing it means is the wrapper's own targets — which
/// [`PendingSubninja::outputs`] names, and which nothing can ask for until the
/// last edge of the subtree has finished, the wrapper being either a phony over
/// every child goal or the recipe's residual segment ordered behind them.
pub(crate) struct SerialJob {
    pub(crate) completion: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
}

struct PendingDeferred {
    outputs: Vec<Node>,
    always_dirty_output: bool,
    dates_do_not_decide: bool,
    heads_the_group: bool,
    always_new_inputs: Vec<Node>,
    excluded_new_inputs: Vec<Node>,
    /// What kati says the published value calls an input it knows by another
    /// name — an archive member, and nothing else there is.
    new_input_names: Vec<(Node, Vec<u8>)>,
}

/// The non-executor description retained between kati's rule and edge calls.
struct SubninjaRule {
    invocations: Vec<SubninjaInvocation>,
    residual_rule: Option<Rule>,
    residual_script: Option<SettledSegment>,
    diagnostic_command: Vec<u8>,
}

/// The files a set of units make, by canonical path, and the node each is
/// made at.
///
/// What a child compilation is handed about the units enclosing it. A child
/// naming one of these paths names the file the enclosing unit makes, so its
/// name resolves to that node and depends on that producer; a rule of its own
/// for the path is not read — see [`GraphSink::begin_subninja`]. Files only:
/// a phony target, or one with no recipe, is nothing a child's spelling of the
/// same name refers to — see [`GraphSink::record_generated`].
pub(crate) type Enclosing = RapidHashMap<Vec<u8>, Node>;

/// What one kati compilation unit contributed to the shared graph.
pub(crate) struct UnitOutput {
    pub(crate) targets: Vec<Node>,
    pub(crate) subninjas: Vec<PendingSubninja>,
    pub(crate) edges: Vec<Edge>,
    /// Whether the makefiles this unit read declared bare `.NOTPARALLEL`.
    ///
    /// Carried out with the unit rather than asked of the sink later, because
    /// by the time the recursive recipes are composed the sink is on another
    /// unit: composing a child begins one, and that overwrites what this read
    /// left. See [`GraphSink::chain_serial_jobs`].
    pub(crate) serial: bool,
    /// The paths this unit generates — its own edges' outputs and its
    /// recursive wrappers' — for the children it composes to see as enclosing.
    pub(crate) generated: Enclosing,
}

/// The targets and complete edge closure contributed by one compiled unit.
#[derive(Clone)]
pub(crate) struct UnitSubgraph {
    pub(crate) targets: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    /// The edges this compilation itself made: the unit's own, its
    /// wrappers, and those of the children it composed rather than reached.
    ///
    /// A child two compositions reach is one copy of one piece of work, made
    /// by the first and merely named by the second, and only the composition
    /// that made an edge may sequence it — behind its wrapper's
    /// prerequisites, behind the group before it, behind the recipe before
    /// it. zsh's `Modules` composes `Zle`'s `complete.mdh` once and reaches
    /// it again from its `modules` goal; fencing the shared copy behind the
    /// second wrapper's `FORCE` too pointed it back at the loop that had
    /// already ordered it, and closed a cycle. GNU Make's second process
    /// would have found the work done and done nothing, which is what a
    /// copy that is sequenced once and named twice does.
    pub(crate) fresh_edges: Vec<Edge>,
}

/// One child compilation a recursive recipe asked for, and whether this recipe
/// is the one that composed it.
pub(crate) struct ChildGroup {
    pub(crate) subgraph: UnitSubgraph,
    /// False where the compilation had already composed this exact child — the
    /// same command, in the same directory, for the same goals — and handed
    /// back the edges it made then.
    ///
    /// It matters only to `.NOTPARALLEL`. Two recipes reaching one cached child
    /// are two names for a single copy of the work, which the build runs once,
    /// so there is no second copy for the serialising wait to hold apart from
    /// the first. Sequencing those edges behind the recipe that made them would
    /// also be a cycle: that recipe's wrapper is already waiting on them.
    pub(crate) fresh: bool,
}

struct Unit {
    scope: Scope,
    path_prefix: PathBuf,
    command_directory: PathBuf,
    root: bool,
    serial_pool: Option<Vec<u8>>,
    /// The pool holding this unit's command edges to the job budget its own
    /// group stands at, and that budget. Absent for a unit reading under the
    /// budget the run itself was given, which nothing narrows.
    job_pool: Option<(Vec<u8>, NonZeroUsize)>,
    recipe_environment: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    /// Why no recipe of this unit can be started — see
    /// [`CommandLayout::unreadable`].
    unreadable: Option<String>,
    targets: Vec<Node>,
    subninjas: Vec<PendingSubninja>,
    edges: Vec<Edge>,
    /// What the units enclosing this one generate, and the nodes they make it
    /// at, as canonical paths. Empty for the root.
    enclosing: std::sync::Arc<Enclosing>,
    /// The nodes in `enclosing`, for the question a rule's outputs ask.
    enclosing_nodes: RapidHashSet<Node>,
    /// What this unit generates, by canonical path.
    generated: Enclosing,
}

/// A [`BuildSink`] that builds a Ronin graph instead of a manifest.
///
/// # `.PHONY`, and what is dropped
///
/// [`SinkEdge::always_dirty`] is `.PHONY`, and it crosses as
/// [`EdgeSpec::always_dirty`]. The writer has to spell the property as
/// something a manifest can hold — a synthetic input no rule produces, or
/// Android ninja's `phony_output` binding — and neither spelling is the
/// property. An edge can state it, so this states it, and the edge is out of
/// date whenever it is reached however its outputs and the build log compare.
///
/// [`SinkRule::sandbox_disabled`] asks Android's ninja fork to run the command
/// outside its sandbox. Ronin has no sandbox, and the binding is not one Ninja
/// itself accepts on a rule, so carrying it would put a value in the graph that
/// nothing could ever read.
// [spec:ronin:req:make.graph-direct]
pub struct GraphSink {
    graph: BuildGraph,
    root_directory: PathBuf,
    unit: Unit,
    bindings: Bindings,
    phony: Rule,
    /// Non-phony rule used only while deciding whether a recursive wrapper's
    /// recipe is current. It is never allowed to execute.
    subninja_probe: Rule,
    /// The scan state that decision uses, kept from one wrapper to the next.
    ///
    /// Holds nothing between uses: it is reset to what a fresh one holds before
    /// each scan reads it, so all it carries across is the allocation. See
    /// [`crate::frontend::BuildGraph::edge_dirty_with`], which sizes it to the
    /// whole graph — a composition staging one wrapper per unit against a graph
    /// that grows with every unit pays for that sizing once per unit.
    subninja_freshness: crate::runtime::RuntimeState,
    /// kati's rule handles to Ronin's. kati mints one rule per edge and
    /// declares it immediately before that edge, so this holds one entry for
    /// as long as it takes to reach the edge that names it.
    rules: RapidHashMap<RuleId, Rule>,
    /// Rules whose recipe kati left unexpanded, waiting for the edge that
    /// names them so the recipe can be recorded against the edge instead.
    deferred_rules: RapidHashMap<RuleId, DeferredRecipeId>,
    /// Edges whose command this graph's own executor will ask for when it is
    /// about to run them.
    deferred_edges: Vec<(Edge, DeferredRecipeId)>,
    /// The launches a recipe read while compiling became, waiting for the edge
    /// that names their rule.
    settled_rules: RapidHashMap<RuleId, SettledSteps>,
    /// Rules whose recipe reaches one shell as a whole script, waiting for the
    /// edge whose output names the response file that script may be read from.
    settled_scripts: RapidHashMap<RuleId, SettledScript>,
    /// Edges whose recipe was read while compiling and which still run a
    /// process per command line.
    settled_edges: Vec<(Edge, SettledSteps)>,
    /// Recursive rules are not executor rules. They wait for their immediately
    /// following edge so the compiler can replace that edge with graph
    /// composition.
    subninja_rules: RapidHashMap<RuleId, SubninjaRule>,
    /// kati's symbols to Ronin's nodes, so a path shared by many edges is
    /// canonicalized and interned once.
    interned: RapidHashMap<Symbol, Node>,
    /// The nodes held by an edge that says nothing but its target's name, and
    /// the edge holding each. See [`Self::mentions_only_its_name`]: a rule
    /// that makes the file may arrive under another spelling of the same path
    /// after one of these, and takes the node off it rather than colliding
    /// with it.
    mentions: RapidHashMap<Node, Edge>,
    observed_members: RapidHashMap<Symbol, Node>,
    /// The nodes an earlier pass of this invocation has already made, gathered
    /// as [`Self::mark_subgraphs_prebuilt`] settles them.
    ///
    /// Across units, because it is what one unit made that the units it
    /// composes are asking about. See [`Self::made_and_absent`].
    prebuilt: RapidHashSet<Node>,
    declared_pools: RapidHashSet<Vec<u8>>,
    completion_proxies: usize,
    recipe_stages: usize,
    serial_units: usize,
    job_groups: usize,
    /// The first construction failure, kept because kati's walk unwinds through
    /// [`anyhow::Error`] and the typed error is what a caller can act on.
    failure: Option<FrontendError>,
    /// When this sink's caller will turn recipes into command text.
    ///
    /// A caller that takes the graph and runs it later needs every command in
    /// it; a caller that runs the build itself, and holds the session while it
    /// does, can ask for a recipe when it reaches it.
    expansion: RecipeExpansion,
}

impl Default for GraphSink {
    fn default() -> Self {
        Self::new()
    }
}

/// The two ways an edge can still have something to say for itself once the
/// graph is built, taken together because they are taken at the same moment.
pub(crate) type LateEdges = (Vec<(Edge, DeferredRecipeId)>, Vec<(Edge, SettledSteps)>);

impl GraphSink {
    /// A sink over an empty graph.
    ///
    /// # Panics
    ///
    /// If a new [`BuildGraph`] does not hold the built-in `phony` rule, which
    /// is an invariant of its own constructor rather than anything a caller can
    /// arrange.
    #[must_use]
    pub fn new() -> Self {
        Self::new_at(Path::new(""), RecipeExpansion::Construction)
    }

    /// A sink whose response files are anchored at the executor's root, and
    /// whose recipes are expanded when `expansion` says.
    #[must_use]
    pub(crate) fn new_at(root_directory: &Path, expansion: RecipeExpansion) -> Self {
        let mut graph = BuildGraph::new();
        let scope = graph.root();
        let bindings = Bindings::intern(&mut graph);
        let phony = graph
            .rule(scope, b"phony")
            .expect("a new graph holds the built-in phony rule");
        let subninja_probe = graph
            .define_rule(
                scope,
                b"__ronin_subninja_freshness_probe",
                vec![
                    (bindings.command, Template::literal(b"false")),
                    (bindings.generator, Template::literal(b"1")),
                ],
            )
            .expect("the internal recursive-freshness rule is unique");
        Self {
            graph,
            subninja_freshness: crate::runtime::RuntimeState::default(),
            root_directory: root_directory.to_owned(),
            unit: Unit {
                scope,
                path_prefix: PathBuf::new(),
                command_directory: PathBuf::new(),
                root: true,
                serial_pool: None,
                job_pool: None,
                recipe_environment: Vec::new(),
                unreadable: None,
                targets: Vec::new(),
                subninjas: Vec::new(),
                edges: Vec::new(),
                enclosing: std::sync::Arc::default(),
                enclosing_nodes: RapidHashSet::default(),
                generated: Enclosing::default(),
            },
            bindings,
            phony,
            subninja_probe,
            rules: RapidHashMap::default(),
            deferred_rules: RapidHashMap::default(),
            deferred_edges: Vec::new(),
            settled_rules: RapidHashMap::default(),
            settled_scripts: RapidHashMap::default(),
            settled_edges: Vec::new(),
            subninja_rules: RapidHashMap::default(),
            interned: RapidHashMap::default(),
            mentions: RapidHashMap::default(),
            observed_members: RapidHashMap::default(),
            prebuilt: RapidHashSet::default(),
            declared_pools: RapidHashSet::default(),
            completion_proxies: 0,
            recipe_stages: 0,
            serial_units: 0,
            job_groups: 0,
            failure: None,
            expansion,
        }
    }

    /// Start emitting a child compilation unit into a scoped, path-qualified
    /// part of the same graph.
    ///
    /// `enclosing` is what the units this one is composed under generate. A
    /// child's targets are otherwise its own — two units may each define
    /// `all`, and two isolated nodes keep them apart — but a FILE an enclosing
    /// unit makes is one file, and GNU Make's child process finds it made:
    /// the parent's phase that made it ran before the recipe that started the
    /// child. So a child's name for such a path resolves to the enclosing
    /// node and depends on that producer, and a rule of the child's own for
    /// it is not read. zsh's every subdirectory writes
    /// `$(dir_top)/Src/zsh.mdh: ; false # should only happen with make -n`,
    /// a stub for a file `Src` makes; read as a generator of a private node it
    /// ran in a clean build, beside the rule that makes the file, and failed.
    /// What is given up is the case where the child's rule would remake the
    /// enclosing unit's file from newer prerequisites of its own, which is the
    /// case the stub's comment says does not happen.
    ///
    /// "Finds it made" is checked rather than assumed: an enclosing unit that
    /// has made the path and left no file there is one whose child GNU Make
    /// would find nothing for, and that child keeps its own rule. See
    /// [`Self::made_and_absent`].
    pub(crate) fn begin_subninja(
        &mut self,
        parent: Scope,
        path_prefix: PathBuf,
        command_directory: PathBuf,
        enclosing: std::sync::Arc<Enclosing>,
    ) {
        debug_assert!(self.rules.is_empty());
        debug_assert!(self.subninja_rules.is_empty());
        self.interned.clear();
        self.mentions.clear();
        self.observed_members.clear();
        let enclosing_nodes = enclosing.values().copied().collect();
        self.unit = Unit {
            scope: self.graph.child_scope(parent),
            path_prefix,
            command_directory,
            root: false,
            serial_pool: None,
            job_pool: None,
            recipe_environment: Vec::new(),
            unreadable: None,
            targets: Vec::new(),
            subninjas: Vec::new(),
            edges: Vec::new(),
            enclosing,
            enclosing_nodes,
            generated: Enclosing::default(),
        };
    }

    /// Hold each recursive recipe of a `.NOTPARALLEL` unit behind the one
    /// before it, for the whole of the sub-make each stands for.
    ///
    /// GNU Make's `.NOTPARALLEL` is a scheduler constraint on the make PROCESS
    /// that read the declaration: `new_job` starts a job and then blocks in
    /// `reap_children` until that job is finished
    /// (reference/make-oracle/make-4.4.1/src/job.c:1962, the same line
    /// `job_slots == 1` takes). The job it blocks on is one target's entire
    /// recipe, so for a `$(MAKE) -C sub` line the serialised span is the whole
    /// child make — while that child, which is handed the jobserver untouched,
    /// stays parallel INSIDE itself unless its own makefiles say otherwise
    /// (doc/make.texi:4416).
    ///
    /// A depth-one pool cannot say that. A pool slot is held while one command
    /// runs, and the composition has dissolved the child make into edges of
    /// this same graph, so the recipe that GNU treats as one long job is a
    /// whole subtree here and its wrapper's own edge is a phony that runs at
    /// the END of it. The wait is therefore expressed as a wait: the previous
    /// recipe's targets become order-only inputs of this recipe's wrapper and
    /// of every edge of every child this recipe composed.
    ///
    /// What is NOT held together is the unit's own command edges against a
    /// composed subtree: those are in the unit's depth-one pool, which holds
    /// them apart from each other but not from a sub-make running beside them,
    /// where GNU Make blocks the whole process. Closing that would mean
    /// ordering the unit's ordinary recipes into the same chain, which is a
    /// total order over work GNU Make leaves the scheduler free to pick from.
    ///
    /// Forgiven, because GNU's block is on the job FINISHING and not on it
    /// succeeding: under `-k` a failed recipe does not stop the next one from
    /// running. See [`crate::graph::Graph::forgive_order`].
    ///
    /// Installed on the settled unit and never during it. `.NOTPARALLEL`
    /// throttles GNU Make at recipe-launch time and does not restructure the
    /// graph — `not_parallel` is read in `new_job` and nowhere in `remake.c` —
    /// so the wait must not reach either of the two questions the composition
    /// asks while it is still running.
    ///
    /// It must not reach whether a recipe HAS to run. The previous recipe's
    /// targets are `.PHONY` names that are never on disk, an order-only
    /// prerequisite that cannot be found is a reason to remake, and a wrapper
    /// carrying one is dirty for good.
    ///
    /// And it must not reach what a staging pass BUILDS. A boundary is built
    /// from the graph as it stands, so a wait threaded into that graph drags
    /// the previous recipe's whole subtree into a provisional build that had
    /// no reason to want it — the subtree then runs there, is composed again
    /// afterwards, and runs twice. Both were measured on zsh: composed with
    /// the waits in place, `Src/Makefile`'s `modobjs` recursion went from one
    /// staged append to three, `stamp-modobjs` held every object file three
    /// times, and the link of `zsh` failed on multiply-defined symbols, with
    /// the clean build making 96 provisional graphs where it makes 52.
    ///
    /// So the chain is wired once, over the jobs a completed composition
    /// settled on. An edge an earlier job already holds is left to that job:
    /// a child composed for one recipe and reached again by a later one is a
    /// single copy of the work, which the build runs once and which the
    /// earlier wait already covers.
    // [spec:ronin:req:make.notparallel-domain]
    pub(crate) fn chain_serial_jobs(&mut self, jobs: &[SerialJob]) {
        let mut claimed = RapidHashSet::default();
        for window in jobs.windows(2) {
            let [before, after] = window else { continue };
            claimed.extend(before.edges.iter().copied());
            for edge in &after.edges {
                if claimed.contains(edge) {
                    continue;
                }
                self.graph.add_order_only_inputs(*edge, &before.completion);
                self.graph.forgive_order_inputs(*edge, &before.completion);
            }
        }
    }

    /// Put this unit in the pool that holds its recipes, and answer with the
    /// job group it ended up in.
    ///
    /// `serial` is `.NOTPARALLEL`, which constrains only this unit's command
    /// edges, so a parent's declaration never becomes a global executor switch
    /// that serialises the child graph. It is half of that domain — the unit's
    /// recursive recipes are the other half, and a pool cannot hold them; see
    /// [`Self::chain_serial_jobs`]. Two pools can claim a unit and an edge
    /// names one, so it wins over the group below: one recipe at a time is
    /// narrower than any budget.
    ///
    /// GNU Make's groups are jobserver groups: a Make whose own makefiles set
    /// a `-j` the command line did not becomes the master of a new one
    /// (main.c:2101), and every Make below it joins that one rather than the
    /// one above. Ronin has one scheduler and one token pool for the whole
    /// tree, so a group is a pool over the units that belong to it instead: the
    /// unit that founded it and every unit composed under it that asked for no
    /// budget of its own.
    ///
    /// `own` is the budget this unit's makefiles named where that is not the
    /// budget of `group`, and founding a group is the whole of what it does.
    /// The run's own budget founds no group here — a unit reading under it is
    /// bounded by the scheduler, and only a run some unit widened needs the
    /// rest of it held back. See [`Self::into_graph`].
    // [spec:ronin:req:make.notparallel-domain]
    // [spec:ronin:req:make.jobserver+3]
    pub(crate) fn hold_unit_jobs(
        &mut self,
        serial: bool,
        group: Option<(Vec<u8>, NonZeroUsize)>,
        own: Option<NonZeroUsize>,
    ) -> Option<(Vec<u8>, NonZeroUsize)> {
        if serial {
            let name = format!("make_serial_{}", self.serial_units).into_bytes();
            self.serial_units += 1;
            self.unit.serial_pool = Some(name);
        }
        self.unit.job_pool = match own {
            Some(budget) => {
                let name = format!("make_jobs_{}", self.job_groups).into_bytes();
                self.job_groups += 1;
                Some((name, budget))
            }
            None => group,
        };
        self.unit.job_pool.clone()
    }

    /// Give this compilation unit the environment changes that differ from
    /// the root Make invocation. They become part of each child command, so a
    /// composed subninja observes its own exports and `MAKELEVEL` without a
    /// nested process boundary.
    ///
    /// `unreadable` is what settling that environment could not read, which
    /// arrives here because it arises there: a name whose value would not
    /// expand is one this unit cannot start a process under, and every recipe
    /// of the unit is refused where a process would have been. See
    /// [`CommandLayout::unreadable`].
    // [spec:ronin:req:make.exported-value-charged-to-the-job]
    pub(crate) fn set_recipe_environment(
        &mut self,
        environment: Vec<(OsString, Option<OsString>)>,
        unreadable: Option<String>,
    ) {
        let mut normalised = BTreeMap::new();
        for (name, value) in environment {
            normalised.insert(
                name.as_os_str().as_bytes().to_vec(),
                value.map(|value| value.as_os_str().as_bytes().to_vec()),
            );
        }
        self.unit.recipe_environment = normalised.into_iter().collect();
        self.unit.unreadable = unreadable;
    }

    /// Finish the current compilation unit without finishing the shared graph.
    pub(crate) fn take_unit(&mut self) -> UnitOutput {
        debug_assert!(self.rules.is_empty());
        debug_assert!(self.subninja_rules.is_empty());
        let subninjas = std::mem::take(&mut self.unit.subninjas);
        // A recursive wrapper's file targets are made here too — by the child
        // it composes, through the wrapper — and a child naming one names that.
        let mut generated = std::mem::take(&mut self.unit.generated);
        for pending in subninjas.iter().filter(|pending| !pending.always_dirty) {
            for output in pending
                .explicit_outputs
                .iter()
                .chain(&pending.implicit_outputs)
            {
                generated.insert(self.graph.path(*output).to_vec(), *output);
            }
        }
        UnitOutput {
            targets: std::mem::take(&mut self.unit.targets),
            subninjas,
            edges: std::mem::take(&mut self.unit.edges),
            serial: self.unit.serial_pool.is_some(),
            generated,
        }
    }

    /// Preserve compiler-input work already run by a provisional graph.
    ///
    /// What it settled is remembered as well as neutered, because a child
    /// compiled after this point is a child GNU Make would have started after
    /// this work ran, and what that child finds on the ground is the question
    /// [`Self::made_and_absent`] asks.
    pub(crate) fn mark_subgraphs_prebuilt(&mut self, roots: &[Node]) {
        let settled = self.graph.mark_subgraphs_prebuilt(roots, self.phony);
        self.prebuilt.extend(settled);
    }

    /// What the edge that makes `node` reads, empty for a node nothing here
    /// makes.
    ///
    /// Held recursive edges are not in the graph while their order is being
    /// decided — they are staged one at a time, in the order that decision
    /// produces — so everything this can walk through is an ordinary target,
    /// which is exactly the ground one wrapper has to be found across from
    /// another.
    pub(crate) fn prerequisites_of(&self, node: Node) -> Vec<Node> {
        self.graph.prerequisites_of(node)
    }

    /// Resolve compiler-input roots while this unit's symbol map is current.
    ///
    /// Generated included Makefiles are emitted like any other target, but the
    /// frontend also needs their graph handles so it can build them before
    /// recompiling the source unit.
    pub(crate) fn unit_nodes(
        &mut self,
        names: &dyn Interner,
        symbols: &[Symbol],
    ) -> Result<Vec<Node>, anyhow::Error> {
        self.node_list(names, symbols)
    }

    /// Replace a recursive wrapper edge with the child goals it requested.
    ///
    /// Parent prerequisites become order-only inputs of every edge in each
    /// child subtree: no indirect child work starts before the wrapper recipe
    /// could have started, while the child's own timestamps still decide what
    /// work it needs. The wrapper becomes a phony alias for child targets whose
    /// identities remain local to their own recursive compilation units.
    // [spec:ronin:req:make.recursive-invocation+4]
    ///
    /// `begun` is whether the recipe has already run part of itself at a
    /// compilation boundary, which the finished edge has to carry: one of
    /// those lines may have written this wrapper's own target, and reading the
    /// date it then has would be reading the recipe's work as a reason not to
    /// finish the recipe. See [`crate::graph::Edge::recipe_begun`].
    pub(crate) fn complete_subninja(
        &mut self,
        edge: Edge,
        pending: PendingSubninja,
        child_groups: &[ChildGroup],
        begun: bool,
    ) -> Result<Edge, FrontendError> {
        debug_assert_eq!(pending.invocations.len(), child_groups.len());
        // Each child group waits for the parent, and then for the group before
        // it: GNU Make runs a recipe's lines in the order they were written, so
        // the Make a later invocation starts reads whatever an earlier one left.
        //
        // Only the work this recipe made waits — see
        // [`UnitSubgraph::fresh_edges`]. A group that only reached a child
        // another recipe composed sequences nothing: that copy was ordered by
        // the recipe that made it.
        let mut waits = pending
            .inputs
            .iter()
            .chain(&pending.order_only_inputs)
            .copied()
            .collect::<Vec<_>>();
        let mut child_targets: Vec<Node> = Vec::new();
        for group in child_groups {
            let child = &group.subgraph;
            let preceding = waits
                .iter()
                .copied()
                .filter(|wait| !child.targets.contains(wait))
                .collect::<Vec<_>>();
            if group.fresh {
                for edge in &child.fresh_edges {
                    self.graph.add_order_only_inputs(*edge, &preceding);
                }
            }
            for target in &child.targets {
                if !child_targets.contains(target) {
                    child_targets.push(*target);
                }
            }
            if !child.targets.is_empty() {
                waits.clear();
                for target in &child.targets {
                    if !waits.contains(target) {
                        waits.push(*target);
                    }
                }
            }
        }

        if child_targets.iter().any(|target| {
            pending.explicit_outputs.contains(target) || pending.implicit_outputs.contains(target)
        }) {
            return Err(FrontendError::UncomposableSubninja {
                command: pending.diagnostic_command,
            });
        }

        let is_deferred = pending.deferred.is_some();
        let rule = if is_deferred {
            pending.residual_rule.unwrap_or(self.phony)
        } else if let Some(rule) = pending.residual_rule {
            self.graph.add_order_only_inputs(edge, &child_targets);
            rule
        } else {
            self.graph.add_explicit_inputs(edge, &child_targets);
            self.phony
        };
        self.graph.set_edge_rule(edge, rule);
        // The wrapper edge is where the parent's trailing lines end up, so it
        // is the edge whose output names their response file.
        if pending.residual_rule.is_some()
            && let Some(held) = &pending.residual_script
            && let Some(output) = pending.explicit_outputs.first().copied()
            && let Some(steps) = held.launch(self.graph.path(output))
        {
            self.settled_edges.push((edge, steps));
        }
        if let Some(deferred) = pending.deferred {
            self.defer_freshness(edge, &deferred);
            self.graph.add_deferred_activations(edge, &child_targets);
        }
        if let Some(output) = pending.completion_output {
            self.graph.set_completion_join(edge, output);
        }
        if begun {
            self.graph.mark_recipe_begun(edge);
        }
        Ok(edge)
    }

    /// Add a recursive wrapper with an inert ordinary rule so its Make
    /// timestamp freshness can be decided before any child Makefile is read.
    pub(crate) fn probe_subninja(
        &mut self,
        pending: &mut PendingSubninja,
    ) -> Result<Edge, FrontendError> {
        let edge = self.graph.add_edge(EdgeSpec {
            scope: pending.scope,
            rule: self.subninja_probe,
            explicit_outputs: &pending.explicit_outputs,
            implicit_outputs: &pending.implicit_outputs,
            explicit_inputs: &pending.inputs,
            implicit_inputs: &[],
            order_only_inputs: &pending.order_only_inputs,
            validations: &[],
            always_dirty: pending.always_dirty,
            intermediate: pending.intermediate,
            // Every line of the recipe this wrapper stands for is a recursive
            // one, which is what made it a wrapper. GNU Make runs those under
            // `-t` rather than standing in for them, and the child run touches
            // whatever it is asked to.
            has_touchable_recipe: false,
            // A recursive wrapper really is an alias: its outputs stand for
            // the child goals that replace it.
            outputs_unaliased: false,
            outputs_low_resolution: pending.low_resolution,
            bindings: std::mem::take(&mut pending.bindings),
        })?;
        // A recursive target is a target: what the child Make left on disk is
        // what the parent's other targets read, not the fact that a sub-build
        // ran.
        self.graph.set_make_target_freshness(edge);
        self.graph
            .forgive_order_inputs(edge, &pending.forgiven_order_inputs);
        // The wrapper edge is the one that will hold the parent's residual
        // recipe once the children are composed, so a failure there is the
        // failure that leaves the parent's own outputs half-made.
        self.graph.set_withdrawal(
            edge,
            pending.withdrawal.outputs.clone(),
            pending.withdrawal.on_error,
        );
        self.graph
            .set_peer_outputs(edge, pending.peer_outputs.clone());
        self.graph
            .set_disposable_outputs(&pending.disposable_outputs);
        Ok(edge)
    }

    /// Put the recipe's own lines written ahead of one invocation into the
    /// graph as work of their own, and name what asking for them asks for.
    ///
    /// `None` when the recipe wrote nothing there. Otherwise the caller stages
    /// the returned node the way it stages the recipe's prerequisites: the
    /// lines run, and only then is the child Makefile read. What one shell line
    /// can leave for the next is on the filesystem and nowhere else, so running
    /// them first is the whole of what makes their effects the child's to see —
    /// GNU Make gets it by starting the child after the line instead of
    /// compiling it before.
    ///
    /// Always dirty, because reaching here means the wrapper is out of date and
    /// GNU Make would be running this recipe. That is also what keeps `-t` off
    /// it: a name that is not a file is not touched.
    pub(crate) fn stage_preceding_lines(
        &mut self,
        pending: &PendingSubninja,
        index: usize,
    ) -> Result<Option<Node>, FrontendError> {
        let Some(rule) = pending.invocations[index].preceding_rule else {
            return Ok(None);
        };
        let proxy = self.recipe_stage_proxy()?;
        let settled = pending
            .inputs
            .iter()
            .chain(&pending.order_only_inputs)
            .copied()
            .collect::<Vec<_>>();
        let built = self.graph.add_edge(EdgeSpec {
            scope: pending.scope,
            rule,
            explicit_outputs: &[proxy],
            implicit_outputs: &[],
            explicit_inputs: &[],
            implicit_inputs: &[],
            // GNU Make settles a recipe's prerequisites before its first line,
            // and the compilation has already brought these to the ground.
            order_only_inputs: &settled,
            validations: &[],
            always_dirty: true,
            intermediate: false,
            // A staging proxy is a name the compiler invented, not a Make
            // target: no recipe was written for it and there is nothing for
            // `-t` to stand in for.
            has_touchable_recipe: false,
            outputs_unaliased: false,
            outputs_low_resolution: false,
            bindings: Vec::new(),
        })?;
        if let Some(held) = &pending.invocations[index].preceding_script
            && let Some(steps) = held.launch(self.graph.path(proxy))
        {
            self.settled_edges.push((built, steps));
        }
        Ok(Some(proxy))
    }

    /// Ask the ordinary graph evaluator whether a staged wrapper must run, and
    /// take the question off the edge when the answer is that it must not.
    ///
    /// One act rather than two, because the probe rule exists only to be asked
    /// and its command is `false`. A wrapper that has to run reaches
    /// [`Self::complete_subninja`], which replaces the rule with what the
    /// recipe left; a wrapper that does not has no children to compose and no
    /// recipe to run, so it never reaches that call and would otherwise carry
    /// the probe into the graph the build is made from. Anything that then
    /// reached the edge — `-B`, a prerequisite that moved after the question
    /// was asked, a makefile update asking for the file — would run `false`.
    ///
    /// What it becomes is a target with no command whose outputs are its own
    /// files: unaliased, because a recursive target's outputs are read off the
    /// disk and are not names standing in for child goals this one turned out
    /// not to need.
    pub(crate) fn settle_subninja_freshness<F>(
        &mut self,
        edge: Edge,
        stat: &mut F,
        asserted: crate::runtime::AssertedDates<'_>,
    ) -> Result<bool, crate::error::GraphError>
    where
        F: FnMut(&Path) -> std::io::Result<i64>,
    {
        if self
            .graph
            .edge_dirty_with(edge, stat, asserted, &mut self.subninja_freshness)?
        {
            return Ok(true);
        }
        self.graph.set_edge_rule(edge, self.phony);
        self.graph.unalias_outputs(edge);
        Ok(false)
    }

    /// The graph, or the first thing kati asked for that a graph cannot hold.
    ///
    /// `ungrouped` is the budget every edge no job group claimed is held to,
    /// which is wanted only where a unit founded a group wider than the run's:
    /// that is the one case where the scheduler runs above what the command
    /// line asked for, and GNU Make keeps the run's own group at its own size
    /// across exactly that — the jobserver a forced `-j` walks away from goes
    /// on bounding every Make still in it. Held here rather than as the edges
    /// are emitted, because which unit asked for the most is not known until
    /// the last of them has been read.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError`] for the failure that stopped construction:
    /// two rules generating one output, an edge naming a pool nobody declared.
    // [spec:ronin:req:make.jobserver+3]
    pub fn into_graph(
        mut self,
        ungrouped: Option<NonZeroUsize>,
    ) -> Result<BuildGraph, FrontendError> {
        if let Some(depth) = ungrouped
            && let Ok(pool) = self.graph.define_pool(b"make_jobs_run")
        {
            self.graph.set_pool_depth(pool, depth);
            self.graph.hold_unpooled_edges(pool);
        }
        match self.failure {
            Some(failure) => Err(failure),
            None => Ok(self.graph),
        }
    }

    /// The typed construction failure behind kati's sink error, if any.
    pub(crate) fn construction_failure(&self) -> Option<FrontendError> {
        self.failure.clone()
    }

    /// Hand one edge's late freshness to the graph.
    ///
    /// `KATI_NEW_INPUTS` is the name kati writes into a recipe for `$?`, and
    /// its two neighbours are the names it writes for `$(?D)` and `$(?F)` —
    /// the same list's directory and file halves, which need names of their
    /// own because there is nothing to halve until the scheduler has picked
    /// the list. The unit's path prefix is what the value's names are spelt
    /// against, because the command runs where the unit's Makefile was read
    /// and GNU Make's recursive child names its prerequisites the way that
    /// Makefile did.
    fn defer_freshness(&mut self, edge: Edge, deferred: &PendingDeferred) {
        let published = deferred
            .new_input_names
            .iter()
            .map(|(node, name)| (*node, name.as_slice()))
            .collect::<Vec<_>>();
        self.graph.set_deferred_freshness(
            edge,
            &crate::frontend::DeferredSpec {
                outputs: &deferred.outputs,
                always_dirty_output: deferred.always_dirty_output,
                dates_do_not_decide: deferred.dates_do_not_decide,
                heads_the_group: deferred.heads_the_group,
                always_new_inputs: &deferred.always_new_inputs,
                excluded_new_inputs: &deferred.excluded_new_inputs,
                new_input_names: &published,
                new_inputs_variable: kati::command::NEW_INPUTS_VARIABLE,
                new_inputs_directories_variable: kati::command::NEW_INPUTS_DIRECTORIES_VARIABLE,
                new_inputs_filenames_variable: kati::command::NEW_INPUTS_FILENAMES_VARIABLE,
                new_inputs_directory: self.unit.path_prefix.as_os_str().as_bytes(),
            },
        );
    }

    /// Record a construction failure and give kati something to unwind with.
    fn refuse(&mut self, failure: FrontendError) -> anyhow::Error {
        let reported = anyhow::Error::new(failure.clone());
        self.failure.get_or_insert(failure);
        reported
    }

    /// The node for one of kati's symbols, interned on first sight.
    fn node(&mut self, names: &dyn Interner, symbol: Symbol) -> Result<Node, anyhow::Error> {
        if let Some(node) = self.interned.get(&symbol) {
            return Ok(*node);
        }
        let path = symbol.as_bytes(&names);
        let path = Path::new(std::ffi::OsStr::from_bytes(&path));
        let qualified = if self.unit.path_prefix.as_os_str().is_empty() || path.is_absolute() {
            path.to_owned()
        } else {
            self.unit.path_prefix.join(path)
        };
        let node = if self.unit.root {
            self.graph.node(qualified.as_os_str().as_bytes())
        } else {
            self.graph.isolated_node(qualified.as_os_str().as_bytes())
        };
        match node {
            Ok(node) => {
                // A path an enclosing unit makes is that unit's file, and the
                // child's name for it is a name for that node. The isolated
                // node just allocated was the way to learn the canonical path;
                // nothing refers to it. See [`Self::begin_subninja`].
                let node = if self.unit.root {
                    node
                } else {
                    self.enclosing_node_for(node).unwrap_or(node)
                };
                self.interned.insert(symbol, node);
                Ok(node)
            }
            Err(failure) => Err(self.refuse(failure)),
        }
    }

    /// The nodes for a whole partition of an edge.
    fn node_list(
        &mut self,
        names: &dyn Interner,
        symbols: &[Symbol],
    ) -> Result<Vec<Node>, anyhow::Error> {
        symbols
            .iter()
            .map(|symbol| self.node(names, *symbol))
            .collect()
    }

    /// The node one name observes, which for a `::` chain's target is the file
    /// rather than the completion proxy everything naming it depends on.
    fn observed_node(
        &mut self,
        names: &dyn Interner,
        symbol: Symbol,
    ) -> Result<Node, anyhow::Error> {
        self.observed_members
            .get(&symbol)
            .copied()
            .map_or_else(|| self.node(names, symbol), Ok)
    }

    /// The spellings kati asked for, with each name resolved to the node it
    /// respells and the text kept as kati wrote it.
    fn published_names(
        &mut self,
        names: &dyn Interner,
        published: &[(Symbol, Symbol)],
    ) -> Result<Vec<(Node, Vec<u8>)>, anyhow::Error> {
        let mut resolved = Vec::with_capacity(published.len());
        for (input, published) in published {
            let node = self.node(names, *input)?;
            resolved.push((node, names.symtab().name(*published).to_vec()));
        }
        Ok(resolved)
    }

    /// Both halves of what the directory search left for the build to settle:
    /// where this target was found, and the references its own command carries
    /// for prerequisites that were.
    ///
    /// The first is said of the output because it is the file that was found,
    /// and everything that asks — the edge's own freshness, and the name a
    /// dependent reads it by — asks about the name. The second is said
    /// relative to the unit's own directory, because the command runs where its
    /// Makefile was read and names its prerequisites the way that Makefile did.
    /// The node a searched-for target's second place to look is said of.
    ///
    /// A completion join's own output is a name the compiler invented to
    /// sequence a `::` chain, and the file the search answered about is the
    /// target the Makefile wrote. Every action in the chain observes that node
    /// rather than the join's proxy — its freshness is deferred to it — so the
    /// found path has to hang off it or nothing consults the search at all.
    /// Every other edge is the file it names.
    const fn searched_target(
        edge: &SinkEdge<'_>,
        outputs: &[Node],
        completion_output: Node,
    ) -> Option<Node> {
        if edge.completion_join {
            Some(completion_output)
        } else {
            outputs.first().copied()
        }
    }

    fn record_searched_spellings(
        &mut self,
        built: Edge,
        names: &dyn Interner,
        edge: &SinkEdge<'_>,
        output: Option<Node>,
    ) -> Result<(), anyhow::Error> {
        // Which name a `::` record filed the target under, said of the same
        // node and for the same reason — the record declares the name the
        // Makefile wrote, which for a completion join is not the edge's own
        // output. Nothing here reads it either.
        if let Some(declared) = edge.declared_by_double_colon
            && let Some(target) = output
        {
            self.graph
                .set_double_colon_target(target, &names.symtab().name(declared));
        }
        let found = edge
            .searched_at
            .zip(output)
            .map(|(found, output)| (output, names.symtab().name(found)));
        // The other half of the same search, for a target `GPATH` moved rather
        // than left where it was written: the node is the found path, and this
        // is the name that carries its rule. Said of the output for the same
        // reason — it is the file that was moved.
        let written = edge
            .written_as
            .zip(output)
            .map(|(written, output)| (output, names.symtab().name(written)));
        // The variable's spelling is kept as kati wrote it, for the same reason
        // `published_names` keeps its own: a name this side does not use is not
        // a path this side may relocate.
        let mut references = Vec::with_capacity(edge.settled_names.len());
        for settled in edge.settled_names {
            // The file the name stands for, and not the proxy that sequences
            // the work behind it: a `::` chain's target is redirected to the
            // completion join so everything naming it waits for the whole
            // chain, and a spelling is a question about the file. The proxy is
            // a name the compiler invented, which no search ever answered
            // about, so reading the spelling off it would answer with that.
            let node = self.observed_node(names, settled.input)?;
            references.push((
                names.symtab().name(settled.variable).to_vec(),
                node,
                match settled.view {
                    kati::build_sink::SettledNameView::Whole => crate::graph::SettledView::Whole,
                    kati::build_sink::SettledNameView::Directory => {
                        crate::graph::SettledView::Directory
                    }
                    kati::build_sink::SettledNameView::Filename => {
                        crate::graph::SettledView::Filename
                    }
                },
            ));
        }
        let directory = self.unit.path_prefix.as_os_str().as_bytes().to_vec();
        self.graph.set_searched_spellings(
            built,
            found.as_ref().map(|(output, found)| (*output, &**found)),
            written
                .as_ref()
                .map(|(output, written)| (*output, &**written)),
            &directory,
            references,
        );
        Ok(())
    }

    fn deferred_edge(
        &mut self,
        names: &dyn Interner,
        edge: &SinkEdge<'_>,
    ) -> Result<Option<PendingDeferred>, anyhow::Error> {
        let mut outputs = Vec::with_capacity(edge.deferred_freshness_outputs.len());
        for symbol in edge.deferred_freshness_outputs {
            outputs.push(self.observed_node(names, *symbol)?);
        }
        if outputs.is_empty() {
            return Ok(None);
        }
        Ok(Some(PendingDeferred {
            outputs,
            always_dirty_output: edge.deferred_freshness_always_dirty,
            dates_do_not_decide: edge.deferred_freshness_ignores_dates,
            heads_the_group: edge.deferred_freshness_heads_the_record,
            always_new_inputs: self.node_list(names, edge.deferred_always_new_inputs)?,
            excluded_new_inputs: self.node_list(names, edge.deferred_excluded_new_inputs)?,
            new_input_names: self.published_names(names, edge.deferred_new_input_names)?,
        }))
    }

    /// Qualify a Makefile-relative auxiliary path the same way as its graph
    /// nodes. The child command writes it after `cd`, while Ronin reads it from
    /// the root, so both names must identify the same file.
    fn qualify_path(&self, bytes: &[u8]) -> Vec<u8> {
        let path = Path::new(OsStr::from_bytes(bytes));
        if self.unit.path_prefix.as_os_str().is_empty() || path.is_absolute() {
            bytes.to_vec()
        } else {
            self.unit
                .path_prefix
                .join(path)
                .as_os_str()
                .as_bytes()
                .to_vec()
        }
    }

    /// Everything about this unit that a command line is built around, kept
    /// separately so a recipe expanded when its edge is launched is wrapped
    /// exactly as the same recipe expanded here would have been.
    pub(crate) fn layout(&self) -> CommandLayout {
        CommandLayout {
            command_directory: self.unit.command_directory.clone(),
            recipe_environment: self.unit.recipe_environment.clone(),
            root_directory: self.root_directory.clone(),
            root: self.unit.root,
            unreadable: self.unit.unreadable.clone(),
        }
    }

    /// The command line that runs one script, and the bindings it needs.
    ///
    /// A script short enough to pass as an argument is quoted into one, which
    /// is why it is escaped for the shell that will unquote it. A script too
    /// long has to reach the shell as a file, and the shell is then given the
    /// file rather than a `-c` and a string.
    fn command_bindings(
        &self,
        shell: &[u8],
        shell_flags: &[u8],
        command: SinkCommand<'_>,
        scoped: &[kati::export::EnvironmentChange],
    ) -> Vec<(Binding, Template)> {
        match command {
            SinkCommand::Inline(script) => {
                // The `cd` and `env` that give a compilation unit its Make `-C`
                // working directory and environment without moving Ronin's
                // executor.
                let mut command = Template::literal(&self.layout().prefix(scoped));
                // The shell is escaped into the line and the flags are not,
                // which is GNU Make's own asymmetry -- see
                // [`kati::simple_command::escaped_shell_name`]. The manifest
                // escaping the writer adds on top composes with it: a `\$` here
                // is written `\$$` there and reaches the shell as one `$`.
                command.push_literal(&kati::simple_command::escaped_shell_name(shell));
                command.push_literal(b" ");
                command.push_literal(shell_flags);
                command.push_literal(b" \"");
                command.push_literal(&escape_shell(&Bytes::copy_from_slice(script)));
                command.push_literal(b"\"");
                vec![(self.bindings.command, command)]
            }
            // The response file is one per edge because the output is, which is
            // a fact about the edge rather than about any format. Naming it by
            // reference to `$out` leaves the escaping to the same expansion
            // that escapes every other path, instead of reimplementing it.
            SinkCommand::ResponseFile(script) => {
                let mut response_file = Template::default();
                if !self.unit.root && !self.root_directory.as_os_str().is_empty() {
                    response_file.push_literal(self.root_directory.as_os_str().as_bytes());
                    response_file.push_literal(std::path::MAIN_SEPARATOR_STR.as_bytes());
                }
                response_file.push_variable(self.bindings.out);
                response_file.push_literal(b".rsp");
                let mut command = Template::literal(&self.layout().prefix(scoped));
                command.push_literal(&kati::simple_command::escaped_shell_name(shell));
                command.push_literal(b" ");
                // The script travels differently and the shell is the same
                // shell: `-e` a `.POSIX:` recipe was given still has to be on
                // this launch, and only the letter that would take the file
                // name for the command comes off.
                let flags = kati::ninja::script_file_flags(shell_flags);
                if !flags.is_empty() {
                    command.push_literal(&flags);
                    command.push_literal(b" ");
                }
                if !self.unit.root && !self.root_directory.as_os_str().is_empty() {
                    command.push_literal(self.root_directory.as_os_str().as_bytes());
                    command.push_literal(std::path::MAIN_SEPARATOR_STR.as_bytes());
                }
                command.push_variable(self.bindings.out);
                command.push_literal(b".rsp");
                vec![
                    (self.bindings.command, command),
                    (self.bindings.rspfile, response_file),
                    (self.bindings.rspfile_content, Template::literal(script)),
                ]
            }
        }
    }

    /// Bind the executor-facing half of a kati rule. Recursive invocations are
    /// deliberately absent: their child graphs are connected by
    /// [`Self::complete_subninja`] instead.
    ///
    /// No binding here describes a dry run. Make's `-n` is Ninja's `-n` on the
    /// graph kati compiled, and the recursion GNU Make would have run a child
    /// process to discover is already in that graph as composed child edges.
    // [spec:ronin:req:make.state-outside-the-tree+3]
    fn executor_rule_bindings(
        &self,
        rule: &SinkRule<'_>,
        command: SinkCommand<'_>,
        ignore_errors: bool,
    ) -> Vec<(Binding, Template)> {
        let mut bindings = self.command_bindings(
            rule.shell,
            rule.shell_flags,
            command,
            rule.recipe_environment,
        );
        // [spec:ronin:req:make.narration+2]
        // The compiler settled this: the description is what GNU Make would
        // have echoed, or the narration the recipe wrote for itself in a
        // silenced echo. There is nothing to add here and nothing to guess.
        // A recipe that echoes nothing binds nothing, which is how a build
        // this Makefile runs in silence says so.
        if let Some(description) = rule.description {
            bindings.push((self.bindings.description, Template::literal(description)));
        }
        // GNU Make decides whether a recipe is current from timestamps alone.
        // Ninja's generator control expresses exactly the command-hash half of
        // that policy, so the executor still needs no front-end provenance.
        bindings.push((self.bindings.generator, Template::literal(b"1")));
        if let Some(depfile) = rule.depfile {
            bindings.push((
                self.bindings.depfile,
                Template::literal(&self.qualify_path(depfile)),
            ));
            // kati emits no other depfile format, and says so.
            bindings.push((self.bindings.deps, Template::literal(b"gcc")));
        }
        // Carried rather than answered for here. kati left the recipe's status
        // in place instead of throwing it away, so that whatever runs the
        // recipe can say what it was and go on, and only the thing running it
        // can do that.
        if ignore_errors {
            bindings.push((self.bindings.ignore_errors, Template::literal(b"1")));
        }
        bindings
    }

    /// The rule bindings that do not come from a recipe's text.
    ///
    /// The command is deliberately one that fails: nothing should ever run it,
    /// because the edge that names it binds its real command as it is
    /// launched, and a placeholder that quietly succeeded would turn a missing
    /// binding into a build that claimed to have done the work.
    fn deferred_rule_bindings(&self) -> Vec<(Binding, Template)> {
        vec![
            (self.bindings.command, Template::literal(b"false")),
            (self.bindings.generator, Template::literal(b"1")),
        ]
    }

    /// The per-edge bindings kati names, and the serialising pool this unit
    /// puts a command edge in when `.NOTPARALLEL` asked for one.
    fn edge_bindings(&self, edge: &SinkEdge<'_>) -> Vec<(Binding, Vec<u8>)> {
        let mut bindings = Vec::new();
        if let Some(pool) = edge.pool {
            bindings.push((self.bindings.pool, pool.to_vec()));
        }
        let subninja_rule = edge.rule.and_then(|id| self.subninja_rules.get(&id));
        let is_subninja = subninja_rule.is_some();
        let has_residual_action = subninja_rule.is_some_and(|rule| rule.residual_rule.is_some());
        // The serialising pool first where a unit has both: it is the narrower
        // of the two, and an edge names one pool.
        if edge.pool.is_none()
            && edge.rule.is_some()
            && (!is_subninja || has_residual_action)
            && let Some(pool) = self
                .unit
                .serial_pool
                .as_ref()
                .or_else(|| self.unit.job_pool.as_ref().map(|(name, _)| name))
        {
            bindings.push((self.bindings.pool, pool.clone()));
        }
        bindings
    }

    /// Claim what a declared rule left for the executor to ask about, for the
    /// edge that names it: the recipe the compiler did not expand, and the
    /// launches of one it did.
    ///
    /// Never both — a deferred recipe carries its steps on the expansion
    /// instead — and neither for an edge with no rule at all. `output` is the
    /// edge's own, which is what a response file is named after.
    fn record_late_bindings(&mut self, built: Edge, rule: Option<RuleId>, output: Option<Node>) {
        let Some(id) = rule else {
            return;
        };
        if let Some(recipe) = self.deferred_rules.remove(&id) {
            self.deferred_edges.push((built, recipe));
        }
        if let Some(steps) = self.settled_rules.remove(&id) {
            self.settled_edges.push((built, steps));
        }
        if let Some(held) = self.settled_scripts.remove(&id)
            && let Some(output) = output
            && let Some(steps) = held.launch(self.graph.path(output))
        {
            self.settled_edges.push((built, steps));
        }
    }

    /// What this unit's edges will be asked about when they run: the ones
    /// whose recipe is still to be expanded, with the recipe each names, and
    /// the ones whose recipe was read while compiling and which run a process
    /// per command line all the same.
    pub(crate) fn take_late_edges(&mut self) -> LateEdges {
        (
            std::mem::take(&mut self.deferred_edges),
            self.take_settled_edges(),
        )
    }

    /// The same launches for the edges a recursive recipe's segments reach,
    /// which are made after the unit that wrote them has been taken.
    ///
    /// A parent's recipe is cut into segments around the invocations lifted out
    /// of it, and those segments become edges while the children are being
    /// compiled — after this unit's own edges were claimed, and possibly after
    /// the last of them.
    pub(crate) fn take_settled_edges(&mut self) -> Vec<(Edge, SettledSteps)> {
        std::mem::take(&mut self.settled_edges)
    }

    /// Bindings for a run of a recipe's own lines that is not the edge making
    /// the target.
    ///
    /// Everything about how the script reaches a shell is the recipe's, and
    /// nothing about how the target is observed is: a depfile and a `restat`
    /// answer for the file the rule makes, and the edge that makes it is the
    /// recursive wrapper rather than this. Giving both the same depfile would
    /// have two edges claiming one discovered dependency file.
    fn recipe_lines_bindings(
        &self,
        rule: &SinkRule<'_>,
        command: SinkCommand<'_>,
        ignore_errors: bool,
    ) -> Vec<(Binding, Template)> {
        let mut bindings = self.command_bindings(
            rule.shell,
            rule.shell_flags,
            command,
            rule.recipe_environment,
        );
        // [spec:ronin:req:make.narration+2]
        // The recipe's own words, and silence where it wrote none. These are
        // the residual lines of a recipe whose recursion was lifted, so the
        // echo that belongs to them was read with the rest of the recipe.
        if let Some(description) = rule.description {
            bindings.push((self.bindings.description, Template::literal(description)));
        }
        bindings.push((self.bindings.generator, Template::literal(b"1")));
        if ignore_errors {
            bindings.push((self.bindings.ignore_errors, Template::literal(b"1")));
        }
        bindings
    }

    fn define_executor_rule(
        &mut self,
        name: &[u8],
        bindings: Vec<(Binding, Template)>,
    ) -> Result<Rule, anyhow::Error> {
        match self.graph.define_rule(self.unit.scope, name, bindings) {
            Ok(defined) => Ok(defined),
            Err(failure) => Err(self.refuse(failure)),
        }
    }
}

impl BuildSink for GraphSink {
    fn new_inputs_timing(&self) -> NewInputsTiming {
        NewInputsTiming::SchedulerBoundary
    }

    fn shell_evaluation(&self) -> ShellEvaluation {
        ShellEvaluation::Expansion
    }

    /// This process runs the build, so a `$(file ...)` a recipe carries is
    /// performed here, at the moment the recipe is expanded, exactly as GNU
    /// Make performs it. Nothing about it needs a shell, and nothing about it
    /// can be written into one: a read has to hand its result back to the
    /// expansion that asked for it, which is why kati refused the whole
    /// function rather than deferring it the way it defers `$(shell)`.
    ///
    /// The Linux kernel is the tree that needs this. `read-file` in
    /// `scripts/Kbuild.include` is `$(subst $(newline),$(space),$(file < $1))`,
    /// `KERNELRELEASE` calls it recursively, and `filechk_utsrelease.h`
    /// interpolates `KERNELRELEASE` into a recipe — so expanding that recipe
    /// reads a file, and refusing to read it stops `headers_install`.
    fn file_evaluation(&self) -> FileEvaluation {
        FileEvaluation::Expansion
    }

    /// GNU Make prints an `$(info ...)` while it expands the recipe, before
    /// any command line exists, so the text is never a command and a recipe
    /// that is nothing but the call has nothing to run. This process expands
    /// the recipe immediately before running it, which is that same moment.
    ///
    /// It matters beyond where the text lands. A recipe of `$(info X)` alone
    /// folds into the empty expansion that starts no shell, so the target
    /// reports up to date and `-q` answers zero rather than one; `$(error)`
    /// becomes a refusal raised out of the expansion rather than a command
    /// contrived to fail, which is why it still fires under `-n`.
    fn output_evaluation(&self) -> OutputEvaluation {
        OutputEvaluation::Expansion
    }

    /// GNU Make expands a recipe when it is about to run it, and this graph is
    /// run by the process that built it, so it can do the same.
    ///
    /// For a recursive `$(MAKE)` child's recipes as much as for the root's. A
    /// child is compiled by a session of its own in a directory of its own,
    /// and both are retained past the compilation that made them —
    /// `PendingRecipes` holds one per unit and an edge finds the one that owns
    /// it — so a child's recipe is expanded in the session that read it and in
    /// the directory it was read from, which is what GNU Make's child process
    /// would have done.
    fn recipe_expansion(&self) -> RecipeExpansion {
        self.expansion
    }

    fn start(&mut self, pools: &[SinkPool<'_>]) -> anyhow::Result<()> {
        for pool in pools {
            if !self.declared_pools.insert(pool.name.to_vec()) {
                continue;
            }
            let declared = match self.graph.define_pool(pool.name) {
                Ok(declared) => declared,
                Err(failure) => return Err(self.refuse(failure)),
            };
            if let Some(depth) = NonZeroUsize::new(pool.depth) {
                self.graph.set_pool_depth(declared, depth);
            }
        }
        if let Some(name) = self.unit.serial_pool.clone() {
            self.declared_pools.insert(name.clone());
            let declared = self
                .graph
                .define_pool(&name)
                .map_err(|failure| self.refuse(failure))?;
            self.graph.set_pool_depth(declared, NonZeroUsize::MIN);
        }
        // A group outlives the unit that founded it, so the second unit to
        // join one finds it declared already and says nothing further about it.
        if let Some((name, budget)) = self.unit.job_pool.clone()
            && self.declared_pools.insert(name.clone())
        {
            let declared = self
                .graph
                .define_pool(&name)
                .map_err(|failure| self.refuse(failure))?;
            self.graph.set_pool_depth(declared, budget);
        }
        Ok(())
    }

    // [spec:ronin:req:make.graph-direct]
    fn declare_rule(&mut self, _names: &dyn Interner, rule: &SinkRule<'_>) -> anyhow::Result<()> {
        let script = match rule.command {
            SinkCommand::Inline(script) | SinkCommand::ResponseFile(script) => script,
        };
        // A recipe naming recursion nothing could be lifted out of arrives with
        // no invocations and is an ordinary recipe here: it runs as the script
        // it is and the Make it names starts, which is what GNU Make does with
        // it. Nothing about a recipe is refused at this point.
        if !rule.subninjas.is_empty() {
            let residual_rule = rule
                .residual_command
                .map(|command| {
                    let bindings =
                        self.executor_rule_bindings(rule, command, rule.residual_ignore_errors);
                    let name = format!("rule{}_residual", rule.id);
                    self.define_executor_rule(name.as_bytes(), bindings)
                })
                .transpose()?;
            // A segment of a recipe is a recipe: GNU Make runs its lines one
            // process each, so a `+`-marked line inside it runs under `-t` and
            // answers under `-q` the way it would in any recipe. The invocation
            // lifted out from between the lines is not among them, so the split
            // is over exactly the lines the segment kept — line by line where
            // each can be an argument, and the assembled script where one is
            // too long to be one.
            let residual_script = rule.residual_command.map(|command| {
                SettledSegment::held(
                    self.layout(),
                    rule,
                    command,
                    rule.residual_steps,
                    rule.residual_ignore_errors,
                )
            });
            let mut invocations = Vec::with_capacity(rule.subninjas.len());
            for (index, subninja) in rule.subninjas.iter().enumerate() {
                let preceding_rule = match subninja.preceding {
                    Some(command) => {
                        let bindings = self.recipe_lines_bindings(
                            rule,
                            command,
                            subninja.preceding_ignore_errors,
                        );
                        let name = format!("rule{}_preceding{index}", rule.id);
                        Some(self.define_executor_rule(name.as_bytes(), bindings)?)
                    }
                    None => None,
                };
                let preceding_script = subninja.preceding.map(|command| {
                    SettledSegment::held(
                        self.layout(),
                        rule,
                        command,
                        subninja.preceding_steps,
                        subninja.preceding_ignore_errors,
                    )
                });
                invocations.push(SubninjaInvocation {
                    command: subninja.command.to_vec(),
                    make: subninja.make.to_vec(),
                    shell: rule.shell.to_vec(),
                    shell_flags: rule.shell_flags.to_vec(),
                    preceding_rule,
                    preceding_script,
                    location: subninja.location.map(ToOwned::to_owned),
                });
            }
            self.subninja_rules.insert(
                rule.id,
                SubninjaRule {
                    invocations,
                    residual_rule,
                    residual_script,
                    diagnostic_command: script.to_vec(),
                },
            );
            return Ok(());
        }

        let bindings = if rule.deferred_recipe.is_some() {
            self.deferred_rule_bindings()
        } else {
            self.executor_rule_bindings(rule, rule.command, rule.ignore_errors)
        };
        let name = format!("rule{}", rule.id);
        let defined = self.define_executor_rule(name.as_bytes(), bindings)?;
        self.rules.insert(rule.id, defined);
        if let Some(recipe) = rule.deferred_recipe {
            self.deferred_rules.insert(rule.id, recipe);
        }
        // GNU Make runs a process per command line whether or not the text was
        // settled early, so a recipe the compiler had to read for itself is
        // still handed over as its launches. The assembled script stays the
        // rule's command — the progress line, the log and `-n` all want the
        // whole of it — and the same rule the launch-time path uses decides
        // whether the split is possible at all: a line too long to be an
        // argument needs a response file, and the file is named per edge.
        if CommandLayout::launches_line_by_line(rule.steps) {
            // Both phases' launches, because the makefile update hands a
            // `$(MAKE)` on one of these lines a `MAKEFLAGS` the goals do not.
            // See [`SettledSteps`].
            self.settled_rules.insert(
                rule.id,
                self.layout()
                    .settled_steps(rule.steps, rule.recipe_environment),
            );
        } else if rule.deferred_recipe.is_none() {
            // A recipe whose lines are not the launches — one whose script a
            // depfile extraction rewrote, and one holding a line too long to be
            // an argument — still runs as one script under one shell, and which
            // shell that is is not the recipe's to change. The recipe expanded
            // at launch is not this: it has no text here to launch anything
            // with, and settles its own script when it gets one.
            let settled =
                SettledScript::held(self.layout(), rule, rule.command, rule.ignore_errors);
            self.settled_scripts.insert(rule.id, settled);
        }
        Ok(())
    }

    // [spec:ronin:req:make.graph-direct]
    // [spec:ronin:req:make.phony-always-dirty]
    fn declare_edge(&mut self, names: &dyn Interner, edge: &SinkEdge<'_>) -> anyhow::Result<()> {
        let completion_output = self.node(names, edge.output)?;
        let implicit_outputs = self.node_list(names, edge.implicit_outputs)?;
        let inputs = self.node_list(names, edge.inputs)?;
        let order_only_inputs = self.node_list(names, edge.order_only_inputs)?;
        let forgiven_order_inputs = self.node_list(names, edge.forgiven_order_only_inputs)?;
        let withdrawal = PendingWithdrawal {
            outputs: self.node_list(names, edge.withdrawable_outputs)?,
            on_error: edge.delete_on_error,
        };
        let peer_outputs = self.node_list(names, edge.peer_outputs)?;
        let disposable_outputs = self.node_list(names, edge.disposable_outputs)?;
        let deferred = self.deferred_edge(names, edge)?;
        let outputs = self.published_outputs(edge, completion_output)?;

        let bindings = self.edge_bindings(edge);
        let low_resolution = Self::dates_in_whole_seconds(names, edge);
        if self.skips_an_enclosing_files_rule(edge, &outputs, &implicit_outputs) {
            return Ok(());
        }
        if let Some(id) = edge.rule
            && let Some(rule) = self.subninja_rules.remove(&id)
        {
            self.unit.subninjas.push(PendingSubninja {
                invocations: rule.invocations,
                scope: self.unit.scope,
                residual_rule: rule.residual_rule,
                residual_script: rule.residual_script,
                diagnostic_command: rule.diagnostic_command,
                explicit_outputs: outputs,
                implicit_outputs,
                inputs,
                order_only_inputs,
                forgiven_order_inputs,
                always_dirty: edge.always_dirty,
                deferred,
                completion_output: edge.completion_join.then_some(completion_output),
                intermediate: edge.intermediate,
                disposable_outputs,
                low_resolution,
                withdrawal,
                peer_outputs,
                bindings,
            });
            return Ok(());
        }

        let mention = Self::mentions_only_its_name(edge);
        if self.settle_one_path_two_spellings(mention, &outputs, &implicit_outputs) {
            return Ok(());
        }

        let rule = match edge.rule {
            Some(id) => self
                .rules
                .remove(&id)
                .ok_or_else(|| anyhow::Error::msg(format!("edge names undeclared rule{id}")))?,
            None => self.phony,
        };

        let spec = EdgeSpec {
            scope: self.unit.scope,
            rule,
            explicit_outputs: &outputs,
            implicit_outputs: &implicit_outputs,
            explicit_inputs: &inputs,
            // A Make prerequisite is either ordinary or order-only. Nothing in
            // a Makefile produces the third partition, so nothing fills it.
            implicit_inputs: &[],
            order_only_inputs: &order_only_inputs,
            validations: &[],
            always_dirty: edge.always_dirty,
            intermediate: edge.intermediate,
            has_touchable_recipe: edge.has_touchable_recipe,
            // A Make target with no commands compiles to the `phony` rule for
            // want of anything else to build it with, and means the opposite of
            // what a manifest's `phony` means: GNU Make made the target, wrote
            // nothing, and left it as absent as it found it. Every edge that
            // reaches here came from a Makefile, so having no rule is the whole
            // of the question.
            outputs_unaliased: edge.rule.is_none(),
            outputs_low_resolution: low_resolution,
            bindings,
        };
        match self.graph.add_edge(spec) {
            Ok(built) => {
                self.graph.set_disposable_outputs(&disposable_outputs);
                self.record_late_bindings(built, edge.rule, outputs.first().copied());
                // Every Make target is one GNU Make decides from the disk, and
                // looks at again once its recipe has run whatever the recipe
                // did, so this is what Make is here rather than something a
                // Makefile asks for.
                self.graph.set_make_target_freshness(built);
                self.graph
                    .forgive_order_inputs(built, &forgiven_order_inputs);
                self.graph
                    .set_withdrawal(built, withdrawal.outputs, withdrawal.on_error);
                self.graph.set_peer_outputs(built, peer_outputs);
                let searched = Self::searched_target(edge, &outputs, completion_output);
                self.record_searched_spellings(built, names, edge, searched)?;
                if let Some(deferred) = deferred {
                    self.defer_freshness(built, &deferred);
                }
                if edge.completion_join {
                    self.graph.set_completion_join(built, completion_output);
                }
                if mention && let Some(output) = outputs.first() {
                    self.mentions.insert(*output, built);
                }
                self.record_generated(edge, &outputs, &implicit_outputs);
                self.unit.edges.push(built);
                Ok(())
            }
            Err(failure) => Err(self.refuse(failure)),
        }
    }

    fn set_default_targets(
        &mut self,
        names: &dyn Interner,
        targets: &[Symbol],
    ) -> anyhow::Result<()> {
        for target in targets {
            let node = self.node(names, *target)?;
            if self.unit.root {
                self.graph.add_default(node);
            }
            self.unit.targets.push(node);
        }
        Ok(())
    }

    fn finish(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
