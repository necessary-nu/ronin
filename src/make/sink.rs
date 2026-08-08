//! kati's [`BuildSink`], implemented against Ronin's graph.
//!
//! The emitter computes an edge and then writes it out as `build.ninja` bytes.
//! This is the same computation with the writing removed: each rule becomes a
//! [`Rule`] and each edge an [`add_edge`](BuildGraph::add_edge), so the graph
//! that reaches the scheduler is the one Make described rather than one
//! recovered from a file.

use crate::frontend::{Binding, BuildGraph, EdgeSpec, FrontendError, Node, Rule, Scope, Template};
use kati::anyhow;
use kati::build_sink::{BuildSink, RuleId, SinkCommand, SinkEdge, SinkPool, SinkRule};
use kati::bytes::Bytes;
use kati::strutil::escape_shell;
use kati::symtab::{Interner, Symbol};
use std::collections::HashMap;
use std::num::NonZeroUsize;

/// The binding names an edge kati produced can carry.
///
/// Interned once. Every edge names most of them, and interning is a hash of the
/// name against the graph's own table.
struct Bindings {
    command: Binding,
    description: Binding,
    depfile: Binding,
    deps: Binding,
    restat: Binding,
    rspfile: Binding,
    rspfile_content: Binding,
    pool: Binding,
    tags: Binding,
    dry_run_command: Binding,
    recipe_location: Binding,
    /// `$out`, for the two bindings whose value is per edge rather than per
    /// rule. kati mints one rule per edge, so this expands to that edge's own
    /// single output.
    out: Binding,
}

impl Bindings {
    fn intern(graph: &mut BuildGraph) -> Self {
        Self {
            command: graph.binding(b"command"),
            description: graph.binding(b"description"),
            depfile: graph.binding(b"depfile"),
            deps: graph.binding(b"deps"),
            restat: graph.binding(b"restat"),
            rspfile: graph.binding(b"rspfile"),
            rspfile_content: graph.binding(b"rspfile_content"),
            pool: graph.binding(b"pool"),
            tags: graph.binding(b"tags"),
            dry_run_command: graph.binding(crate::build::DRY_RUN_COMMAND),
            recipe_location: graph.binding(crate::build::RECIPE_LOCATION),
            out: graph.binding(b"out"),
        }
    }
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
    scope: Scope,
    bindings: Bindings,
    phony: Rule,
    /// kati's rule handles to Ronin's. kati mints one rule per edge and
    /// declares it immediately before that edge, so this holds one entry for
    /// as long as it takes to reach the edge that names it.
    rules: HashMap<RuleId, Rule>,
    /// kati's symbols to Ronin's nodes, so a path shared by many edges is
    /// canonicalized and interned once.
    interned: HashMap<Symbol, Node>,
    /// The first construction failure, kept because kati's walk unwinds through
    /// [`anyhow::Error`] and the typed error is what a caller can act on.
    failure: Option<FrontendError>,
}

