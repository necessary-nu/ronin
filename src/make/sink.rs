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
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

mod invented;

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
    restat: Binding,
    rspfile: Binding,
    rspfile_content: Binding,
    pool: Binding,
    tags: Binding,
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
            restat: graph.binding(b"restat"),
            rspfile: graph.binding(b"rspfile"),
            rspfile_content: graph.binding(b"rspfile_content"),
            pool: graph.binding(b"pool"),
            tags: graph.binding(b"tags"),
            ignore_errors: graph.binding(crate::build::IGNORE_ERRORS),
            out: graph.binding(b"out"),
        }
    }
}

pub(crate) use super::layout::{CommandLayout, SettledScript, SettledSteps};

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
    /// How that rule's script is launched, once its edge exists.
    preceding_script: Option<SettledScript>,
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
    /// How the residual rule's script is launched, once its edge exists.
    residual_script: Option<SettledScript>,
    diagnostic_command: Vec<u8>,
    explicit_outputs: Vec<Node>,
    implicit_outputs: Vec<Node>,
    inputs: Vec<Node>,
    order_only_inputs: Vec<Node>,
    /// The subset of `order_only_inputs` this wrapper outlives a failure of.
    forgiven_order_inputs: Vec<Node>,
    validations: Vec<Node>,
    always_dirty: bool,
    deferred: Option<PendingDeferred>,
    completion_output: Option<Node>,
    intermediate: bool,
    disposable: bool,
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
    residual_script: Option<SettledScript>,
    diagnostic_command: Vec<u8>,
}

/// What one kati compilation unit contributed to the shared graph.
pub(crate) struct UnitOutput {
    pub(crate) targets: Vec<Node>,
    pub(crate) subninjas: Vec<PendingSubninja>,
    pub(crate) edges: Vec<Edge>,
}

/// The targets and complete edge closure contributed by one compiled unit.
#[derive(Clone)]
pub(crate) struct UnitSubgraph {
    pub(crate) targets: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
}

