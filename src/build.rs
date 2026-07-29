//! Build scheduling state translated from `build.c`.

use crate::error::{BuildError, ProcessError};
use crate::graph::{
    edgeadddeps, nodeget, nodestat_with, recompute_dirty_with_validations,
    recompute_edge_dirty_with, EdgeId, Graph, NodeId,
};
use crate::os::{RealDiskInterface, MTIME_MISSING};
use crate::subprocess::{status_interrupted, ProcessOutput, ProcessSupervisor};
use crate::util::{BString, ByteSlice};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use self::command::{CommandSpec, PreparedEdge, ResponseFile};

type BuildResult<T> = Result<T, BuildError>;

// [spec:samurai:def:build.buildoptions]
#[derive(Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent Ninja CLI switches are clearer as named options than a synthetic state machine"
)]
pub(crate) struct BuildOptions {
    pub(crate) maxjobs: usize,
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
    pub(crate) jobserver: crate::jobserver::JobserverConfig,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            maxjobs: 0,
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
            jobserver: crate::jobserver::JobserverConfig::default(),
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
    weight: i64,
    edge: Reverse<EdgeId>,
}

impl ReadyEdge {
    fn from_graph(graph: &Graph, edge: EdgeId) -> Self {
        Self {
            weight: graph.edge(edge).critical_path_weight,
            edge: Reverse(edge),
        }
    }
}

#[derive(Default)]
pub(crate) struct Plan {
    wanted: Vec<bool>,
    wanted_count: usize,
    expanded_weight: Vec<i64>,
    pending: Vec<usize>,
    dependents: Vec<Vec<EdgeId>>,
    ready: BinaryHeap<ReadyEdge>,
    running: Vec<bool>,
    completed: Vec<bool>,
    completed_count: usize,
    failures: usize,
}

