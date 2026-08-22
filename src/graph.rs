//! Dense graph arenas and dependency operations.

mod cycles;
mod deferred;
mod edge;
mod forgiven;
mod ids;
mod index;
mod intermediate;
mod marks;
mod path;
mod peer;
mod searched;
mod unmade;
mod validation;
mod withdrawal;

use crate::env::{Environment, EnvironmentId, Pool, PoolId, Rule, RuleId};
use crate::error::GraphError;
use crate::htab::rapidhashv1;
use crate::runtime::{CommandHash, FileTime, RuntimeState};
use crate::util::{BStr, BString, ByteSlice, IdVec, arena_id};
pub(crate) use cycles::dependency_cycles;
pub(crate) use deferred::{DeferredFreshness, edgeaddorderonly};
use deferred::{
    capture_deferred_freshness, recompute_completion_join, recompute_deferred_freshness,
};
use edge::EdgePartitions;
use index::NodeIndex;
pub(crate) use index::{allocate_node, mknode, nodeget};
use intermediate::{record_absent_intermediate, stand_in_for_an_intermediate};
pub(crate) use marks::MarkSet;
use marks::{VisitMarks, VisitState};
pub(crate) use path::{nodepath_bytes, shell_escape_path};
pub(crate) use peer::trigger_output;
use searched::settle_searched_outputs;
pub(crate) use searched::{
    SettledNameReference, SettledNames, SettledView, elsewhere_mtime, found_name_stands,
    mark_written_here,
};
use std::io;
use std::path::Path;
use withdrawal::Withdrawal;

arena_id!(NodeId, pub(in crate::graph));
arena_id!(EdgeId, pub(in crate::graph));

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PathStyle {
    #[default]
    Raw,
    ShellEscaped,
}

impl PathStyle {
    const fn shell_escaped(self) -> bool {
        matches!(self, Self::ShellEscaped)
    }
}

/// A run of bytes in the graph's path arena.
///
/// Interning a path hashes its bytes, probes the index, follows a `NodeId`
/// into the node arena and compares — four steps that each dereferenced a
/// separately allocated buffer at an unrelated address. Holding an offset
/// instead puts every path in one sequential region, which is what those
/// four steps walk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PathSpan {
    offset: u32,
    len: u32,
}

// [spec:ronin:def:graph.node]
pub(crate) struct Node {
    pub(crate) path: PathSpan,
    /// Shell-quoted form, present only when quoting actually changes the path.
    pub(crate) shellpath: Option<PathSpan>,
    pub(crate) generator: Option<EdgeId>,
    pub(crate) uses: IdVec<EdgeId>,
}

/// Which timestamp history participates in an edge's freshness decision.
///
/// Ninja compares both the filesystem and the mtime persisted in its build
/// log. A Make target is current from the filesystem alone: the build log may
/// describe an earlier recipe run, but it is not part of Make's semantics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FreshnessHistory {
    #[default]
    BuildLogAware,
    FilesystemOnly,
}

// [spec:ronin:def:graph.edge]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the lint guards a positional argument list, and this is only ever filled in by name"
)]
pub(crate) struct Edge {
    pub(crate) rule: Option<RuleId>,
    pub(crate) pool: Option<PoolId>,
    /// The scope this edge's bindings and variables resolve against.
    ///
    /// This is the enclosing manifest scope directly. Edge-local bindings live
    /// in `bindings`, so giving each edge its own environment only added an
    /// arena entry and one more link to walk on every lookup that missed.
    pub(crate) env: EnvironmentId,
    pub(crate) bindings: crate::names::Bindings<BString>,
    pub(crate) out: IdVec<NodeId>,
    pub(crate) input: IdVec<NodeId>,
    pub(crate) validation: IdVec<NodeId>,
    pub(crate) dyndep: Option<NodeId>,
    /// Whether the edge is out of date whenever it is reached.
    ///
    /// A GNU Make `.PHONY` target has no file behind it, so neither the
    /// filesystem nor the build log can answer whether its recipe has to run:
    /// it runs whenever the target is asked for. Nothing about an edge's
    /// inputs, outputs, or recorded history expresses that, so the edge says
    /// it itself.
    ///
    /// This is not the built-in `phony` rule, which says an output aliases its
    /// inputs and runs nothing at all. An edge can be either, both, or
    /// neither.
    pub(crate) always_dirty: bool,
    /// Whether the outputs are files the Makefile never names, invented by the
    /// implicit rule search to complete a chain.
    ///
    /// GNU Make will not remake what reads such a file merely because it is
    /// absent: the file stands in for the newest thing behind it, and only a
    /// consumer that has to be rebuilt anyway makes it worth creating. It is
    /// deleted once the build has finished.
    pub(crate) intermediate: bool,
    /// Whether the build throws this edge's outputs away once it has finished
    /// with them, which every intermediate but a `.SECONDARY` one is.
    pub(crate) disposable: bool,
    /// Whether an output this edge does not write is absent rather than an
    /// alias for what the edge reads.
    ///
    /// An edge with no command is Ninja's `phony`, where the output stands for
    /// its inputs: one that is not on disk takes their newest date, and its
    /// absence is no reason for anything to run. A Makefile target whose
    /// recipe writes nothing compiles to the same commandless shape and means
    /// the opposite — GNU Make ran the recipe, and a prerequisite that does not
    /// exist once it has been made is newer than whatever reads it
    /// (`remake.c`: `notice_finished_file` leaves `last_mtime` unknown, the
    /// re-read finds nothing, and `must_make` follows). Which of the two an
    /// edge is cannot be read off its having no command, so the front end that
    /// compiled it says.
    pub(crate) outputs_unaliased: bool,
    /// Whether the record these outputs' dates are read from keeps only whole
    /// seconds, so a comparison that has one on the target side must read it as
    /// the end of its second.
    ///
    /// An archive index is the one such record GNU Make has: a member filed
    /// from an object written part way through a second is dated a fraction
    /// earlier than the object it copies, and without the rounding the archive
    /// is rewritten on every invocation. GNU Make marks the file
    /// `low_resolution_time` and rounds in `update_file_1`, which is the file
    /// being updated and nowhere else — a member read as a prerequisite keeps
    /// the plain date. So the rounding belongs to the comparison rather than to
    /// the stat, and the edge that makes the file is what knows its date is
    /// coarse.
    pub(crate) outputs_low_resolution: bool,
    /// Whether what the outputs did is read off the disk once the command has
    /// run, rather than taken from the command having run.
    ///
    /// GNU Make stats a target it has just remade and compares the timestamp
    /// it then has against the targets that read it, so a recipe that ran
    /// without moving its target leaves them up to date. Ninja's `restat` asks
    /// for the same second look but grants the outcome only to an output whose
    /// timestamp did not move at all, where Make lets the comparison decide,
    /// so the two are not the same property and an edge carries this one
    /// itself.
    // [spec:ronin:req:make.remade-target-re-observed]
    pub(crate) outputs_reobserved: bool,
    /// Whether part of this edge's recipe has already run, somewhere no
    /// reading of the edge itself could show.
    ///
    /// A recursive recipe is cut into segments around its `$(MAKE)` line: the
    /// lines written ahead of the invocation run at a compilation boundary, so
    /// that the child Makefile is read off the disk they write to, and this
    /// edge carries what is left of the recipe. Those lines are the recipe's
    /// own, and one of them may write the very target this edge makes — after
    /// which the file is on the ground with nothing newer behind it and every
    /// ordinary reading says the target is current.
    ///
    /// GNU Make is never in that position: it decides whether a target is out
    /// of date once, before the recipe starts, and then runs the whole recipe.
    /// So the verdict taken when the first segment was staged is recorded here
    /// and the rest of the recipe runs whatever date its target has acquired
    /// in the meantime. A target whose recipe has begun is a target being
    /// remade.
    ///
    /// Not [`Self::always_dirty`], which is `.PHONY` and yields to `-W`: a
    /// file the switch names has a date, and a `.PHONY` name has none, so the
    /// switch answers one and cannot answer the other. This is a fact about
    /// work that has already happened, and no switch can make it untrue.
    pub(crate) recipe_begun: bool,
    pub(crate) freshness_history: FreshnessHistory,
    partitions: EdgePartitions,
}

