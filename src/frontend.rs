//! Front-end-agnostic graph construction and execution.
//!
//! A front end supplies rules, pools, edges, and default targets through
//! [`BuildGraph`] and receives a graph the build, tool, and persistence paths
//! accept without knowing which front end produced it. Ronin's Ninja manifest
//! parser is one such front end and is built on nothing else; [`load_manifest`]
//! is its entry point.
//!
//! A graph is then run through [`Build`], over the [`Persistence`] that makes a
//! second build incremental. Ronin's command line is built on those too, so
//! what a front end can ask for is what the Ninja front end asks for.
//!
//! The arenas behind a graph enforce their invariants through the operations
//! that mutate them: a node's generating edge and its use list, the side table
//! of validation uses, an edge's input partitions and explicit output count,
//! the pool an edge's `pool` binding names, and the rule that a `dyndep`
//! binding must name one of the edge's own inputs. Those operations are this
//! module's methods, so the invariants hold for whatever supplied the graph
//! rather than for whichever caller remembered them.

use crate::env::{
    EnvState, EnvironmentId, PoolId, RuleId, edgevar, envaddrule, envaddvar, envrule, envvar_named,
    mkenv, mkpool, mkrule, poolget, ruleaddvar,
};
use crate::graph::{
    EdgeId, Graph, NodeId, PathStyle, TraversalScratch, allocate_node, mkedge, mknode, nodeget,
    nodeuse, recompute_dirty_with_validations,
};
use crate::names::{Names, VarId};
use crate::runtime::RuntimeState;
use crate::util::{BStr, BString, ByteSlice, EvalPart, EvalString, canonpath, is_canonical};
use std::fmt;
use std::num::NonZeroUsize;

mod deferred;
mod execute;
mod ordering;

pub use crate::parse::{Manifest, ManifestOptions, load_manifest};
pub use deferred::DeferredSpec;
pub use execute::{Build, Jobs, Outcome, Persistence, Planned};

/// A path interned in a graph.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Node(NodeId);

/// A build statement in a graph.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Edge(EdgeId);

impl Edge {
    /// This edge's identity in the graph the engine runs.
    pub(crate) const fn id(self) -> EdgeId {
        self.0
    }
}

/// A named command template shared by the edges that use it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rule(RuleId);

/// A named limit on how many edges using it run at once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pool(PoolId);

/// A variable scope, which lookups search before their enclosing scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scope(EnvironmentId);

/// An interned binding name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding(VarId);

/// A rule binding's value, expanded once per edge that uses the rule.
///
/// Ninja's `command = cc $cflags -c $in -o $out` is a template: the variables
/// in it resolve against each edge rather than against the scope the rule was
/// defined in, which is what lets one rule serve every edge that names it. A
/// front end that has already expanded everything it intends to builds one
/// from [`Template::literal`] and never pushes a variable.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Template(EvalString);

impl Template {
    /// A template that expands to `value` unchanged.
    #[must_use]
    pub fn literal(value: &[u8]) -> Self {
        let mut template = Self::default();
        template.push_literal(value);
        template
    }

    /// Appends bytes that expand to themselves.
    pub fn push_literal(&mut self, value: &[u8]) {
        if value.is_empty() {
            return;
        }
        match self.0.parts.last_mut() {
            Some(EvalPart::Literal(literal)) => literal.extend_from_slice(value),
            _ => self.0.parts.push(EvalPart::Literal(BString::from(value))),
        }
    }

    /// Appends a reference to `name`, resolved against each edge in turn.
    pub fn push_variable(&mut self, name: Binding) {
        self.0.parts.push(EvalPart::Variable(name.0));
    }
}

/// The build statement a front end asks for.
///
/// Inputs and outputs arrive already separated into Ninja's partitions rather
/// than as one list with counts beside it. The counts are what the arenas
/// store, and a front end that computes them itself is a front end that can
/// compute them wrongly.
#[derive(Clone, Debug)]
pub struct EdgeSpec<'a> {
    /// The scope this edge's bindings and its rule's bindings resolve against.
    pub scope: Scope,
    /// The rule whose command builds the outputs.
    pub rule: Rule,
    /// The outputs `$out` expands to.
    pub explicit_outputs: &'a [Node],
    /// Outputs the edge also produces, which `$out` does not name.
    pub implicit_outputs: &'a [Node],
    /// The inputs `$in` expands to.
    pub explicit_inputs: &'a [Node],
    /// Inputs the outputs depend on, which `$in` does not name.
    pub implicit_inputs: &'a [Node],
    /// Inputs that must exist first but do not make the outputs out of date.
    pub order_only_inputs: &'a [Node],
    /// Targets built alongside this edge without being depended on.
    pub validations: &'a [Node],
    /// Whether the edge is out of date whenever it is reached, however its
    /// outputs compare against its inputs and against what the last build
    /// recorded.
    ///
    /// This is GNU Make's `.PHONY`: a target with no file behind it, whose
    /// recipe runs every time it is asked for. A Ninja manifest has no syntax
    /// for it and this is the only way to ask for it, so a graph parsed from a
    /// manifest never carries it.
    ///
    /// Distinct from building the edge with the `phony` rule, which makes an
    /// output an alias for its inputs and runs nothing. The two combine: an
    /// alias can also be one that is never up to date.
    // [spec:ronin:req:make.phony-always-dirty]
    pub always_dirty: bool,
    /// Whether the outputs' absence is no reason to rebuild what reads them,
    /// which is GNU Make's intermediate file: one its implicit rule search
    /// invented to complete a chain, or one `.INTERMEDIATE` or `.SECONDARY`
    /// named. A graph parsed from a manifest never carries it: Ninja has no way
    /// to say it.
    pub intermediate: bool,
    /// Whether the front end should throw the outputs away once the build has
    /// finished with them.
    pub disposable: bool,
    /// Bindings local to this edge, already expanded.
    pub bindings: Vec<(Binding, Vec<u8>)>,
}

