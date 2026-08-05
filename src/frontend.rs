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
    edgevar, envaddrule, envaddvar, envrule, envvar_named, mkenv, mkpool, mkrule, poolget,
    ruleaddvar, EnvState, EnvironmentId, PoolId, RuleId,
};
use crate::graph::{mkedge, mknode, nodeget, nodeuse, EdgeId, Graph, NodeId, PathStyle};
use crate::names::{Names, VarId};
use crate::util::{canonpath, is_canonical, BStr, BString, ByteSlice, EvalPart, EvalString, IdVec};
use std::fmt;
use std::num::NonZeroUsize;

mod execute;

pub use crate::parse::{load_manifest, Manifest, ManifestOptions};
pub use execute::{Build, Jobs, Outcome, Persistence, Planned};

/// A path interned in a graph.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Node(NodeId);

/// A build statement in a graph.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Edge(EdgeId);

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
}

impl fmt::Display for FrontendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("empty path"),
            Self::EdgeWithoutOutputs => formatter.write_str("expected path"),
            Self::DuplicateOutput { path } => {
                write!(formatter, "multiple rules generate {}", path.as_bstr())
            }
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
        self.arenas.node(node.0).gen.map(Edge)
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
    /// another edge already generates or this one names twice,
    /// [`FrontendError::UnknownPool`] for a `pool` binding naming a pool that
    /// was never defined, and [`FrontendError::DyndepNotInput`] for a `dyndep`
    /// binding naming a path the edge does not depend on.
    // [spec:ronin:req:frontend.graph-construction]
    pub fn add_edge(&mut self, spec: EdgeSpec<'_>) -> Result<Edge, FrontendError> {
        if spec.explicit_outputs.is_empty() && spec.implicit_outputs.is_empty() {
            return Err(FrontendError::EdgeWithoutOutputs);
        }
        let mut out =
            IdVec::with_capacity(spec.explicit_outputs.len() + spec.implicit_outputs.len());
        for output in spec.explicit_outputs.iter().chain(spec.implicit_outputs) {
            if self.arenas.node(output.0).gen.is_some() || out.contains(&output.0) {
                return Err(FrontendError::DuplicateOutput {
                    path: self.arenas.node_path(output.0).to_vec(),
                });
            }
            out.push(output.0);
        }
        let input = spec
            .explicit_inputs
            .iter()
            .chain(spec.implicit_inputs)
            .chain(spec.order_only_inputs)
            .map(|node| node.0)
            .collect::<IdVec<_>>();
        let validation = spec
            .validations
            .iter()
            .map(|node| node.0)
            .collect::<IdVec<_>>();

        let edge = mkedge(&mut self.arenas, spec.scope.0);
        for output in &out {
            self.arenas.node_mut(*output).gen = Some(edge);
        }
        for node in &input {
            nodeuse(&mut self.arenas, *node, edge);
        }
        for node in &validation {
            self.arenas.add_validation_use(*node, edge);
        }
        let explicit_inputs = spec.explicit_inputs.len();
        let non_order_only_inputs = explicit_inputs + spec.implicit_inputs.len();
        {
            let stored = self.arenas.edge_mut(edge);
            stored.rule = Some(spec.rule.0);
            stored.out = out;
            stored.input = input;
            stored.validation = validation;
            stored.set_explicit_output_count(spec.explicit_outputs.len());
            stored.set_input_partitions(explicit_inputs, non_order_only_inputs);
            stored.always_dirty = spec.always_dirty;
        }
        for (name, value) in spec.bindings {
            self.arenas
                .edge_mut(edge)
                .bindings
                .insert(name.0, BString::from(value));
        }
        self.resolve_pool(edge)?;
        self.resolve_dyndep(edge)?;
        Ok(Edge(edge))
    }

    fn resolve_pool(&mut self, edge: EdgeId) -> Result<(), FrontendError> {
        let Some(name) = edgevar(&self.arenas, edge, Names::POOL, PathStyle::ShellEscaped)
            .filter(|name| !name.is_empty())
        else {
            return Ok(());
        };
        let Ok(pool) = poolget(&self.state, BStr::new(name.as_slice())) else {
            return Err(FrontendError::UnknownPool { name: name.into() });
        };
        self.arenas.edge_mut(edge).pool = Some(pool);
        Ok(())
    }

    fn resolve_dyndep(&mut self, edge: EdgeId) -> Result<(), FrontendError> {
        let Some(mut path) = edgevar(&self.arenas, edge, Names::DYNDEP, PathStyle::Raw)
            .filter(|path| !path.is_empty())
        else {
            return Ok(());
        };
        canonpath(&mut path);
        let dyndep = mknode(&mut self.arenas, path.as_slice());
        if !self.arenas.edge(edge).input.contains(&dyndep) {
            return Err(FrontendError::DyndepNotInput { path: path.into() });
        }
        self.arenas.edge_mut(edge).dyndep = Some(dyndep);
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
                node.gen.is_some() && node.uses.is_empty()
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
            assert_eq!(arenas.node(output.0).gen, Some(edge.0));
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
            Err(FrontendError::DuplicateOutput {
                path: b"twice".to_vec()
            })
        );
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