// [spec:ronin:def:graph.graphinit-fn]
// [spec:ronin:sem:graph.graphinit-fn]
#[derive(Default)]
pub(crate) struct Graph {
    // Fixed-seed rapidhash follows Ninja and C samurai: manifests are trusted
    // input (executing them runs arbitrary commands), so SipHash DoS
    // hardening buys nothing here. Observable graph order comes from the
    // arenas, never index iteration.
    /// Nodes interned by the Ninja-global path namespace. Front ends may also
    /// allocate isolated nodes whose identity is local to one source unit;
    /// those retain a physical path but deliberately do not enter this index.
    node_by_path: NodeIndex,
    /// Every node path and shell-quoted path, appended and never moved.
    paths: Vec<u8>,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    environments: Vec<Environment>,
    rules: Vec<Rule>,
    pools: Vec<Pool>,
    /// Edges that name a node as a validation, kept aside from the node.
    ///
    /// Ninja's `|@` validations are rare — no node in a typical manifest has
    /// one — but an inline list cost every node twenty-four bytes whether it
    /// used the feature or not, a third of `Node`, and the node arena is the
    /// largest structure a large manifest builds. Holding them aside keeps the
    /// feature exactly and charges only the nodes that use it.
    validation_uses: crate::htab::RapidHashMap<NodeId, IdVec<EdgeId>>,
    deferred_freshness: crate::htab::RapidHashMap<EdgeId, DeferredFreshness>,
    completion_joins: crate::htab::RapidHashMap<EdgeId, NodeId>,
    /// What a stopped command may be made to give back, for the edges a front
    /// end answered for.
    ///
    /// Beside the edge arena for the reason validations are beside the node
    /// arena: withdrawal exclusions are a thing a Makefile states and a
    /// manifest cannot, and an inline list would charge every edge in every
    /// manifest for a Make feature no Ninja build has.
    ///
    /// An edge missing from here is one nothing narrowed, which is every edge
    /// of a Ninja manifest: Ninja withdraws whatever a cut-short command wrote
    /// and has no `.PRECIOUS` to except from it. An edge present with an empty
    /// list is one a Makefile narrowed to nothing.
    withdrawal: crate::htab::RapidHashMap<EdgeId, Withdrawal>,
    /// Makefiles this read tried to remake and did not, which the goals must
    /// neither run again nor believe in. Beside the arena for the reason
    /// `withdrawal` is: no node of a Ninja manifest is ever in it.
    unmade_makefiles: crate::htab::RapidHashSet<NodeId>,
    /// Makefiles this read asked about under `-q` and was told were not up to
    /// date, which the goals must refuse over without calling it a failure.
    /// Beside the arena for the reason `unmade_makefiles` is.
    questioned_makefiles: crate::htab::RapidHashSet<NodeId>,
    /// Makefiles the read wanted and did not get, whatever stands at their name.
    /// Beside the arena for the same reason `unmade_makefiles` is.
    unread_makefiles: crate::htab::RapidHashSet<NodeId>,
    /// Names a front end invented to ask for work by, which no command writes
    /// and no `stat` can find.
    ///
    /// A recipe segment run for its effects makes no file the Makefile named,
    /// so the compilation gives it an output of its own to be reached through.
    /// The build must be told, or it does to that name everything it does to a
    /// file: create the directory it appears to sit in, and stat it. Beside the
    /// arena for the reason `withdrawal` is — no node of a Ninja manifest is
    /// ever in it.
    invented_outputs: crate::htab::RapidHashSet<NodeId>,
    /// Waits whose consumer outlives a failure of what it waited for. See
    /// [`mod@forgiven`]; beside the arena for the reason `withdrawal` is.
    forgiven_order: crate::htab::RapidHashSet<(EdgeId, NodeId)>,
    /// Outputs a recipe makes only on the way to making something else, for the
    /// edges that have any. Beside the arena for the reason `withdrawal`
    /// is: almost no edge in almost any graph has one.
    peer_outputs: crate::htab::RapidHashMap<EdgeId, IdVec<NodeId>>,
    /// A second place to look for an output, for the outputs a front end found
    /// somewhere other than where the build file named them.
    ///
    /// GNU Make's directory search answers about a target that is not here, and
    /// the answer does not replace the name: `f_mtime` hangs the found path off
    /// the file object beside the written one and takes the found file's date
    /// for the target, and only after the prerequisites have settled does
    /// `update_file_1` choose between them. So this is where the target is
    /// observed while nothing has remade it, and the name in the arena is where
    /// it is made when something has. Beside the arena for the reason
    /// `withdrawal` is: no node of a Ninja manifest is ever in it.
    searched_at: crate::htab::RapidHashMap<NodeId, BString>,
    /// The spellings a front end left for the build to fill in, for the edges
    /// whose command it had to read before the build could settle them.
    ///
    /// The other end of `searched_at`: a command that names a node with two
    /// names, written down before the build chose between them, carries a
    /// reference where the name would have gone. Beside the arena for the same
    /// reason: no manifest edge has one.
    settled_names: crate::htab::RapidHashMap<EdgeId, SettledNames>,
    phony_rule: Option<RuleId>,
    console_pool: Option<PoolId>,
    names: crate::names::Names,
}

impl Graph {
    pub(crate) fn node_ids(&self) -> impl ExactSizeIterator<Item = NodeId> + use<> {
        (0..self.nodes.len()).map(NodeId::from_index)
    }

    pub(crate) fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub(crate) fn edge(&self, id: EdgeId) -> &Edge {
        &self.edges[id.index()]
    }

    pub(crate) fn edge_mut(&mut self, id: EdgeId) -> &mut Edge {
        &mut self.edges[id.index()]
    }

    pub(crate) fn edge_ids(&self) -> impl ExactSizeIterator<Item = EdgeId> + use<> {
        (0..self.edges.len()).map(EdgeId::from_index)
    }

    pub(crate) const fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub(crate) fn environment(&self, id: EnvironmentId) -> &Environment {
        &self.environments[id.index()]
    }

    pub(crate) fn environment_mut(&mut self, id: EnvironmentId) -> &mut Environment {
        &mut self.environments[id.index()]
    }

    pub(crate) fn push_environment(&mut self, environment: Environment) -> EnvironmentId {
        let id = EnvironmentId::from_index(self.environments.len());
        self.environments.push(environment);
        id
    }

    pub(crate) fn rule(&self, id: RuleId) -> &Rule {
        &self.rules[id.index()]
    }

    pub(crate) fn rule_mut(&mut self, id: RuleId) -> &mut Rule {
        &mut self.rules[id.index()]
    }

    pub(crate) fn rule_ids(&self) -> impl Iterator<Item = RuleId> + '_ {
        (0..self.rules.len()).map(RuleId::from_index)
    }

    pub(crate) fn push_rule(&mut self, rule: Rule) -> RuleId {
        let id = RuleId::from_index(self.rules.len());
        self.rules.push(rule);
        id
    }

    pub(crate) const fn names(&self) -> &crate::names::Names {
        &self.names
    }

    pub(crate) const fn names_mut(&mut self) -> &mut crate::names::Names {
        &mut self.names
    }

    pub(crate) const fn set_phony_rule(&mut self, rule: RuleId) {
        self.phony_rule = Some(rule);
    }

    pub(crate) const fn set_console_pool(&mut self, pool: PoolId) {
        self.console_pool = Some(pool);
    }

    /// Whether `rule` is the built-in phony rule, by identity as in Ninja.
    ///
    /// A manifest-defined rule that shadows the name `phony` in a subninja
    /// scope is an ordinary rule and must not match.
    pub(crate) const fn is_phony_rule(&self, rule: Option<RuleId>) -> bool {
        match (rule, self.phony_rule) {
            (Some(rule), Some(phony)) => rule.index() == phony.index(),
            _ => false,
        }
    }

    /// Whether `pool` is the built-in console pool, by identity as in Ninja.
    pub(crate) const fn is_console_pool(&self, pool: Option<PoolId>) -> bool {
        match (pool, self.console_pool) {
            (Some(pool), Some(console)) => pool.index() == console.index(),
            _ => false,
        }
    }

    pub(crate) fn pool(&self, id: PoolId) -> &Pool {
        &self.pools[id.index()]
    }

    pub(crate) fn pool_mut(&mut self, id: PoolId) -> &mut Pool {
        &mut self.pools[id.index()]
    }

    pub(crate) const fn pool_count(&self) -> usize {
        self.pools.len()
    }

    pub(crate) fn push_pool(&mut self, pool: Pool) -> PoolId {
        let id = PoolId::from_index(self.pools.len());
        self.pools.push(pool);
        id
    }
}