/// A graph a front end asked for that cannot exist.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FrontendError {
    /// A path had no bytes to intern.
    EmptyPath,
    /// A build statement named no outputs at all.
    EdgeWithoutOutputs,
    /// Two build statements generate the same output.
    DuplicateOutput {
        /// The output that is already generated elsewhere.
        path: Vec<u8>,
    },
    /// One build statement names the same output twice.
    RepeatedOutput {
        /// The output the statement names more than once.
        path: Vec<u8>,
    },
    /// A scope already defines a rule of that name.
    DuplicateRule {
        /// The rule name asked for twice.
        name: Vec<u8>,
    },
    /// A pool of that name is already defined.
    DuplicatePool {
        /// The pool name asked for twice.
        name: Vec<u8>,
    },
    /// An edge's `pool` binding names a pool that was never defined.
    UnknownPool {
        /// The pool name the edge asked for.
        name: Vec<u8>,
    },
    /// An edge's `dyndep` binding names a path that is not one of its inputs.
    DyndepNotInput {
        /// The dyndep path the edge asked for.
        path: Vec<u8>,
    },
    /// A recipe mixes a recursive `$(MAKE)` line with shell work that cannot
    /// be represented as one static subninja inclusion.
    UncomposableSubninja {
        /// The expanded recipe, retained for an actionable compiler error.
        command: Vec<u8>,
    },
}

impl fmt::Display for FrontendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("empty path"),
            Self::EdgeWithoutOutputs => formatter.write_str("expected path"),
            Self::DuplicateOutput { path } => {
                write!(formatter, "multiple rules generate {}", path.as_bstr())
            }
            Self::RepeatedOutput { path } => write!(
                formatter,
                "{} is defined as an output multiple times",
                path.as_bstr()
            ),
            Self::DuplicateRule { name } => {
                write!(formatter, "duplicate rule '{}'", name.as_bstr())
            }
            Self::DuplicatePool { name } => {
                write!(formatter, "duplicate pool '{}'", name.as_bstr())
            }
            Self::UnknownPool { name } => {
                write!(formatter, "unknown pool name '{}'", name.as_bstr())
            }
            Self::DyndepNotInput { path } => {
                write!(formatter, "dyndep '{}' is not an input", path.as_bstr())
            }
            Self::UncomposableSubninja { command } => write!(
                formatter,
                "recursive Make recipe cannot compile as subninja: {}",
                command.as_bstr()
            ),
        }
    }
}

impl std::error::Error for FrontendError {}

/// A dependency graph, and the operations that construct one.
///
/// A new graph already holds the two definitions the engine identifies by
/// identity rather than by name: the built-in `phony` rule, whose edges run no
/// command, and the `console` pool, whose edges take the terminal.
// [spec:ronin:req:frontend.graph-construction]
pub struct BuildGraph {
    arenas: Graph,
    state: EnvState,
    defaults: Vec<NodeId>,
    /// One buffer for canonicalizing the paths that are not canonical already.
    canonical: Vec<u8>,
}

impl Default for BuildGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildGraph {
    /// An empty graph holding only the built-in `phony` rule and `console` pool.
    #[must_use]
    pub fn new() -> Self {
        let mut arenas = Graph::default();
        let state = EnvState::new(&mut arenas);
        Self {
            arenas,
            state,
            defaults: Vec::new(),
            canonical: Vec::new(),
        }
    }

    /// The scope every other scope descends from.
    #[must_use]
    pub const fn root(&self) -> Scope {
        Scope(self.state.root)
    }

    /// Creates a scope whose failed lookups continue into `parent`.
    pub fn child_scope(&mut self, parent: Scope) -> Scope {
        Scope(mkenv(&mut self.arenas, Some(parent.0)))
    }

    /// Interns a binding name.
    pub fn binding(&mut self, name: &[u8]) -> Binding {
        Binding(self.arenas.names_mut().intern(BStr::new(name)))
    }

    /// Binds `name` to `value` in `scope`, replacing any binding it had there.
    pub fn bind(&mut self, scope: Scope, name: &[u8], value: Vec<u8>) {
        let name = self.arenas.names_mut().intern(BStr::new(name));
        envaddvar(&mut self.arenas, scope.0, name, BString::from(value));
    }

    /// The value `name` has in `scope` or the nearest enclosing scope that
    /// binds it.
    #[must_use]
    pub fn variable(&self, scope: Scope, name: &[u8]) -> Option<&[u8]> {
        envvar_named(&self.arenas, scope.0, BStr::new(name)).map(|value| &value[..])
    }

    /// Interns `path`, canonicalizing it first.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::EmptyPath`] when there is nothing to intern,
    /// which is what a front end's own expansion produces from a name nothing
    /// bound.
    pub fn node(&mut self, path: &[u8]) -> Result<Node, FrontendError> {
        if path.is_empty() {
            return Err(FrontendError::EmptyPath);
        }
        if is_canonical(path) {
            return Ok(Node(mknode(&mut self.arenas, path)));
        }
        self.canonical.clear();
        self.canonical.extend_from_slice(path);
        canonpath(&mut self.canonical);
        Ok(Node(mknode(&mut self.arenas, &self.canonical)))
    }

