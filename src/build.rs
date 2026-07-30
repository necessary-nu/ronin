//! Build scheduling state translated from `build.c`.

use crate::error::{BuildError, BuildOperation, ProcessError};
use crate::graph::{
    edgeadddeps, edgehash, nodeget, nodestat_with, recompute_dirty_with_validations,
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

type BuildResult<T> = Result<T, BuildError>;

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

// [spec:samurai:def:build.buildoptions]
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
    pub(crate) quiet: bool,
    pub(crate) statusfmt: String,
    pub(crate) status_from_cli: bool,
    pub(crate) maxload: f64,
    pub(crate) jobserver: Option<crate::jobserver::Transport>,
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
            quiet: false,
            statusfmt: "[%f/%t] ".into(),
            status_from_cli: false,
            maxload: 0.0,
            jobserver: None,
            working_directory: crate::os::WorkingDirectory::default(),
        }
    }
}

pub(crate) struct BuildState {
    pub(crate) started: usize,
    pub(crate) finished: usize,
    pub(crate) total: usize,
    pub(crate) start: Instant,
}

impl BuildState {
    pub(crate) fn new(_options: BuildOptions) -> Self {
        Self {
            started: 0,
            finished: 0,
            total: 0,
            start: Instant::now(),
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
struct TraversalMark(usize);

impl TraversalMark {
    const UNMARKED: Self = Self(usize::MAX);

    const fn is_marked_for(self, edge: EdgeId) -> bool {
        self.0 == edge.index()
    }

    const fn mark(&mut self, edge: EdgeId) {
        self.0 = edge.index();
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
    dependency_marks: Vec<TraversalMark>,
    completed_count: usize,
    failures: usize,
}

// [spec:samurai:req:compat.graph-semantics]
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
        self.dependency_marks
            .resize(edge_count, TraversalMark::UNMARKED);
    }

    // [spec:samurai:def:build.buildreset-fn]
    // [spec:samurai:sem:build.buildreset-fn]
    // [spec:samurai:def:build.isnewer-fn]
    // [spec:samurai:sem:build.isnewer-fn]
    // [spec:samurai:def:build.isdirty-fn]
    // [spec:samurai:sem:build.isdirty-fn]
    // [spec:samurai:def:build.queue-fn]
    // [spec:samurai:sem:build.queue-fn]
    // [spec:samurai:def:build.buildadd-fn]
    // [spec:samurai:sem:build.buildadd-fn]
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
                    let path = graph.node(node).path.clone();
                    let needed_by =
                        needed_by.map(|needed_by| (needed_by, graph.node(needed_by).path.clone()));
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
            for index in (0..edge.input.len()).rev() {
                let input = edge.input[index];
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
        self.dependency_marks.fill(TraversalMark::UNMARKED);
        for index in 0..graph.edge_count() {
            let edge = EdgeId::from_index(index);
            if !self.wanted[index] || self.completed[index] {
                continue;
            }
            for input in graph.edge(edge).input.iter().copied() {
                let Some(generator) = graph.node(input).gen else {
                    continue;
                };
                if self.wanted[generator.index()]
                    && !self.completed[generator.index()]
                    && !self.dependency_marks[generator.index()].is_marked_for(edge)
                {
                    self.dependency_marks[generator.index()].mark(edge);
                    self.pending[index] += 1;
                    self.dependents[generator.index()].push(edge);
                }
            }
        }
        for index in 0..graph.edge_count() {
            if self.wanted[index]
                && !self.completed[index]
                && !self.running[index]
                && self.pending[index] == 0
            {
                let edge = EdgeId::from_index(index);
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
        for index in 0..graph.edge_count() {
            if !self.wanted[index] {
                continue;
            }
            let edge = EdgeId::from_index(index);
            let weight = self.weight[index];
            for input_index in (0..graph.edge(edge).input.len()).rev() {
                self.add_node(
                    graph,
                    runtime,
                    graph.edge(edge).input[input_index],
                    weight.next(),
                )?;
            }
        }
        self.rebuild_frontier(graph);
        Ok(())
    }

    pub(crate) fn wanted_edges(&self) -> Vec<EdgeId> {
        self.wanted
            .iter()
            .enumerate()
            .filter_map(|(index, wanted)| wanted.then_some(EdgeId::from_index(index)))
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
        self.wanted
            .iter()
            .enumerate()
            .filter(|(index, wanted)| {
                let rule = graph.edge(EdgeId::from_index(*index)).rule;
                **wanted && rule.is_some() && !graph.is_phony_rule(rule)
            })
            .count()
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.wanted_count == 0
    }
}

// [spec:samurai:def:build.job]
pub(crate) struct Builder<'a> {
    graph: &'a mut Graph,
    runtime: RuntimeState,
    options: BuildOptions,
    disk: RealDiskInterface,
    plan: Plan,
    scratch: TraversalScratch,
    visited_edges: crate::graph::MarkSet,
    build_log: Option<&'a mut crate::log::BuildLog>,
    deps_log: Option<&'a mut crate::deps::DepsLog>,
    targets: Vec<NodeId>,
    executed_edges: BTreeSet<EdgeId>,
    command_cache: Vec<Option<CommandSpec>>,
    command_scratch: Vec<u8>,
    progress: BuildState,
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
    fn from_parts(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: Option<&'a mut crate::log::BuildLog>,
        deps_log: Option<&'a mut crate::deps::DepsLog>,
        output_sink: Option<&'a mut dyn Write>,
        diagnostic_sink: Option<&'a mut dyn Write>,
    ) -> Self {
        let progress = BuildState::new(options.clone());
        let disk = RealDiskInterface::new(options.working_directory.clone());
        let mut runtime = RuntimeState::new(graph);
        if let Some(log) = build_log.as_deref() {
            log.hydrate_runtime(graph, &mut runtime, 0..graph.node_ids().len());
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
            visited_edges: crate::graph::MarkSet::default(),
            build_log,
            deps_log,
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            command_cache: Vec::new(),
            command_scratch: Vec::new(),
            progress,
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

    pub(crate) fn with_logs(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
        deps_log: &'a mut crate::deps::DepsLog,
    ) -> Self {
        Self::from_parts(graph, options, Some(build_log), Some(deps_log), None, None)
    }

    pub(crate) fn with_logs_and_output(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
        deps_log: &'a mut crate::deps::DepsLog,
        output: &'a mut dyn Write,
    ) -> Self {
        Self::from_parts(
            graph,
            options,
            Some(build_log),
            Some(deps_log),
            Some(output),
            None,
        )
    }

    pub(crate) fn with_logs_and_sinks(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
        deps_log: &'a mut crate::deps::DepsLog,
        output: &'a mut dyn Write,
        diagnostics: &'a mut dyn Write,
    ) -> Self {
        Self::from_parts(
            graph,
            options,
            Some(build_log),
            Some(deps_log),
            Some(output),
            Some(diagnostics),
        )
    }

    fn synchronize_runtime(&mut self) {
        let nodes = self.runtime.synchronize(self.graph);
        if let Some(log) = self.build_log.as_deref() {
            log.hydrate_runtime(self.graph, &mut self.runtime, nodes);
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
                let path = self.graph.node(dyndep).path.clone();
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
            self.refresh_command_hash(edge)?;
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
        for edge in self.plan.wanted_edges() {
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
                let output_node = self.graph.node(*output);
                let output_state = self.runtime.node(*output);
                if !output_state.dirty() {
                    continue;
                }
                let path = output_node.path.to_str_lossy();
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
                        self.graph.node(input).path.to_str_lossy(),
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

    pub(crate) fn add_target(&mut self, path: impl AsRef<[u8]>) -> BuildResult<()> {
        let path = path.as_ref();
        let node = nodeget(self.graph, path).ok_or_else(|| BuildError::UnknownTarget {
            path: BString::from(path),
        })?;
        if !self.targets.contains(&node) {
            self.targets.push(node);
        }
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
                        path: BString::from(path),
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

    pub(crate) const fn already_up_to_date(&self) -> bool {
        self.plan.is_empty()
    }

    pub(crate) fn ran_edge(&self, edge: EdgeId) -> bool {
        self.executed_edges.contains(&edge)
    }

    pub(crate) fn ran_edge_without_restat_pruning(&self, edge: EdgeId) -> bool {
        self.ran_edge(edge) && !self.runtime.edge(edge).restat_clean()
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
            let path = self.graph.node(*output).path.clone();
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
            _response_file: response_file,
        })
    }

    // [spec:samurai:def:build.nodedone-fn]
    // [spec:samurai:sem:build.nodedone-fn]
    // [spec:samurai:def:build.shouldprune-fn]
    // [spec:samurai:sem:build.shouldprune-fn]
    // [spec:samurai:def:build.edgedone-fn]
    // [spec:samurai:sem:build.edgedone-fn]
    // [spec:samurai:def:build.jobdone-fn]
    // [spec:samurai:sem:build.jobdone-fn]
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
            _response_file,
        } = prepared;
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
            self.command_finished(
                edge,
                &command,
                (!status.success()).then(|| Self::exit_code(status)),
                &visible_output,
            )?;
            if !status.success() {
                if status_interrupted(status) {
                    let disk = self.disk.clone();
                    for (output, old_mtime) in self.graph.edge(edge).out.iter().zip(&old_mtimes) {
                        let path = self.graph.node(*output).path.clone();
                        if disk
                            .stat(path.to_path().expect("byte paths are valid on Unix"))
                            .ok()
                            != Some(*old_mtime)
                        {
                            let _ = disk
                                .remove_file(path.to_path().expect("byte paths are valid on Unix"));
                        }
                    }
                }
                if status_interrupted(status) {
                    return Err(BuildError::Interrupted {
                        status: Some(status),
                    });
                }
                return Err(BuildError::SubcommandFailed {
                    edge,
                    command: command.command,
                    status,
                });
            }
        } else {
            self.command_finished(edge, &command, None, &[])?;
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
            let path = self.graph.node(output).path.clone();
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
        if let Some(build_log) = self.build_log.as_deref_mut() {
            crate::log::logrecordedge(build_log, self.graph, edge, edge_hash, 0, 0, record_mtime)?;
        }
        self.runtime.edge_mut(edge).set_restat_clean(all_pruned);
        Ok((pruned, loaded_dyndeps))
    }

    fn finish_phony_edge(&mut self, edge: EdgeId) -> (bool, Vec<NodeId>) {
        for index in 0..self.graph.edge(edge).out.len() {
            let output = self.graph.edge(edge).out[index];
            self.runtime.node_mut(output).set_dirty(false);
        }
        (false, Vec::new())
    }

    fn recompute_consumers_after_restat(&mut self, edge: EdgeId) -> BuildResult<()> {
        let mut queue = Vec::new();
        for output in &self.graph.edge(edge).out {
            let output = self.graph.node(*output);
            queue.extend(output.uses.iter().copied());
            queue.extend(output.validation_uses.iter().copied());
        }
        self.visited_edges.begin(self.graph.edge_count());
        let disk = self.disk.clone();
        while let Some(dependent) = queue.pop() {
            if self.visited_edges.replace(dependent.index()) {
                continue;
            }
            for index in 0..self.graph.edge(dependent).out.len() {
                let output = self.graph.edge(dependent).out[index];
                let mut stat = |path: &Path| disk.stat(path);
                recompute_dirty_with_validations(
                    self.graph,
                    &mut self.runtime,
                    &mut self.scratch,
                    output,
                    &mut stat,
                )?;
            }
            for index in 0..self.graph.edge(dependent).out.len() {
                let output = self.graph.edge(dependent).out[index];
                let output = self.graph.node(output);
                queue.extend(output.uses.iter().copied());
                queue.extend(output.validation_uses.iter().copied());
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
                .wanted_edges()
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
        for index in 0..self.graph.edge_count() {
            let edge = EdgeId::from_index(index);
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
                self.refresh_command_hash(edge)?;
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

    // [spec:samurai:req:compat.scheduling]
    // [spec:samurai:req:compat.process-integration]
    // [spec:samurai:def:build.catchsig-fn]
    // [spec:samurai:sem:build.catchsig-fn]
    // [spec:samurai:def:build.build-fn]
    // [spec:samurai:sem:build.build-fn]
    // [spec:samurai:req:compat.command-runtime]
    // [spec:samurai:def:build.formatstatus-fn]
    // [spec:samurai:sem:build.formatstatus-fn]
    // [spec:samurai:def:build.printstatus-fn]
    // [spec:samurai:sem:build.printstatus-fn]
    // [spec:samurai:def:build.jobstart-fn]
    // [spec:samurai:sem:build.jobstart-fn]
    // [spec:samurai:def:build.jobwork-fn]
    // [spec:samurai:sem:build.jobwork-fn]
    // [spec:samurai:def:build.queryload-fn]
    // [spec:samurai:sem:build.queryload-fn]
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
        let mut failures = 0;
        let mut last_error = None;
        let failure_limit = self.options.maxfail.max(1);
        let mut running = Vec::new();
        running.resize_with(self.graph.edge_count(), || None);
        let mut running_slots = Vec::new();
        running_slots.resize_with(self.graph.edge_count(), || None);
        let mut console_running = false;
        let mut processes = ProcessSupervisor::<crate::jobserver::Acquisition>::in_directory(
            self.options.working_directory.as_path(),
        )?;
        let mut jobserver = if self.options.dryrun {
            None
        } else {
            self.options
                .jobserver
                .clone()
                .map(|transport| {
                    let sender = processes.external_sender();
                    crate::jobserver::JobserverClient::new(transport, move |result| {
                        sender.send(result);
                    })
                })
                .transpose()?
        };
        let mut available_slot = None;
        let mut load = status::LoadSampler::default();

        loop {
            if let Some(signal) = crate::signal::interrupted() {
                processes.interrupt(signal)?;
                failures = failure_limit;
                last_error = Some(BuildError::Interrupted { status: None });
            }
            let maxjobs = if self.options.maxload > 0.0 && load.current() > self.options.maxload {
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
                    if let Some(slot) = available_slot
                        .take()
                        .or_else(|| client.try_acquire_implicit())
                    {
                        Some(slot)
                    } else {
                        self.plan.defer_work(self.graph, edge);
                        client.request_token();
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
                        let command = prepared.command.command.clone();
                        let dryrun = self.options.dryrun;
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
            let Some(wake) = processes.wait(None)? else {
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

        if let Some(error) = last_error {
            Err(error)
        } else if self.plan.more_to_do() {
            Err(BuildError::DependenciesBlocked)
        } else {
            Ok(())
        }
    }
}

mod command;
mod status;
#[cfg(test)]
pub(crate) use status::format_progress_status;

#[cfg(test)]
#[path = "build/tests.rs"]
mod tests;