// [spec:ronin:def:graph.nodestat-fn]
// [spec:ronin:sem:graph.nodestat-fn]
pub(crate) fn nodestat_with<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    node: NodeId,
    stat: &mut F,
) -> Result<(), GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    // A Makefile the read wanted and did not get is not there as far as
    // anything after the read is concerned, whatever the filesystem says.
    // GNU Make writes exactly that, and writes it as a timestamp rather than
    // as a flag: `eval_makefile` sets `last_mtime = NONEXISTENT_MTIME` on the
    // file it could not open (reference/gnumake/src/read.c:409). It is what
    // makes the rule for an unopenable makefile run — a recipe with no
    // prerequisites is up to date the moment its target exists, and a file
    // with no read permission exists — and so what lets such a rule repair
    // the file and send the read around again.
    // Ahead of both, because GNU Make stamps the `-W` files after the whole
    // read has finished (main.c:2325) and so over whatever the read concluded
    // about them, and because there is no syscall to make for a name the
    // invocation has already answered about.
    if runtime.assumed_new.contains(node) {
        runtime.node_mut(node).observe(FileTime::NEWEST);
        return Ok(());
    }
    // `-o` is the same stamp with the sign turned round, and it is asked after
    // `-W` because `main` writes it before: `OLD_MTIME` goes down first
    // (main.c:2312) and `NEW_MTIME` goes over it (main.c:2325), so a name given
    // to both switches is new however the words were ordered.
    if runtime.assumed_old.contains(node) {
        runtime.node_mut(node).observe(FileTime::OLDEST);
        return Ok(());
    }
    if graph.is_unread_makefile(node) {
        runtime.node_mut(node).observe(FileTime::MISSING);
        return Ok(());
    }
    // Borrow the interned path for the syscall; only the error path needs an
    // owned copy, and scans stat every node.
    let path = graph.node_path(node);
    let mtime = stat(path.to_path().expect("byte paths are valid on Unix")).map_err(|source| {
        GraphError::Stat {
            node,
            path: path.to_owned(),
            source,
        }
    })?;
    let mtime = elsewhere_mtime(graph, node, mtime, stat)?;
    runtime.node_mut(node).observe(FileTime::observed(mtime));
    Ok(())
}

/// Collect the nodes a dirty scan from `target` is going to want to stat.
///
/// Correctness does not depend on this set being exact, which is what makes it
/// safe to use a plain walk rather than shadowing [`DirtyEvaluator`]'s state
/// machine: a node collected but never reached costs one wasted `stat`, and a
/// node reached but never collected is stat'ed by the scan itself, because
/// `nodestat_with` is still guarded by `is_unobserved`. The scan's behaviour
/// is unchanged either way — this only decides which syscalls happen early.
pub(crate) fn collect_stat_targets(
    graph: &Graph,
    scratch: &mut TraversalScratch,
    target: NodeId,
    out: &mut Vec<NodeId>,
) {
    out.clear();
    scratch.seen_nodes.begin(graph.nodes.len());
    scratch.seen_edges.begin(graph.edges.len());
    let mut work = vec![target];
    while let Some(node) = work.pop() {
        if scratch.seen_nodes.replace(node.index()) {
            continue;
        }
        if !graph.is_virtual_output(node) {
            out.push(node);
        }
        let Some(edge) = graph.node(node).generator else {
            continue;
        };
        if scratch.seen_edges.replace(edge.index()) {
            continue;
        }
        let edge = graph.edge(edge);
        work.extend(edge.out.iter().copied());
        work.extend(edge.input.iter().copied());
        work.extend(edge.validation.iter().copied());
    }
}

/// Recompute one edge after all of its inputs have already been evaluated.
// [spec:ronin:req:make.phony-always-dirty]
pub(crate) fn recompute_edge_dirty_with<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    edge: EdgeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    if graph.deferred_freshness(edge).is_some() {
        return recompute_deferred_freshness(graph, runtime, edge, stat);
    }
    if graph.is_completion_join(edge) {
        return recompute_completion_join(graph, runtime, edge, stat);
    }
    if runtime.edge(edge).restat_clean() {
        for output in &graph.edge(edge).out {
            runtime.node_mut(*output).set_dirty(false);
        }
        return Ok(false);
    }

    for output in &graph.edge(edge).out {
        if runtime.node(*output).mtime().is_unobserved() {
            nodestat_with(graph, runtime, *output, stat)?;
        }
    }

    let edge_data = graph.edge(edge);
    // What the recipe also writes on the way is not what decides whether it has
    // to run: GNU Make asks that of each name it was asked for, and a pattern
    // rule's other targets are entered beside them rather than among them.
    let peers = graph.peer_outputs(edge);
    let mut input_dirty = false;
    let mut newest_input = FileTime::MISSING;
    for input in edge_data.non_order_only_inputs() {
        let input = runtime.node(*input);
        input_dirty |= input.dirty();
        newest_input = newest_input.max(input.mtime());
    }

    let absent_intermediate = record_absent_intermediate(graph, runtime, edge, peers);
    let edge_data = graph.edge(edge);

    let out_of_date = if absent_intermediate {
        stand_in_for_an_intermediate(runtime, edge, &edge_data.out, newest_input, true);
        input_dirty
    } else if graph.is_phony_rule(edge_data.rule) && !edge_data.outputs_unaliased {
        // An alias stands in for an output it does not have, and is out of date
        // only when what it stands for is. A target whose recipe wrote nothing
        // shares the commandless shape and nothing else: it is read below, with
        // every other target, because that is what GNU Make does with it.
        let mut any_output_missing = false;
        for output in &edge_data.out {
            if runtime.node(*output).mtime().is_missing() {
                any_output_missing = true;
                runtime.node_mut(*output).set_mtime(newest_input);
            }
        }
        let missing_without_inputs =
            edge_data.input.is_empty() && edge_data.validation.is_empty() && any_output_missing;
        input_dirty || missing_without_inputs
    } else {
        let mut oldest_output: Option<FileTime> = None;
        let mut oldest_recorded_output: Option<FileTime> = None;
        for output in edge_data
            .out
            .iter()
            .filter(|output| !peers.contains(output))
        {
            let output = runtime.node(*output);
            let mtime = edge_data.target_mtime(output.mtime());
            oldest_output = Some(oldest_output.map_or(mtime, |oldest: FileTime| oldest.min(mtime)));
            if edge_data.freshness_history == FreshnessHistory::BuildLogAware
                && output.log_mtime().is_observed()
            {
                oldest_recorded_output = Some(oldest_recorded_output.map_or_else(
                    || output.log_mtime(),
                    |oldest| oldest.min(output.log_mtime()),
                ));
            }
        }
        let oldest_output = oldest_output.unwrap_or(FileTime::MISSING);
        let edge_state = runtime.edge(edge);
        let comparison = oldest_output.is_missing()
            || edge_state.deps_missing()
            || edge_state.command_dirty()
            || input_dirty
            || oldest_recorded_output.is_some_and(|output_mtime| newest_input > output_mtime)
            || newest_input > oldest_output;
        if edge_data.intermediate {
            stand_in_for_an_intermediate(runtime, edge, &edge_data.out, newest_input, comparison);
            input_dirty
        } else {
            comparison
        }
    };

    // An edge that declares itself never up to date is dirty whatever the
    // comparison above concluded, which is what makes the build log's record
    // of it beside the point rather than merely wrong. The comparison still
    // runs: a phony edge settles its outputs' mtimes there, and a consumer of
    // one reads them.
    //
    // An edge whose recipe has already begun is the same conclusion reached
    // from the other end: the comparison above is reading a target its own
    // recipe wrote, so what it says about the target is evidence of the work
    // rather than a reason to skip it.
    //
    // A scan answering `-B` says the same thing about every edge that has
    // something to run. GNU Make's own test is `!must_make && file->cmds != 0
    // && always_make_flag` (remake.c), so a name with no recipe behind it is
    // not forced — there is nothing the forcing could ask for — and a source
    // file has no generator edge here to be asked about at all.
    // What forces a `.PHONY` target is that it does not exist: GNU Make starts
    // `must_make = noexist` and a phony's date is `NONEXISTENT_MTIME`
    // (remake.c:550). A file `-W` named HAS a date — the switch writes
    // `NEW_MTIME` over it — so a phony the switch named stops being forced.
    // Only that one reason yields: `-B` still forces it, because
    // `always_make_flag` is a separate arm asking only whether there is a
    // recipe, and a prerequisite that really changed still remakes it. Probed
    // against 4.4.1 over `out: in ph`: `-W ph` rebuilds `out` and does not run
    // `ph`, `-W ph -B` runs both, and `-W out` runs both because the phony ran.
    let assumed_new = !graph.edge(edge).out.is_empty()
        && graph
            .edge(edge)
            .out
            .iter()
            .all(|output| runtime.assumed_new.contains(*output));
    let dirty = edge_data.recipe_begun
        || (edge_data.always_dirty && !assumed_new)
        || (runtime.always_make && !graph.is_phony_rule(edge_data.rule))
        || out_of_date;

    for output in &graph.edge(edge).out {
        runtime.node_mut(*output).set_dirty(dirty);
    }
    settle_searched_outputs(graph, runtime, edge, dirty);
    Ok(dirty)
}

