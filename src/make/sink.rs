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
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

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
    ignore_errors: Binding,
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
            ignore_errors: graph.binding(crate::build::IGNORE_ERRORS),
            out: graph.binding(b"out"),
        }
    }
}

/// One static recursive invocation within a held recipe.
pub(crate) struct SubninjaInvocation {
    pub(crate) command: Vec<u8>,
    pub(crate) make: Vec<u8>,
}

/// A recursive recipe held until all its child Makefiles have been compiled.
pub(crate) struct PendingSubninja {
    pub(crate) invocations: Vec<SubninjaInvocation>,
    pub(crate) scope: Scope,
    residual_rule: Option<Rule>,
    diagnostic_command: Vec<u8>,
    explicit_outputs: Vec<Node>,
    implicit_outputs: Vec<Node>,
    inputs: Vec<Node>,
    order_only_inputs: Vec<Node>,
    validations: Vec<Node>,
    always_dirty: bool,
    intermediate: bool,
    disposable: bool,
    bindings: Vec<(Binding, Vec<u8>)>,
}

/// The non-executor description retained between kati's rule and edge calls.
struct SubninjaRule {
    invocations: Vec<SubninjaInvocation>,
    residual_rule: Option<Rule>,
    diagnostic_command: Vec<u8>,
}