    /// Allocates a node whose identity is private while its filesystem path is
    /// still `path`.
    ///
    /// A recursive front end uses this when two independently evaluated source
    /// units may both define a target with the same spelling. Repeated names
    /// within one unit remain canonical through that front end's own map.
    pub(crate) fn isolated_node(&mut self, path: &[u8]) -> Result<Node, FrontendError> {
        if path.is_empty() {
            return Err(FrontendError::EmptyPath);
        }
        if is_canonical(path) {
            return Ok(Node(allocate_node(&mut self.arenas, path)));
        }
        self.canonical.clear();
        self.canonical.extend_from_slice(path);
        canonpath(&mut self.canonical);
        Ok(Node(allocate_node(&mut self.arenas, &self.canonical)))
    }

    /// Finds an already-interned path, canonicalizing it first.
    #[must_use]
    pub fn lookup(&self, path: &[u8]) -> Option<Node> {
        if is_canonical(path) {
            return nodeget(&self.arenas, path).map(Node);
        }
        let mut canonical = path.to_vec();
        canonpath(&mut canonical);
        nodeget(&self.arenas, &canonical).map(Node)
    }

    /// The edge that generates `node`, absent for a file nothing builds.
    #[must_use]
    pub fn generator(&self, node: Node) -> Option<Edge> {
        self.arenas.node(node.0).generator.map(Edge)
    }

    /// The interned path of `node`.
    #[must_use]
    pub fn path(&self, node: Node) -> &[u8] {
        self.arenas.node_path(node.0).as_bytes()
    }

    /// Defines `name` in `scope` with `bindings`.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::DuplicateRule`] when `scope` itself already
    /// defines `name`. An enclosing scope defining it is not a conflict: the
    /// inner definition shadows the outer one for the edges that resolve
    /// through it.
    pub fn define_rule(
        &mut self,
        scope: Scope,
        name: &[u8],
        bindings: Vec<(Binding, Template)>,
    ) -> Result<Rule, FrontendError> {
        let rule = mkrule(&mut self.arenas, BString::from(name));
        for (binding, template) in bindings {
            ruleaddvar(&mut self.arenas, rule, binding.0, template.0);
        }
        envaddrule(&mut self.arenas, scope.0, rule).map_err(|_| FrontendError::DuplicateRule {
            name: name.to_vec(),
        })?;
        Ok(Rule(rule))
    }

    /// The rule `name` resolves to from `scope`, searching enclosing scopes.
    #[must_use]
    pub fn rule(&self, scope: Scope, name: &[u8]) -> Option<Rule> {
        envrule(&self.arenas, scope.0, BStr::new(name)).map(Rule)
    }

    /// Defines a pool, which starts with no limit until one is set.
    ///
    /// Pool names are global rather than scoped, as they are in Ninja.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::DuplicatePool`] when `name` is already defined.
    pub fn define_pool(&mut self, name: &[u8]) -> Result<Pool, FrontendError> {
        mkpool(&mut self.arenas, &mut self.state, BString::from(name))
            .map(Pool)
            .map_err(|_| FrontendError::DuplicatePool {
                name: name.to_vec(),
            })
    }

    /// Limits `pool` to `depth` edges running at once.
    pub fn set_pool_depth(&mut self, pool: Pool, depth: NonZeroUsize) {
        self.arenas.pool_mut(pool.0).set_depth(depth);
    }

    /// The limit `pool` carries, absent while it has none.
    #[must_use]
    pub fn pool_depth(&self, pool: Pool) -> Option<NonZeroUsize> {
        self.arenas.pool(pool.0).depth()
    }

    /// Adds one build statement.
    ///
    /// Every output is recorded as generated by the new edge, every input and
    /// validation records the edge among its uses, and the partitions the
    /// spec's separate lists describe are stored as the counts the engine
    /// reads. The edge's `pool` and `dyndep` bindings are resolved here too,
    /// because both are bindings whose value is only knowable once the edge's
    /// own bindings and its rule's are in place.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::EdgeWithoutOutputs`] for a statement that
    /// generates nothing, [`FrontendError::DuplicateOutput`] for an output
    /// another edge already generates, [`FrontendError::RepeatedOutput`] for
    /// one this statement names twice, [`FrontendError::UnknownPool`] for a
    /// `pool` binding naming a pool that was never defined, and
    /// [`FrontendError::DyndepNotInput`] for a `dyndep` binding naming a path
    /// the edge does not depend on.
    // [spec:ronin:req:frontend.graph-construction]
    pub fn add_edge(&mut self, spec: EdgeSpec<'_>) -> Result<Edge, FrontendError> {
        if spec.explicit_outputs.is_empty() && spec.implicit_outputs.is_empty() {
            return Err(FrontendError::EdgeWithoutOutputs);
        }
        let edge = self.begin_edge(spec.scope, spec.rule, spec.bindings);
        {
            let stored = self.arenas.edge_mut(edge.0);
            stored.always_dirty = spec.always_dirty;
            stored.intermediate = spec.intermediate;
            stored.disposable = spec.disposable;
        }
        self.resolve_pool(edge)?;
        for output in spec.explicit_outputs.iter().chain(spec.implicit_outputs) {
            self.attach_output(edge, *output)?;
        }
        self.set_explicit_outputs(edge, spec.explicit_outputs.len());
        for input in spec
            .explicit_inputs
            .iter()
            .chain(spec.implicit_inputs)
            .chain(spec.order_only_inputs)
        {
            self.attach_input(edge, *input);
        }
        self.set_input_partitions(
            edge,
            spec.explicit_inputs.len(),
            spec.explicit_inputs.len() + spec.implicit_inputs.len(),
        );
        for validation in spec.validations {
            self.attach_validation(edge, *validation);
        }
        self.resolve_dyndep(edge)?;
        Ok(edge)
    }