#[derive(Default)]
struct DirtyEvaluator {
    nodes: VisitMarks,
    edges: VisitMarks,
    pushed: MarkSet,
}

impl DirtyEvaluator {
    fn begin(&mut self, graph: &Graph) {
        self.nodes.begin(graph.nodes.len());
        self.edges.begin(graph.edges.len());
    }
}

/// Traversal buffers reused across every scan of one build.
#[derive(Default)]
pub(crate) struct TraversalScratch {
    evaluator: DirtyEvaluator,
    seen_nodes: MarkSet,
    seen_edges: MarkSet,
}

impl DirtyEvaluator {
    fn evaluate<F>(
        &mut self,
        graph: &Graph,
        runtime: &mut RuntimeState,
        target: NodeId,
        stat: &mut F,
    ) -> Result<bool, GraphError>
    where
        F: FnMut(&Path) -> io::Result<i64>,
    {
        enum Work {
            Enter(NodeId),
            Finish(EdgeId),
        }

        let mut work = vec![Work::Enter(target)];
        // The nodes on the way down, so a cycle can be named by the path around
        // it rather than merely reported. One entry per active edge, holding
        // the node the edge was reached through.
        let mut path = Vec::new();
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => match self.nodes.get(node.index()) {
                    VisitState::Done => {}
                    VisitState::Active => {
                        return Err(cycle_through(graph, &path, node));
                    }
                    VisitState::New => {
                        // A name `-o` asserted a date for is where the walk
                        // stops. GNU Make's `-o` sets `updated`, `us_success`
                        // and `cs_finished` beside the date (main.c:2312), and
                        // `update_file_1` returns on `file->updated` before it
                        // looks at a recipe, a prerequisite or the switches —
                        // so the name is not remade, nothing beneath it is
                        // considered, `-B` does not reach it, and a name with
                        // no rule that is not there is no longer an error. All
                        // four measured against 4.4.1.
                        //
                        // The stamp is not enough on its own: a scan that
                        // descended would find a stale prerequisite, call the
                        // edge dirty and remake the very name the switch named.
                        if runtime.assumed_old.contains(node) {
                            if runtime.node(node).mtime().is_unobserved() {
                                runtime.node_mut(node).observe(FileTime::OLDEST);
                            }
                            runtime.node_mut(node).set_dirty(false);
                            self.nodes.set(node.index(), VisitState::Done);
                            continue;
                        }
                        let Some(edge) = graph.node(node).generator else {
                            if runtime.node(node).mtime().is_unobserved() {
                                nodestat_with(graph, runtime, node, stat)?;
                            }
                            let dirty = runtime.node(node).mtime().is_missing();
                            runtime.node_mut(node).set_dirty(dirty);
                            self.nodes.set(node.index(), VisitState::Done);
                            continue;
                        };

                        match self.edges.get(edge.index()) {
                            VisitState::Done => {
                                self.nodes.set(node.index(), VisitState::Done);
                                continue;
                            }
                            VisitState::Active => {
                                return Err(cycle_through(graph, &path, node));
                            }
                            VisitState::New => {}
                        }

                        self.edges.set(edge.index(), VisitState::Active);
                        let outputs: &[NodeId] = &graph.edge(edge).out;
                        if runtime.edge(edge).restat_clean() {
                            for &output in outputs {
                                runtime.node_mut(output).set_dirty(false);
                                self.nodes.set(output.index(), VisitState::Done);
                            }
                            self.edges.set(edge.index(), VisitState::Done);
                            continue;
                        }

                        if graph.deferred_freshness(edge).is_some() {
                            capture_deferred_freshness(graph, runtime, edge, stat)?;
                        }
                        for &output in outputs {
                            if !graph.is_virtual_output(output)
                                && runtime.node(output).mtime().is_unobserved()
                            {
                                nodestat_with(graph, runtime, output, stat)?;
                            }
                            self.nodes.set(output.index(), VisitState::Active);
                        }
                        work.push(Work::Finish(edge));
                        path.push(node);
                        let inputs: &[NodeId] = &graph.edge(edge).input;
                        for &input in inputs.iter().rev() {
                            work.push(Work::Enter(input));
                        }
                    }
                },
                Work::Finish(edge) => {
                    path.pop();
                    recompute_edge_dirty_with(graph, runtime, edge, stat)?;
                    let outputs: &[NodeId] = &graph.edge(edge).out;
                    for &output in outputs {
                        self.nodes.set(output.index(), VisitState::Done);
                    }
                    self.edges.set(edge.index(), VisitState::Done);
                }
            }
        }
        self.push_intermediates(graph, runtime, target);
        Ok(runtime.node(target).dirty())
    }
}

/// Names a cycle by the path around it, the way Ninja names one.
///
/// The cycle starts at the first node on the way down that shares a generating
/// edge with the one just met again. That node is reported as the node just
/// met, not as whichever output of the shared edge happened to be entered:
/// building `b` where `build a b: cat c` and `build c: cat a` reports
/// `a -> c -> a`, not `b -> c -> a`.
// [spec:ronin:req:compat.graph-semantics]
pub(super) fn cycle_through(graph: &Graph, path: &[NodeId], node: NodeId) -> GraphError {
    let Some(edge) = graph.node(node).generator else {
        return GraphError::DependencyCycle {
            node: Some(node),
            path: Vec::new(),
            phony_self_cycle: false,
        };
    };
    let start = path
        .iter()
        .position(|entry| graph.node(*entry).generator == Some(edge))
        .unwrap_or(path.len());
    let mut names = vec![BString::from(nodepath_bytes(graph, node, PathStyle::Raw))];
    names.extend(
        path[start.saturating_add(1)..]
            .iter()
            .map(|entry| BString::from(nodepath_bytes(graph, *entry, PathStyle::Raw))),
    );
    // CMake 2.8.12 and 3.0 emitted `build a: phony … a …`, and `-w phonycycle`
    // exists for that shape alone; a longer cycle, or one with implicit edges,
    // is an ordinary cycle whoever wrote it has to fix.
    let edge_data = graph.edge(edge);
    let phony_self_cycle = names.len() == 1
        && graph.is_phony_rule(edge_data.rule)
        && edge_data.out.len() == 1
        && edge_data.explicit_output_count() == edge_data.out.len()
        && edge_data.explicit_input_count() == edge_data.non_order_only_input_count();
    GraphError::DependencyCycle {
        node: Some(node),
        path: names,
        phony_self_cycle,
    }
}

/// Stat a dependency graph in one iterative pass and update each node's dirty bit.
// [spec:ronin:req:compat.graph-semantics]
#[cfg(test)]
pub(crate) fn recompute_dirty_with<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    node: NodeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let mut evaluator = DirtyEvaluator::default();
    evaluator.begin(graph);
    evaluator.evaluate(graph, runtime, node, stat)
}

pub(crate) fn recompute_dirty_with_validations<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    scratch: &mut TraversalScratch,
    node: NodeId,
    stat: &mut F,
) -> Result<Vec<NodeId>, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    enum Work {
        Enter(NodeId),
        EvaluateValidation(NodeId),
        RecordValidation(NodeId),
    }

    scratch.evaluator.begin(graph);
    scratch.evaluator.evaluate(graph, runtime, node, stat)?;
    scratch.seen_nodes.begin(graph.nodes.len());
    scratch.seen_edges.begin(graph.edges.len());
    let mut validations = Vec::new();
    let mut work = vec![Work::Enter(node)];
    while let Some(item) = work.pop() {
        match item {
            Work::Enter(node) => {
                let Some(edge) = graph.node(node).generator else {
                    continue;
                };
                if scratch.seen_edges.replace(edge.index()) {
                    continue;
                }
                let edge_validations: &[NodeId] = &graph.edge(edge).validation;
                for &validation in edge_validations.iter().rev() {
                    work.push(Work::EvaluateValidation(validation));
                }
                let inputs: &[NodeId] = &graph.edge(edge).input;
                for &input in inputs.iter().rev() {
                    work.push(Work::Enter(input));
                }
            }
            Work::EvaluateValidation(validation) => {
                if scratch.seen_nodes.replace(validation.index()) {
                    continue;
                }
                scratch
                    .evaluator
                    .evaluate(graph, runtime, validation, stat)?;
                work.push(Work::RecordValidation(validation));
                work.push(Work::Enter(validation));
            }
            Work::RecordValidation(validation) => validations.push(validation),
        }
    }
    Ok(validations)
}

// [spec:ronin:def:graph.nodeuse-fn]
// [spec:ronin:sem:graph.nodeuse-fn]
pub(crate) fn nodeuse(graph: &mut Graph, node: NodeId, edge: EdgeId) {
    graph.node_mut(node).uses.push(edge);
}

