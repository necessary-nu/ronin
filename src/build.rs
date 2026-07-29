//! Build scheduling state translated from `build.c`.

use crate::graph::{
    edgeadddeps, edgehash, nodeget, recompute_dirty_with_validations, EdgeId, Graph, NodeId,
    FLAG_DIRTY, FLAG_DIRTY_OUT, FLAG_WORK,
};
use crate::os::{osmkdirs, RealDiskInterface, MTIME_MISSING};
use crate::util::{BString, ByteSlice};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// [spec:samurai:def:build.buildoptions]
#[derive(Clone)]
pub struct BuildOptions {
    pub maxjobs: usize,
    pub maxfail: usize,
    pub verbose: bool,
    pub explain: bool,
    pub keepdepfile: bool,
    pub keeprsp: bool,
    pub dryrun: bool,
    pub statusfmt: String,
    pub maxload: f64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            maxjobs: 0,
            maxfail: 1,
            verbose: false,
            explain: false,
            keepdepfile: false,
            keeprsp: false,
            dryrun: false,
            statusfmt: "[%s/%t] ".into(),
            maxload: 0.0,
        }
    }
}

// [spec:samurai:def:build.job]
pub struct Job {
    pub command: BString,
    pub edge: EdgeId,
    pub output: Vec<u8>,
    pub failed: bool,
}

pub struct BuildState {
    pub options: BuildOptions,
    pub work: Vec<EdgeId>,
    pub started: usize,
    pub finished: usize,
    pub total: usize,
    pub start: Instant,
}