    /// Creates an edge carrying its own bindings and nothing else.
    ///
    /// Ninja builds a build statement in this order rather than all at once,
    /// and the order is observable: the statement's `pool` is resolved through
    /// the edge before any path is interned, so a statement that names an
    /// unknown pool *and* an output another statement already generates is
    /// reported as the pool. A front end holding a whole statement calls
    /// [`Self::add_edge`], which is these operations in that order; the
    /// manifest parser calls them itself because it also has to say where each
    /// failure happened.
    pub(crate) fn begin_edge(
        &mut self,
        scope: Scope,
        rule: Rule,
        bindings: Vec<(Binding, Vec<u8>)>,
    ) -> Edge {
        let edge = mkedge(&mut self.arenas, scope.0);
        let stored = self.arenas.edge_mut(edge);
        stored.rule = Some(rule.0);
        for (name, value) in bindings {
            stored.bindings.insert(name.0, BString::from(value));
        }
        Edge(edge)
    }

    /// Decide `edge`'s currency the way GNU Make decides a target's: from what
    /// is on the disk, before its recipe runs and again after.
    ///
    /// Before, because GNU Make does not make its recipe history part of
    /// target freshness. A direct Make graph still records Ninja-compatible
    /// history for timings and tools, but an older record cannot override
    /// equal on-disk mtimes.
    ///
    /// After, because GNU Make stats a target it has just remade and lets the
    /// timestamp it then finds decide what the targets reading it do. A recipe
    /// that ran without moving its target — autoconf's universal
    /// `config.h: stamp-h1` is one — leaves them up to date; without that, the
    /// first no-op recipe in a tree cascades a full rebuild through everything
    /// behind it, on every invocation. An edge that runs no command never
    /// reaches the question.
    ///
    /// Nothing in a Ninja manifest says either half, so a graph parsed from one
    /// carries neither — the same bounded divergence `intermediate` and
    /// `disposable` already have. `restat` is the near neighbour of the second
    /// half rather than the same thing: it asks for the second look on one rule
    /// and grants the outcome only to an output whose timestamp did not move at
    /// all.
    // [spec:ronin:req:make.state-outside-the-tree+2]
    // [spec:ronin:req:make.remade-target-re-observed]
    pub(crate) fn set_make_target_freshness(&mut self, edge: Edge) {
        let stored = self.arenas.edge_mut(edge.0);
        stored.freshness_history = crate::graph::FreshnessHistory::FilesystemOnly;
        stored.outputs_reobserved = true;
    }

    /// Name the outputs of `edge` a stopped command may be made to give back,
    /// and say whether an ordinary failure is reason enough to take them.
    ///
    /// The eligible names rather than a switch: `.PRECIOUS` and `.PHONY` take
    /// individual outputs off the list, so a grouped recipe may have to leave
    /// one member and withdraw the rest. They are named whatever `on_error`
    /// says, because a recipe killed by a signal is cleaned up after without
    /// `.DELETE_ON_ERROR` having asked.
    ///
    /// Nothing in a Ninja manifest says this, so a graph parsed from one never
    /// calls this at all — the same bounded divergence `intermediate` and
    /// `disposable` already have — and an edge nobody answered for keeps
    /// Ninja's answer, which is that a cut-short command gives everything back.
    pub(crate) fn set_withdrawal(&mut self, edge: Edge, outputs: Vec<Node>, on_error: bool) {
        self.arenas.set_withdrawal(
            edge.0,
            outputs.into_iter().map(|node| node.0).collect(),
            on_error,
        );
    }

    /// Declare which of `edge`'s outputs the recipe makes only on the way to
    /// making one that was asked for — GNU Make's `also_make`.
    ///
    /// They stay outputs: the edge is what writes them, and a failed recipe
    /// withdraws them like any other. What they are not is part of the question
    /// the edge answers before it runs, nor part of what the build sweeps up
    /// afterwards. An empty list stores nothing.
    ///
    /// Nothing in a Ninja manifest says this, so a graph parsed from one never
    /// carries it — the same bounded divergence `intermediate` and `disposable`
    /// already have.
    pub(crate) fn set_peer_outputs(&mut self, edge: Edge, outputs: Vec<Node>) {
        self.arenas
            .set_peer_outputs(edge.0, outputs.into_iter().map(|node| node.0).collect());
    }

    /// The value `edge`'s own bindings give `name`, before its rule or the
    /// scope around it are consulted.
    ///
    /// Ninja evaluates a build statement's paths against a scope holding that
    /// statement's bindings, so `build $stem.o: cc` sees a `stem` the statement
    /// itself declares. The values were expanded against the enclosing scope
    /// before they were stored, so they cannot see each other.
    pub(crate) fn edge_binding(&self, edge: Edge, name: &[u8]) -> Option<&[u8]> {
        let name = self.arenas.names().lookup(BStr::new(name))?;
        self.arenas
            .edge(edge.0)
            .bindings
            .get(name)
            .map(|value| &value[..])
    }

    /// Records `output` as generated by `edge`.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::DuplicateOutput`] when another edge already
    /// generates it and [`FrontendError::RepeatedOutput`] when this one does.
    pub(crate) fn attach_output(&mut self, edge: Edge, output: Node) -> Result<(), FrontendError> {
        if let Some(other) = self.arenas.node(output.0).generator {
            let path = self.arenas.node_path(output.0).to_vec();
            return Err(if other == edge.0 {
                FrontendError::RepeatedOutput { path }
            } else {
                FrontendError::DuplicateOutput { path }
            });
        }
        self.arenas.node_mut(output.0).generator = Some(edge.0);
        self.arenas.edge_mut(edge.0).out.push(output.0);
        Ok(())
    }