// [spec:ronin:def:graph.mkedge-fn]
// [spec:ronin:sem:graph.mkedge-fn]
// [spec:ronin:def:graph.mkphony-fn]
// [spec:ronin:sem:graph.mkphony-fn]
pub(crate) fn mkedge(graph: &mut Graph, scope: EnvironmentId) -> EdgeId {
    let id = EdgeId::from_index(graph.edges.len());
    graph.edges.push(Edge {
        rule: None,
        pool: None,
        env: scope,
        bindings: crate::names::Bindings::default(),
        out: IdVec::new(),
        input: IdVec::new(),
        validation: IdVec::new(),
        dyndep: None,
        always_dirty: false,
        intermediate: false,
        disposable: false,
        outputs_unaliased: false,
        outputs_low_resolution: false,
        outputs_reobserved: false,
        recipe_begun: false,
        freshness_history: FreshnessHistory::default(),
        partitions: EdgePartitions::default(),
    });
    id
}

// [spec:ronin:def:graph.edgehash-fn]
// [spec:ronin:sem:graph.edgehash-fn]
pub(crate) fn edgehash(
    runtime: &mut RuntimeState,
    edge: EdgeId,
    command: &BStr,
    rspfile_content: Option<&BStr>,
) -> CommandHash {
    if let Some(cached) = runtime.edge(edge).command_hash() {
        return cached;
    }
    let hash = rspfile_content.filter(|rsp| !rsp.is_empty()).map_or_else(
        || rapidhashv1(command.as_bytes()),
        |rsp| rapidhashv1(&[command.as_bytes(), b";rspfile=", rsp.as_bytes()][..]),
    );
    let hash = CommandHash::from_raw(hash);
    runtime.edge_mut(edge).set_command_hash(hash);
    hash
}

// [spec:ronin:def:graph.edgeadddeps-fn]
// [spec:ronin:sem:graph.edgeadddeps-fn]
pub(crate) fn edgeadddeps(graph: &mut Graph, edge: EdgeId, deps: &[NodeId]) {
    for node in deps {
        nodeuse(graph, *node, edge);
    }
    graph.edge_mut(edge).insert_implicit_inputs(deps);
}

/// Return generated outputs that are not consumed by another build edge.
pub(crate) fn rootnodes(graph: &Graph) -> Result<Vec<NodeId>, GraphError> {
    let roots = graph
        .node_ids()
        .filter(|node| {
            let node = graph.node(*node);
            node.generator.is_some() && node.uses.is_empty()
        })
        .collect::<Vec<_>>();
    if roots.is_empty() && graph.edge_count() != 0 {
        Err(GraphError::NoRootNodes)
    } else {
        Ok(roots)
    }
}

#[derive(Default)]
pub(crate) struct InputsCollector {
    inputs: Vec<NodeId>,
    visited_nodes: Vec<bool>,
}

