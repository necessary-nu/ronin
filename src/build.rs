//! Build scheduling state translated from `build.c`.

use crate::error::{BuildError, BuildOperation, BuildStop, ProcessError};
use crate::graph::{
    edgeadddeps, edgehash, nodestat_with, recompute_dirty_with_validations,
    recompute_edge_dirty_with, EdgeId, Graph, NodeId, PathStyle, TraversalScratch,
};
use crate::names::Names;
use crate::os::RealDiskInterface;
use crate::runtime::{FileTime, RuntimeState};
use crate::subprocess::{status_interrupted, ProcessOutput, ProcessSupervisor, SupervisorWake};
use crate::util::{BString, ByteSlice};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use self::command::{CommandSpec, DepsType, PreparedEdge, ResponseFile};
use self::reporter::Reporter;
pub(crate) use self::reporter::{ColorChoice, OutputGroup, OutputStyle, TerminalContext};
pub(crate) use self::status::BuildState;

type BuildResult<T> = Result<T, BuildError>;

pub(crate) use command::{DRY_RUN_COMMAND, IGNORE_ERRORS, RECIPE_LOCATION};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum JobLimit {
    #[default]
    Auto,
    Unlimited,
    Fixed(NonZeroUsize),
}

impl JobLimit {
    pub(crate) const fn fixed(jobs: usize) -> Option<Self> {
        match NonZeroUsize::new(jobs) {
            Some(jobs) => Some(Self::Fixed(jobs)),
            None => None,
        }
    }
}