    /// Records how many of the outputs attached so far `$out` names.
    pub(crate) fn set_explicit_outputs(&mut self, edge: Edge, count: usize) {
        self.arenas
            .edge_mut(edge.0)
            .set_explicit_output_count(count);
    }

    /// Records `input` as one `edge` depends on.
    pub(crate) fn attach_input(&mut self, edge: Edge, input: Node) {
        nodeuse(&mut self.arenas, input.0, edge.0);
        self.arenas.edge_mut(edge.0).input.push(input.0);
    }

    /// Records where the inputs attached so far stop being explicit and stop
    /// making the outputs out of date.
    pub(crate) fn set_input_partitions(
        &mut self,
        edge: Edge,
        explicit: usize,
        non_order_only: usize,
    ) {
        self.arenas
            .edge_mut(edge.0)
            .set_input_partitions(explicit, non_order_only);
    }

    /// Records `validation` as a target built alongside `edge`.
    pub(crate) fn attach_validation(&mut self, edge: Edge, validation: Node) {
        self.arenas.add_validation_use(validation.0, edge.0);
        self.arenas.edge_mut(edge.0).validation.push(validation.0);
    }

    /// Promote additional inputs into the ordinary explicit partition.
    pub(crate) fn add_explicit_inputs(&mut self, edge: Edge, inputs: &[Node]) {
        for input in inputs {
            let partitions = {
                let stored = self.arenas.edge(edge.0);
                (
                    stored.explicit_input_count(),
                    stored.non_order_only_input_count(),
                )
            };
            if self
                .arenas
                .edge(edge.0)
                .explicit_inputs()
                .contains(&input.0)
            {
                continue;
            }
            let existing = self
                .arenas
                .edge(edge.0)
                .input
                .iter()
                .position(|node| *node == input.0);
            if let Some(index) = existing {
                self.arenas.edge_mut(edge.0).remove_input(index);
            } else {
                nodeuse(&mut self.arenas, input.0, edge.0);
            }
            let explicit = partitions.0.min(self.arenas.edge(edge.0).input.len());
            self.arenas.edge_mut(edge.0).input.insert(explicit, input.0);
            let non_order_only = partitions.1.saturating_sub(usize::from(
                existing.is_some_and(|index| index < partitions.1),
            ));
            self.arenas
                .edge_mut(edge.0)
                .set_input_partitions(explicit + 1, non_order_only + 1);
        }
    }

    /// Replace the command rule of an edge whose structure was staged first.
    pub(crate) fn set_edge_rule(&mut self, edge: Edge, rule: Rule) {
        self.arenas.edge_mut(edge.0).rule = Some(rule.0);
    }

    /// Evaluate one staged edge's timestamp freshness without executing it.
    pub(crate) fn edge_dirty_with<F>(
        &self,
        edge: Edge,
        stat: &mut F,
    ) -> Result<bool, crate::error::GraphError>
    where
        F: FnMut(&std::path::Path) -> std::io::Result<i64>,
    {
        let target = self.arenas.edge(edge.0).out[0];
        let mut runtime = RuntimeState::new(&self.arenas);
        recompute_dirty_with_validations(
            &self.arenas,
            &mut runtime,
            &mut TraversalScratch::default(),
            target,
            stat,
        )?;
        Ok(runtime.node(target).dirty())
    }

    /// Keep work completed through a provisional compiler graph completed in
    /// the final graph for the same invocation.
    ///
    /// Make evaluates a recursive child only after the parent target's
    /// prerequisites have run. The Make frontend therefore builds that input
    /// closure through a provisional graph and evaluates again. Replacing the
    /// closure's commands with the built-in phony rule preserves its graph
    /// ordering and dirty propagation without running any recipe a second
    /// time. Real outputs retain the files and timestamps the provisional
    /// build produced; phony outputs settle immediately when the final graph
    /// reaches them.
    pub(crate) fn mark_subgraphs_prebuilt(&mut self, roots: &[Node], phony: Rule) {
        let mut seen = std::collections::HashSet::new();
        let mut work = roots.iter().map(|node| node.0).collect::<Vec<_>>();
        while let Some(node) = work.pop() {
            let Some(edge) = self.arenas.node(node).generator else {
                continue;
            };
            if !seen.insert(edge) {
                continue;
            }
            let (inputs, validations, activations) = {
                let stored = self.arenas.edge(edge);
                let activations = self
                    .arenas
                    .deferred_freshness(edge)
                    .map(|freshness| freshness.activations.to_vec())
                    .unwrap_or_default();
                (
                    stored.input.to_vec(),
                    stored.validation.to_vec(),
                    activations,
                )
            };
            self.arenas.edge_mut(edge).rule = Some(phony.0);
            work.extend(inputs);
            work.extend(validations);
            work.extend(activations);
        }
    }