impl BuildState {
    pub fn new(options: BuildOptions) -> Self {
        Self {
            options,
            work: Vec::new(),
            started: 0,
            finished: 0,
            total: 0,
            start: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeResult {
    Succeeded,
    Failed,
}

#[derive(Default)]
pub struct Plan {
    wanted: BTreeSet<EdgeId>,
    pending: BTreeMap<EdgeId, usize>,
    dependents: BTreeMap<EdgeId, Vec<EdgeId>>,
    ready: Vec<EdgeId>,
    running: BTreeSet<EdgeId>,
    completed: BTreeSet<EdgeId>,
    failures: usize,
}

impl Plan {
    pub fn add_target(&mut self, graph: &mut Graph, node: NodeId) -> Result<(), String> {
        self.add_node(graph, node, 1)
    }

    fn add_node(&mut self, graph: &mut Graph, node: NodeId, weight: i64) -> Result<(), String> {
        let Some(edge) = graph.node(node).gen else {
            if graph.node(node).dirty {
                return Err("file is missing and has no generating edge".into());
            }
            return Ok(());
        };
        let edge_dirty = graph
            .edge(edge)
            .out
            .iter()
            .any(|output| graph.node(*output).dirty);
        if !edge_dirty {
            let (inputs, depfile_start, depfile_end) = {
                let edge = graph.edge(edge);
                (
                    edge.input.clone(),
                    edge.inorderidx.saturating_sub(edge.depfile_deps),
                    edge.inorderidx,
                )
            };
            for (index, input) in inputs.into_iter().enumerate() {
                if index >= depfile_start && index < depfile_end && graph.node(input).gen.is_none()
                {
                    continue;
                }
                self.add_node(graph, input, weight + 1)?;
            }
            return Ok(());
        }
        let phony_with_no_inputs = {
            let edge = graph.edge(edge);
            edge.rule
                .is_some_and(|rule| graph.rule(rule).name == "phony")
                && edge.input.is_empty()
        };
        if phony_with_no_inputs {
            return Ok(());
        }

        let previous_weight = graph.edge(edge).critical_path_weight;
        let newly_wanted = self.wanted.insert(edge);
        if !newly_wanted && weight <= previous_weight {
            return Ok(());
        }
        graph.edge_mut(edge).critical_path_weight = weight.max(previous_weight);
        let (inputs, depfile_start, depfile_end) = {
            let edge = graph.edge(edge);
            (
                edge.input.clone(),
                edge.inorderidx.saturating_sub(edge.depfile_deps),
                edge.inorderidx,
            )
        };
        for (index, input) in inputs.into_iter().enumerate() {
            if index >= depfile_start && index < depfile_end && graph.node(input).gen.is_none() {
                continue;
            }
            self.add_node(graph, input, weight + 1)?;
        }
        Ok(())
    }

    pub fn prepare_queue(&mut self, graph: &Graph) {
        self.running.clear();
        self.completed.clear();
        self.failures = 0;
        self.rebuild_frontier(graph);
    }

    fn rebuild_frontier(&mut self, graph: &Graph) {
        self.pending.clear();
        self.dependents.clear();
        self.ready.clear();
        for edge in &self.wanted {
            if self.completed.contains(edge) {
                continue;
            }
            let mut dependencies = BTreeSet::new();
            for input in graph.edge(*edge).input.iter().copied() {
                let Some(generator) = graph.node(input).gen else {
                    continue;
                };
                if self.wanted.contains(&generator) && !self.completed.contains(&generator) {
                    dependencies.insert(generator);
                }
            }
            self.pending.insert(*edge, dependencies.len());
            for dependency in dependencies {
                self.dependents.entry(dependency).or_default().push(*edge);
            }
        }
        self.ready.extend(
            self.wanted
                .iter()
                .filter(|edge| {
                    !self.completed.contains(edge)
                        && !self.running.contains(edge)
                        && self.pending.get(edge) == Some(&0)
                })
                .copied(),
        );
    }

    pub fn refresh_dependencies(&mut self, graph: &mut Graph) -> Result<(), String> {
        loop {
            let previous = self.wanted.len();
            let edges = self.wanted.iter().copied().collect::<Vec<_>>();
            for edge in edges {
                let weight = graph.edge(edge).critical_path_weight;
                for input in graph.edge(edge).input.clone() {
                    self.add_node(graph, input, weight + 1)?;
                }
            }
            if self.wanted.len() == previous {
                break;
            }
        }
        self.rebuild_frontier(graph);
        Ok(())
    }

    pub fn wanted_edges(&self) -> Vec<EdgeId> {
        self.wanted.iter().copied().collect()
    }

    pub fn find_work(&mut self, graph: &mut Graph) -> Option<EdgeId> {
        let index = self
            .ready
            .iter()
            .enumerate()
            .filter(|(_, edge)| {
                graph.edge(**edge).pool.is_none_or(|pool| {
                    let pool = graph.pool(pool);
                    pool.numjobs < pool.maxjobs
                })
            })
            .max_by(|(_, left), (_, right)| {
                let left = graph.edge(**left);
                let right = graph.edge(**right);
                left.critical_path_weight
                    .cmp(&right.critical_path_weight)
                    .then_with(|| right.id.cmp(&left.id))
            })
            .map(|(index, _)| index)?;
        let edge = self.ready.remove(index);
        if let Some(pool) = graph.edge(edge).pool {
            graph.pool_mut(pool).numjobs += 1;
        }
        self.running.insert(edge);
        Some(edge)
    }

    fn defer_work(&mut self, graph: &mut Graph, edge: EdgeId) {
        if self.running.remove(&edge) {
            if let Some(pool) = graph.edge(edge).pool {
                graph.pool_mut(pool).numjobs -= 1;
            }
            self.ready.push(edge);
        }
    }

    pub fn edge_finished(
        &mut self,
        graph: &mut Graph,
        edge: EdgeId,
        result: EdgeResult,
    ) -> Result<(), String> {
        if !self.running.remove(&edge) {
            return Err("edge was not running".into());
        }
        if let Some(pool) = graph.edge(edge).pool {
            graph.pool_mut(pool).numjobs -= 1;
        }
        self.completed.insert(edge);
        if result == EdgeResult::Failed {
            self.failures += 1;
            return Ok(());
        }
        self.release_dependents(graph, edge);
        Ok(())
    }

    fn release_dependents(&mut self, graph: &Graph, edge: EdgeId) {
        for dependent in self.dependents.get(&edge).cloned().unwrap_or_default() {
            let pending = self
                .pending
                .get_mut(&dependent)
                .expect("planned dependent has a pending count");
            *pending -= 1;
            if *pending == 0 {
                let dirty = graph
                    .edge(dependent)
                    .out
                    .iter()
                    .any(|output| graph.node(*output).dirty);
                if dirty {
                    self.ready.push(dependent);
                } else if self.completed.insert(dependent) {
                    self.release_dependents(graph, dependent);
                }
            }
        }
    }

    pub fn more_to_do(&self) -> bool {
        self.failures != 0 || self.completed.len() < self.wanted.len()
    }

    pub fn command_edge_count(&self, graph: &Graph) -> usize {
        self.wanted
            .iter()
            .filter(|edge| {
                graph
                    .edge(**edge)
                    .rule
                    .is_some_and(|rule| graph.rule(rule).name != "phony")
            })
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.wanted.is_empty()
    }
}

pub struct Builder<'a> {
    graph: &'a mut Graph,
    options: BuildOptions,
    plan: Plan,
    build_log: Option<&'a mut crate::log::BuildLog>,
    deps_log: Option<&'a mut crate::deps::DepsLog>,
    targets: Vec<NodeId>,
    executed_edges: BTreeSet<EdgeId>,
    pub commands_ran: Vec<BString>,
    pub command_output: Vec<u8>,
}

struct PreparedEdge {
    edge: EdgeId,
    old_mtimes: Vec<i64>,
    rspfile: Option<BString>,
    command: BString,
    deps_type: String,
    depfile_path: Option<BString>,
    command_start_mtime: i64,
    use_console: bool,
}

struct ShellOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn status_interrupted(status: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        matches!(status.signal(), Some(1 | 2 | 3 | 15))
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

impl<'a> Builder<'a> {
    pub fn new(graph: &'a mut Graph, options: BuildOptions) -> Self {
        Self {
            graph,
            options,
            plan: Plan::default(),
            build_log: None,
            deps_log: None,
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            commands_ran: Vec::new(),
            command_output: Vec::new(),
        }
    }

    pub fn with_build_log(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
    ) -> Self {
        Self {
            graph,
            options,
            plan: Plan::default(),
            build_log: Some(build_log),
            deps_log: None,
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            commands_ran: Vec::new(),
            command_output: Vec::new(),
        }
    }

    pub fn with_deps_log(
        graph: &'a mut Graph,
        options: BuildOptions,
        deps_log: &'a mut crate::deps::DepsLog,
    ) -> Self {
        Self {
            graph,
            options,
            plan: Plan::default(),
            build_log: None,
            deps_log: Some(deps_log),
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            commands_ran: Vec::new(),
            command_output: Vec::new(),
        }
    }

    pub fn with_logs(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
        deps_log: &'a mut crate::deps::DepsLog,
    ) -> Self {
        Self {
            graph,
            options,
            plan: Plan::default(),
            build_log: Some(build_log),
            deps_log: Some(deps_log),
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            commands_ran: Vec::new(),
            command_output: Vec::new(),
        }
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

    fn load_depfiles_for(
        &mut self,
        node: NodeId,
        visited: &mut BTreeSet<EdgeId>,
    ) -> Result<(), String> {
        let Some(edge) = self.graph.node(node).gen else {
            return Ok(());
        };
        if !visited.insert(edge) {
            return Ok(());
        }
        if self.graph.edge(edge).deps_loaded {
            for input in self.graph.edge(edge).input.clone() {
                self.load_depfiles_for(input, visited)?;
            }
            return Ok(());
        }
        for input in self.graph.edge(edge).input.clone() {
            self.load_depfiles_for(input, visited)?;
        }
        let base_dirty = if let Some(output) = self.graph.edge(edge).out.first().copied() {
            let disk = RealDiskInterface;
            let mut stat = |path: &Path| disk.stat(path);
            crate::graph::recompute_dirty_with(self.graph, output, &mut stat)
                .map_err(|error| error.to_string())?
        } else {
            false
        };
        let uses_deps_log = self.deps_log.is_some()
            && crate::env::edgevar(self.graph, edge, "deps", false)
                .is_some_and(|value| !value.is_empty());
        if uses_deps_log {
            let output = self.graph.edge(edge).out.first().copied();
            let entry_is_current = if let Some(output) = output {
                let disk = RealDiskInterface;
                let output_path = self.graph.node(output).path.clone();
                let output_mtime = disk
                    .stat(output_path.to_path().expect("byte paths are valid on Unix"))
                    .map_err(|error| error.to_string())?;
                self.graph.node_mut(output).mtime = output_mtime;
                self.deps_log
                    .as_deref()
                    .and_then(|log| crate::deps::depsentry(log, output))
                    .is_some_and(|entry| output_mtime <= entry.mtime)
            } else {
                false
            };
            if !base_dirty && entry_is_current {
                if let Some(log) = self.deps_log.as_deref() {
                    crate::deps::depsload(self.graph, edge, log);
                }
            }
            let edge = self.graph.edge_mut(edge);
            edge.deps_loaded = true;
            edge.deps_missing = !entry_is_current;
        } else if let Some(depfile) =
            crate::env::edgevar(self.graph, edge, "depfile", false).filter(|path| !path.is_empty())
        {
            if base_dirty {
                let edge = self.graph.edge_mut(edge);
                edge.deps_loaded = true;
                edge.deps_missing = !depfile
                    .to_path()
                    .expect("byte paths are valid on Unix")
                    .exists();
            } else if depfile
                .to_path()
                .expect("byte paths are valid on Unix")
                .exists()
            {
                self.graph.edge_mut(edge).deps_loaded = true;
                match crate::deps::depsparse_for_edge(
                    self.graph,
                    depfile.to_path().expect("byte paths are valid on Unix"),
                    edge,
                )
                .map_err(|error| format!("{depfile}: {error}"))?
                {
                    Some(deps) => {
                        Self::replace_depfile_deps(self.graph, edge, &deps.nodes);
                        self.graph.edge_mut(edge).deps_missing = false;
                    }
                    None => self.graph.edge_mut(edge).deps_missing = true,
                }
            } else {
                let edge = self.graph.edge_mut(edge);
                edge.deps_loaded = true;
                edge.deps_missing = true;
            }
        }
        for input in self.graph.edge(edge).input.clone() {
            self.load_depfiles_for(input, visited)?;
        }
        Ok(())
    }

    fn load_ready_dyndeps_for(
        &mut self,
        node: NodeId,
        visited_edges: &mut BTreeSet<EdgeId>,
        loaded_files: &mut BTreeSet<NodeId>,
    ) -> Result<(), String> {
        let Some(edge) = self.graph.node(node).gen else {
            return Ok(());
        };
        if !visited_edges.insert(edge) {
            return Ok(());
        }
        let dyndep = self.graph.edge(edge).dyndep;
        if let Some(dyndep) = dyndep.filter(|dyndep| self.graph.node(*dyndep).dyndep_pending) {
            let path = self.graph.node(dyndep).path.clone();
            if path
                .to_path()
                .expect("byte paths are valid on Unix")
                .exists()
                && loaded_files.insert(dyndep)
            {
                crate::dyndep::load_dyndep(self.graph, dyndep)?;
            }
        }
        for input in self.graph.edge(edge).input.clone() {
            self.load_ready_dyndeps_for(input, visited_edges, loaded_files)?;
        }
        Ok(())
    }

    fn prepare_build_log_for(
        &mut self,
        node: NodeId,
        visited: &mut BTreeSet<EdgeId>,
    ) -> Result<(), String> {
        let Some(edge) = self.graph.node(node).gen else {
            return Ok(());
        };
        if !visited.insert(edge) {
            return Ok(());
        }
        let command = crate::env::edgevar(self.graph, edge, "command", true).unwrap_or_default();
        let rspfile_content = crate::env::edgevar(self.graph, edge, "rspfile_content", false);
        edgehash(
            self.graph,
            edge,
            command.as_bstr(),
            rspfile_content.as_ref().map(|content| content.as_bstr()),
        );
        let (hash, outputs) = {
            let edge = self.graph.edge(edge);
            (edge.hash, edge.out.clone())
        };
        let generator = crate::env::edgevar(self.graph, edge, "generator", false)
            .is_some_and(|value| !value.is_empty());
        let command_dirty = !generator
            && outputs.iter().any(|output| {
                let output = self.graph.node(*output);
                output.hash == 0 || output.hash != hash
            });
        self.graph.edge_mut(edge).command_dirty = command_dirty;
        for input in self.graph.edge(edge).input.clone() {
            self.prepare_build_log_for(input, visited)?;
        }
        Ok(())
    }

    pub fn add_target(&mut self, path: impl AsRef<[u8]>) -> Result<(), String> {
        let path = path.as_ref();
        let node = nodeget(self.graph, path)
            .ok_or_else(|| format!("unknown target: '{}'", String::from_utf8_lossy(path)))?;
        if !self.targets.contains(&node) {
            self.targets.push(node);
        }
        if self.build_log.is_some() {
            self.prepare_build_log_for(node, &mut BTreeSet::new())?;
        }
        self.load_depfiles_for(node, &mut BTreeSet::new())?;
        self.load_ready_dyndeps_for(node, &mut BTreeSet::new(), &mut BTreeSet::new())?;
        let disk = RealDiskInterface;
        let mut stat = |path: &Path| disk.stat(path);
        let validations = recompute_dirty_with_validations(self.graph, node, &mut stat)
            .map_err(|error| error.to_string())?;
        self.plan.add_target(self.graph, node).map_err(|error| {
            if self.graph.node(node).gen.is_none() {
                format!(
                    "'{}' missing and no known rule to make it",
                    String::from_utf8_lossy(path)
                )
            } else {
                error
            }
        })?;
        for validation in validations {
            self.plan.add_target(self.graph, validation)?;
        }
        Ok(())
    }

    pub fn already_up_to_date(&self) -> bool {
        self.plan.is_empty()
    }

    pub fn ran_edge(&self, edge: EdgeId) -> bool {
        self.executed_edges.contains(&edge)
    }

    pub fn ran_edge_without_restat_pruning(&self, edge: EdgeId) -> bool {
        self.ran_edge(edge) && !self.graph.edge(edge).restat_clean
    }

    fn prepare_edge(&mut self, edge: EdgeId) -> Result<PreparedEdge, String> {
        let old_mtimes = self
            .graph
            .edge(edge)
            .out
            .iter()
            .map(|output| self.graph.node(*output).mtime)
            .collect::<Vec<_>>();

        for output in &self.graph.edge(edge).out {
            let path = self.graph.node(*output).path.clone();
            osmkdirs(path.to_path().expect("byte paths are valid on Unix"), true)
                .map_err(|error| error.to_string())?;
        }

        let rspfile =
            crate::env::edgevar(self.graph, edge, "rspfile", false).filter(|path| !path.is_empty());
        if let Some(path) = &rspfile {
            let contents = crate::env::edgevar(self.graph, edge, "rspfile_content", false)
                .map(|contents| contents.as_bytes().to_vec())
                .unwrap_or_default();
            osmkdirs(path.to_path().expect("byte paths are valid on Unix"), true)
                .map_err(|error| error.to_string())?;
            fs::write(
                path.to_path().expect("byte paths are valid on Unix"),
                contents,
            )
            .map_err(|error| error.to_string())?;
        }

        let command = crate::env::edgevar(self.graph, edge, "command", true).unwrap_or_default();
        let deps_type = crate::env::edgevar(self.graph, edge, "deps", false)
            .map(|value| {
                String::from_utf8(Vec::from(value))
                    .map_err(|_| "deps binding is not valid UTF-8".to_owned())
            })
            .transpose()?
            .unwrap_or_default();
        let depfile_path =
            crate::env::edgevar(self.graph, edge, "depfile", false).filter(|path| !path.is_empty());
        let command_start_mtime = if self.options.dryrun {
            0
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
                .try_into()
                .unwrap_or(i64::MAX)
        };
        self.executed_edges.insert(edge);
        self.commands_ran.push(command.clone());
        let use_console = self
            .graph
            .edge(edge)
            .pool
            .is_some_and(|pool| self.graph.pool(pool).name == "console");
        Ok(PreparedEdge {
            edge,
            old_mtimes,
            rspfile,
            command,
            deps_type,
            depfile_path,
            command_start_mtime,
            use_console,
        })
    }

    fn execute_edge(
        command: &BString,
        use_console: bool,
        dryrun: bool,
    ) -> Result<Option<ShellOutput>, String> {
        if dryrun {
            return Ok(None);
        }
        if use_console {
            let status = Command::new("/bin/sh")
                .arg("-c")
                .arg(command.to_os_str().expect("byte strings are valid on Unix"))
                .status()
                .map_err(|error| error.to_string())?;
            Ok(Some(ShellOutput {
                status,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))
        } else {
            let result = Command::new("/bin/sh")
                .arg("-c")
                .arg(command.to_os_str().expect("byte strings are valid on Unix"))
                .stdin(Stdio::null())
                .output()
                .map_err(|error| error.to_string())?;
            Ok(Some(ShellOutput {
                status: result.status,
                stdout: result.stdout,
                stderr: result.stderr,
            }))
        }
    }

    fn finish_edge(
        &mut self,
        prepared: PreparedEdge,
        result: Result<Option<ShellOutput>, String>,
    ) -> Result<(bool, Vec<NodeId>), String> {
        let PreparedEdge {
            edge,
            old_mtimes,
            rspfile,
            command,
            deps_type,
            depfile_path,
            command_start_mtime,
            ..
        } = prepared;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(path) = &rspfile {
                    if !self.options.keeprsp {
                        let _ =
                            fs::remove_file(path.to_path().expect("byte paths are valid on Unix"));
                    }
                }
                return Err(error);
            }
        };
        let mut msvc_deps = Vec::new();
        if let Some(ShellOutput {
            status,
            stdout,
            stderr,
        }) = result
        {
            if deps_type == "msvc" {
                let prefix = crate::env::edgevar(self.graph, edge, "msvc_deps_prefix", false)
                    .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                    .unwrap_or_default();
                let mut parser = crate::msvc::ClParser::default();
                let filtered = parser.parse(&String::from_utf8_lossy(&stdout), &prefix);
                self.command_output.extend_from_slice(filtered.as_bytes());
                msvc_deps.extend(parser.includes.into_iter().map(|include| {
                    crate::graph::mknode(
                        self.graph,
                        crate::util::xasprintf(format_args!("{include}")),
                    )
                }));
            } else {
                self.command_output.extend_from_slice(&stdout);
            }
            self.command_output.extend_from_slice(&stderr);
            if !status.success() {
                if status_interrupted(&status) {
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
                if let Some(path) = &rspfile {
                    if !self.options.keeprsp {
                        let _ =
                            fs::remove_file(path.to_path().expect("byte paths are valid on Unix"));
                    }
                }
                if status_interrupted(&status) {
                    return Err("interrupted by user".into());
                }
                return Err(format!("subcommand failed: {command}"));
            }
        }

        if deps_type == "gcc" && !self.options.dryrun {
            let path = depfile_path
                .as_ref()
                .ok_or_else(|| "subcommand succeeded but dependency file is missing".to_string())?;
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
            }
        }

        let disk = RealDiskInterface;
        let mut new_mtimes = Vec::new();
        let output_ids = self.graph.edge(edge).out.clone();
        let edge_hash = self.graph.edge(edge).hash;
        for output in output_ids {
            let path = self.graph.node(output).path.clone();
            let mtime = disk
                .stat(path.to_path().expect("byte paths are valid on Unix"))
                .map_err(|error| error.to_string())?;
            let output = self.graph.node_mut(output);
            output.mtime = mtime;
            output.dirty = false;
            output.hash = edge_hash;
            new_mtimes.push(output.mtime);
        }
        if !deps_type.is_empty() && !self.options.dryrun {
            match deps_type.as_str() {
                "gcc" => {
                    if !depfile_path.as_ref().is_some_and(|path| {
                        path.to_path()
                            .expect("byte paths are valid on Unix")
                            .exists()
                    }) {
                        return Err("subcommand succeeded but dependency file is missing".into());
                    }
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecord(deps_log, edge, self.graph)
                            .map_err(|error| error.to_string())?;
                    }
                }
                "msvc" => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecordnodes(deps_log, self.graph, edge, &msvc_deps)
                            .map_err(|error| error.to_string())?;
                    }
                }
                _ => return Err(format!("unsupported deps type '{deps_type}'")),
            }
        }
        if deps_type == "gcc" {
            if let Some(path) = &depfile_path {
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
        if let Some(path) = rspfile {
            if !self.options.keeprsp {
                let _ = fs::remove_file(path.to_path().expect("byte paths are valid on Unix"));
            }
        }
        let restat = crate::env::edgevar(self.graph, edge, "restat", false)
            .is_some_and(|value| !value.is_empty());
        let generator = crate::env::edgevar(self.graph, edge, "generator", false)
            .is_some_and(|value| !value.is_empty());
        let unchanged_outputs = old_mtimes
            .iter()
            .zip(&new_mtimes)
            .map(|(old, new)| old == new)
            .collect::<Vec<_>>();
        let pruned = restat && !self.options.dryrun && unchanged_outputs.iter().any(|same| *same);
        let all_pruned =
            restat && !self.options.dryrun && unchanged_outputs.iter().all(|same| *same);
        let mut record_mtime = command_start_mtime;
        if !self.options.dryrun && (restat || generator) {
            record_mtime = record_mtime.max(new_mtimes.iter().copied().max().unwrap_or_default());
        }
        if pruned {
            record_mtime = command_start_mtime;
        }
        for output in self.graph.edge(edge).out.clone() {
            self.graph.node_mut(output).logmtime = record_mtime;
        }
        if let Some(build_log) = self.build_log.as_deref_mut() {
            crate::log::logrecordedge(build_log, self.graph, edge, 0, 0, record_mtime)
                .map_err(|error| error.to_string())?;
        }
        self.graph.edge_mut(edge).restat_clean = all_pruned;
        Ok((pruned, loaded_dyndeps))
    }

    fn run_edge(&mut self, edge: EdgeId) -> Result<(bool, Vec<NodeId>), String> {
        let is_phony = self
            .graph
            .edge(edge)
            .rule
            .is_some_and(|rule| self.graph.rule(rule).name == "phony");
        if is_phony {
            for output in self.graph.edge(edge).out.clone() {
                self.graph.node_mut(output).dirty = false;
            }
            return Ok((false, Vec::new()));
        }
        let prepared = self.prepare_edge(edge)?;
        let result =
            Self::execute_edge(&prepared.command, prepared.use_console, self.options.dryrun);
        self.finish_edge(prepared, result)
    }

    fn recompute_consumers_after_restat(&mut self, edge: EdgeId) -> Result<(), String> {
        let mut queue = self
            .graph
            .edge(edge)
            .out
            .iter()
            .flat_map(|output| {
                let output = self.graph.node(*output);
                output
                    .uses
                    .iter()
                    .chain(output.validation_uses.iter())
                    .copied()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let disk = RealDiskInterface;
        while let Some(dependent) = queue.pop() {
            if !visited.insert(dependent) {
                continue;
            }
            let outputs = self.graph.edge(dependent).out.clone();
            for output in &outputs {
                let mut stat = |path: &Path| disk.stat(path);
                recompute_dirty_with_validations(self.graph, *output, &mut stat)
                    .map_err(|error| error.to_string())?;
            }
            for output in outputs {
                let output = self.graph.node(output);
                queue.extend(output.uses.iter().copied());
                queue.extend(output.validation_uses.iter().copied());
            }
        }
        Ok(())
    }

    fn recompute_planned_after_dyndep(&mut self, loaded_dyndeps: &[NodeId]) -> Result<(), String> {
        let disk = RealDiskInterface;
        let mut nodes = self.targets.clone();
        nodes.extend(
            self.plan
                .wanted_edges()
                .into_iter()
                .filter_map(|edge| self.graph.edge(edge).out.first().copied()),
        );
        let affected = self
            .graph
            .edge_ids()
            .into_iter()
            .filter(|edge| {
                self.graph
                    .edge(*edge)
                    .dyndep
                    .is_some_and(|dyndep| loaded_dyndeps.contains(&dyndep))
            })
            .filter_map(|edge| self.graph.edge(edge).out.first().copied())
            .collect::<Vec<_>>();
        nodes.extend(affected.iter().copied());
        let mut visited_edges = BTreeSet::new();
        let mut loaded_files = BTreeSet::new();
        for node in nodes.iter().copied() {
            self.load_ready_dyndeps_for(node, &mut visited_edges, &mut loaded_files)?;
        }
        let mut visited = BTreeSet::new();
        let mut validations = Vec::new();
        for node in nodes {
            if !visited.insert(node) {
                continue;
            }
            let mut stat = |path: &Path| disk.stat(path);
            validations.extend(
                recompute_dirty_with_validations(self.graph, node, &mut stat)
                    .map_err(|error| error.to_string())?,
            );
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
        Ok(())
    }

    fn settle_edge(
        &mut self,
        edge: EdgeId,
        result: Result<(bool, Vec<NodeId>), String>,
    ) -> Result<(), String> {
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

    pub fn build(&mut self) -> Result<(), String> {
        self.plan.prepare_queue(self.graph);
        let mut failures = 0;
        let mut last_error = None;
        let failure_limit = self.options.maxfail.max(1);
        loop {
            if failures >= failure_limit {
                break;
            }
            let maxjobs =
                if self.options.maxload > 0.0 && legacy::queryload() > self.options.maxload {
                    1
                } else {
                    self.options.maxjobs.max(1)
                };
            let mut batch = Vec::new();
            while batch.len() < maxjobs && failures < failure_limit {
                let Some(edge) = self.plan.find_work(self.graph) else {
                    break;
                };
                let is_phony = self
                    .graph
                    .edge(edge)
                    .rule
                    .is_some_and(|rule| self.graph.rule(rule).name == "phony");
                if is_phony {
                    let result = self.run_edge(edge);
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
                if use_console && !batch.is_empty() {
                    self.plan.defer_work(self.graph, edge);
                    break;
                }
                match self.prepare_edge(edge) {
                    Ok(prepared) => {
                        batch.push((edge, prepared));
                        if use_console {
                            break;
                        }
                    }
                    Err(error) => {
                        self.plan
                            .edge_finished(self.graph, edge, EdgeResult::Failed)?;
                        failures += 1;
                        last_error = Some(error);
                    }
                }
            }
            if batch.is_empty() {
                break;
            }
            let dryrun = self.options.dryrun;
            let results = std::thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|(_, prepared)| {
                        let command = prepared.command.clone();
                        let use_console = prepared.use_console;
                        scope.spawn(move || Self::execute_edge(&command, use_console, dryrun))
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("subcommand thread panicked".into()))
                    })
                    .collect::<Vec<_>>()
            });
            for ((edge, prepared), result) in batch.into_iter().zip(results) {
                let result = self.finish_edge(prepared, result);
                if let Err(error) = self.settle_edge(edge, result) {
                    failures += 1;
                    last_error = Some(error);
                }
            }
        }
        if let Some(error) = last_error {
            Err(error)
        } else if self.plan.more_to_do() {
            Err("build stopped: dependencies are blocked".into())
        } else {
            Ok(())
        }
    }
}

// [spec:samurai:def:build.buildreset-fn]
// [spec:samurai:sem:build.buildreset-fn]
pub fn buildreset(graph: &mut Graph) {
    for edge in graph.edge_ids() {
        graph.edge_mut(edge).flags &= !FLAG_WORK;
    }
}

// [spec:samurai:def:build.isnewer-fn]
// [spec:samurai:sem:build.isnewer-fn]
fn isnewer(graph: &Graph, left: Option<NodeId>, right: NodeId) -> bool {
    left.is_some_and(|left| graph.node(left).mtime > graph.node(right).mtime)
}

// [spec:samurai:def:build.isdirty-fn]
// [spec:samurai:sem:build.isdirty-fn]
fn isdirty(
    graph: &Graph,
    node: NodeId,
    newest: Option<NodeId>,
    generator: bool,
    restat: bool,
) -> bool {
    let newer = isnewer(graph, newest, node);
    let node = graph.node(node);
    if node.mtime == MTIME_MISSING {
        return true;
    }
    if newer && (!restat || node.logmtime == MTIME_MISSING) {
        return true;
    }
    !generator && node.hash == 0
}

// [spec:samurai:def:build.queue-fn]
// [spec:samurai:sem:build.queue-fn]
fn queue(state: &mut BuildState, edge: EdgeId) {
    state.work.push(edge);
}

// [spec:samurai:def:build.buildadd-fn]
// [spec:samurai:sem:build.buildadd-fn]
pub fn buildadd(state: &mut BuildState, graph: &mut Graph, node: NodeId) -> Result<(), String> {
    let Some(edge) = graph.node(node).gen else {
        if graph.node(node).mtime == MTIME_MISSING {
            return Err("file is missing and has no generating edge".into());
        }
        return Ok(());
    };
    if graph.edge(edge).flags & crate::graph::FLAG_CYCLE != 0 {
        return Err(format!(
            "dependency cycle involving '{}'",
            node_path_string(graph, node)
        ));
    }
    if graph.edge(edge).flags & FLAG_WORK != 0 {
        return Ok(());
    }
    {
        let outputs = graph.edge(edge).out.clone();
        {
            let edge = graph.edge_mut(edge);
            edge.flags |= FLAG_WORK | crate::graph::FLAG_CYCLE;
            edge.flags &= !FLAG_DIRTY;
            edge.nblock = 0;
            edge.nprune = 0;
        }
        for output in outputs {
            graph.node_mut(output).dirty = false;
        }
    }
    let inputs = graph.edge(edge).input.clone();
    for input in &inputs {
        buildadd(state, graph, *input)?;
    }
    let inorderidx = graph.edge(edge).inorderidx;
    let mut newest: Option<NodeId> = None;
    let mut nblock = 0;
    let mut dirty_input = false;
    for (index, input) in inputs.iter().enumerate() {
        if index < inorderidx {
            dirty_input |= graph.node(*input).dirty;
            let mtime = graph.node(*input).mtime;
            if mtime != MTIME_MISSING
                && newest.is_none_or(|current| graph.node(current).mtime < mtime)
            {
                newest = Some(*input);
            }
        }
        let generated_blocked = graph
            .node(*input)
            .gen
            .is_some_and(|edge| graph.edge(edge).nblock > 0);
        if graph.node(*input).dirty || generated_blocked {
            nblock += 1;
        }
    }
    let generator =
        crate::env::edgevar(graph, edge, "generator", false).is_some_and(|value| !value.is_empty());
    let restat =
        crate::env::edgevar(graph, edge, "restat", false).is_some_and(|value| !value.is_empty());
    let dirty_output = graph
        .edge(edge)
        .out
        .iter()
        .any(|output| isdirty(graph, *output, newest, generator, restat));
    let is_phony = graph
        .edge(edge)
        .rule
        .is_some_and(|rule| graph.rule(rule).name == "phony");
    {
        let outputs = graph.edge(edge).out.clone();
        let dirty = {
            let edge = graph.edge_mut(edge);
            edge.nblock = nblock;
            if dirty_input {
                edge.flags |= crate::graph::FLAG_DIRTY_IN;
            }
            if dirty_output {
                edge.flags |= FLAG_DIRTY_OUT;
            }
            edge.flags & FLAG_DIRTY != 0
        };
        if dirty {
            for output in outputs {
                graph.node_mut(output).dirty = true;
            }
        }
        let edge = graph.edge_mut(edge);
        if edge.flags & FLAG_DIRTY_OUT == 0 {
            edge.nprune = edge.nblock;
        }
        edge.flags &= !crate::graph::FLAG_CYCLE;
    }
    if graph.edge(edge).flags & FLAG_DIRTY != 0 {
        state.total += 1;
        if graph.edge(edge).nblock == 0 {
            queue(state, edge);
        }
        if is_phony {
            state.total = state.total.saturating_sub(1);
        }
    }
    Ok(())
}

fn node_path_string(graph: &Graph, node: NodeId) -> String {
    let node = graph.node(node);
    String::from_utf8_lossy(node.path.as_bytes()).into_owned()
}

// [spec:samurai:def:build.formatstatus-fn]
// [spec:samurai:sem:build.formatstatus-fn]
// [spec:samurai:def:build.printstatus-fn]
// [spec:samurai:sem:build.printstatus-fn]
// [spec:samurai:def:build.jobstart-fn]
// [spec:samurai:sem:build.jobstart-fn]
// [spec:samurai:def:build.nodedone-fn]
// [spec:samurai:sem:build.nodedone-fn]
// [spec:samurai:def:build.shouldprune-fn]
// [spec:samurai:sem:build.shouldprune-fn]
// [spec:samurai:def:build.edgedone-fn]
// [spec:samurai:sem:build.edgedone-fn]
// [spec:samurai:def:build.jobdone-fn]
// [spec:samurai:sem:build.jobdone-fn]
// [spec:samurai:def:build.jobwork-fn]
// [spec:samurai:sem:build.jobwork-fn]
// [spec:samurai:def:build.queryload-fn]
// [spec:samurai:sem:build.queryload-fn]
// [spec:samurai:def:build.catchsig-fn]
// [spec:samurai:sem:build.catchsig-fn]
// [spec:samurai:def:build.build-fn]
// [spec:samurai:sem:build.build-fn]
mod legacy;
pub use legacy::{build, format_progress_status};

#[cfg(test)]
#[path = "build/tests.rs"]
mod tests;