/// What one kati compilation unit contributed to the shared graph.
pub(crate) struct UnitOutput {
    pub(crate) targets: Vec<Node>,
    pub(crate) subninjas: Vec<PendingSubninja>,
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
    /// kati's rule handles to Ronin's. kati mints one rule per edge and
    /// declares it immediately before that edge, so this holds one entry for
    /// as long as it takes to reach the edge that names it.
    rules: HashMap<RuleId, Rule>,
    /// Recursive rules are not executor rules. They wait for their immediately
    /// following edge so the compiler can replace that edge with graph
    /// composition.
    subninja_rules: HashMap<RuleId, SubninjaRule>,
    /// kati's symbols to Ronin's nodes, so a path shared by many edges is
    /// canonicalized and interned once.
    interned: HashMap<Symbol, Node>,
    declared_pools: HashSet<Vec<u8>>,
    serial_units: usize,
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
        Self::new_at(Path::new(""))
    }

    /// A sink whose response files are anchored at the executor's root.
    #[must_use]
    pub(crate) fn new_at(root_directory: &Path) -> Self {
        let mut graph = BuildGraph::new();
        let scope = graph.root();
        let bindings = Bindings::intern(&mut graph);
        let phony = graph
            .rule(scope, b"phony")
            .expect("a new graph holds the built-in phony rule");
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
            },
            bindings,
            phony,
            rules: HashMap::new(),
            subninja_rules: HashMap::new(),
            interned: HashMap::new(),
            declared_pools: HashSet::new(),
            serial_units: 0,
            failure: None,
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
        self.unit = Unit {
            scope: self.graph.child_scope(parent),
            path_prefix,
            command_directory,
            root: false,
            serial_pool: None,
            recipe_environment: Vec::new(),
            targets: Vec::new(),
            subninjas: Vec::new(),
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
        }
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
    /// Parent prerequisites become order-only inputs of each child goal: the
    /// subgraph starts only once the wrapper recipe could have started, while
    /// the child's own timestamps still decide what work it needs. When parent
    /// and child name the same goal, the child edge subsumes the held wrapper;
    /// otherwise the wrapper becomes a phony alias for the child targets.
    // [spec:ronin:req:make.recursive-invocation+1]
    pub(crate) fn complete_subninja(
        &mut self,
        pending: PendingSubninja,
        child_target_groups: &[Vec<Node>],
    ) -> Result<(), FrontendError> {
        debug_assert_eq!(pending.invocations.len(), child_target_groups.len());
        let mut waits = pending
            .inputs
            .iter()
            .chain(&pending.order_only_inputs)
            .copied()
            .collect::<Vec<_>>();
        let mut child_targets = Vec::new();
        for targets in child_target_groups {
            for target in targets {
                if let Some(edge) = self.graph.generator(*target) {
                    let preceding = waits
                        .iter()
                        .copied()
                        .filter(|wait| wait != target)
                        .collect::<Vec<_>>();
                    self.graph.add_order_only_inputs(edge, &preceding);
                }
                if !child_targets.contains(target) {
                    child_targets.push(*target);
                }
            }
            if !targets.is_empty() {
                waits.clear();
                for target in targets {
                    if !waits.contains(target) {
                        waits.push(*target);
                    }
                }
            }
        }

        let collapsed = pending.residual_rule.is_none()
            && child_target_groups.len() == 1
            && pending.implicit_outputs.is_empty()
            && pending.explicit_outputs.len() == 1
            && child_targets.contains(&pending.explicit_outputs[0])
            && self.graph.generator(pending.explicit_outputs[0]).is_some();
        if collapsed {
            let edge = self
                .graph
                .generator(pending.explicit_outputs[0])
                .expect("the collapse predicate found the child edge");
            self.graph.merge_edge_properties(
                edge,
                pending.always_dirty,
                pending.intermediate,
                pending.disposable,
                &pending.validations,
            );
            return Ok(());
        }

        if child_targets.iter().any(|target| {
            pending.explicit_outputs.contains(target) || pending.implicit_outputs.contains(target)
        }) {
            return Err(FrontendError::UncomposableSubninja {
                command: pending.diagnostic_command,
            });
        }

        let (rule, inputs, order_only_inputs) = if let Some(rule) = pending.residual_rule {
            let mut order_only_inputs = pending.order_only_inputs;
            for target in &child_targets {
                if !order_only_inputs.contains(target) {
                    order_only_inputs.push(*target);
                }
            }
            (rule, pending.inputs, order_only_inputs)
        } else {
            let mut inputs = pending.inputs;
            for target in &child_targets {
                if !inputs.contains(target) {
                    inputs.push(*target);
                }
            }
            (self.phony, inputs, pending.order_only_inputs)
        };
        self.graph.add_edge(EdgeSpec {
            scope: pending.scope,
            rule,
            explicit_outputs: &pending.explicit_outputs,
            implicit_outputs: &pending.implicit_outputs,
            explicit_inputs: &inputs,
            implicit_inputs: &[],
            order_only_inputs: &order_only_inputs,
            validations: &pending.validations,
            always_dirty: pending.always_dirty,
            intermediate: pending.intermediate,
            disposable: pending.disposable,
            bindings: pending.bindings,
        })?;
        Ok(())
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
        match self.graph.node(qualified.as_os_str().as_bytes()) {
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

    fn push_shell_word(command: &mut Template, word: &[u8]) {
        command.push_literal(b"'");
        for byte in word {
            if *byte == b'\'' {
                command.push_literal(b"'\\''");
            } else {
                command.push_literal(&[*byte]);
            }
        }
        command.push_literal(b"'");
    }

    /// The shell prefix that gives a child compilation unit its Make `-C`
    /// working directory without moving Ronin's executor.
    fn command_prefix(&self) -> Template {
        let mut command = Template::default();
        if !self.unit.command_directory.as_os_str().is_empty() {
            command.push_literal(b"cd ");
            Self::push_shell_word(
                &mut command,
                self.unit.command_directory.as_os_str().as_bytes(),
            );
            command.push_literal(b" && ");
        }

        if !self.unit.recipe_environment.is_empty() {
            command.push_literal(b"env");
            for (name, value) in &self.unit.recipe_environment {
                if value.is_none() {
                    command.push_literal(b" -u ");
                    Self::push_shell_word(&mut command, name);
                }
            }
            for (name, value) in &self.unit.recipe_environment {
                if let Some(value) = value {
                    let mut assignment = Vec::with_capacity(name.len() + value.len() + 1);
                    assignment.extend_from_slice(name);
                    assignment.push(b'=');
                    assignment.extend_from_slice(value);
                    command.push_literal(b" ");
                    Self::push_shell_word(&mut command, &assignment);
                }
            }
            command.push_literal(b" ");
        }
        command
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
    ) -> Vec<(Binding, Template)> {
        match command {
            SinkCommand::Inline(script) => {
                let mut command = self.command_prefix();
                command.push_literal(shell);
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
                let mut command = self.command_prefix();
                command.push_literal(shell);
                command.push_literal(b" ");
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
    fn executor_rule_bindings(
        &self,
        rule: &SinkRule<'_>,
        command: SinkCommand<'_>,
        ignore_errors: bool,
    ) -> Vec<(Binding, Template)> {
        let mut bindings = self.command_bindings(rule.shell, rule.shell_flags, command);
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
        if rule.contains_recursive {
            if rule.subninjas.is_empty() {
                return Err(self.refuse(FrontendError::UncomposableSubninja {
                    command: script.to_vec(),
                }));
            }
            let residual_rule = rule
                .residual_command
                .map(|command| {
                    let bindings =
                        self.executor_rule_bindings(rule, command, rule.residual_ignore_errors);
                    let name = format!("rule{}_residual", rule.id);
                    self.define_executor_rule(name.as_bytes(), bindings)
                })
                .transpose()?;
            let invocations = rule
                .subninjas
                .iter()
                .map(|subninja| SubninjaInvocation {
                    command: subninja.command.to_vec(),
                    make: subninja.make.to_vec(),
                })
                .collect();
            self.subninja_rules.insert(
                rule.id,
                SubninjaRule {
                    invocations,
                    residual_rule,
                    diagnostic_command: script.to_vec(),
                },
            );
            return Ok(());
        }

        let bindings = self.executor_rule_bindings(rule, rule.command, rule.ignore_errors);
        let name = format!("rule{}", rule.id);
        let defined = self.define_executor_rule(name.as_bytes(), bindings)?;
        self.rules.insert(rule.id, defined);
        Ok(())
    }

    // [spec:ronin:req:make.graph-direct]
    // [spec:ronin:req:make.phony-always-dirty]
    fn declare_edge(&mut self, names: &dyn Interner, edge: &SinkEdge<'_>) -> anyhow::Result<()> {
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
        let subninja_rule = edge.rule.and_then(|id| self.subninja_rules.get(&id));
        let is_subninja = subninja_rule.is_some();
        let has_residual_action = subninja_rule.is_some_and(|rule| rule.residual_rule.is_some());
        if edge.pool.is_none() && edge.rule.is_some() && (!is_subninja || has_residual_action) {
            if let Some(pool) = &self.unit.serial_pool {
                bindings.push((self.bindings.pool, pool.clone()));
            }
        }
        if let Some(id) = edge.rule {
            if let Some(rule) = self.subninja_rules.remove(&id) {
                self.unit.subninjas.push(PendingSubninja {
                    invocations: rule.invocations,
                    scope: self.unit.scope,
                    residual_rule: rule.residual_rule,
                    diagnostic_command: rule.diagnostic_command,
                    explicit_outputs: outputs,
                    implicit_outputs,
                    inputs,
                    order_only_inputs,
                    validations,
                    always_dirty: edge.always_dirty,
                    intermediate: edge.intermediate,
                    disposable: edge.disposable,
                    bindings,
                });
                return Ok(());
            }
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