    /// Keep the Makefiles this invocation already dealt with out of the goals'
    /// way, in the two ways it dealt with them.
    ///
    /// GNU Make updates every Makefile it read before it chooses a goal, and a
    /// file it has updated is not considered again: `update_file_1` reads
    /// `updated` back before it looks at anything else. What happens then
    /// depends on the verdict that update left, which is why this takes two
    /// lists and not one.
    ///
    /// `remade` is the ones it reached and won. What the goals then compare
    /// against is the Makefile's timestamp rather than the rule behind it, so a
    /// `gen.mk: force` whose recipe left the file alone leaves whatever reads
    /// gen.mk up to date — though `force` is out of date whenever it is looked
    /// at. Two halves to that, because those are two different things to stop.
    /// Nothing the update reached runs a command again, which is
    /// [`Self::mark_subgraphs_prebuilt`] and the built-in phony rule. The
    /// Makefiles themselves additionally stop carrying their prerequisites'
    /// dirtiness onward: the edge keeps its outputs and loses its inputs, which
    /// is what makes the file on disk the answer. A phony prerequisite reached
    /// any other way is left alone and still drives whatever else asked for it,
    /// which is GNU Make's answer too.
    ///
    /// `unmade` is the ones whose rule really ran, really lost, and was
    /// forgiven because `-include` said the file need not be there. GNU Make
    /// leaves those `updated` with a failing `update_status`, which the same
    /// early read finds, so a goal that reaches the name is refused rather than
    /// served and the recipe is not run again.
    ///
    /// Said only once the update has settled. A pass that ends in a restart
    /// says nothing at all, because the read that follows plans a new graph and
    /// attempts the rule again, which is GNU Make's behaviour too.
    pub(crate) fn mark_makefiles_settled(&mut self, remade: &[Node], unmade: &[Node]) {
        for node in unmade {
            self.arenas.mark_makefile_unmade(node.0);
        }
        let Some(phony) = self.rule(self.root(), b"phony") else {
            return;
        };
        self.mark_subgraphs_prebuilt(remade, phony);
        for node in remade {
            let Some(edge) = self.arenas.node(node.0).generator else {
                continue;
            };
            let inputs = self.arenas.edge(edge).input.to_vec();
            for index in (0..inputs.len()).rev() {
                self.arenas.edge_mut(edge).remove_input(index);
            }
            for input in inputs {
                self.arenas
                    .node_mut(input)
                    .uses
                    .retain(|candidate| *candidate != edge);
            }
            self.arenas.edge_mut(edge).always_dirty = false;
        }
    }

    /// Resolves `edge`'s `pool` binding against the declared pools.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::UnknownPool`] when it names one nobody declared.
    pub(crate) fn resolve_pool(&mut self, edge: Edge) -> Result<(), FrontendError> {
        let Some(name) = edgevar(&self.arenas, edge.0, Names::POOL, PathStyle::ShellEscaped)
            .filter(|name| !name.is_empty())
        else {
            return Ok(());
        };
        let Ok(pool) = poolget(&self.state, BStr::new(name.as_slice())) else {
            return Err(FrontendError::UnknownPool { name: name.into() });
        };
        self.arenas.edge_mut(edge.0).pool = Some(pool);
        Ok(())
    }

    /// Resolves `edge`'s `dyndep` binding against its own inputs.
    ///
    /// # Errors
    ///
    /// Returns [`FrontendError::DyndepNotInput`] when it names a path the edge
    /// does not depend on.
    pub(crate) fn resolve_dyndep(&mut self, edge: Edge) -> Result<(), FrontendError> {
        let Some(mut path) = edgevar(&self.arenas, edge.0, Names::DYNDEP, PathStyle::Raw)
            .filter(|path| !path.is_empty())
        else {
            return Ok(());
        };
        canonpath(&mut path);
        let dyndep = mknode(&mut self.arenas, path.as_slice());
        if !self.arenas.edge(edge.0).input.contains(&dyndep) {
            return Err(FrontendError::DyndepNotInput { path: path.into() });
        }
        self.arenas.edge_mut(edge.0).dyndep = Some(dyndep);
        Ok(())
    }

    /// Drops a phony edge's dependency on its own output, if it has one.
    ///
    /// `CMake` 2.8.12 and 3.0 emitted `build a: phony … a …`, and the shape
    /// recognised here is exactly that one: a phony edge with a single output,
    /// all of it explicit, and no implicit or order-only inputs. Anything
    /// longer is an ordinary cycle and stays one. Returns the output whose
    /// self-reference was dropped.
    pub fn drop_phony_self_reference(&mut self, edge: Edge) -> Option<Node> {
        let stored = self.arenas.edge(edge.0);
        let cmake_shaped = self.arenas.is_phony_rule(stored.rule)
            && stored.out.len() == 1
            && stored.explicit_output_count() == 1
            && stored.explicit_input_count() == stored.input.len();
        if !cmake_shaped {
            return None;
        }
        let output = stored.out[0];
        let referenced = self
            .arenas
            .edge(edge.0)
            .input
            .iter()
            .enumerate()
            .filter_map(|(index, input)| (*input == output).then_some(index))
            .collect::<Vec<_>>();
        if referenced.is_empty() {
            return None;
        }
        for index in referenced.into_iter().rev() {
            self.arenas.edge_mut(edge.0).remove_input(index);
        }
        self.arenas
            .node_mut(output)
            .uses
            .retain(|candidate| *candidate != edge.0);
        Some(Node(output))
    }

    /// Records `node` as a target to build when none are named.
    pub fn add_default(&mut self, node: Node) {
        self.defaults.push(node.0);
    }

    // [spec:ronin:def:parse.defaultnodes-fn]
    // [spec:ronin:sem:parse.defaultnodes-fn]
    /// The targets an invocation that names none builds.
    ///
    /// The declared defaults, or every generated output that nothing else
    /// consumes when the front end declared none.
    #[must_use]
    pub fn default_targets(&self) -> Vec<Node> {
        if !self.defaults.is_empty() {
            return self.defaults.iter().copied().map(Node).collect();
        }
        self.arenas
            .node_ids()
            .filter(|node| {
                let node = self.arenas.node(*node);
                node.generator.is_some() && node.uses.is_empty()
            })
            .map(Node)
            .collect()
    }