impl InputsCollector {
    pub(crate) fn visit_node(&mut self, graph: &Graph, node: NodeId) {
        enum Work {
            Enter(NodeId),
            Record(NodeId),
        }

        self.visited_nodes.resize(graph.nodes.len(), false);
        let mut work = Vec::new();
        if let Some(edge) = graph.node(node).generator {
            for input in graph.edge(edge).input.iter().rev() {
                work.push(Work::Enter(*input));
            }
        }
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(input) => {
                    if std::mem::replace(&mut self.visited_nodes[input.index()], true) {
                        continue;
                    }
                    work.push(Work::Record(input));
                    if let Some(edge) = graph.node(input).generator {
                        for child in graph.edge(edge).input.iter().rev() {
                            work.push(Work::Enter(*child));
                        }
                    }
                }
                Work::Record(input) => {
                    let generated_by_phony = graph
                        .node(input)
                        .generator
                        .is_some_and(|edge| graph.is_phony_rule(graph.edge(edge).rule));
                    if !generated_by_phony {
                        self.inputs.push(input);
                    }
                }
            }
        }
    }

    pub(crate) fn input_strings(&self, graph: &Graph, style: PathStyle) -> Vec<BString> {
        self.inputs
            .iter()
            .map(|node| BString::from(nodepath_bytes(graph, *node, style)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn reset(&mut self) {
        self.inputs.clear();
        self.visited_nodes.fill(false);
    }
}

#[derive(Default)]
pub(crate) struct CommandCollector {
    pub(crate) edges: Vec<EdgeId>,
    visited_nodes: Vec<bool>,
    visited_edges: Vec<bool>,
}

impl CommandCollector {
    pub(crate) fn collect_from(&mut self, graph: &Graph, node: NodeId) {
        enum Work {
            Enter(NodeId),
            Record(EdgeId),
        }

        self.visited_nodes.resize(graph.nodes.len(), false);
        self.visited_edges.resize(graph.edges.len(), false);
        let mut work = vec![Work::Enter(node)];
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => {
                    if std::mem::replace(&mut self.visited_nodes[node.index()], true) {
                        continue;
                    }
                    let Some(edge) = graph.node(node).generator else {
                        continue;
                    };
                    if std::mem::replace(&mut self.visited_edges[edge.index()], true) {
                        continue;
                    }
                    work.push(Work::Record(edge));
                    for input in graph.edge(edge).input.iter().rev() {
                        work.push(Work::Enter(*input));
                    }
                }
                Work::Record(edge) => {
                    if !graph.is_phony_rule(graph.edge(edge).rule) {
                        self.edges.push(edge);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::names::Names;
    use crate::util::xasprintf;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_GRAPH: AtomicUsize = AtomicUsize::new(0);

    fn parse_graph(source: &str) -> Graph {
        let path = std::env::temp_dir().join(format!(
            "ronin-graph-test-{}-{}.ninja",
            std::process::id(),
            NEXT_GRAPH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            format!("rule cat\n  command = cat $in > $out\n{source}"),
        )
        .unwrap();
        let graph = crate::parse::load_manifest_in(
            path.to_str().unwrap(),
            crate::os::WorkingDirectory::default(),
            crate::frontend::ManifestOptions::default(),
        )
        .unwrap()
        .graph
        .into_arenas();
        fs::remove_file(path).unwrap();
        graph
    }

    #[test]
    fn arena_identifiers_are_niche_packed_and_index_ordered() {
        use std::mem::size_of;

        assert_eq!(size_of::<NodeId>(), 4);
        assert_eq!(size_of::<EdgeId>(), 4);
        // The niche is what shrinks Node.generator, Edge.rule, Edge.pool, and
        // Edge.dyndep from sixteen bytes to four.
        assert_eq!(size_of::<Option<NodeId>>(), 4);
        assert_eq!(size_of::<Option<EdgeId>>(), 4);

        assert_eq!(NodeId::from_index(0).index(), 0);
        assert_eq!(
            NodeId::from_index(u32::MAX as usize - 1).index(),
            u32::MAX as usize - 1
        );

        // The scheduler's ready heap orders edges by Reverse(EdgeId), so the
        // shifted encoding must keep comparing by index.
        let ids = (0..8).map(EdgeId::from_index).collect::<Vec<_>>();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn node_index_interns_across_growth_and_collisions() {
        const PATHS: usize = 2_000;

        let mut graph = Graph::default();
        let mut ids = Vec::new();
        for index in 0..PATHS {
            ids.push(mknode(&mut graph, xasprintf(format_args!("out/{index}.o"))));
        }
        // A byte path that is not valid UTF-8 must intern like any other.
        let raw = mknode(&mut graph, BString::from(b"out/\xff.o".as_slice()));

        // Growth rehashes every occupied slot, so every path must still map to
        // its original node and re-interning must not allocate a new one.
        assert_eq!(graph.node_ids().len(), PATHS + 1);
        for (index, id) in ids.iter().enumerate() {
            let path = xasprintf(format_args!("out/{index}.o"));
            assert_eq!(nodeget(&graph, path.as_bytes()), Some(*id));
            assert_eq!(mknode(&mut graph, path), *id);
        }
        assert_eq!(nodeget(&graph, b"out/\xff.o"), Some(raw));
        assert_eq!(nodeget(&graph, b"absent"), None);
        assert_eq!(graph.node_ids().len(), PATHS + 1);
    }

    #[test]
    fn unquoted_paths_do_not_store_a_second_copy() {
        let mut graph = Graph::default();
        let plain = mknode(&mut graph, xasprintf(format_args!("src/main.c")));
        let quoted = mknode(&mut graph, xasprintf(format_args!("src/a b.c")));

        // The common case renders identically in both styles from one buffer.
        assert_eq!(graph.node_shellpath(plain), graph.node_path(plain));
        assert_eq!(nodepath_bytes(&graph, plain, PathStyle::Raw), b"src/main.c");
        assert_eq!(
            nodepath_bytes(&graph, plain, PathStyle::ShellEscaped),
            b"src/main.c"
        );
        assert_ne!(graph.node_shellpath(quoted), graph.node_path(quoted));
        assert_eq!(
            nodepath_bytes(&graph, quoted, PathStyle::ShellEscaped),
            b"'src/a b.c'"
        );
    }

    #[test]
    fn interns_nodes_and_quotes_shell_paths() {
        let mut graph = Graph::default();
        let first = mknode(&mut graph, xasprintf(format_args!("a b")));
        let second = mknode(&mut graph, xasprintf(format_args!("a b")));
        assert_eq!(first, second);
        assert_eq!(
            nodepath_bytes(&graph, first, PathStyle::ShellEscaped),
            b"'a b'"
        );
    }

    #[test]
    fn ninja_shell_path_escaping_torture_case() {
        let mut graph = Graph::default();
        let node = mknode(
            &mut graph,
            xasprintf(format_args!("foo bar\"/'$@d!st!c'/path'")),
        );
        let path = nodepath_bytes(&graph, node, PathStyle::ShellEscaped);
        assert_eq!(
            std::str::from_utf8(path).unwrap(),
            "'foo bar\"/'\\''$@d!st!c'\\''/path'\\'''"
        );
    }

    fn generated_node(
        graph: &mut Graph,
        root: EnvironmentId,
        output: &str,
        inputs: &[&str],
    ) -> NodeId {
        let output = mknode(graph, xasprintf(format_args!("{output}")));
        let edge = mkedge(graph, root);
        graph.edge_mut(edge).out.push(output);
        for input in inputs {
            let input = mknode(graph, xasprintf(format_args!("{input}")));
            nodeuse(graph, input, edge);
            graph.edge_mut(edge).input.push(input);
        }
        let input_count = graph.edge(edge).input.len();
        graph
            .edge_mut(edge)
            .set_input_partitions(input_count, input_count);
        graph.node_mut(output).generator = Some(edge);
        output
    }

    fn scan_graph(
        graph: &Graph,
        node: NodeId,
        mtimes: &[(&str, i64)],
        stats: &mut Vec<String>,
    ) -> Result<RuntimeState, GraphError> {
        let mut runtime = RuntimeState::new(graph);
        let mtimes = mtimes
            .iter()
            .map(|(path, mtime)| (path.to_string(), *mtime))
            .collect::<BTreeMap<_, _>>();
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy().into_owned();
            stats.push(path.clone());
            Ok(*mtimes.get(&path).unwrap_or(&0))
        };
        nodestat_with(graph, &mut runtime, node, &mut stat)?;
        recompute_dirty_with(graph, &mut runtime, node, &mut stat)?;
        Ok(runtime)
    }

    #[test]
    fn ninja_stat_scan_simple() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "in"]);
    }

    #[test]
    fn ninja_stat_scan_two_step() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid"]);
        let middle = generated_node(&mut graph, root, "mid", &["in"]);
        let mut stats = Vec::new();
        let runtime = scan_graph(&graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "mid", "in"]);
        assert!(runtime.node(output).dirty());
        assert!(runtime.node(middle).dirty());
    }

    #[test]
    fn ninja_stat_scan_tree() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid1", "mid2"]);
        let middle1 = generated_node(&mut graph, root, "mid1", &["in11", "in12"]);
        generated_node(&mut graph, root, "mid2", &["in21", "in22"]);
        let mut stats = Vec::new();
        let runtime = scan_graph(&graph, output, &[], &mut stats).unwrap();
        assert_eq!(
            stats,
            ["out", "mid1", "in11", "in12", "mid2", "in21", "in22"]
        );
        assert!(runtime.node(middle1).dirty());
    }

    #[test]
    // [spec:ronin:req:compat.graph-semantics/test]
    fn ronin_deep_graph_evaluation_uses_an_iterative_worklist() {
        const DEPTH: usize = 20_000;

        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let mut input = "source".to_owned();
        let mut target = None;
        for index in 0..DEPTH {
            let output = format!("node/{index}");
            target = Some(generated_node(&mut graph, root, &output, &[&input]));
            input = output;
        }

        let mut stat_count = 0;
        let mut stat = |_path: &Path| {
            stat_count += 1;
            Ok(0)
        };
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, target.unwrap(), &mut stat).unwrap());
        assert_eq!(stat_count, DEPTH + 1);
    }

    #[test]
    fn ninja_stat_scan_middle_missing() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid"]);
        let middle = generated_node(&mut graph, root, "mid", &["in"]);
        let input = nodeget(&graph, b"in").unwrap();
        let mut stats = Vec::new();
        let runtime = scan_graph(
            &graph,
            output,
            &[("in", 1), ("mid", 0), ("out", 1)],
            &mut stats,
        )
        .unwrap();
        assert!(!runtime.node(input).dirty());
        assert!(runtime.node(middle).dirty());
        assert!(runtime.node(output).dirty());
    }

    #[test]
    fn ninja_state_basic_command_evaluation() {
        fn text(
            value: &str,
            next: Option<Box<crate::util::EvalString>>,
        ) -> crate::util::EvalString {
            let mut result = crate::util::EvalString::literal(value);
            if let Some(next) = next {
                result.parts.extend(next.parts);
            }
            result
        }

        fn variable(
            name: crate::names::VarId,
            next: Option<Box<crate::util::EvalString>>,
        ) -> crate::util::EvalString {
            let mut result = crate::util::EvalString::variable(name);
            if let Some(next) = next {
                result.parts.extend(next.parts);
            }
            result
        }

        let mut graph = Graph::default();
        let state = crate::env::EnvState::new(&mut graph);
        let rule = crate::env::mkrule(&mut graph, "cat".into());
        let command = text(
            "cat ",
            Some(Box::new(variable(
                crate::names::Names::IN,
                Some(Box::new(text(
                    " > ",
                    Some(Box::new(variable(crate::names::Names::OUT, None))),
                ))),
            ))),
        );
        let command_name = graph.names_mut().intern(BStr::new("command"));
        crate::env::ruleaddvar(&mut graph, rule, command_name, command);

        let edge = mkedge(&mut graph, state.root);
        graph.edge_mut(edge).rule = Some(rule);
        let input1 = mknode(&mut graph, xasprintf(format_args!("in1")));
        let input2 = mknode(&mut graph, xasprintf(format_args!("in2")));
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        {
            let edge = graph.edge_mut(edge);
            edge.input.extend([input1, input2]);
            edge.set_input_partitions(2, 2);
            edge.out.push(output);
            edge.set_explicit_output_count(1);
        }
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(command.as_bytes(), b"cat in1 in2 > out");
    }

    #[test]
    fn ninja_graph_root_nodes() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\n",
        );
        let roots = rootnodes(&graph).unwrap();
        assert_eq!(roots.len(), 4);
        assert!(
            roots
                .iter()
                .all(|node| graph.node_path(*node).as_bytes().starts_with(b"out"))
        );
    }

    #[test]
    fn ninja_graph_inputs_collector() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\nbuild all: phony out1 out2 out3\n",
        );
        let mut collector = InputsCollector::default();
        collector.visit_node(&graph, nodeget(&graph, b"out1").unwrap());
        assert_eq!(collector.input_strings(&graph, PathStyle::Raw), ["in1"]);
        collector.visit_node(&graph, nodeget(&graph, b"out2").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "mid1"]
        );
        collector.visit_node(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "mid1", "out1", "out2", "out3"]
        );

        collector.reset();
        collector.visit_node(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "out1", "mid1", "out2", "out3"]
        );
    }

    #[test]
    fn ninja_graph_inputs_collector_with_escapes() {
        let graph =
            parse_graph("build out$ 1: cat in1 in2 in$ with$ space | implicit || order_only\n");
        let mut collector = InputsCollector::default();
        collector.visit_node(&graph, nodeget(&graph, b"out 1").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "in2", "in with space", "implicit", "order_only"]
        );
        assert_eq!(
            collector.input_strings(&graph, PathStyle::ShellEscaped),
            ["in1", "in2", "'in with space'", "implicit", "order_only"]
        );
    }

    fn commands(graph: &Graph, collector: &CommandCollector) -> Vec<String> {
        collector
            .edges
            .iter()
            .map(|edge| {
                let command =
                    crate::env::edgevar(graph, *edge, Names::COMMAND, PathStyle::Raw).unwrap();
                String::from_utf8_lossy(command.as_bytes()).into_owned()
            })
            .collect()
    }

    fn recompute_state_with_mtimes(
        graph: &Graph,
        target: &[u8],
        mtimes: &[(&str, i64)],
    ) -> Result<(bool, RuntimeState), GraphError> {
        let mut runtime = RuntimeState::new(graph);
        let mtimes = mtimes
            .iter()
            .map(|(path, mtime)| (path.to_string(), *mtime))
            .collect::<BTreeMap<_, _>>();
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy();
            Ok(*mtimes.get(path.as_ref()).unwrap_or(&0))
        };
        let dirty = recompute_dirty_with(
            graph,
            &mut runtime,
            nodeget(graph, target).unwrap(),
            &mut stat,
        )?;
        Ok((dirty, runtime))
    }

    fn recompute_with_mtimes(
        graph: &Graph,
        target: &[u8],
        mtimes: &[(&str, i64)],
    ) -> Result<bool, GraphError> {
        recompute_state_with_mtimes(graph, target, mtimes).map(|(dirty, _)| dirty)
    }

    #[test]
    fn ninja_graph_command_collector() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\nbuild all: phony out1 out2 out3\n",
        );
        let mut collector = CommandCollector::default();
        collector.collect_from(&graph, nodeget(&graph, b"out2").unwrap());
        assert_eq!(
            commands(&graph, &collector),
            ["cat in1 > mid1", "cat mid1 > out2"]
        );
        collector.collect_from(&graph, nodeget(&graph, b"out1").unwrap());
        assert_eq!(
            commands(&graph, &collector),
            ["cat in1 > mid1", "cat mid1 > out2", "cat in1 > out1"]
        );
        collector.collect_from(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            commands(&graph, &collector),
            [
                "cat in1 > mid1",
                "cat mid1 > out2",
                "cat in1 > out1",
                "cat mid1 > out3 out4"
            ]
        );

        let mut collector = CommandCollector::default();
        collector.collect_from(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            commands(&graph, &collector),
            [
                "cat in1 > out1",
                "cat in1 > mid1",
                "cat mid1 > out2",
                "cat mid1 > out3 out4"
            ]
        );
    }

    #[test]
    fn ninja_graph_variable_paths_are_shell_escaped() {
        let graph = parse_graph("build a$ b: cat no'space with$ space$$ no\"space2\n");
        let edge = nodeget(&graph, b"a b").unwrap();
        let edge = graph.node(edge).generator.unwrap();
        let command =
            crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::ShellEscaped).unwrap();
        assert_eq!(
            command.as_bytes(),
            b"cat 'no'\\''space' 'with space$' 'no\"space2' > 'a b'"
        );
    }

    #[test]
    fn ninja_graph_rule_variables_are_in_scope() {
        let graph = parse_graph(
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n",
        );
        let edge = nodeget(&graph, b"out").unwrap();
        let edge = graph.node(edge).generator.unwrap();
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(command.as_bytes(), b"depfile is x");
    }

    #[test]
    fn ninja_graph_edge_binding_overrides_rule_binding() {
        let graph = parse_graph(
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n  depfile = y\n",
        );
        let edge = nodeget(&graph, b"out").unwrap();
        let edge = graph.node(edge).generator.unwrap();
        let depfile = crate::env::edgevar(&graph, edge, Names::DEPFILE, PathStyle::Raw).unwrap();
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(depfile.as_bytes(), b"y");
        assert_eq!(command.as_bytes(), b"depfile is y");
    }

    #[test]
    fn ninja_graph_missing_implicit_input_is_dirty() {
        let graph = parse_graph("build out: cat in | implicit\n");
        assert!(recompute_with_mtimes(&graph, b"out", &[("in", 1), ("out", 1)]).unwrap());
    }

    #[test]
    fn ninja_graph_modified_implicit_input_is_dirty() {
        let graph = parse_graph("build out: cat in | implicit\n");
        assert!(
            recompute_with_mtimes(&graph, b"out", &[("in", 1), ("out", 1), ("implicit", 2)])
                .unwrap()
        );
    }

    #[test]
    fn ninja_graph_newer_order_only_input_is_clean() {
        let graph = parse_graph("build out: cat in || order_only\n");
        assert!(
            !recompute_with_mtimes(&graph, b"out", &[("in", 1), ("out", 1), ("order_only", 2)])
                .unwrap()
        );
    }

    #[test]
    fn ninja_graph_missing_implicit_output_dirties_all_outputs() {
        let graph = parse_graph("build out | out.imp: cat in\n");
        let (dirty, runtime) =
            recompute_state_with_mtimes(&graph, b"out", &[("in", 1), ("out", 1)]).unwrap();
        assert!(dirty);
        assert!(runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert!(runtime.node(nodeget(&graph, b"out.imp").unwrap()).dirty());
    }

    #[test]
    fn ninja_graph_old_implicit_output_dirties_all_outputs() {
        let graph = parse_graph("build out | out.imp: cat in\n");
        let (dirty, runtime) =
            recompute_state_with_mtimes(&graph, b"out", &[("out.imp", 1), ("in", 2), ("out", 2)])
                .unwrap();
        assert!(dirty);
        assert!(runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert!(runtime.node(nodeget(&graph, b"out.imp").unwrap()).dirty());
    }

    #[test]
    fn ninja_graph_implicit_only_output_missing() {
        let graph = parse_graph("build | out.imp: cat in\n");
        assert!(recompute_with_mtimes(&graph, b"out.imp", &[("in", 1)]).unwrap());
    }

    #[test]
    fn ninja_graph_implicit_only_output_outdated() {
        let graph = parse_graph("build | out.imp: cat in\n");
        assert!(recompute_with_mtimes(&graph, b"out.imp", &[("out.imp", 1), ("in", 2)]).unwrap());
    }

    #[test]
    fn ninja_graph_validation_is_scanned_separately() {
        let graph = parse_graph("build out: cat in |@ validate\nbuild validate: cat in\n");
        let mtimes = BTreeMap::from([("in".to_owned(), 1)]);
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy();
            Ok(*mtimes.get(path.as_ref()).unwrap_or(&0))
        };
        let output = nodeget(&graph, b"out").unwrap();
        let mut runtime = RuntimeState::new(&graph);
        let validations = recompute_dirty_with_validations(
            &graph,
            &mut runtime,
            &mut TraversalScratch::default(),
            output,
            &mut stat,
        )
        .unwrap();
        assert_eq!(validations.len(), 1);
        assert!(runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert!(runtime.node(nodeget(&graph, b"validate").unwrap()).dirty());
    }

    #[test]
    fn ninja_graph_phony_dependency_propagates_mtime() {
        let graph = parse_graph("build in_ph: phony in1\nbuild out1: cat in_ph\n");
        assert!(!recompute_with_mtimes(&graph, b"out1", &[("in1", 1), ("out1", 2)]).unwrap());
        assert!(recompute_with_mtimes(&graph, b"out1", &[("in1", 3), ("out1", 2)]).unwrap());
    }

    /// An edge that declares itself never up to date is dirty however the
    /// filesystem and the build log answer, and its outputs carry that to
    /// whatever consumes them — which is what `.PHONY` means and what neither
    /// an mtime nor a recorded entry can say.
    // [spec:ronin:req:make.phony-always-dirty/test]
    #[test]
    fn ronin_graph_an_always_dirty_edge_outranks_mtimes_and_the_build_log() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["in"]);
        let consumer = generated_node(&mut graph, root, "downstream", &["out"]);
        let mtimes = [("in", 1), ("out", 2), ("downstream", 3)];

        // Newer than its input, which is the whole of what the filesystem has
        // to say about it.
        let (dirty, _) = recompute_state_with_mtimes(&graph, b"downstream", &mtimes).unwrap();
        assert!(!dirty);

        let edge = graph.node(output).generator.unwrap();
        graph.edge_mut(edge).always_dirty = true;
        let mtimes = BTreeMap::from_iter(mtimes.map(|(path, mtime)| (path.to_owned(), mtime)));
        let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        // And recorded by an earlier build with the mtime it still has, which
        // is the whole of what the build log has to say about it.
        runtime
            .node_mut(output)
            .set_log_mtime(FileTime::observed(2));
        assert!(recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap());
        assert!(runtime.node(output).dirty());
    }

    /// The two senses of phony are independent: an alias for its inputs can
    /// also be one that is never up to date, and declaring the second must not
    /// stop the first from settling the mtime a consumer compares against.
    // [spec:ronin:req:make.phony-always-dirty/test]
    #[test]
    fn ronin_graph_an_always_dirty_phony_edge_still_propagates_its_input_mtime() {
        let mut graph = parse_graph("build in_ph: phony in1\nbuild out1: cat in_ph\n");
        let alias_output = nodeget(&graph, b"in_ph").unwrap();
        let consumer = nodeget(&graph, b"out1").unwrap();
        let alias = graph.node(alias_output).generator.unwrap();
        graph.edge_mut(alias).always_dirty = true;

        let mtimes = BTreeMap::from([("in1".to_owned(), 1), ("out1".to_owned(), 2)]);
        let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap());
        assert!(runtime.node(alias_output).dirty());
        assert_eq!(runtime.node(alias_output).mtime(), FileTime::observed(1));
    }

    /// A Makefile target whose recipe writes nothing has the shape of an alias
    /// and the opposite meaning: GNU Make ran the recipe, found the name still
    /// absent, and remakes what reads it for exactly that reason. The alias in
    /// the same graph is the control — nothing about it changes.
    #[test]
    fn ronin_graph_empty_recipe_stays_absent() {
        let mut graph =
            parse_graph("build mid: phony src\nbuild alias: phony src\nbuild out: cat mid alias\n");
        let middle = nodeget(&graph, b"mid").unwrap();
        let aliased = nodeget(&graph, b"alias").unwrap();
        let consumer = nodeget(&graph, b"out").unwrap();
        let empty_recipe = graph.node(middle).generator.unwrap();

        // As an alias both stand in for `src`, so `out` is newer than either
        // and there is nothing to do.
        let mtimes = BTreeMap::from([("src".to_owned(), 1), ("out".to_owned(), 2)]);
        let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        assert!(!recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap());
        assert_eq!(runtime.node(middle).mtime(), FileTime::observed(1));

        graph.edge_mut(empty_recipe).outputs_unaliased = true;
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap());
        assert!(runtime.node(middle).dirty());
        assert_eq!(runtime.node(middle).mtime(), FileTime::MISSING);
        assert!(!runtime.node(aliased).dirty());
        assert_eq!(runtime.node(aliased).mtime(), FileTime::observed(1));
    }

    /// A target the front end found somewhere else is read there while nothing
    /// has put it here, and read here the moment something has — which is
    /// exactly the order `f_mtime` asks in, and is what decides whether the
    /// edge that makes it has anything to do.
    #[test]
    fn ronin_graph_searched_output_reads_elsewhere() {
        let mut graph = parse_graph("build out.o: cat out.c\n");
        let output = nodeget(&graph, b"out.o").unwrap();
        graph.set_searched_at(output, BString::from("build/out.o"));

        // Nothing at `out.o`, a newer copy at `build/out.o`: the edge has
        // nothing to do and the copy's date is the one the target has.
        let found = BTreeMap::from([
            ("out.c".to_owned(), 1),
            ("build/out.o".to_owned(), 2),
            ("out.o".to_owned(), 0),
        ]);
        let mut stat = |path: &Path| Ok(*found.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        assert!(!recompute_dirty_with(&graph, &mut runtime, output, &mut stat).unwrap());
        assert_eq!(runtime.node(output).mtime(), FileTime::observed(2));
        assert!(crate::graph::found_name_stands(&runtime, output));

        // The same copy, older than the source: the edge has to run, and the
        // found name is not the one anything will read.
        let stale = BTreeMap::from([("out.c".to_owned(), 3), ("build/out.o".to_owned(), 2)]);
        let mut stat = |path: &Path| Ok(*stale.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, output, &mut stat).unwrap());
        assert!(!crate::graph::found_name_stands(&runtime, output));

        // And a file that really is here is read here, whatever the search
        // found: the second place is only ever a second place.
        let here = BTreeMap::from([
            ("out.c".to_owned(), 1),
            ("build/out.o".to_owned(), 9),
            ("out.o".to_owned(), 5),
        ]);
        let mut stat = |path: &Path| Ok(*here.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        assert!(!recompute_dirty_with(&graph, &mut runtime, output, &mut stat).unwrap());
        assert_eq!(runtime.node(output).mtime(), FileTime::observed(5));
    }

    /// The build having written the target here is the one answer a later scan
    /// may not take back. The scan after the work reads the file the work
    /// wrote and would say the target was current all along.
    #[test]
    fn ronin_graph_written_name_is_final() {
        let mut graph = parse_graph("build out.o: cat out.c\n");
        let output = nodeget(&graph, b"out.o").unwrap();
        let edge = graph.node(output).generator.unwrap();
        graph.set_searched_at(output, BString::from("build/out.o"));

        let stale = BTreeMap::from([("out.c".to_owned(), 3), ("build/out.o".to_owned(), 2)]);
        let mut stat = |path: &Path| Ok(*stale.get(&*path.to_string_lossy()).unwrap_or(&0));
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, output, &mut stat).unwrap());
        crate::graph::mark_written_here(&graph, &mut runtime, edge);

        // What the build left behind: the edge ran and the target is here, and
        // the run re-observes it exactly as `outputs_reobserved` says.
        runtime.node_mut(output).observe(FileTime::observed(4));
        let made = BTreeMap::from([("out.c".to_owned(), 3), ("out.o".to_owned(), 4)]);
        let mut stat = |path: &Path| Ok(*made.get(&*path.to_string_lossy()).unwrap_or(&0));
        assert!(!recompute_edge_dirty_with(&graph, &mut runtime, edge, &mut stat).unwrap());
        assert!(!crate::graph::found_name_stands(&runtime, output));
    }

    /// A whole-second record dates a file a fraction before the thing it was
    /// copied from, and GNU Make rounds that up where the file is the one being
    /// updated and nowhere else. Both readings are in this graph at once: `mid`
    /// is `src`'s copy and is up to date, and `out`, which reads `mid` as a
    /// prerequisite, is not.
    #[test]
    fn ronin_graph_low_resolution_rounds_once() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let middle = generated_node(&mut graph, root, "mid", &["src"]);
        let consumer = generated_node(&mut graph, root, "out", &["mid"]);
        let producer = graph.node(middle).generator.unwrap();

        // `mid` is dated to the start of the second `src` was written part way
        // through, and `out` was written a whole second before either.
        let second = 1_700_000_000_000_000_000;
        let mtimes = BTreeMap::from([
            ("src".to_owned(), second + 700_000_000),
            ("mid".to_owned(), second),
            ("out".to_owned(), second - 1_000_000_000),
        ]);
        let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));

        // Read plainly, `mid` is older than the thing it is a copy of.
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap());
        assert!(runtime.node(middle).dirty());

        graph.edge_mut(producer).outputs_low_resolution = true;
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap());
        assert!(
            !runtime.node(middle).dirty(),
            "the round-up applies where the file is the one being made"
        );
        assert_eq!(
            runtime.node(middle).mtime(),
            FileTime::observed(second),
            "and nowhere else: what reads it as a prerequisite sees the plain date"
        );
    }

    /// GNU Make's intermediate file: one the implicit rule search invented, so
    /// its absence says nothing. A consumer sees the newest thing behind it
    /// where the file itself would be, and only a consumer that has to run
    /// anyway asks for the file to exist at all.
    #[test]
    fn ronin_graph_an_absent_intermediate_is_made_only_for_a_consumer_that_must_run() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let middle = generated_node(&mut graph, root, "mid", &["src"]);
        let consumer = generated_node(&mut graph, root, "out", &["mid"]);
        let producer = graph.node(middle).generator.unwrap();
        graph.edge_mut(producer).intermediate = true;

        let settled = |mtimes: [(&str, i64); 2]| {
            let mtimes = BTreeMap::from_iter(mtimes.map(|(path, mtime)| (path.to_owned(), mtime)));
            let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
            let mut runtime = RuntimeState::new(&graph);
            let dirty = recompute_dirty_with(&graph, &mut runtime, consumer, &mut stat).unwrap();
            (dirty, runtime)
        };

        // `mid` is not there and nothing minds: `out` is newer than `src`, and
        // `mid` stands in for `src` rather than for a missing file.
        let (dirty, runtime) = settled([("src", 1), ("out", 2)]);
        assert!(!dirty);
        assert!(!runtime.node(middle).dirty());
        assert_eq!(runtime.node(middle).mtime(), FileTime::observed(1));

        // `src` moved ahead of `out`, so `out` has to run and now needs it.
        let (dirty, runtime) = settled([("src", 3), ("out", 2)]);
        assert!(dirty);
        assert!(runtime.node(middle).dirty());
    }

    #[test]
    fn ninja_graph_phony_output_with_validation_is_clean() {
        let graph = parse_graph("build valid: phony\nbuild out: phony |@ valid\n");
        let mut stat = |_path: &Path| Ok(0);
        let output = nodeget(&graph, b"out").unwrap();
        let mut runtime = RuntimeState::new(&graph);
        let validations = recompute_dirty_with_validations(
            &graph,
            &mut runtime,
            &mut TraversalScratch::default(),
            output,
            &mut stat,
        )
        .unwrap();
        assert!(!runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert_eq!(validations.len(), 1);
        assert_eq!(graph.node_path(validations[0]).as_bytes(), b"valid");
    }
}