// [spec:samurai:req:compat.graph-semantics]
impl Plan {
    fn synchronize_arenas(&mut self, edge_count: usize) {
        self.wanted.resize(edge_count, false);
        self.expanded_weight.resize(edge_count, i64::MIN);
        self.pending.resize(edge_count, 0);
        self.dependents.resize_with(edge_count, Vec::new);
        self.running.resize(edge_count, false);
        self.completed.resize(edge_count, false);
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
    pub(crate) fn add_target(&mut self, graph: &mut Graph, node: NodeId) -> BuildResult<()> {
        self.synchronize_arenas(graph.edge_count());
        self.add_node(graph, node, 1)
    }

    fn add_node(&mut self, graph: &mut Graph, node: NodeId, weight: i64) -> BuildResult<()> {
        let mut work = vec![(node, weight, None)];
        while let Some((node, weight, needed_by)) = work.pop() {
            let Some(edge) = graph.node(node).gen else {
                if graph.node(node).dirty {
                    let path = graph.node(node).path.to_str_lossy();
                    return Err(needed_by
                        .map_or_else(
                            || format!("'{path}' missing and no known rule to make it"),
                            |needed_by| {
                                format!(
                                    "'{path}', needed by '{}', missing and no known rule to make it",
                                    graph.node(needed_by).path.to_str_lossy()
                                )
                            },
                        )
                        .into());
                }
                continue;
            };
            let edge_dirty = graph
                .edge(edge)
                .out
                .iter()
                .any(|output| graph.node(*output).dirty);
            let phony_with_no_inputs = {
                let edge = graph.edge(edge);
                edge.rule
                    .is_some_and(|rule| graph.rule(rule).name == "phony")
                    && edge.input.is_empty()
            };
            if edge_dirty && phony_with_no_inputs {
                continue;
            }

            if edge_dirty {
                let previous_weight = graph.edge(edge).critical_path_weight;
                let newly_wanted = !self.wanted[edge.index()];
                if !newly_wanted && weight <= previous_weight {
                    continue;
                }
                if newly_wanted {
                    self.wanted[edge.index()] = true;
                    self.wanted_count += 1;
                }
                graph.edge_mut(edge).critical_path_weight = weight.max(previous_weight);
            } else {
                if weight <= self.expanded_weight[edge.index()] {
                    continue;
                }
                self.expanded_weight[edge.index()] = weight;
            }

            let edge = graph.edge(edge);
            let needed_by = edge.out.first().copied();
            let depfile_start = edge.inorderidx.saturating_sub(edge.depfile_deps);
            let depfile_end = edge.inorderidx;
            for index in (0..edge.input.len()).rev() {
                let input = edge.input[index];
                if !(index >= depfile_start
                    && index < depfile_end
                    && graph.node(input).gen.is_none())
                {
                    work.push((input, weight + 1, needed_by));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_queue(&mut self, graph: &Graph) {
        self.synchronize_arenas(graph.edge_count());
        self.running.fill(false);
        self.completed.fill(false);
        self.completed_count = 0;
        self.failures = 0;
        self.rebuild_frontier(graph);
    }

    fn rebuild_frontier(&mut self, graph: &Graph) {
        self.synchronize_arenas(graph.edge_count());
        self.pending.fill(0);
        for dependents in &mut self.dependents {
            dependents.clear();
        }
        self.ready.clear();
        let mut dependency_marks = vec![usize::MAX; graph.edge_count()];
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
                    && dependency_marks[generator.index()] != index
                {
                    dependency_marks[generator.index()] = index;
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
                self.ready
                    .push(ReadyEdge::from_graph(graph, EdgeId::from_index(index)));
            }
        }
    }

    pub(crate) fn refresh_dependencies(&mut self, graph: &mut Graph) -> BuildResult<()> {
        self.synchronize_arenas(graph.edge_count());
        for index in 0..graph.edge_count() {
            if !self.wanted[index] {
                continue;
            }
            let edge = EdgeId::from_index(index);
            let weight = graph.edge(edge).critical_path_weight;
            for input_index in (0..graph.edge(edge).input.len()).rev() {
                self.add_node(graph, graph.edge(edge).input[input_index], weight + 1)?;
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

    pub(crate) fn find_work(&mut self, graph: &mut Graph) -> Option<EdgeId> {
        let mut blocked = Vec::new();
        let edge = loop {
            let Some(candidate) = self.ready.pop() else {
                self.ready.extend(blocked);
                return None;
            };
            let edge = candidate.edge.0;
            if graph.edge(edge).pool.is_none_or(|pool| {
                let pool = graph.pool(pool);
                pool.numjobs < pool.maxjobs
            }) {
                break edge;
            }
            blocked.push(candidate);
        };
        self.ready.extend(blocked);
        if let Some(pool) = graph.edge(edge).pool {
            graph.pool_mut(pool).numjobs += 1;
        }
        self.running[edge.index()] = true;
        Some(edge)
    }

    fn defer_work(&mut self, graph: &mut Graph, edge: EdgeId) {
        if std::mem::replace(&mut self.running[edge.index()], false) {
            if let Some(pool) = graph.edge(edge).pool {
                graph.pool_mut(pool).numjobs -= 1;
            }
            self.ready.push(ReadyEdge::from_graph(graph, edge));
        }
    }

    pub(crate) fn edge_finished(
        &mut self,
        graph: &mut Graph,
        edge: EdgeId,
        result: EdgeResult,
    ) -> BuildResult<()> {
        if !std::mem::replace(&mut self.running[edge.index()], false) {
            return Err("edge was not running".into());
        }
        if let Some(pool) = graph.edge(edge).pool {
            graph.pool_mut(pool).numjobs -= 1;
        }
        if !std::mem::replace(&mut self.completed[edge.index()], true) {
            self.completed_count += 1;
        }
        if result == EdgeResult::Failed {
            self.failures += 1;
            return Ok(());
        }
        self.release_dependents(graph, edge);
        Ok(())
    }

    fn release_dependents(&mut self, graph: &Graph, finished: EdgeId) {
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
                    .any(|output| graph.node(*output).dirty);
                if dirty {
                    self.ready.push(ReadyEdge::from_graph(graph, dependent));
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
                **wanted
                    && graph
                        .edge(EdgeId::from_index(*index))
                        .rule
                        .is_some_and(|rule| graph.rule(rule).name != "phony")
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
    options: BuildOptions,
    plan: Plan,
    build_log: Option<&'a mut crate::log::BuildLog>,
    deps_log: Option<&'a mut crate::deps::DepsLog>,
    targets: Vec<NodeId>,
    executed_edges: BTreeSet<EdgeId>,
    command_cache: Vec<Option<CommandSpec>>,
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
        let explanations = options
            .explain
            .then(crate::explanations::Explanations::default);
        Self {
            graph,
            options,
            plan: Plan::default(),
            build_log,
            deps_log,
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            command_cache: Vec::new(),
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

    fn replace_depfile_deps(graph: &mut Graph, edge: EdgeId, deps: &[NodeId]) {
        {
            let edge = graph.edge_mut(edge);
            let start = edge.inorderidx.saturating_sub(edge.depfile_deps);
            let end = edge.inorderidx;
            edge.input.drain(start..end);
            edge.inorderidx -= edge.depfile_deps;
            edge.depfile_deps = 0;
        }
        edgeadddeps(graph, edge, deps);
        graph.edge_mut(edge).depfile_deps = deps.len();
    }

    fn load_depfiles_for(&mut self, target: NodeId) -> BuildResult<()> {
        enum Work {
            Enter(NodeId),
            Load(EdgeId),
            Refresh(EdgeId),
        }

        let disk = RealDiskInterface;
        let mut visited = vec![false; self.graph.edge_count()];
        let mut work = vec![Work::Enter(target)];
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => {
                    let Some(edge) = self.graph.node(node).gen else {
                        if self.graph.node(node).mtime == crate::graph::MTIME_UNKNOWN {
                            let mut stat = |path: &Path| disk.stat(path);
                            nodestat_with(self.graph, node, &mut stat)?;
                        }
                        self.graph.node_mut(node).dirty = self.graph.node(node).mtime == 0;
                        continue;
                    };
                    if std::mem::replace(&mut visited[edge.index()], true) {
                        continue;
                    }
                    work.push(if self.graph.edge(edge).deps_loaded {
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
                    let base_dirty = recompute_edge_dirty_with(self.graph, edge, &mut stat)?;
                    let mut dependencies_changed = false;
                    let uses_deps_log = self.deps_log.is_some()
                        && crate::env::edgevar(self.graph, edge, "deps", false)
                            .is_some_and(|value| !value.is_empty());
                    let depfile = (!uses_deps_log)
                        .then(|| crate::env::edgevar(self.graph, edge, "depfile", false))
                        .flatten()
                        .filter(|path| !path.is_empty());

                    if uses_deps_log {
                        let output = self.graph.edge(edge).out.first().copied();
                        let entry_is_current = output.is_some_and(|output| {
                            self.deps_log
                                .as_deref()
                                .and_then(|log| crate::deps::depsentry(log, output))
                                .is_some_and(|entry| self.graph.node(output).mtime <= entry.mtime)
                        });
                        if !base_dirty && entry_is_current {
                            if let Some(log) = self.deps_log.as_deref() {
                                crate::deps::depsload(self.graph, edge, log);
                                dependencies_changed = true;
                            }
                        }
                        let edge = self.graph.edge_mut(edge);
                        edge.deps_loaded = true;
                        edge.deps_missing = !entry_is_current;
                    } else if let Some(depfile) = depfile {
                        let path = depfile.to_path().expect("byte paths are valid on Unix");
                        if base_dirty {
                            let edge = self.graph.edge_mut(edge);
                            edge.deps_loaded = true;
                            edge.deps_missing = !path.exists();
                        } else if path.exists() {
                            self.graph.edge_mut(edge).deps_loaded = true;
                            match crate::deps::depsparse_for_edge(self.graph, path, edge)
                                .map_err(|error| format!("{depfile}: {error}"))?
                            {
                                Some(deps) => {
                                    Self::replace_depfile_deps(self.graph, edge, &deps.nodes);
                                    self.graph.edge_mut(edge).deps_missing = false;
                                    dependencies_changed = true;
                                }
                                None => self.graph.edge_mut(edge).deps_missing = true,
                            }
                        } else {
                            let edge = self.graph.edge_mut(edge);
                            edge.deps_loaded = true;
                            edge.deps_missing = true;
                        }
                    } else {
                        self.graph.edge_mut(edge).deps_loaded = true;
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
                    recompute_edge_dirty_with(self.graph, edge, &mut stat)?;
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
            if let Some(dyndep) = dyndep.filter(|dyndep| self.graph.node(*dyndep).dyndep_pending) {
                loaded_files.resize(loaded_files.len().max(dyndep.index() + 1), false);
                let path = self.graph.node(dyndep).path.clone();
                if path
                    .to_path()
                    .expect("byte paths are valid on Unix")
                    .exists()
                    && !std::mem::replace(&mut loaded_files[dyndep.index()], true)
                {
                    crate::dyndep::load_dyndep(self.graph, dyndep)?;
                }
            }
            for input in self.graph.edge(edge).input.iter().rev() {
                work.push(*input);
            }
        }
        Ok(())
    }

    fn prepare_build_log_for(&mut self, node: NodeId) -> BuildResult<()> {
        let mut visited = vec![false; self.graph.edge_count()];
        let mut work = vec![node];
        while let Some(node) = work.pop() {
            let Some(edge) = self.graph.node(node).gen else {
                continue;
            };
            if std::mem::replace(&mut visited[edge.index()], true) {
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
            let inputs = &self.graph.edge(edge).input[..self.graph.edge(edge).inorderidx];
            let newest = inputs
                .iter()
                .filter(|input| self.graph.node(**input).mtime != MTIME_MISSING)
                .max_by_key(|input| self.graph.node(**input).mtime)
                .copied();
            for output in &self.graph.edge(edge).out {
                let output_node = self.graph.node(*output);
                if !output_node.dirty {
                    continue;
                }
                let path = output_node.path.to_str_lossy();
                let message = if output_node.mtime <= 0 {
                    format!("output {path} doesn't exist")
                } else if self.graph.edge(edge).command_dirty {
                    format!("command line changed for {path}")
                } else if self.graph.edge(edge).deps_missing {
                    format!("dependency information for {path} is missing")
                } else if let Some(input) =
                    newest.filter(|input| self.graph.node(*input).mtime > output_node.mtime)
                {
                    format!(
                        "output {path} older than most recent input {} ({} vs {})",
                        self.graph.node(input).path.to_str_lossy(),
                        output_node.mtime,
                        self.graph.node(input).mtime
                    )
                } else if inputs.iter().any(|input| self.graph.node(*input).dirty) {
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
        let node = nodeget(self.graph, path)
            .ok_or_else(|| format!("unknown target: '{}'", String::from_utf8_lossy(path)))?;
        if !self.targets.contains(&node) {
            self.targets.push(node);
        }
        self.load_depfiles_for(node)?;
        self.load_ready_dyndeps_for(node, &mut Vec::new(), &mut Vec::new())?;
        if self.build_log.is_some() {
            self.prepare_build_log_for(node)?;
        }
        let disk = RealDiskInterface;
        let mut stat = |path: &Path| disk.stat(path);
        let validations = recompute_dirty_with_validations(self.graph, node, &mut stat)?;
        self.plan.add_target(self.graph, node).map_err(|error| {
            if self.graph.node(node).gen.is_none() {
                BuildError::from(format!(
                    "'{}' missing and no known rule to make it",
                    String::from_utf8_lossy(path)
                ))
            } else {
                error
            }
        })?;
        for validation in validations {
            self.plan.add_target(self.graph, validation)?;
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
        self.ran_edge(edge) && !self.graph.edge(edge).restat_clean
    }

    fn prepare_edge(&mut self, edge: EdgeId) -> BuildResult<PreparedEdge> {
        let command = self.take_command(edge)?;
        let old_mtimes = self
            .graph
            .edge(edge)
            .out
            .iter()
            .map(|output| self.graph.node(*output).mtime)
            .collect::<Vec<_>>();

        for output in &self.graph.edge(edge).out {
            let path = self.graph.node(*output).path.clone();
            RealDiskInterface.make_dirs(path.to_path().expect("byte paths are valid on Unix"))?;
        }

        let response_file = command.rspfile.as_ref().map(|path| ResponseFile {
            path: path.clone(),
            remove_on_drop: !self.options.keeprsp,
        });
        if let Some(response_file) = &response_file {
            RealDiskInterface.make_dirs(
                response_file
                    .path
                    .to_path()
                    .expect("byte paths are valid on Unix"),
            )?;
            fs::write(
                response_file
                    .path
                    .to_path()
                    .expect("byte paths are valid on Unix"),
                command.rspfile_content.as_bytes(),
            )?;
        }

        let command_start_mtime = if self.options.dryrun {
            0
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(BuildError::source)?
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
            if command.deps_type == "msvc" {
                let mut parser = crate::msvc::ClParser::default();
                let filtered =
                    parser.parse(&String::from_utf8_lossy(&stdout), &command.msvc_deps_prefix);
                self.record_child_output(filtered.as_bytes());
                visible_output.extend_from_slice(filtered.as_bytes());
                msvc_deps.extend(parser.includes.into_iter().map(|include| {
                    crate::graph::mknode(
                        self.graph,
                        crate::util::xasprintf(format_args!("{include}")),
                    )
                }));
            } else {
                self.record_child_output(&stdout);
                visible_output.extend_from_slice(&stdout);
            }
            self.record_child_output(&stderr);
            visible_output.extend_from_slice(&stderr);
            let dependency_result = (|| -> BuildResult<()> {
                if status.success() && !self.options.dryrun {
                    match command.deps_type.as_str() {
                        "" | "msvc" => Ok(()),
                        "gcc" => {
                            let path = command.depfile_path.as_ref().ok_or_else(|| {
                                "subcommand succeeded but dependency file is missing".to_string()
                            })?;
                            if path
                                .to_path()
                                .expect("byte paths are valid on Unix")
                                .exists()
                            {
                                let deps = crate::deps::depsparse(
                                    self.graph,
                                    path.to_path().expect("byte paths are valid on Unix"),
                                    false,
                                )
                                .map_err(|error| format!("{path}: {error}"))?;
                                Self::replace_depfile_deps(self.graph, edge, &deps.nodes);
                                let edge = self.graph.edge_mut(edge);
                                edge.deps_loaded = true;
                                edge.deps_missing = false;
                                Ok(())
                            } else {
                                Err("subcommand succeeded but dependency file is missing".into())
                            }
                        }
                        deps_type => Err(format!("unsupported deps type '{deps_type}'").into()),
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
                    let disk = RealDiskInterface;
                    for (output, old_mtime) in self.graph.edge(edge).out.iter().zip(&old_mtimes) {
                        let path = self.graph.node(*output).path.clone();
                        if disk
                            .stat(path.to_path().expect("byte paths are valid on Unix"))
                            .ok()
                            != Some(*old_mtime)
                        {
                            let _ = fs::remove_file(
                                path.to_path().expect("byte paths are valid on Unix"),
                            );
                        }
                    }
                }
                if status_interrupted(status) {
                    return Err("interrupted by user".into());
                }
                return Err(format!("subcommand failed: {}", command.command).into());
            }
        } else {
            self.command_finished(edge, &command, None, &[])?;
        }

        let disk = RealDiskInterface;
        let mut new_mtimes = Vec::new();
        let output_ids = self.graph.edge(edge).out.clone();
        let edge_hash = self.graph.edge(edge).hash;
        for output in output_ids {
            let path = self.graph.node(output).path.clone();
            let mtime = disk.stat(path.to_path().expect("byte paths are valid on Unix"))?;
            let output = self.graph.node_mut(output);
            output.mtime = mtime;
            output.dirty = false;
            output.hash = edge_hash;
            new_mtimes.push(output.mtime);
        }
        if !command.deps_type.is_empty() && !self.options.dryrun {
            match command.deps_type.as_str() {
                "gcc" => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecord(deps_log, edge, self.graph)?;
                    }
                }
                "msvc" => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecordnodes(deps_log, self.graph, edge, &msvc_deps)?;
                    }
                }
                _ => unreachable!("dependency type was validated before status output"),
            }
        }
        if command.deps_type == "gcc" {
            if let Some(path) = &command.depfile_path {
                if !self.options.keepdepfile {
                    let _ = fs::remove_file(path.to_path().expect("byte paths are valid on Unix"));
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
                .filter(|output| self.graph.node(**output).dyndep_pending)
                .copied()
                .collect::<Vec<_>>();
            for dyndep in generated_dyndeps {
                crate::dyndep::load_dyndep(self.graph, dyndep)?;
                loaded_dyndeps.push(dyndep);
            }
        }
        self.graph.edge_mut(edge).command_dirty = false;
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
            self.graph.node_mut(output).logmtime = record_mtime;
        }
        if let Some(build_log) = self.build_log.as_deref_mut() {
            crate::log::logrecordedge(build_log, self.graph, edge, 0, 0, record_mtime)?;
        }
        self.graph.edge_mut(edge).restat_clean = all_pruned;
        Ok((pruned, loaded_dyndeps))
    }

    fn finish_phony_edge(&mut self, edge: EdgeId) -> (bool, Vec<NodeId>) {
        for index in 0..self.graph.edge(edge).out.len() {
            let output = self.graph.edge(edge).out[index];
            self.graph.node_mut(output).dirty = false;
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
        let mut visited = vec![false; self.graph.edge_count()];
        let disk = RealDiskInterface;
        while let Some(dependent) = queue.pop() {
            if std::mem::replace(&mut visited[dependent.index()], true) {
                continue;
            }
            for index in 0..self.graph.edge(dependent).out.len() {
                let output = self.graph.edge(dependent).out[index];
                let mut stat = |path: &Path| disk.stat(path);
                recompute_dirty_with_validations(self.graph, output, &mut stat)?;
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
        let disk = RealDiskInterface;
        self.plan.expanded_weight.fill(i64::MIN);
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
                self.graph, node, &mut stat,
            )?);
        }
        for target in self.targets.iter().copied() {
            self.plan.add_target(self.graph, target)?;
        }
        for output in affected {
            self.plan.add_target(self.graph, output)?;
        }
        for validation in validations {
            self.plan.add_target(self.graph, validation)?;
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
                    self.plan.refresh_dependencies(self.graph)?;
                }
                self.plan
                    .edge_finished(self.graph, edge, EdgeResult::Succeeded)
            }
            Err(error) => {
                self.plan
                    .edge_finished(self.graph, edge, EdgeResult::Failed)?;
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
        running_slots.resize_with(self.graph.edge_count(), crate::jobserver::Slot::default);
        let mut console_running = false;
        let mut processes = ProcessSupervisor::default();
        #[cfg(unix)]
        let mut jobserver = if !self.options.dryrun && self.options.jobserver.has_mode() {
            Some(crate::jobserver::create_client(&self.options.jobserver)?)
        } else {
            None
        };

        std::thread::scope(|scope| {
            loop {
                if let Some(signal) = crate::subprocess::interrupted_signal() {
                    processes.interrupt(signal);
                    failures = failure_limit;
                    last_error = Some("interrupted by user".into());
                }
                let mut waiting_for_jobserver = false;
                let maxjobs =
                    if self.options.maxload > 0.0 && status::queryload() > self.options.maxload {
                        1
                    } else {
                        self.options.maxjobs.max(1)
                    };
                while !console_running
                    && processes.running_len() < maxjobs
                    && failures < failure_limit
                {
                    let Some(edge) = self.plan.find_work(self.graph) else {
                        break;
                    };
                    let is_phony = self
                        .graph
                        .edge(edge)
                        .rule
                        .is_some_and(|rule| self.graph.rule(rule).name == "phony");
                    if is_phony {
                        let result = Ok(self.finish_phony_edge(edge));
                        if let Err(error) = self.settle_edge(edge, result) {
                            failures += 1;
                            last_error = Some(error);
                        }
                        continue;
                    }
                    let use_console = self
                        .graph
                        .edge(edge)
                        .pool
                        .is_some_and(|pool| self.graph.pool(pool).name == "console");
                    if use_console && processes.running_len() != 0 {
                        self.plan.defer_work(self.graph, edge);
                        break;
                    }
                    let mut slot = crate::jobserver::Slot::default();
                    #[cfg(unix)]
                    if let Some(client) = jobserver.as_mut() {
                        slot = client.try_acquire();
                        if !slot.is_valid() {
                            self.plan.defer_work(self.graph, edge);
                            waiting_for_jobserver = true;
                            break;
                        }
                    }
                    let prepared = self.prepare_edge(edge).and_then(|prepared| {
                        self.command_started(edge, &prepared.command)?;
                        Ok(prepared)
                    });
                    match prepared {
                        Ok(prepared) => {
                            let command = prepared.command.command.clone();
                            let dryrun = self.options.dryrun;
                            running[edge.index()] = Some(prepared);
                            running_slots[edge.index()] = slot;
                            console_running = use_console;
                            processes.spawn(scope, edge, command, use_console, dryrun);
                            if use_console {
                                break;
                            }
                        }
                        Err(error) => {
                            #[cfg(unix)]
                            if let Some(client) = jobserver.as_mut() {
                                client.release(slot);
                            }
                            self.plan
                                .edge_finished(self.graph, edge, EdgeResult::Failed)?;
                            failures += 1;
                            last_error = Some(error);
                        }
                    }
                }

                if processes.running_len() == 0 {
                    break;
                }
                let timeout = Some(std::time::Duration::from_millis(if waiting_for_jobserver {
                    1
                } else {
                    10
                }));
                let completion = processes.wait(timeout)?;
                let Some(completion) = completion else {
                    continue;
                };
                let edge = completion.edge;
                let prepared = running[edge.index()]
                    .take()
                    .expect("completed edges have running preparation state");
                let slot = running_slots[edge.index()].take();
                #[cfg(unix)]
                if let Some(client) = jobserver.as_mut() {
                    client.release(slot);
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
            Ok::<(), BuildError>(())
        })?;

        if let Some(error) = last_error {
            Err(error)
        } else if self.plan.more_to_do() {
            Err("build stopped: dependencies are blocked".into())
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