    /// The arenas, for the engine paths that read a graph a front end built.
    pub(crate) const fn arenas(&self) -> &Graph {
        &self.arenas
    }

    /// The arenas, for the engine paths that extend one during a build: the
    /// dependency log's recorded inputs, and dyndep's discovered outputs.
    pub(crate) const fn arenas_mut(&mut self) -> &mut Graph {
        &mut self.arenas
    }

    /// Gives up the graph, for the tests that drive the arenas directly.
    ///
    /// No build path needs this: a build runs over the graph a front end holds
    /// rather than taking it away.
    #[cfg(test)]
    pub(crate) fn into_arenas(self) -> Graph {
        self.arenas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(graph: &mut BuildGraph, paths: &[&str]) -> Vec<Node> {
        paths
            .iter()
            .map(|path| graph.node(path.as_bytes()).unwrap())
            .collect()
    }

    fn cat_rule(graph: &mut BuildGraph) -> Rule {
        let command = graph.binding(b"command");
        let mut template = Template::literal(b"cat ");
        let input = graph.binding(b"in");
        template.push_variable(input);
        template.push_literal(b" > ");
        let output = graph.binding(b"out");
        template.push_variable(output);
        let root = graph.root();
        graph
            .define_rule(root, b"cat", vec![(command, template)])
            .unwrap()
    }

    fn spec<'a>(scope: Scope, rule: Rule, outputs: &'a [Node], inputs: &'a [Node]) -> EdgeSpec<'a> {
        EdgeSpec {
            scope,
            rule,
            explicit_outputs: outputs,
            implicit_outputs: &[],
            explicit_inputs: inputs,
            implicit_inputs: &[],
            order_only_inputs: &[],
            validations: &[],
            always_dirty: false,
            intermediate: false,
            disposable: false,
            bindings: Vec::new(),
        }
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn a_graph_built_without_a_manifest_carries_every_partition() {
        let mut graph = BuildGraph::new();
        let rule = cat_rule(&mut graph);
        let root = graph.root();
        let built = nodes(&mut graph, &["out", "out.imp"]);
        let used = nodes(&mut graph, &["in", "implicit", "order", "check"]);
        let description = graph.binding(b"description");
        let edge = graph
            .add_edge(EdgeSpec {
                scope: root,
                rule,
                explicit_outputs: &built[..1],
                implicit_outputs: &built[1..],
                explicit_inputs: &used[..1],
                implicit_inputs: &used[1..2],
                order_only_inputs: &used[2..3],
                validations: &used[3..],
                always_dirty: true,
                intermediate: false,
                disposable: false,
                bindings: vec![(description, b"copying".to_vec())],
            })
            .unwrap();

        let arenas = graph.arenas();
        let stored = arenas.edge(edge.0);
        assert_eq!(stored.explicit_output_count(), 1);
        assert_eq!(stored.explicit_input_count(), 1);
        assert_eq!(stored.non_order_only_input_count(), 2);
        assert_eq!(stored.input.len(), 3);
        assert_eq!(stored.validation.len(), 1);
        assert!(stored.always_dirty);
        // Every output names the edge, every input and validation is recorded
        // as using it, and `$in` and `$out` see only the explicit partitions.
        for output in &built {
            assert_eq!(arenas.node(output.0).generator, Some(edge.0));
        }
        for input in &used[..3] {
            assert_eq!(arenas.node(input.0).uses.as_slice(), [edge.0]);
        }
        assert!(arenas.node(used[3].0).uses.is_empty());
        assert_eq!(arenas.node_validation_uses(used[3].0), [edge.0]);
        let command = crate::env::edgevar(arenas, edge.0, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(command.as_bytes(), b"cat in > out");
        let description =
            crate::env::edgevar(arenas, edge.0, Names::DESCRIPTION, PathStyle::Raw).unwrap();
        assert_eq!(description.as_bytes(), b"copying");
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn construction_refuses_the_graphs_the_arenas_could_not_represent() {
        let mut graph = BuildGraph::new();
        let rule = cat_rule(&mut graph);
        let root = graph.root();
        let out = nodes(&mut graph, &["out"]);
        let input = nodes(&mut graph, &["in"]);
        graph.add_edge(spec(root, rule, &out, &input)).unwrap();

        assert_eq!(graph.node(b""), Err(FrontendError::EmptyPath));
        assert_eq!(
            graph.add_edge(spec(root, rule, &[], &input)),
            Err(FrontendError::EdgeWithoutOutputs)
        );
        assert_eq!(
            graph.add_edge(spec(root, rule, &out, &input)),
            Err(FrontendError::DuplicateOutput {
                path: b"out".to_vec()
            })
        );
        let twice = nodes(&mut graph, &["twice", "twice"]);
        assert_eq!(
            graph.add_edge(spec(root, rule, &twice, &input)),
            Err(FrontendError::RepeatedOutput {
                path: b"twice".to_vec()
            })
        );
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn isolated_nodes_keep_distinct_identities() {
        let mut graph = BuildGraph::new();
        let rule = cat_rule(&mut graph);
        let root = graph.root();
        let input = nodes(&mut graph, &["in"]);
        let first = graph.isolated_node(b"same").unwrap();
        let second = graph.isolated_node(b"./same").unwrap();

        assert_ne!(first, second);
        assert_eq!(graph.path(first), b"same");
        assert_eq!(graph.path(second), b"same");
        assert_eq!(graph.lookup(b"same"), None);
        graph.add_edge(spec(root, rule, &[first], &input)).unwrap();
        graph.add_edge(spec(root, rule, &[second], &input)).unwrap();

        let indexed = graph.node(b"same").unwrap();
        assert_ne!(indexed, first);
        assert_ne!(indexed, second);
        assert_eq!(graph.lookup(b"same"), Some(indexed));
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn pool_and_dyndep_bindings_are_resolved_where_the_edge_is_made() {
        let mut graph = BuildGraph::new();
        let rule = cat_rule(&mut graph);
        let root = graph.root();
        let pool = graph.define_pool(b"link").unwrap();
        assert_eq!(graph.pool_depth(pool), None);
        graph.set_pool_depth(pool, NonZeroUsize::new(3).unwrap());
        assert_eq!(graph.pool_depth(pool).unwrap().get(), 3);
        assert_eq!(
            graph.define_pool(b"link"),
            Err(FrontendError::DuplicatePool {
                name: b"link".to_vec()
            })
        );

        let out = nodes(&mut graph, &["out"]);
        let input = nodes(&mut graph, &["in"]);
        let pool_binding = graph.binding(b"pool");
        let dyndep_binding = graph.binding(b"dyndep");
        let mut pooled = spec(root, rule, &out, &input);
        pooled.bindings = vec![
            (pool_binding, b"link".to_vec()),
            (dyndep_binding, b"in".to_vec()),
        ];
        let edge = graph.add_edge(pooled).unwrap();
        let stored = graph.arenas().edge(edge.0);
        assert_eq!(stored.pool, Some(pool.0));
        assert_eq!(stored.dyndep, Some(input[0].0));

        let elsewhere = nodes(&mut graph, &["other"]);
        let mut unknown = spec(root, rule, &elsewhere, &input);
        unknown.bindings = vec![(pool_binding, b"absent".to_vec())];
        assert_eq!(
            graph.add_edge(unknown),
            Err(FrontendError::UnknownPool {
                name: b"absent".to_vec()
            })
        );
        let stray = nodes(&mut graph, &["stray"]);
        let mut detached = spec(root, rule, &stray, &input);
        detached.bindings = vec![(dyndep_binding, b"elsewhere".to_vec())];
        assert_eq!(
            graph.add_edge(detached),
            Err(FrontendError::DyndepNotInput {
                path: b"elsewhere".to_vec()
            })
        );
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn scopes_shadow_rules_and_variables_without_disturbing_their_parent() {
        let mut graph = BuildGraph::new();
        let outer = cat_rule(&mut graph);
        let root = graph.root();
        graph.bind(root, b"where", b"outer".to_vec());
        let child = graph.child_scope(root);
        graph.bind(child, b"where", b"inner".to_vec());
        let command = graph.binding(b"command");
        let inner = graph
            .define_rule(child, b"cat", vec![(command, Template::literal(b"inner"))])
            .unwrap();

        assert_eq!(graph.variable(root, b"where"), Some(&b"outer"[..]));
        assert_eq!(graph.variable(child, b"where"), Some(&b"inner"[..]));
        assert_eq!(graph.variable(child, b"unbound"), None);
        assert_eq!(graph.rule(root, b"cat"), Some(outer));
        assert_eq!(graph.rule(child, b"cat"), Some(inner));
        assert_eq!(
            graph.define_rule(child, b"cat", Vec::new()),
            Err(FrontendError::DuplicateRule {
                name: b"cat".to_vec()
            })
        );
        // The built-in rule and pool are defined in the root scope already.
        assert!(graph.rule(root, b"phony").is_some());
        assert_eq!(
            graph.define_pool(b"console"),
            Err(FrontendError::DuplicatePool {
                name: b"console".to_vec()
            })
        );
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn default_targets_fall_back_to_the_outputs_nothing_consumes() {
        let mut graph = BuildGraph::new();
        let rule = cat_rule(&mut graph);
        let root = graph.root();
        let middle = nodes(&mut graph, &["mid"]);
        let source = nodes(&mut graph, &["src"]);
        let final_output = nodes(&mut graph, &["out"]);
        graph.add_edge(spec(root, rule, &middle, &source)).unwrap();
        graph
            .add_edge(spec(root, rule, &final_output, &middle))
            .unwrap();
        assert_eq!(graph.default_targets(), final_output);

        graph.add_default(middle[0]);
        assert_eq!(graph.default_targets(), middle);
        assert_eq!(graph.path(middle[0]), b"mid");
        assert_eq!(graph.lookup(b"./mid"), Some(middle[0]));
        assert_eq!(graph.lookup(b"absent"), None);
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn dropping_a_phony_self_reference_also_drops_the_use_it_recorded() {
        let mut graph = BuildGraph::new();
        let root = graph.root();
        let phony = graph.rule(root, b"phony").unwrap();
        let out = nodes(&mut graph, &["a"]);
        let inputs = nodes(&mut graph, &["a", "b"]);
        let edge = graph.add_edge(spec(root, phony, &out, &inputs)).unwrap();
        assert_eq!(graph.arenas().node(out[0].0).uses.as_slice(), [edge.0]);

        assert_eq!(graph.drop_phony_self_reference(edge), Some(out[0]));
        assert!(graph.arenas().node(out[0].0).uses.is_empty());
        let stored = graph.arenas().edge(edge.0);
        assert_eq!(stored.input.as_slice(), [inputs[1].0]);
        assert_eq!(stored.explicit_input_count(), 1);
        assert_eq!(graph.drop_phony_self_reference(edge), None);
    }
}