struct Unit {
    scope: Scope,
    path_prefix: PathBuf,
    command_directory: PathBuf,
    root: bool,
    serial_pool: Option<Vec<u8>>,
    recipe_environment: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    targets: Vec<Node>,
    subninjas: Vec<PendingSubninja>,
    edges: Vec<Edge>,
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
///
/// `.KATI_TAGS` is carried rather than dropped: it is opaque metadata for
/// whoever consumes the graph rather than an instruction to the build, so it
/// crosses as an edge binding under its own name and is left alone.
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
    /// kati's rule handles to Ronin's. kati mints one rule per edge and
    /// declares it immediately before that edge, so this holds one entry for
    /// as long as it takes to reach the edge that names it.
    rules: HashMap<RuleId, Rule>,
    /// Rules whose recipe kati left unexpanded, waiting for the edge that
    /// names them so the recipe can be recorded against the edge instead.
    deferred_rules: HashMap<RuleId, DeferredRecipeId>,
    /// Edges whose command this graph's own executor will ask for when it is
    /// about to run them.
    deferred_edges: Vec<(Edge, DeferredRecipeId)>,
    /// The launches a recipe read while compiling became, waiting for the edge
    /// that names their rule.
    settled_rules: HashMap<RuleId, SettledSteps>,
    /// Rules whose recipe reaches one shell as a whole script, waiting for the
    /// edge whose output names the response file that script may be read from.
    settled_scripts: HashMap<RuleId, SettledScript>,
    /// Edges whose recipe was read while compiling and which still run a
    /// process per command line.
    settled_edges: Vec<(Edge, SettledSteps)>,
    /// Recursive rules are not executor rules. They wait for their immediately
    /// following edge so the compiler can replace that edge with graph
    /// composition.
    subninja_rules: HashMap<RuleId, SubninjaRule>,
    /// kati's symbols to Ronin's nodes, so a path shared by many edges is
    /// canonicalized and interned once.
    interned: HashMap<Symbol, Node>,
    observed_members: HashMap<Symbol, Node>,
    declared_pools: HashSet<Vec<u8>>,
    completion_proxies: usize,
    recipe_stages: usize,
    serial_units: usize,
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
            root_directory: root_directory.to_owned(),
            unit: Unit {
                scope,
                path_prefix: PathBuf::new(),
                command_directory: PathBuf::new(),
                root: true,
                serial_pool: None,
                recipe_environment: Vec::new(),
                targets: Vec::new(),
                subninjas: Vec::new(),
                edges: Vec::new(),
            },
            bindings,
            phony,
            subninja_probe,
            rules: HashMap::new(),
            deferred_rules: HashMap::new(),
            deferred_edges: Vec::new(),
            settled_rules: HashMap::new(),
            settled_scripts: HashMap::new(),
            settled_edges: Vec::new(),
            subninja_rules: HashMap::new(),
            interned: HashMap::new(),
            observed_members: HashMap::new(),
            declared_pools: HashSet::new(),
            completion_proxies: 0,
            recipe_stages: 0,
            serial_units: 0,
            failure: None,
            expansion,
        }
    }

    /// Start emitting a child compilation unit into a scoped, path-qualified
    /// part of the same graph.
    pub(crate) fn begin_subninja(
        &mut self,
        parent: Scope,
        path_prefix: PathBuf,
        command_directory: PathBuf,
    ) {
        debug_assert!(self.rules.is_empty());
        debug_assert!(self.subninja_rules.is_empty());
        self.interned.clear();
        self.observed_members.clear();
        self.unit = Unit {
            scope: self.graph.child_scope(parent),
            path_prefix,
            command_directory,
            root: false,
            serial_pool: None,
            recipe_environment: Vec::new(),
            targets: Vec::new(),
            subninjas: Vec::new(),
            edges: Vec::new(),
        };
    }

    /// Constrain only this compilation unit's command edges to depth one.
    /// A semantic child gets its own unit, so a parent's `.NOTPARALLEL` never
    /// turns into a global executor switch that serialises the child graph.
    pub(crate) fn serialise_unit(&mut self, serial: bool) {
        if serial {
            let name = format!("make_serial_{}", self.serial_units).into_bytes();
            self.serial_units += 1;
            self.unit.serial_pool = Some(name);
        }
    }

    /// Give this compilation unit the environment changes that differ from
    /// the root Make invocation. They become part of each child command, so a
    /// composed subninja observes its own exports and `MAKELEVEL` without a
    /// nested process boundary.
    pub(crate) fn set_recipe_environment(
        &mut self,
        environment: Vec<(OsString, Option<OsString>)>,
    ) {
        let mut normalised = BTreeMap::new();
        for (name, value) in environment {
            normalised.insert(
                name.as_os_str().as_bytes().to_vec(),
                value.map(|value| value.as_os_str().as_bytes().to_vec()),
            );
        }
        self.unit.recipe_environment = normalised.into_iter().collect();
    }

    /// Finish the current compilation unit without finishing the shared graph.
    pub(crate) fn take_unit(&mut self) -> UnitOutput {
        debug_assert!(self.rules.is_empty());
        debug_assert!(self.subninja_rules.is_empty());
        UnitOutput {
            targets: std::mem::take(&mut self.unit.targets),
            subninjas: std::mem::take(&mut self.unit.subninjas),
            edges: std::mem::take(&mut self.unit.edges),
        }
    }

    /// Preserve compiler-input work already run by a provisional graph.
    pub(crate) fn mark_subgraphs_prebuilt(&mut self, roots: &[Node]) {
        self.graph.mark_subgraphs_prebuilt(roots, self.phony);
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

    /// Make each child group wait for the parent or preceding child group.
    fn attach_child_ordering(
        &mut self,
        pending: &PendingSubninja,
        child_groups: &[UnitSubgraph],
    ) -> Vec<Node> {
        let mut waits = pending
            .inputs
            .iter()
            .chain(&pending.order_only_inputs)
            .copied()
            .collect::<Vec<_>>();
        let mut child_targets = Vec::new();
        for child in child_groups {
            let preceding = waits
                .iter()
                .copied()
                .filter(|wait| !child.targets.contains(wait))
                .collect::<Vec<_>>();
            for edge in &child.edges {
                self.graph.add_order_only_inputs(*edge, &preceding);
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
        child_targets
    }

    /// Replace a recursive wrapper edge with the child goals it requested.
    ///
    /// Parent prerequisites become order-only inputs of every edge in each
    /// child subtree: no indirect child work starts before the wrapper recipe
    /// could have started, while the child's own timestamps still decide what
    /// work it needs. The wrapper becomes a phony alias for child targets whose
    /// identities remain local to their own recursive compilation units.
    // [spec:ronin:req:make.recursive-invocation+2]
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
        child_groups: &[UnitSubgraph],
        begun: bool,
    ) -> Result<Edge, FrontendError> {
        debug_assert_eq!(pending.invocations.len(), child_groups.len());
        let child_targets = self.attach_child_ordering(&pending, child_groups);

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
            validations: &pending.validations,
            always_dirty: pending.always_dirty,
            intermediate: pending.intermediate,
            disposable: pending.disposable,
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
            disposable: false,
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
        if self.graph.edge_dirty_with(edge, stat, asserted)? {
            return Ok(true);
        }
        self.graph.set_edge_rule(edge, self.phony);
        self.graph.unalias_outputs(edge);
        Ok(false)
    }

    /// The graph, or the first thing kati asked for that a graph cannot hold.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError`] for the failure that stopped construction:
    /// two rules generating one output, an edge naming a pool nobody declared.
    pub fn into_graph(self) -> Result<BuildGraph, FrontendError> {
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
    // [spec:ronin:req:make.state-outside-the-tree+2]
    fn executor_rule_bindings(
        &self,
        rule: &SinkRule<'_>,
        command: SinkCommand<'_>,
        ignore_errors: bool,
    ) -> Vec<(Binding, Template)> {
        // A recipe that continued across a newline reaches the shell with the
        // newline still in it, and a description is one line: `single_line`
        // takes the continuation back out for the narration alone, leaving the
        // command the recipe's own bytes.
        let described = match command {
            SinkCommand::Inline(script) => Some(kati::ninja::single_line(script)),
            SinkCommand::ResponseFile(_) => None,
        };
        let description = rule.description.map(Bytes::copy_from_slice).or(described);
        let mut bindings = self.command_bindings(
            rule.shell,
            rule.shell_flags,
            command,
            rule.recipe_environment,
        );
        // [spec:ronin:req:make.narration+1]
        // Prefer what the Makefile said. Otherwise narrate a short inline
        // recipe with its own expanded text, without exposing the environment
        // and shell wrapper needed to execute it.
        if let Some(description) = description {
            bindings.push((self.bindings.description, Template::literal(&description)));
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
        if rule.restat {
            bindings.push((self.bindings.restat, Template::literal(b"1")));
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
    fn deferred_rule_bindings(&self, rule: &SinkRule<'_>) -> Vec<(Binding, Template)> {
        let mut bindings = vec![
            (self.bindings.command, Template::literal(b"false")),
            (self.bindings.generator, Template::literal(b"1")),
        ];
        if rule.restat {
            bindings.push((self.bindings.restat, Template::literal(b"1")));
        }
        bindings
    }

    /// The per-edge bindings kati names, and the serialising pool this unit
    /// puts a command edge in when `.NOTPARALLEL` asked for one.
    fn edge_bindings(&self, edge: &SinkEdge<'_>) -> Vec<(Binding, Vec<u8>)> {
        let mut bindings = Vec::new();
        if let Some(pool) = edge.pool {
            bindings.push((self.bindings.pool, pool.to_vec()));
        }
        if let Some(tags) = edge.tags {
            bindings.push((self.bindings.tags, tags.to_vec()));
        }
        let subninja_rule = edge.rule.and_then(|id| self.subninja_rules.get(&id));
        let is_subninja = subninja_rule.is_some();
        let has_residual_action = subninja_rule.is_some_and(|rule| rule.residual_rule.is_some());
        if edge.pool.is_none()
            && edge.rule.is_some()
            && (!is_subninja || has_residual_action)
            && let Some(pool) = &self.unit.serial_pool
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
        // [spec:ronin:req:make.narration+1]
        // The recipe's own words where it chose them, and otherwise the text of
        // the lines being run, which is what the line itself says.
        let description = rule
            .description
            .map(Bytes::copy_from_slice)
            .or(match command {
                SinkCommand::Inline(script) => Some(kati::ninja::single_line(script)),
                SinkCommand::ResponseFile(_) => None,
            });
        if let Some(description) = description {
            bindings.push((self.bindings.description, Template::literal(&description)));
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
            // A segment of a recipe is a script like any other, and reaches one
            // shell like any other: the lines it holds were never the ones the
            // split would have launched separately, because the invocation
            // lifted out from between them is not among them.
            let residual_script = rule.residual_command.map(|command| {
                SettledScript::held(self.layout(), rule, command, rule.residual_ignore_errors)
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
                    SettledScript::held(
                        self.layout(),
                        rule,
                        command,
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
            self.deferred_rule_bindings(rule)
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
            let layout = self.layout();
            let launched = |environment: &[kati::export::EnvironmentChange]| {
                rule.steps
                    .iter()
                    .map(|step| crate::build::LateStep {
                        launch: layout.launch_step(step, environment),
                        ignore_errors: step.ignore_error,
                        runs_while_pretending: step.recursive_line,
                    })
                    .collect::<Vec<_>>()
            };
            // Both phases' launches, because the makefile update hands a
            // `$(MAKE)` on one of these lines a `MAKEFLAGS` the goals do not.
            // See [`SettledSteps`].
            let remaking = layout.while_remaking_makefiles(rule.recipe_environment);
            self.settled_rules.insert(
                rule.id,
                if remaking.is_empty() {
                    SettledSteps::same(launched(rule.recipe_environment))
                } else {
                    let mut environment = rule.recipe_environment.to_vec();
                    environment.extend(remaking);
                    SettledSteps::split(launched(rule.recipe_environment), launched(&environment))
                },
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
        let validations = self.node_list(names, edge.validations)?;
        let withdrawal = PendingWithdrawal {
            outputs: self.node_list(names, edge.withdrawable_outputs)?,
            on_error: edge.delete_on_error,
        };
        let peer_outputs = self.node_list(names, edge.peer_outputs)?;
        let deferred = self.deferred_edge(names, edge)?;
        let outputs = if edge.completion_join {
            self.observed_members.insert(edge.output, completion_output);
            let proxy = self.completion_proxy()?;
            self.graph.redirect_node_uses(completion_output, proxy);
            self.interned.insert(edge.output, proxy);
            vec![proxy]
        } else {
            vec![completion_output]
        };

        let bindings = self.edge_bindings(edge);
        // An archive index dates its members in whole seconds, which the
        // comparisons that put one on their target side have to read as the end
        // of that second. Whether an output is one is decided from the name the
        // Makefile wrote, here, and never from a path the build engine looks at.
        let low_resolution = std::iter::once(&edge.output)
            .chain(edge.implicit_outputs)
            .any(|output| kati::archive::split_archive_name(&output.as_bytes(&names)).is_some());
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
                validations,
                always_dirty: edge.always_dirty,
                deferred,
                completion_output: edge.completion_join.then_some(completion_output),
                intermediate: edge.intermediate,
                disposable: edge.disposable,
                low_resolution,
                withdrawal,
                peer_outputs,
                bindings,
            });
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
            validations: &validations,
            always_dirty: edge.always_dirty,
            intermediate: edge.intermediate,
            disposable: edge.disposable,
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
                self.record_late_bindings(built, edge.rule, outputs.first().copied());
                // Every Make target is one GNU Make decides from the disk, and
                // looks at again once its recipe has run whatever the recipe
                // did, so this is what Make is here rather than something a
                // Makefile asks for. `.KATI_RESTAT` is a separate and narrower
                // request that still emits its own `restat` binding.
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