impl Default for GraphSink {
    fn default() -> Self {
        Self::new()
    }
}

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
        let mut graph = BuildGraph::new();
        let scope = graph.root();
        let bindings = Bindings::intern(&mut graph);
        let phony = graph
            .rule(scope, b"phony")
            .expect("a new graph holds the built-in phony rule");
        Self {
            graph,
            scope,
            bindings,
            phony,
            rules: HashMap::new(),
            interned: HashMap::new(),
            failure: None,
        }
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
        match self.graph.node(&path) {
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

    /// The command line that runs `rule`'s script, and the bindings it needs.
    ///
    /// A script short enough to pass as an argument is quoted into one, which
    /// is why it is escaped for the shell that will unquote it. A script too
    /// long has to reach the shell as a file, and the shell is then given the
    /// file rather than a `-c` and a string.
    fn command_bindings(&self, rule: &SinkRule<'_>) -> Vec<(Binding, Template)> {
        match rule.command {
            SinkCommand::Inline(script) => {
                let mut command = Template::literal(rule.shell);
                command.push_literal(b" ");
                command.push_literal(rule.shell_flags);
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
                response_file.push_variable(self.bindings.out);
                response_file.push_literal(b".rsp");
                let mut command = Template::literal(rule.shell);
                command.push_literal(b" ");
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
}

impl BuildSink for GraphSink {
    fn start(&mut self, pools: &[SinkPool<'_>]) -> anyhow::Result<()> {
        for pool in pools {
            let declared = match self.graph.define_pool(pool.name) {
                Ok(declared) => declared,
                Err(failure) => return Err(self.refuse(failure)),
            };
            if let Some(depth) = NonZeroUsize::new(pool.depth) {
                self.graph.set_pool_depth(declared, depth);
            }
        }
        Ok(())
    }

    // [spec:ronin:req:make.graph-direct]
    fn declare_rule(&mut self, _names: &dyn Interner, rule: &SinkRule<'_>) -> anyhow::Result<()> {
        let mut bindings = self.command_bindings(rule);
        if !rule.dry_run_command.is_empty() {
            let mut command = Template::literal(rule.shell);
            command.push_literal(b" ");
            command.push_literal(rule.shell_flags);
            command.push_literal(b" \"");
            command.push_literal(&escape_shell(&Bytes::copy_from_slice(rule.dry_run_command)));
            command.push_literal(b"\"");
            bindings.push((self.bindings.dry_run_command, command));
        }
        bindings.push((
            self.bindings.description,
            rule.description.map_or_else(
                // What a build prints when the Makefile did not say. The
                // manifest writer picks the same thing, in the same terms.
                || {
                    let mut default = Template::literal(b"build ");
                    default.push_variable(self.bindings.out);
                    default
                },
                Template::literal,
            ),
        ));
        if let Some(depfile) = rule.depfile {
            bindings.push((self.bindings.depfile, Template::literal(depfile)));
            // kati emits no other depfile format, and says so.
            bindings.push((self.bindings.deps, Template::literal(b"gcc")));
        }
        if rule.restat {
            bindings.push((self.bindings.restat, Template::literal(b"1")));
        }

        let name = format!("rule{}", rule.id);
        match self
            .graph
            .define_rule(self.scope, name.as_bytes(), bindings)
        {
            Ok(defined) => {
                self.rules.insert(rule.id, defined);
                Ok(())
            }
            Err(failure) => Err(self.refuse(failure)),
        }
    }

    // [spec:ronin:req:make.graph-direct]
    // [spec:ronin:req:make.phony-always-dirty]
    fn declare_edge(&mut self, names: &dyn Interner, edge: &SinkEdge<'_>) -> anyhow::Result<()> {
        let rule = match edge.rule {
            Some(id) => *self
                .rules
                .get(&id)
                .ok_or_else(|| anyhow::Error::msg(format!("edge names undeclared rule{id}")))?,
            None => self.phony,
        };
        let outputs = vec![self.node(names, edge.output)?];
        let implicit_outputs = self.node_list(names, edge.implicit_outputs)?;
        let inputs = self.node_list(names, edge.inputs)?;
        let order_only_inputs = self.node_list(names, edge.order_only_inputs)?;
        let validations = self.node_list(names, edge.validations)?;

        let mut bindings = Vec::new();
        if let Some(pool) = edge.pool {
            bindings.push((self.bindings.pool, pool.to_vec()));
        }
        if let Some(tags) = edge.tags {
            bindings.push((self.bindings.tags, tags.to_vec()));
        }
        // Where the rule was written. Make leads the diagnostics that are about
        // the rule rather than about the file with it, and nothing else an edge
        // carries can say where it came from.
        if let Some(loc) = edge.loc {
            bindings.push((
                self.bindings.recipe_location,
                loc.display(names).to_string().into_bytes(),
            ));
        }

        let spec = EdgeSpec {
            scope: self.scope,
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
            bindings,
        };
        match self.graph.add_edge(spec) {
            Ok(_) => Ok(()),
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
            self.graph.add_default(node);
        }
        Ok(())
    }

    fn finish(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