// [spec:ronin:def:build.buildoptions]
#[derive(Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent Ninja CLI switches are clearer as named options than a synthetic state machine"
)]
pub(crate) struct BuildOptions {
    pub(crate) jobs: JobLimit,
    pub(crate) maxfail: usize,
    pub(crate) verbose: bool,
    pub(crate) explain: bool,
    pub(crate) stats: bool,
    pub(crate) keepdepfile: bool,
    pub(crate) keeprsp: bool,
    pub(crate) dryrun: bool,
    /// Make's `-t`: give each output a fresh timestamp instead of running the
    /// recipe that would have produced it.
    pub(crate) touch: bool,
    pub(crate) quiet: bool,
    /// Make's `--trace`: name the rule and the reason before each recipe runs.
    pub(crate) trace: bool,
    pub(crate) statusfmt: String,
    pub(crate) status_from_cli: bool,
    pub(crate) shell: crate::subprocess::ShellMode,
    pub(crate) style: OutputStyle,
    pub(crate) color: ColorChoice,
    pub(crate) terminal: TerminalContext,
    pub(crate) maxload: f64,
    /// Make's `.NOTPARALLEL`: one recipe at a time here, whatever budget this
    /// build holds or hands on. Local like `maxload`, not a smaller budget —
    /// clamping the budget would stop a jobserver being served at all, and a
    /// sub-make is meant to keep the full one.
    pub(crate) serial: bool,
    pub(crate) jobserver: Option<crate::jobserver::Transport>,
    /// Whether this build may create a jobserver of its own.
    ///
    /// Set when this build is not spending an inherited budget, which covers
    /// the top of a tree and a build that declined one — an explicit `-j`
    /// under a parent Make, which asks for that count below here as well and
    /// has to serve it to get it there.
    pub(crate) serve_jobserver: bool,
    /// Variables the front end imposes on every command it runs, beside
    /// whatever the jobserver publishes.
    ///
    /// Make mode counts recursion with one: a recipe is handed a `MAKELEVEL`
    /// one deeper than the makefile it came from, which is how a sub-make
    /// knows it is one.
    /// `None` removes the name instead of binding it, which is Make's
    /// `unexport` of a variable that arrived from outside.
    pub(crate) environment: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
    /// Set when Make's `-O` asked for each command's held output to be
    /// bracketed with the directory it was produced in.
    pub(crate) output_group: Option<OutputGroup>,
    /// The name a failed recipe's own line leads with, carrying the level.
    ///
    /// Make mode only. Ninja narrates a failure with the `FAILED:` block and a
    /// stopped line at the end; Make names the makefile line, the target and
    /// the recipe's status in one, and says nothing further. `None` leaves the
    /// Ninja shape, which is what a manifest build gets.
    pub(crate) recipe_failure: Option<String>,
    /// What an output the build log does not name says about whether it is
    /// current. The graph's front end decides it; [`crate::frontend::Build`]
    /// carries it here, and nothing else sets it.
    pub(crate) unrecorded_output: crate::frontend::UnrecordedOutput,
    pub(crate) working_directory: crate::os::WorkingDirectory,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            jobs: JobLimit::Auto,
            maxfail: 1,
            verbose: false,
            explain: false,
            stats: false,
            keepdepfile: false,
            keeprsp: false,
            dryrun: false,
            touch: false,
            quiet: false,
            trace: false,
            statusfmt: "[%f/%t] ".into(),
            status_from_cli: false,
            shell: crate::subprocess::ShellMode::default(),
            style: OutputStyle::Ninja,
            color: ColorChoice::Auto,
            terminal: TerminalContext::default(),
            maxload: 0.0,
            serial: false,
            jobserver: None,
            serve_jobserver: false,
            environment: Vec::new(),
            output_group: None,
            recipe_failure: None,
            unrecorded_output: crate::frontend::UnrecordedOutput::default(),
            working_directory: crate::os::WorkingDirectory::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EdgeResult {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct ReadyEdge {
    weight: CriticalPathWeight,
    edge: Reverse<EdgeId>,
}

impl ReadyEdge {
    const fn new(weight: CriticalPathWeight, edge: EdgeId) -> Self {
        Self {
            weight,
            edge: Reverse(edge),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
struct CriticalPathWeight(usize);

impl CriticalPathWeight {
    const ROOT: Self = Self(1);

    const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    const fn max(self, other: Self) -> Self {
        if self.0 >= other.0 {
            self
        } else {
            other
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
struct PoolOccupancy(usize);

impl PoolOccupancy {
    const fn has_capacity(self, depth: usize) -> bool {
        self.0 < depth
    }

    const fn acquire(&mut self) {
        self.0 += 1;
    }

    const fn release(&mut self) {
        debug_assert!(self.0 > 0);
        self.0 -= 1;
    }
}

#[derive(Default)]
pub(crate) struct Plan {
    wanted: Vec<bool>,
    wanted_count: usize,
    weight: Vec<CriticalPathWeight>,
    expanded_weight: Vec<CriticalPathWeight>,
    pending: Vec<usize>,
    dependents: Vec<Vec<EdgeId>>,
    ready: BinaryHeap<ReadyEdge>,
    running: Vec<bool>,
    completed: Vec<bool>,
    pool_occupancy: Vec<PoolOccupancy>,
    /// Which consuming edge last recorded a dependency on each generator.
    ///
    /// Deduplicates the edges pushed onto `dependents` within one rebuild.
    /// A niche-packed identifier makes the empty case free, so this is half
    /// the width of the index-plus-sentinel it replaced.
    dependency_marks: Vec<Option<EdgeId>>,
    completed_count: usize,
    failures: usize,
}

// [spec:ronin:req:compat.graph-semantics]
impl Plan {
    fn synchronize_arenas(&mut self, graph: &Graph) {
        let edge_count = graph.edge_count();
        self.wanted.resize(edge_count, false);
        self.weight
            .resize(edge_count, CriticalPathWeight::default());
        self.expanded_weight
            .resize(edge_count, CriticalPathWeight::default());
        self.pending.resize(edge_count, 0);
        self.dependents.resize_with(edge_count, Vec::new);
        self.running.resize(edge_count, false);
        self.completed.resize(edge_count, false);
        self.pool_occupancy
            .resize(graph.pool_count(), PoolOccupancy::default());
        self.dependency_marks.resize(edge_count, None);
    }

    // [spec:ronin:def:build.buildreset-fn]
    // [spec:ronin:sem:build.buildreset-fn]
    // [spec:ronin:def:build.isnewer-fn]
    // [spec:ronin:sem:build.isnewer-fn]
    // [spec:ronin:def:build.isdirty-fn]
    // [spec:ronin:sem:build.isdirty-fn]
    // [spec:ronin:def:build.queue-fn]
    // [spec:ronin:sem:build.queue-fn]
    // [spec:ronin:def:build.buildadd-fn]
    // [spec:ronin:sem:build.buildadd-fn]
    pub(crate) fn add_target(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        node: NodeId,
    ) -> BuildResult<()> {
        self.synchronize_arenas(graph);
        self.add_node(graph, runtime, node, CriticalPathWeight::ROOT)
    }

    fn add_node(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        node: NodeId,
        weight: CriticalPathWeight,
    ) -> BuildResult<()> {
        let mut work = vec![(node, weight, None)];
        while let Some((node, weight, needed_by)) = work.pop() {
            let Some(edge) = graph.node(node).gen else {
                if runtime.node(node).dirty() {
                    let path = graph.node_path(node).to_owned();
                    let needed_by = needed_by
                        .map(|needed_by| (needed_by, graph.node_path(needed_by).to_owned()));
                    return Err(BuildError::MissingInput {
                        node,
                        path,
                        needed_by,
                    });
                }
                continue;
            };
            let edge_dirty = graph
                .edge(edge)
                .out
                .iter()
                .any(|output| runtime.node(*output).dirty());
            let phony_with_no_inputs = {
                let edge = graph.edge(edge);
                graph.is_phony_rule(edge.rule) && edge.input.is_empty()
            };
            if edge_dirty && phony_with_no_inputs {
                continue;
            }

            if edge_dirty {
                let previous_weight = self.weight[edge.index()];
                let newly_wanted = !self.wanted[edge.index()];
                if !newly_wanted && weight <= previous_weight {
                    continue;
                }
                if newly_wanted {
                    self.wanted[edge.index()] = true;
                    self.wanted_count += 1;
                }
                self.weight[edge.index()] = weight.max(previous_weight);
            } else {
                if weight <= self.expanded_weight[edge.index()] {
                    continue;
                }
                self.expanded_weight[edge.index()] = weight;
            }

            let edge_id = edge;
            let edge = graph.edge(edge_id);
            let needed_by = edge.out.first().copied();
            let depfile_end = edge.non_order_only_input_count();
            let depfile_start =
                depfile_end.saturating_sub(runtime.edge(edge_id).depfile_dependencies());
            let inputs: &[NodeId] = &edge.input;
            for (index, &input) in inputs.iter().enumerate().rev() {
                if !(index >= depfile_start
                    && index < depfile_end
                    && graph.node(input).gen.is_none())
                {
                    work.push((input, weight.next(), needed_by));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_queue(&mut self, graph: &Graph) {
        self.synchronize_arenas(graph);
        self.running.fill(false);
        self.completed.fill(false);
        self.pool_occupancy.fill(PoolOccupancy::default());
        self.completed_count = 0;
        self.failures = 0;
        self.rebuild_frontier(graph);
    }

    fn rebuild_frontier(&mut self, graph: &Graph) {
        self.synchronize_arenas(graph);
        self.pending.fill(0);
        for dependents in &mut self.dependents {
            dependents.clear();
        }
        self.ready.clear();
        // Marks persist across rebuilds, so a stale mark from the previous
        // frontier would wrongly suppress a dependency; reset before reuse.
        self.dependency_marks.fill(None);
        for edge in graph.edge_ids() {
            let index = edge.index();
            if !self.wanted[index] || self.completed[index] {
                continue;
            }
            for input in graph.edge(edge).input.iter().copied() {
                let Some(generator) = graph.node(input).gen else {
                    continue;
                };
                if self.wanted[generator.index()]
                    && !self.completed[generator.index()]
                    && self.dependency_marks[generator.index()] != Some(edge)
                {
                    self.dependency_marks[generator.index()] = Some(edge);
                    self.pending[index] += 1;
                    self.dependents[generator.index()].push(edge);
                }
            }
        }
        for edge in graph.edge_ids() {
            let index = edge.index();
            if self.wanted[index]
                && !self.completed[index]
                && !self.running[index]
                && self.pending[index] == 0
            {
                self.ready.push(ReadyEdge::new(self.weight[index], edge));
            }
        }
    }

    pub(crate) fn refresh_dependencies(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
    ) -> BuildResult<()> {
        self.synchronize_arenas(graph);
        for edge in graph.edge_ids() {
            let index = edge.index();
            if !self.wanted[index] {
                continue;
            }
            let weight = self.weight[index];
            let inputs: &[NodeId] = &graph.edge(edge).input;
            for &input in inputs.iter().rev() {
                self.add_node(graph, runtime, input, weight.next())?;
            }
        }
        self.rebuild_frontier(graph);
        Ok(())
    }

    /// The side tables run parallel to the edge arena, so walking the arena
    /// alongside them keeps identifiers coming from the graph that owns them.
    pub(crate) fn wanted_edges(&self, graph: &Graph) -> Vec<EdgeId> {
        self.wanted
            .iter()
            .zip(graph.edge_ids())
            .filter_map(|(wanted, edge)| wanted.then_some(edge))
            .collect()
    }

    pub(crate) fn find_work(&mut self, graph: &Graph) -> Option<EdgeId> {
        let mut blocked = Vec::new();
        let edge = loop {
            let Some(candidate) = self.ready.pop() else {
                self.ready.extend(blocked);
                return None;
            };
            let edge = candidate.edge.0;
            if graph.edge(edge).pool.is_none_or(|pool| {
                let depth = graph
                    .pool(pool)
                    .depth()
                    .expect("validated pools have a depth")
                    .get();
                self.pool_occupancy[pool.index()].has_capacity(depth)
            }) {
                break edge;
            }
            blocked.push(candidate);
        };
        self.ready.extend(blocked);
        if let Some(pool) = graph.edge(edge).pool {
            self.pool_occupancy[pool.index()].acquire();
        }
        self.running[edge.index()] = true;
        Some(edge)
    }

    fn defer_work(&mut self, graph: &Graph, edge: EdgeId) {
        if std::mem::replace(&mut self.running[edge.index()], false) {
            if let Some(pool) = graph.edge(edge).pool {
                self.pool_occupancy[pool.index()].release();
            }
            self.ready
                .push(ReadyEdge::new(self.weight[edge.index()], edge));
        }
    }

    pub(crate) fn edge_finished(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        edge: EdgeId,
        result: EdgeResult,
    ) -> BuildResult<()> {
        if !std::mem::replace(&mut self.running[edge.index()], false) {
            return Err(BuildError::EdgeNotRunning { edge });
        }
        if let Some(pool) = graph.edge(edge).pool {
            self.pool_occupancy[pool.index()].release();
        }
        if !std::mem::replace(&mut self.completed[edge.index()], true) {
            self.completed_count += 1;
        }
        if result == EdgeResult::Failed {
            self.failures += 1;
            return Ok(());
        }
        self.release_dependents(graph, runtime, edge);
        Ok(())
    }

    fn release_dependents(&mut self, graph: &Graph, runtime: &RuntimeState, finished: EdgeId) {
        let mut work = vec![finished];
        while let Some(edge) = work.pop() {
            for index in 0..self.dependents[edge.index()].len() {
                let dependent = self.dependents[edge.index()][index];
                self.pending[dependent.index()] -= 1;
                if self.pending[dependent.index()] != 0 {
                    continue;
                }
                let dirty = graph
                    .edge(dependent)
                    .out
                    .iter()
                    .any(|output| runtime.node(*output).dirty());
                if dirty {
                    self.ready
                        .push(ReadyEdge::new(self.weight[dependent.index()], dependent));
                } else if !std::mem::replace(&mut self.completed[dependent.index()], true) {
                    self.completed_count += 1;
                    work.push(dependent);
                }
            }
        }
    }

    pub(crate) const fn more_to_do(&self) -> bool {
        self.failures != 0 || self.completed_count < self.wanted_count
    }

    pub(crate) fn command_edge_count(&self, graph: &Graph) -> usize {
        self.command_edges(graph).count()
    }

    /// Every planned edge that will actually run a command.
    pub(crate) fn command_edges<'a>(
        &'a self,
        graph: &'a Graph,
    ) -> impl Iterator<Item = EdgeId> + 'a {
        self.wanted
            .iter()
            .zip(graph.edge_ids())
            .filter(|(wanted, edge)| {
                let rule = graph.edge(*edge).rule;
                **wanted && rule.is_some() && !graph.is_phony_rule(rule)
            })
            .map(|(_, edge)| edge)
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.wanted_count == 0
    }
}

// [spec:ronin:def:build.job]
pub(crate) struct Builder<'a> {
    graph: &'a mut Graph,
    runtime: RuntimeState,
    options: BuildOptions,
    disk: RealDiskInterface,
    plan: Plan,
    scratch: TraversalScratch,
    /// Nodes awaiting an mtime, reused across targets by `prefetch_mtimes`.
    stat_targets: Vec<NodeId>,
    visited_edges: crate::graph::MarkSet,
    build_log: Option<&'a mut crate::log::BuildLog>,
    deps_log: Option<&'a mut crate::deps::DepsLog>,
    targets: Vec<NodeId>,
    executed_edges: BTreeSet<EdgeId>,
    command_cache: Vec<Option<CommandSpec>>,
    command_scratch: Vec<u8>,
    progress: BuildState,
    reporter: Reporter,
    /// Buffer every rendered line is built in, reused for the whole build.
    ///
    /// Rendering used to allocate a `String` for the status template and then
    /// a second `Vec` to splice the description into it, once per finished
    /// command. One reused buffer removes the second of those and makes the
    /// first the only allocation left on the path.
    status_scratch: Vec<u8>,
    output_sink: Option<&'a mut dyn Write>,
    diagnostic_sink: Option<&'a mut dyn Write>,
    explanations: Option<crate::explanations::Explanations>,
    explanations_recorded: Vec<bool>,
    explanations_emitted: Vec<bool>,
    pub(crate) commands_ran: Vec<BString>,
    pub(crate) command_output: Vec<u8>,
    pub(crate) build_output: Vec<u8>,
}

impl<'a> Builder<'a> {
    /// Builds over `graph`, writing through whichever logs and sinks the
    /// invocation has. All four are optional because a test, a library caller
    /// collecting output, and the command line each have a different subset.
    pub(crate) fn from_parts(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: Option<&'a mut crate::log::BuildLog>,
        deps_log: Option<&'a mut crate::deps::DepsLog>,
        output_sink: Option<&'a mut dyn Write>,
        diagnostic_sink: Option<&'a mut dyn Write>,
    ) -> Self {
        let progress = BuildState::new(options.clone());
        let options_style = options.style;
        let options_color = options.color.resolve(options.terminal);
        let disk = RealDiskInterface::new(options.working_directory.clone());
        let mut runtime = RuntimeState::new(graph);
        if let Some(log) = build_log.as_deref() {
            log.hydrate_runtime(graph, &mut runtime, graph.node_ids());
        }
        let explanations = options
            .explain
            .then(crate::explanations::Explanations::default);
        Self {
            graph,
            runtime,
            options,
            disk,
            plan: Plan::default(),
            scratch: TraversalScratch::default(),
            stat_targets: Vec::new(),
            visited_edges: crate::graph::MarkSet::default(),
            build_log,
            deps_log,
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            command_cache: Vec::new(),
            command_scratch: Vec::new(),
            progress,
            reporter: Reporter::new(options_style, options_color),
            status_scratch: Vec::new(),
            output_sink,
            diagnostic_sink,
            explanations,
            explanations_recorded: Vec::new(),
            explanations_emitted: Vec::new(),
            commands_ran: Vec::new(),
            command_output: Vec::new(),
            build_output: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(graph: &'a mut Graph, options: BuildOptions) -> Self {
        Self::from_parts(graph, options, None, None, None, None)
    }

    #[cfg(test)]
    pub(crate) fn with_output(
        graph: &'a mut Graph,
        options: BuildOptions,
        output: &'a mut dyn Write,
    ) -> Self {
        Self::from_parts(graph, options, None, None, Some(output), None)
    }

    #[cfg(test)]
    pub(crate) fn with_build_log(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
    ) -> Self {
        Self::from_parts(graph, options, Some(build_log), None, None, None)
    }

    #[cfg(test)]
    pub(crate) fn with_deps_log(
        graph: &'a mut Graph,
        options: BuildOptions,
        deps_log: &'a mut crate::deps::DepsLog,
    ) -> Self {
        Self::from_parts(graph, options, None, Some(deps_log), None, None)
    }

    fn synchronize_runtime(&mut self) {
        // `synchronize` reports the newly grown span as indices; take the
        // identifiers for it from the arena that just grew.
        let nodes = self.runtime.synchronize(self.graph);
        if let Some(log) = self.build_log.as_deref() {
            let added = self.graph.node_ids().skip(nodes.start);
            log.hydrate_runtime(self.graph, &mut self.runtime, added);
        }
    }

    fn replace_depfile_deps(&mut self, edge: EdgeId, deps: &[NodeId]) {
        self.synchronize_runtime();
        let previous_count = self.runtime.edge(edge).depfile_dependencies();
        self.graph
            .edge_mut(edge)
            .drain_discovered_inputs(previous_count);
        edgeadddeps(self.graph, edge, deps);
        self.runtime
            .edge_mut(edge)
            .set_depfile_dependencies(deps.len());
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the iterative enter/load/refresh traversal is one explicit dependency-loading state machine"
    )]
    fn load_depfiles_for(&mut self, target: NodeId) -> BuildResult<()> {
        enum Work {
            Enter(NodeId),
            Load(EdgeId),
            Refresh(EdgeId),
        }

        let disk = self.disk.clone();
        self.visited_edges.begin(self.graph.edge_count());
        let mut work = vec![Work::Enter(target)];
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => {
                    let Some(edge) = self.graph.node(node).gen else {
                        if self.runtime.node(node).mtime().is_unobserved() {
                            let mut stat = |path: &Path| disk.stat(path);
                            nodestat_with(self.graph, &mut self.runtime, node, &mut stat)?;
                        }
                        let dirty = self.runtime.node(node).mtime().is_missing();
                        self.runtime.node_mut(node).set_dirty(dirty);
                        continue;
                    };
                    if self.visited_edges.replace(edge.index()) {
                        continue;
                    }
                    work.push(if self.runtime.edge(edge).deps_loaded() {
                        Work::Refresh(edge)
                    } else {
                        Work::Load(edge)
                    });
                    for input in self.graph.edge(edge).input.iter().rev() {
                        work.push(Work::Enter(*input));
                    }
                }
                Work::Load(edge) => {
                    let mut stat = |path: &Path| disk.stat(path);
                    let base_dirty =
                        recompute_edge_dirty_with(self.graph, &mut self.runtime, edge, &mut stat)?;
                    let mut dependencies_changed = false;
                    let uses_deps_log = self.deps_log.is_some()
                        && crate::env::edgevar(self.graph, edge, Names::DEPS, PathStyle::Raw)
                            .is_some_and(|value| !value.is_empty());
                    let depfile = (!uses_deps_log)
                        .then(|| {
                            crate::env::edgevar(self.graph, edge, Names::DEPFILE, PathStyle::Raw)
                        })
                        .flatten()
                        .filter(|path| !path.is_empty());

                    if uses_deps_log {
                        let output = self.graph.edge(edge).out.first().copied();
                        let entry_is_current = output.is_some_and(|output| {
                            self.deps_log
                                .as_deref()
                                .and_then(|log| crate::deps::depsentry(log, output))
                                .is_some_and(|entry| {
                                    self.runtime.node(output).mtime().raw() <= entry.mtime
                                })
                        });
                        if !base_dirty && entry_is_current {
                            if let Some(log) = self.deps_log.as_deref() {
                                crate::deps::depsload(self.graph, edge, log);
                                dependencies_changed = true;
                            }
                        }
                        let state = self.runtime.edge_mut(edge);
                        state.set_deps_loaded(true);
                        state.set_deps_missing(!entry_is_current);
                    } else if let Some(depfile) = depfile {
                        let path = depfile.to_path().expect("byte paths are valid on Unix");
                        if base_dirty {
                            let state = self.runtime.edge_mut(edge);
                            state.set_deps_loaded(true);
                            state.set_deps_missing(!disk.exists(path));
                        } else if disk.exists(path) {
                            self.runtime.edge_mut(edge).set_deps_loaded(true);
                            match crate::deps::depsparse_for_edge(
                                self.graph,
                                &disk.resolve(path),
                                edge,
                            )? {
                                Some(deps) => {
                                    self.replace_depfile_deps(edge, &deps.nodes);
                                    self.runtime.edge_mut(edge).set_deps_missing(false);
                                    dependencies_changed = true;
                                }
                                None => self.runtime.edge_mut(edge).set_deps_missing(true),
                            }
                        } else {
                            let state = self.runtime.edge_mut(edge);
                            state.set_deps_loaded(true);
                            state.set_deps_missing(true);
                        }
                    } else {
                        self.runtime.edge_mut(edge).set_deps_loaded(true);
                    }

                    if dependencies_changed {
                        work.push(Work::Refresh(edge));
                        for input in self.graph.edge(edge).input.iter().rev() {
                            work.push(Work::Enter(*input));
                        }
                    }
                }
                Work::Refresh(edge) => {
                    let mut stat = |path: &Path| disk.stat(path);
                    recompute_edge_dirty_with(self.graph, &mut self.runtime, edge, &mut stat)?;
                }
            }
        }
        Ok(())
    }

    fn load_ready_dyndeps_for(
        &mut self,
        node: NodeId,
        visited_edges: &mut Vec<bool>,
        loaded_files: &mut Vec<bool>,
    ) -> BuildResult<()> {
        visited_edges.resize(self.graph.edge_count(), false);
        let mut work = vec![node];
        while let Some(node) = work.pop() {
            let Some(edge) = self.graph.node(node).gen else {
                continue;
            };
            if std::mem::replace(&mut visited_edges[edge.index()], true) {
                continue;
            }
            let dyndep = self.graph.edge(edge).dyndep;
            if let Some(dyndep) =
                dyndep.filter(|dyndep| self.runtime.node(*dyndep).dyndep_pending())
            {
                loaded_files.resize(loaded_files.len().max(dyndep.index() + 1), false);
                let path = self.graph.node_path(dyndep).to_owned();
                if self
                    .disk
                    .exists(path.to_path().expect("byte paths are valid on Unix"))
                    && !std::mem::replace(&mut loaded_files[dyndep.index()], true)
                {
                    crate::dyndep::load_dyndep(self.graph, &mut self.runtime, dyndep, &self.disk)?;
                    self.synchronize_runtime();
                }
            }
            for input in self.graph.edge(edge).input.iter().rev() {
                work.push(*input);
            }
        }
        Ok(())
    }

    fn prepare_build_log_for(&mut self, node: NodeId) -> BuildResult<()> {
        self.visited_edges.begin(self.graph.edge_count());
        let mut work = vec![node];
        while let Some(node) = work.pop() {
            let Some(edge) = self.graph.node(node).gen else {
                continue;
            };
            if self.visited_edges.replace(edge.index()) {
                continue;
            }
            // A phony edge has no command to evaluate, hash, or log, and the
            // dirty rule never consults a phony edge's command hash.
            if !self.graph.is_phony_rule(self.graph.edge(edge).rule) {
                self.refresh_command_hash(edge)?;
            }
            for input in self.graph.edge(edge).input.iter().rev() {
                work.push(*input);
            }
        }
        Ok(())
    }

    fn record_dirty_explanations(&mut self) {
        let Some(explanations) = self.explanations.as_mut() else {
            return;
        };
        self.explanations_recorded
            .resize(self.graph.edge_count(), false);
        for edge in self.plan.wanted_edges(self.graph) {
            if std::mem::replace(&mut self.explanations_recorded[edge.index()], true) {
                continue;
            }
            let inputs = self.graph.edge(edge).non_order_only_inputs();
            let newest = inputs
                .iter()
                .filter(|input| !self.runtime.node(**input).mtime().is_missing())
                .max_by_key(|input| self.runtime.node(**input).mtime())
                .copied();
            for output in &self.graph.edge(edge).out {
                let output_state = self.runtime.node(*output);
                if !output_state.dirty() {
                    continue;
                }
                let path = self.graph.node_path(*output).to_str_lossy();
                let message = if output_state.mtime().is_missing() {
                    format!("output {path} doesn't exist")
                } else if self.runtime.edge(edge).command_dirty() {
                    format!("command line changed for {path}")
                } else if self.runtime.edge(edge).deps_missing() {
                    format!("dependency information for {path} is missing")
                } else if let Some(input) =
                    newest.filter(|input| self.runtime.node(*input).mtime() > output_state.mtime())
                {
                    format!(
                        "output {path} older than most recent input {} ({} vs {})",
                        self.graph.node_path(input).to_str_lossy(),
                        output_state.mtime().raw(),
                        self.runtime.node(input).mtime().raw()
                    )
                } else if inputs.iter().any(|input| self.runtime.node(*input).dirty()) {
                    format!("input to {path} is dirty")
                } else {
                    format!("output {path} is dirty")
                };
                explanations.record(output.index(), message);
            }
        }
    }

    /// The target a path names, for the tests that describe one that way.
    #[cfg(test)]
    pub(crate) fn add_target(&mut self, path: impl AsRef<[u8]>) -> BuildResult<()> {
        let path = path.as_ref();
        let node =
            crate::graph::nodeget(self.graph, path).ok_or_else(|| BuildError::UnknownTarget {
                path: BString::from(path),
            })?;
        self.add_target_node(node)
    }

    pub(crate) fn add_target_node(&mut self, node: NodeId) -> BuildResult<()> {
        if !self.targets.contains(&node) {
            self.targets.push(node);
        }
        // Ahead of every traversal below, not just the dirty scan: all three
        // of `load_depfiles_for`, `prepare_build_log_for` and the scan itself
        // walk this graph and stat what they find.
        self.prefetch_mtimes(node);
        self.load_depfiles_for(node)?;
        self.load_ready_dyndeps_for(node, &mut Vec::new(), &mut Vec::new())?;
        if self.build_log.is_some() {
            self.prepare_build_log_for(node)?;
        }
        let disk = self.disk.clone();
        let mut stat = |path: &Path| disk.stat(path);
        let validations = recompute_dirty_with_validations(
            self.graph,
            &mut self.runtime,
            &mut self.scratch,
            node,
            &mut stat,
        )?;
        self.plan
            .add_target(self.graph, &self.runtime, node)
            .map_err(|error| {
                if self.graph.node(node).gen.is_none() {
                    BuildError::MissingRule {
                        node,
                        path: self.graph.node_path(node).to_owned(),
                    }
                } else {
                    error
                }
            })?;
        for validation in validations {
            self.plan
                .add_target(self.graph, &self.runtime, validation)?;
        }
        self.record_dirty_explanations();
        Ok(())
    }

    /// Warm every mtime the coming scan will ask for, in parallel.
    ///
    /// The scan reads mtimes in dependency order but the reads themselves are
    /// independent, so issuing them one at a time leaves the process blocked
    /// in the kernel for most of an up-to-date build. Filling them first turns
    /// `nodestat_with`'s `is_unobserved` guard into a hit and leaves the scan
    /// itself untouched.
    ///
    /// Nodes already observed are skipped, so a second target costs only the
    /// paths the first did not cover, and a failed stat is simply not recorded
    /// — the scan then takes its usual serial path and reports the usual error.
    fn prefetch_mtimes(&mut self, target: NodeId) {
        crate::graph::collect_stat_targets(
            self.graph,
            &mut self.scratch,
            target,
            &mut self.stat_targets,
        );
        self.stat_targets
            .retain(|node| self.runtime.node(*node).mtime().is_unobserved());
        if self.stat_targets.len() < 2 {
            return;
        }

        // These borrow from the graph, so they cannot outlive the call and
        // cannot be reused buffers; two allocations amortize over thousands
        // of syscalls.
        let graph = &*self.graph;
        let paths = self
            .stat_targets
            .iter()
            .map(|node| {
                graph
                    .node_path(*node)
                    .to_path()
                    .expect("byte paths are valid on Unix")
            })
            .collect::<Vec<_>>();
        let mut results = vec![None; paths.len()];
        self.disk.stat_many(&paths, &mut results);

        for (node, mtime) in self.stat_targets.iter().zip(&results) {
            if let Some(mtime) = *mtime {
                self.runtime
                    .node_mut(*node)
                    .set_mtime(FileTime::observed(mtime));
            }
        }
    }

    /// Whether the build has nothing to run, as Ninja judges it.
    ///
    /// Ninja's `more_to_do` requires *both* a wanted edge and a command edge,
    /// so a plan holding only phony work is up to date. Testing the wanted
    /// count alone diverges on any graph whose default target is a phony over
    /// other phonies — abseil's is, so Ronin stayed silent there where Ninja
    /// says `no work to do.`, while the Ninja project's own graph never hits
    /// the shape and looked correct.
    pub(crate) fn already_up_to_date(&self) -> bool {
        self.plan.is_empty() || self.plan.command_edge_count(self.graph) == 0
    }

    /// The intermediate files this plan is going to create, which is GNU Make's
    /// own test for which of them it may delete afterwards: one it was never
    /// going to make is not one it put there.
    pub(crate) fn disposable_outputs(&self) -> Vec<BString> {
        self.plan
            .wanted_edges(self.graph)
            .into_iter()
            .filter(|edge| self.graph.edge(*edge).disposable)
            .flat_map(|edge| self.graph.edge(edge).out.iter().copied())
            .map(|output| self.graph.node_path(output).to_owned())
            .collect()
    }

    /// Whether the build ran the command that generates `node`, and a `restat`
    /// rule did not then find the output unchanged.
    pub(crate) fn regenerated(&self, node: NodeId) -> bool {
        self.graph.node(node).gen.is_some_and(|edge| {
            self.executed_edges.contains(&edge) && !self.runtime.edge(edge).restat_clean()
        })
    }

    fn prepare_edge(&mut self, edge: EdgeId) -> BuildResult<PreparedEdge> {
        let command = self.take_command(edge)?;
        let old_mtimes = self
            .graph
            .edge(edge)
            .out
            .iter()
            .map(|output| self.runtime.node(*output).mtime().raw())
            .collect::<Vec<_>>();

        for output in &self.graph.edge(edge).out {
            let path = self.graph.node_path(*output).to_owned();
            self.disk
                .make_dirs(path.to_path().expect("byte paths are valid on Unix"))
                .map_err(|source| {
                    BuildError::io(
                        BuildOperation::CreateOutputDirectory,
                        Some(path),
                        Some(edge),
                        source,
                    )
                })?;
        }

        let response_file = command.rspfile.as_ref().map(|path| ResponseFile {
            path: self
                .disk
                .resolve(path.to_path().expect("byte paths are valid on Unix")),
            remove_on_drop: !self.options.keeprsp,
        });
        if response_file.is_some() {
            let logical_path = command
                .rspfile
                .as_ref()
                .expect("response file guard follows a response file")
                .clone();
            self.disk
                .make_dirs(
                    logical_path
                        .to_path()
                        .expect("byte paths are valid on Unix"),
                )
                .map_err(|source| {
                    BuildError::io(
                        BuildOperation::CreateOutputDirectory,
                        Some(logical_path.clone()),
                        Some(edge),
                        source,
                    )
                })?;
            self.disk
                .write(
                    logical_path
                        .to_path()
                        .expect("byte paths are valid on Unix"),
                    command.rspfile_content.as_bytes(),
                )
                .map_err(|source| {
                    BuildError::io(
                        BuildOperation::WriteResponseFile,
                        Some(logical_path),
                        Some(edge),
                        source,
                    )
                })?;
        }

        let command_start_mtime = if self.options.dryrun {
            0
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|source| BuildError::Clock { source })?
                .as_nanos()
                .try_into()
                .unwrap_or(i64::MAX)
        };
        self.executed_edges.insert(edge);
        if self.output_sink.is_none() {
            self.commands_ran.push(command.command.clone());
        }
        Ok(PreparedEdge {
            edge,
            old_mtimes,
            command,
            command_start_mtime,
            start_millis: self.progress.offset_millis(),
            _response_file: response_file,
        })
    }

    // [spec:ronin:def:build.nodedone-fn]
    // [spec:ronin:sem:build.nodedone-fn]
    // [spec:ronin:def:build.shouldprune-fn]
    // [spec:ronin:sem:build.shouldprune-fn]
    // [spec:ronin:def:build.edgedone-fn]
    // [spec:ronin:sem:build.edgedone-fn]
    // [spec:ronin:def:build.jobdone-fn]
    // [spec:ronin:sem:build.jobdone-fn]
    #[allow(
        clippy::too_many_lines,
        reason = "edge completion is one ordered transaction whose cleanup and log updates must stay together"
    )]
    fn finish_edge(
        &mut self,
        prepared: PreparedEdge,
        result: Result<Option<ProcessOutput>, ProcessError>,
    ) -> BuildResult<(bool, Vec<NodeId>)> {
        let PreparedEdge {
            edge,
            old_mtimes,
            command,
            command_start_mtime,
            start_millis,
            _response_file,
        } = prepared;
        // Account for the edge before anything reports progress: the status
        // line about to be printed is the one that has to show it as done.
        // Reading the previous duration has to happen here too, before this
        // run's own entry replaces last run's in the log.
        let end_millis = self.progress.offset_millis();
        let previous_duration =
            status::previous_duration(self.graph, self.build_log.as_deref(), edge);
        self.progress
            .retire_edge(i64::from(end_millis - start_millis), previous_duration);
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.command_finished(edge, &command, Some(1), &[])?;
                return Err(error.into());
            }
        };
        let mut msvc_deps = Vec::new();
        let mut visible_output = Vec::new();
        if let Some(ProcessOutput {
            status,
            stdout,
            stderr,
        }) = result
        {
            if command.deps_type == DepsType::Msvc {
                let mut parser = crate::msvc::ClParser::default();
                let filtered = parser.parse(&stdout, command.msvc_deps_prefix.as_bytes());
                self.record_child_output(filtered.as_bytes());
                visible_output.extend_from_slice(filtered.as_bytes());
                msvc_deps.extend(
                    parser
                        .includes
                        .into_iter()
                        .map(|include| crate::graph::mknode(self.graph, include)),
                );
            } else {
                self.record_child_output(&stdout);
                visible_output.extend_from_slice(&stdout);
            }
            self.record_child_output(&stderr);
            visible_output.extend_from_slice(&stderr);
            let dependency_result = (|| -> BuildResult<()> {
                if status.success() && !self.options.dryrun {
                    match &command.deps_type {
                        DepsType::None | DepsType::Msvc => Ok(()),
                        DepsType::Gcc => {
                            let path = command.depfile_path.as_ref().ok_or({
                                BuildError::DependencyFileMissing { edge, path: None }
                            })?;
                            if self
                                .disk
                                .exists(path.to_path().expect("byte paths are valid on Unix"))
                            {
                                let deps = crate::deps::depsparse(
                                    self.graph,
                                    &self.disk.resolve(
                                        path.to_path().expect("byte paths are valid on Unix"),
                                    ),
                                    false,
                                )?;
                                self.replace_depfile_deps(edge, &deps.nodes);
                                let state = self.runtime.edge_mut(edge);
                                state.set_deps_loaded(true);
                                state.set_deps_missing(false);
                                Ok(())
                            } else {
                                Err(BuildError::DependencyFileMissing {
                                    edge,
                                    path: Some(path.clone()),
                                })
                            }
                        }
                        DepsType::Unsupported(deps_type) => Err(BuildError::UnsupportedDepsType {
                            edge,
                            deps_type: deps_type.clone(),
                        }),
                    }
                } else {
                    Ok(())
                }
            })();
            if let Err(error) = dependency_result {
                self.command_finished(edge, &command, Some(1), &visible_output)?;
                return Err(error);
            }
            // An interrupted command is not a failed one, so it is never
            // reported as failed: Ninja tests for the interrupt before it
            // finishes the command, which is why a build cut short by SIGTERM
            // prints no `FAILED:` line. Half-written outputs still go.
            if status_interrupted(status) {
                let disk = self.disk.clone();
                for (output, old_mtime) in self.graph.edge(edge).out.iter().zip(&old_mtimes) {
                    let path = self.graph.node_path(*output).to_owned();
                    if disk
                        .stat(path.to_path().expect("byte paths are valid on Unix"))
                        .ok()
                        != Some(*old_mtime)
                    {
                        let _ =
                            disk.remove_file(path.to_path().expect("byte paths are valid on Unix"));
                    }
                }
                return Err(BuildError::Interrupted {
                    status: Some(status),
                });
            }
            self.command_finished(
                edge,
                &command,
                (!status.success()).then(|| crate::subprocess::exit_status_code(status)),
                &visible_output,
            )?;
            // A recipe whose errors Make was told to ignore leaves its target
            // made: the status has been reported and the build carries on.
            if !status.success() && !command.ignore_errors {
                return Err(BuildError::SubcommandFailed {
                    edge,
                    command: command.command,
                    status,
                });
            }
        } else {
            self.command_finished(edge, &command, None, &[])?;
        }

        // Before the outputs are stat'ed, so the time `-t` just gave them is
        // the time this run records.
        if self.touching(edge, &command) {
            self.touch_outputs(edge)?;
        }

        let disk = self.disk.clone();
        let mut new_mtimes = Vec::new();
        let output_ids = self.graph.edge(edge).out.clone();
        let edge_hash = edgehash(
            &mut self.runtime,
            edge,
            command.command.as_bstr(),
            (!command.rspfile_content.is_empty()).then_some(command.rspfile_content.as_bstr()),
        );
        for output in output_ids {
            let path = self.graph.node_path(output).to_owned();
            let mtime = disk
                .stat(path.to_path().expect("byte paths are valid on Unix"))
                .map_err(|source| {
                    BuildError::io(
                        BuildOperation::StatOutput,
                        Some(path.clone()),
                        Some(edge),
                        source,
                    )
                })?;
            let output = self.runtime.node_mut(output);
            output.set_mtime(FileTime::observed(mtime));
            output.set_dirty(false);
            output.set_logged_command_hash(edge_hash);
            new_mtimes.push(mtime);
        }
        if !self.options.dryrun {
            match &command.deps_type {
                DepsType::Gcc => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecord(
                            deps_log,
                            edge,
                            self.graph,
                            &self.runtime,
                            &self.disk,
                        )?;
                    }
                }
                DepsType::Msvc => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecordnodes(
                            deps_log,
                            self.graph,
                            &self.runtime,
                            edge,
                            &msvc_deps,
                        )?;
                    }
                }
                DepsType::None => {}
                DepsType::Unsupported(_) => {
                    unreachable!("dependency type was validated before status output")
                }
            }
        }
        if command.deps_type == DepsType::Gcc {
            if let Some(path) = &command.depfile_path {
                if !self.options.keepdepfile {
                    let _ = self
                        .disk
                        .remove_file(path.to_path().expect("byte paths are valid on Unix"));
                }
            }
        }
        let mut loaded_dyndeps = Vec::new();
        if !self.options.dryrun {
            let generated_dyndeps = self
                .graph
                .edge(edge)
                .out
                .iter()
                .filter(|output| self.runtime.node(**output).dyndep_pending())
                .copied()
                .collect::<Vec<_>>();
            for dyndep in generated_dyndeps {
                crate::dyndep::load_dyndep(self.graph, &mut self.runtime, dyndep, &self.disk)?;
                self.synchronize_runtime();
                loaded_dyndeps.push(dyndep);
            }
        }
        self.runtime.edge_mut(edge).set_command_dirty(false);
        let unchanged_outputs = old_mtimes
            .iter()
            .zip(&new_mtimes)
            .map(|(old, new)| old == new)
            .collect::<Vec<_>>();
        let pruned =
            command.restat && !self.options.dryrun && unchanged_outputs.iter().any(|same| *same);
        let all_pruned =
            command.restat && !self.options.dryrun && unchanged_outputs.iter().all(|same| *same);
        let mut record_mtime = command_start_mtime;
        if !self.options.dryrun && (command.restat || command.generator) {
            record_mtime = record_mtime.max(new_mtimes.iter().copied().max().unwrap_or_default());
        }
        if pruned {
            record_mtime = command_start_mtime;
        }
        for output in self.graph.edge(edge).out.clone() {
            self.runtime
                .node_mut(output)
                .set_log_mtime(FileTime::observed(record_mtime));
        }
        // A dry run must leave the log alone. Ninja records nothing for a
        // command it did not run, and recording one entry per planned edge
        // grows the log without bound under any workflow that dry-runs often,
        // which every later invocation of any tool then pays to load. The
        // in-memory mtime above is still set, because the rest of this run's
        // planning depends on it; only the persistent write is skipped.
        if !self.options.dryrun {
            if let Some(build_log) = self.build_log.as_deref_mut() {
                crate::log::logrecordedge(
                    build_log,
                    self.graph,
                    edge,
                    edge_hash,
                    start_millis,
                    end_millis,
                    record_mtime,
                )?;
            }
        }
        self.runtime.edge_mut(edge).set_restat_clean(all_pruned);
        Ok((pruned, loaded_dyndeps))
    }

    fn finish_phony_edge(&mut self, edge: EdgeId) -> (bool, Vec<NodeId>) {
        let outputs: &[NodeId] = &self.graph.edge(edge).out;
        for &output in outputs {
            self.runtime.node_mut(output).set_dirty(false);
        }
        (false, Vec::new())
    }

    fn recompute_consumers_after_restat(&mut self, edge: EdgeId) -> BuildResult<()> {
        let mut queue = Vec::new();
        for output in &self.graph.edge(edge).out {
            queue.extend(self.graph.node(*output).uses.iter().copied());
            queue.extend(self.graph.node_validation_uses(*output).iter().copied());
        }
        self.visited_edges.begin(self.graph.edge_count());
        let disk = self.disk.clone();
        while let Some(dependent) = queue.pop() {
            if self.visited_edges.replace(dependent.index()) {
                continue;
            }
            let outputs: &[NodeId] = &self.graph.edge(dependent).out;
            for &output in outputs {
                let mut stat = |path: &Path| disk.stat(path);
                recompute_dirty_with_validations(
                    self.graph,
                    &mut self.runtime,
                    &mut self.scratch,
                    output,
                    &mut stat,
                )?;
            }
            for &output in outputs {
                queue.extend(self.graph.node(output).uses.iter().copied());
                queue.extend(self.graph.node_validation_uses(output).iter().copied());
            }
        }
        Ok(())
    }

    fn recompute_planned_after_dyndep(&mut self, loaded_dyndeps: &[NodeId]) -> BuildResult<()> {
        let disk = self.disk.clone();
        self.plan
            .expanded_weight
            .fill(CriticalPathWeight::default());
        let mut nodes = self.targets.clone();
        nodes.extend(
            self.plan
                .wanted_edges(self.graph)
                .into_iter()
                .filter_map(|edge| self.graph.edge(edge).out.first().copied()),
        );
        let mut loaded_marks = Vec::new();
        for dyndep in loaded_dyndeps {
            loaded_marks.resize(loaded_marks.len().max(dyndep.index() + 1), false);
            loaded_marks[dyndep.index()] = true;
        }
        let mut affected = Vec::new();
        let mut affected_edges = Vec::new();
        for edge in self.graph.edge_ids() {
            if self
                .graph
                .edge(edge)
                .dyndep
                .is_some_and(|dyndep| loaded_marks.get(dyndep.index()).copied().unwrap_or(false))
            {
                affected_edges.push(edge);
                affected.extend(self.graph.edge(edge).out.first().copied());
            }
        }
        for edge in affected_edges.iter().copied() {
            self.invalidate_command(edge);
        }
        nodes.extend(affected.iter().copied());
        let mut visited_edges = Vec::new();
        let mut loaded_files = Vec::new();
        for node in nodes.iter().copied() {
            self.load_ready_dyndeps_for(node, &mut visited_edges, &mut loaded_files)?;
        }
        if self.build_log.is_some() {
            for edge in affected_edges {
                if !self.graph.is_phony_rule(self.graph.edge(edge).rule) {
                    self.refresh_command_hash(edge)?;
                }
            }
        }
        let mut visited = Vec::new();
        let mut validations = Vec::new();
        for node in nodes {
            visited.resize(visited.len().max(node.index() + 1), false);
            if std::mem::replace(&mut visited[node.index()], true) {
                continue;
            }
            let mut stat = |path: &Path| disk.stat(path);
            validations.extend(recompute_dirty_with_validations(
                self.graph,
                &mut self.runtime,
                &mut self.scratch,
                node,
                &mut stat,
            )?);
        }
        for target in self.targets.iter().copied() {
            self.plan.add_target(self.graph, &self.runtime, target)?;
        }
        for output in affected {
            self.plan.add_target(self.graph, &self.runtime, output)?;
        }
        for validation in validations {
            self.plan
                .add_target(self.graph, &self.runtime, validation)?;
        }
        self.record_dirty_explanations();
        Ok(())
    }

    fn settle_edge(
        &mut self,
        edge: EdgeId,
        result: BuildResult<(bool, Vec<NodeId>)>,
    ) -> BuildResult<()> {
        match result {
            Ok((pruned, loaded_dyndeps)) => {
                if pruned {
                    self.recompute_consumers_after_restat(edge)?;
                }
                if !loaded_dyndeps.is_empty() {
                    self.recompute_planned_after_dyndep(&loaded_dyndeps)?;
                    self.plan.refresh_dependencies(self.graph, &self.runtime)?;
                }
                self.plan
                    .edge_finished(self.graph, &self.runtime, edge, EdgeResult::Succeeded)
            }
            Err(error) => {
                self.plan
                    .edge_finished(self.graph, &self.runtime, edge, EdgeResult::Failed)?;
                Err(error)
            }
        }
    }

    // [spec:ronin:req:compat.scheduling]
    // [spec:ronin:req:compat.process-integration]
    // [spec:ronin:def:build.catchsig-fn]
    // [spec:ronin:sem:build.catchsig-fn]
    // [spec:ronin:def:build.build-fn]
    // [spec:ronin:sem:build.build-fn]
    // [spec:ronin:req:compat.command-runtime]
    // [spec:ronin:def:build.formatstatus-fn]
    // [spec:ronin:sem:build.formatstatus-fn]
    // [spec:ronin:def:build.printstatus-fn]
    // [spec:ronin:sem:build.printstatus-fn]
    // [spec:ronin:def:build.jobstart-fn]
    // [spec:ronin:sem:build.jobstart-fn]
    // [spec:ronin:def:build.jobwork-fn]
    // [spec:ronin:sem:build.jobwork-fn]
    // [spec:ronin:def:build.queryload-fn]
    // [spec:ronin:sem:build.queryload-fn]
    #[allow(
        clippy::too_many_lines,
        reason = "the completion-driven scheduler loop is clearer as one explicit state machine"
    )]
    pub(crate) fn build(&mut self) -> BuildResult<()> {
        self.plan.prepare_queue(self.graph);
        self.progress.started = 0;
        self.progress.finished = 0;
        self.progress.total = self.plan.command_edge_count(self.graph);
        self.progress.start = Instant::now();
        status::seed_prediction(
            &mut self.progress,
            &self.plan,
            self.graph,
            self.build_log.as_deref(),
        );
        let mut failures = 0;
        let mut last_error = None;
        let failure_limit = self.options.maxfail.max(1);
        let mut running = Vec::new();
        running.resize_with(self.graph.edge_count(), || None);
        let mut running_slots = Vec::new();
        running_slots.resize_with(self.graph.edge_count(), || None);
        let mut console_running = false;
        // A dry run starts no process, so it must claim no slot and publish no
        // budget: a jobserver there would be a budget nothing is spending.
        let transport = if self.options.dryrun {
            None
        } else if let Some(inherited) = self.options.jobserver.clone() {
            Some(inherited)
        } else if let (true, JobLimit::Fixed(jobs)) =
            (self.options.serve_jobserver, self.options.jobs)
        {
            // A budget of one has nothing to share, and Ninja's `-j0` has no
            // budget at all. GNU Make publishes no jobserver in either case.
            // Neither has a build with no command to run, which is most of
            // them: an up-to-date tree must not pay to create and remove a
            // fifo nothing was ever going to open.
            (jobs.get() > 1 && self.progress.total != 0)
                .then(|| crate::jobserver::Transport::serve(jobs))
                .transpose()?
                .flatten()
        } else {
            None
        };
        let mut environment = self.options.environment.clone();
        // The jobserver publication and the Make switches both belong in
        // MAKEFLAGS, which is how GNU Make writes it: the single-letter group
        // leads, then the job count and the auth token.
        if let Some(transport) = transport.as_ref() {
            transport.publish_into(&mut environment);
        }
        let mut processes = ProcessSupervisor::<crate::jobserver::Acquisition>::in_directory(
            self.options.working_directory.as_path(),
            self.options.shell.clone(),
            &environment,
        )?;
        let mut jobserver = transport
            .map(|transport| {
                let sender = processes.external_sender();
                crate::jobserver::JobserverClient::new(transport, move |result| {
                    sender.send(result);
                })
            })
            .transpose()?;
        let mut available_slot = None;
        let mut load = status::LoadSampler::default();

        loop {
            // Set when work was deferred for want of a shared slot, which is
            // the one wait a served jobserver cannot wake by itself.
            let mut starved = false;
            if let Some(signal) = crate::signal::interrupted() {
                processes.interrupt(signal)?;
                failures = failure_limit;
                last_error = Some(BuildError::Interrupted { status: None });
            }
            let maxjobs = if self.options.serial
                || (self.options.maxload > 0.0 && load.current() > self.options.maxload)
            {
                1
            } else {
                match self.options.jobs {
                    JobLimit::Auto => 1,
                    JobLimit::Unlimited => usize::MAX,
                    JobLimit::Fixed(jobs) => jobs.get(),
                }
            };
            while !console_running && processes.running_len() < maxjobs && failures < failure_limit
            {
                let Some(edge) = self.plan.find_work(self.graph) else {
                    break;
                };
                let is_phony = self.graph.is_phony_rule(self.graph.edge(edge).rule);
                if is_phony {
                    let result = Ok(self.finish_phony_edge(edge));
                    if let Err(error) = self.settle_edge(edge, result) {
                        failures += 1;
                        last_error = Some(error);
                    }
                    continue;
                }
                let use_console = self.graph.is_console_pool(self.graph.edge(edge).pool);
                if use_console && processes.running_len() != 0 {
                    self.plan.defer_work(self.graph, edge);
                    break;
                }
                let slot = if let Some(client) = jobserver.as_mut() {
                    // The implicit slot first, because it is the one slot that
                    // costs the shared budget nothing. Only past it does a
                    // command of Ronin's own take capacity a child could have.
                    let held = match available_slot
                        .take()
                        .or_else(|| client.try_acquire_implicit())
                    {
                        Some(slot) => Some(slot),
                        None => client.try_acquire_token()?,
                    };
                    if let Some(slot) = held {
                        Some(slot)
                    } else {
                        self.plan.defer_work(self.graph, edge);
                        client.request_token();
                        starved = true;
                        break;
                    }
                } else {
                    None
                };
                let prepared = self.prepare_edge(edge).and_then(|prepared| {
                    self.command_started(edge, &prepared.command)?;
                    Ok(prepared)
                });
                match prepared {
                    Ok(prepared) => {
                        // Make's `+` lines run even under -n, and under -t for
                        // the same reason. Nothing else on the edge does, so
                        // the command becomes just those and the run stops
                        // pretending for this one.
                        let pretending = self.options.dryrun || self.options.touch;
                        let dry_run_only = pretending
                            .then(|| prepared.command.dry_run_command.clone())
                            .flatten();
                        let dryrun = pretending && dry_run_only.is_none();
                        let command =
                            dry_run_only.unwrap_or_else(|| prepared.command.command.clone());
                        match processes.spawn(edge, command, use_console, dryrun) {
                            Ok(()) => {
                                running[edge.index()] = Some(prepared);
                                running_slots[edge.index()] = slot;
                                console_running = use_console;
                                if use_console {
                                    break;
                                }
                            }
                            Err(error) => {
                                if let Some(slot) = slot {
                                    slot.release();
                                }
                                let result = self.finish_edge(prepared, Err(error));
                                if let Err(error) = self.settle_edge(edge, result) {
                                    failures += 1;
                                    last_error = Some(error);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(slot) = slot {
                            slot.release();
                        }
                        self.plan.edge_finished(
                            self.graph,
                            &self.runtime,
                            edge,
                            EdgeResult::Failed,
                        )?;
                        failures += 1;
                        last_error = Some(error);
                    }
                }
            }

            if processes.running_len() == 0 {
                break;
            }
            let deadline = starved
                .then(|| {
                    jobserver
                        .as_ref()
                        .and_then(crate::jobserver::JobserverClient::retry_interval)
                })
                .flatten();
            let Some(wake) = processes.wait(deadline)? else {
                continue;
            };
            let completion = match wake {
                SupervisorWake::Process(completion) => completion,
                SupervisorWake::External(result) => {
                    let client = jobserver
                        .as_mut()
                        .expect("jobserver events require an active client");
                    available_slot = Some(client.receive_token(result)?);
                    continue;
                }
            };
            let edge = completion.edge;
            let prepared = running[edge.index()]
                .take()
                .expect("completed edges have running preparation state");
            if let Some(slot) = running_slots[edge.index()].take() {
                slot.release();
            }
            if prepared.command.use_console {
                console_running = false;
            }
            let result = self.finish_edge(prepared, completion.result);
            if let Err(error) = self.settle_edge(edge, result) {
                failures += 1;
                last_error = Some(error);
            }
        }

        // Ninja carries the last failing command's status out of the build, and
        // records the last failure as it goes; those are the same thing, so the
        // status is read back off the error rather than tracked beside it.
        // [spec:ronin:req:product.build-outcome]
        let outcome = if let Some(error) = last_error {
            Err(BuildError::Stopped {
                status: error.exit_code(),
                reason: BuildStop::from_failure(
                    error,
                    failures,
                    failure_limit,
                    self.options.maxfail,
                ),
            })
        } else if self.plan.more_to_do() {
            // Ninja returns success here, having recorded no failure to take a
            // status from. A build that did not finish must not report that it
            // did, so this one deliberately does not.
            Err(BuildError::Stopped {
                status: 1,
                reason: BuildStop::Stuck,
            })
        } else {
            Ok(())
        };
        // The bar has to be given back whatever happened, so this runs on the
        // failure path too — and the build's own error outranks any trouble
        // writing the closing line.
        let closing = self.emit_summary(outcome.is_ok());
        match outcome {
            Err(error) => Err(error),
            Ok(()) => closing,
        }
    }
}

mod command;
mod reporter;
mod status;
#[cfg(test)]
pub(crate) use status::format_progress_status;

#[cfg(test)]
#[path = "build/tests.rs"]
mod tests;
