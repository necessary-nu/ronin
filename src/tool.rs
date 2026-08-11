//! Graph-inspection and cleanup tools translated from `tool.c`.

use crate::env::edgevar;
use crate::error::{ManifestError, ToolAvailability, ToolError, ToolOperation};
use crate::graph::{EdgeId, Graph, NodeId, PathStyle, nodeget};
use crate::names::Names;
use crate::source::Source;
use crate::util::{BStr, BString, ByteSlice};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
#[cfg(test)]
use std::fs;
use std::io::{self, ErrorKind, Write as _};

type ToolResult<T> = Result<T, ToolError>;

// [spec:ronin:req:runtime.iterative-tool-traversals]
#[derive(Default)]
struct EdgeSet(Vec<bool>);

impl EdgeSet {
    fn new(edge_count: usize) -> Self {
        Self(vec![false; edge_count])
    }

    fn insert(&mut self, edge: EdgeId) -> bool {
        !std::mem::replace(&mut self.0[edge.index()], true)
    }
}

mod compdb;
mod input;
mod state;
#[cfg(test)]
mod test_support;
mod urtle;

pub(crate) use urtle::decode as urtle;

pub(crate) use compdb::{compdb, compdb_for_targets};
pub(crate) use input::{inputs, multi_inputs};
pub(crate) use state::{deps_in, missing_deps};

// [spec:ronin:def:tool.tool]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tool {
    Browse,
    Clean,
    Commands,
    Inputs,
    MultiInputs,
    Deps,
    MissingDeps,
    Compdb,
    CompdbTargets,
    Graph,
    Query,
    Targets,
    Recompact,
    Restat,
    Rules,
    CleanDead,
    Urtle,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolStage {
    Flags,
    Manifest,
    Logs,
}

impl Tool {
    pub(crate) const fn stage(self) -> ToolStage {
        match self {
            Self::List | Self::Restat | Self::Urtle => ToolStage::Flags,
            Self::Browse
            | Self::Clean
            | Self::Commands
            | Self::Inputs
            | Self::MultiInputs
            | Self::Compdb
            | Self::CompdbTargets
            | Self::Graph
            | Self::Targets
            | Self::Rules => ToolStage::Manifest,
            Self::Deps | Self::MissingDeps | Self::Query | Self::Recompact | Self::CleanDead => {
                ToolStage::Logs
            }
        }
    }
}

const TOOLS: &[(Tool, &str, &str)] = &[
    (
        Tool::Browse,
        "browse",
        "browse dependency graph in a web browser",
    ),
    (Tool::Clean, "clean", "clean built files"),
    (
        Tool::Commands,
        "commands",
        "list all commands required to rebuild given targets",
    ),
    (
        Tool::Inputs,
        "inputs",
        "list all inputs required to rebuild given targets",
    ),
    (
        Tool::MultiInputs,
        "multi-inputs",
        "print one or more sets of inputs required to build targets",
    ),
    (
        Tool::Deps,
        "deps",
        "show dependencies stored in the deps log",
    ),
    (
        Tool::MissingDeps,
        "missingdeps",
        "check deps log dependencies on generated files",
    ),
    (Tool::Graph, "graph", "output graphviz dot file for targets"),
    (Tool::Query, "query", "show inputs/outputs for a path"),
    (
        Tool::Targets,
        "targets",
        "list targets by their rule or depth in the DAG",
    ),
    (
        Tool::Compdb,
        "compdb",
        "dump JSON compilation database to stdout",
    ),
    (
        Tool::CompdbTargets,
        "compdb-targets",
        "dump JSON compilation database for a given list of targets to stdout",
    ),
    (
        Tool::Recompact,
        "recompact",
        "recompacts ninja-internal data structures",
    ),
    (
        Tool::Restat,
        "restat",
        "restats all outputs in the build log",
    ),
    (Tool::Rules, "rules", "list all rules"),
    (
        Tool::CleanDead,
        "cleandead",
        "clean built files that are no longer produced by the manifest",
    ),
];

pub(crate) fn tool_list() -> String {
    let mut output = String::from("ronin subtools:\n");
    for (_, name, description) in TOOLS {
        let _ = writeln!(output, "{name:>11}  {description}");
    }
    output
}

fn edge_name(graph: &Graph, edge: EdgeId) -> String {
    graph.edge(edge).rule.map_or_else(
        || "phony".into(),
        |rule| graph.rule(rule).name.to_str_lossy().into_owned(),
    )
}

// [spec:ronin:def:tool.cleanpath-fn]
// [spec:ronin:sem:tool.cleanpath-fn]
#[cfg(test)]
pub(crate) fn cleanpath(path: Option<&BStr>) -> io::Result<bool> {
    cleanpath_mode(path, false, &crate::os::RealDiskInterface::default())
}

fn cleanpath_mode(
    path: Option<&BStr>,
    dry_run: bool,
    disk: &crate::os::RealDiskInterface,
) -> io::Result<bool> {
    let Some(path) = path else { return Ok(false) };
    let path = path.to_path().expect("byte paths are valid on Unix");
    if dry_run {
        return match disk.symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        };
    }
    match disk.remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[derive(Default)]
struct Cleaner {
    dry_run: bool,
    disk: crate::os::RealDiskInterface,
    seen_paths: HashSet<BString>,
    visited_edges: EdgeSet,
    dyndep_files: HashMap<BString, Option<crate::dyndep::DyndepFile>>,
    removed: Vec<BString>,
}

impl Cleaner {
    fn new(dry_run: bool, disk: crate::os::RealDiskInterface, edge_count: usize) -> Self {
        Self {
            dry_run,
            disk,
            visited_edges: EdgeSet::new(edge_count),
            ..Self::default()
        }
    }

    fn remove(&mut self, path: Option<&BStr>) -> ToolResult<()> {
        let Some(path) = path else { return Ok(()) };
        let removed = if self.seen_paths.insert(path.to_owned()) {
            cleanpath_mode(Some(path), self.dry_run, &self.disk).map_err(|source| {
                ToolError::io(ToolOperation::Clean, Some(path.to_owned()), source)
            })?
        } else {
            false
        };
        if removed {
            self.removed.push(path.to_owned());
        }
        Ok(())
    }

    // [spec:ronin:def:tool.cleanedge-fn]
    // [spec:ronin:sem:tool.cleanedge-fn]
    fn clean_edge(&mut self, graph: &Graph, edge: EdgeId) -> ToolResult<()> {
        if !self.visited_edges.insert(edge) {
            return Ok(());
        }
        let dyndep_outputs = self.dyndep_outputs(graph, edge)?;
        for output in graph.edge(edge).out.clone() {
            self.remove(Some(graph.node_path(output)))?;
        }
        for output in dyndep_outputs {
            self.remove(Some(output.as_ref()))?;
        }
        for variable in [Names::RSPFILE, Names::DEPFILE] {
            self.remove(
                edgevar(graph, edge, variable, PathStyle::Raw)
                    .as_deref()
                    .map(BStr::new),
            )?;
        }
        Ok(())
    }

    // [spec:ronin:def:tool.cleantarget-fn]
    // [spec:ronin:sem:tool.cleantarget-fn]
    fn clean_target(&mut self, graph: &Graph, node: NodeId) -> ToolResult<()> {
        let mut work = vec![node];
        while let Some(node) = work.pop() {
            let Some(edge) = graph.node(node).generator else {
                continue;
            };
            if !self.visited_edges.insert(edge) {
                continue;
            }
            // A rule-less edge is displayed as phony and cleans like one.
            let rule = graph.edge(edge).rule;
            if rule.is_some() && !graph.is_phony_rule(rule) {
                let dyndep_outputs = self.dyndep_outputs(graph, edge)?;
                for output in graph.edge(edge).out.clone() {
                    self.remove(Some(graph.node_path(output)))?;
                }
                for output in dyndep_outputs {
                    self.remove(Some(output.as_ref()))?;
                }
                for variable in [Names::RSPFILE, Names::DEPFILE] {
                    self.remove(
                        edgevar(graph, edge, variable, PathStyle::Raw)
                            .as_deref()
                            .map(BStr::new),
                    )?;
                }
            }
            work.extend(graph.edge(edge).input.iter().rev().copied());
        }
        Ok(())
    }

    // [spec:ronin:req:runtime.dyndep-transaction]
    fn dyndep_outputs(&mut self, graph: &Graph, edge: EdgeId) -> ToolResult<Vec<BString>> {
        let Some(path) = edgevar(graph, edge, Names::DYNDEP, PathStyle::Raw) else {
            return Ok(Vec::new());
        };
        if !self.dyndep_files.contains_key(&path) {
            let source = match self
                .disk
                .read(path.to_path().expect("byte paths are valid on Unix"))
            {
                Ok(input) => Some(Source::from_bytes(
                    path.to_path().expect("byte paths are valid on Unix"),
                    input,
                )),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(ManifestError::DyndepRead {
                        path: path.clone(),
                        source,
                    }
                    .into());
                }
            };
            let file = source
                .map(|source| crate::dyndep::parse_dyndep_source(&source, graph))
                .transpose()
                .map_err(ManifestError::from)?;
            self.dyndep_files.insert(path.clone(), file);
        }
        Ok(self
            .dyndep_files
            .get(&path)
            .and_then(Option::as_ref)
            .into_iter()
            .flat_map(|file| file.implicit_outputs(edge))
            .cloned()
            .collect())
    }
}

// [spec:ronin:def:tool.clean-fn]
// [spec:ronin:sem:tool.clean-fn]
pub(crate) fn clean(
    graph: &Graph,
    targets: &[BString],
    rules: &[String],
    include_generators: bool,
) -> ToolResult<usize> {
    clean_with_options(graph, targets, rules, include_generators, false)
}

pub(crate) fn clean_with_options(
    graph: &Graph,
    targets: &[BString],
    rules: &[String],
    include_generators: bool,
    dry_run: bool,
) -> ToolResult<usize> {
    clean_with_report(graph, targets, rules, include_generators, dry_run)
        .map(|removed| removed.len())
}

pub(crate) fn clean_with_report(
    graph: &Graph,
    targets: &[BString],
    rules: &[String],
    include_generators: bool,
    dry_run: bool,
) -> ToolResult<Vec<BString>> {
    clean_with_report_in(
        graph,
        targets,
        rules,
        include_generators,
        dry_run,
        crate::os::RealDiskInterface::default(),
    )
}

pub(crate) fn clean_with_report_in(
    graph: &Graph,
    targets: &[BString],
    rules: &[String],
    include_generators: bool,
    dry_run: bool,
    disk: crate::os::RealDiskInterface,
) -> ToolResult<Vec<BString>> {
    let mut cleaner = Cleaner::new(dry_run, disk, graph.edge_count());
    if !rules.is_empty() {
        for edge in graph
            .edge_ids()
            .filter(|edge| rules.iter().any(|rule| rule == &edge_name(graph, *edge)))
        {
            cleaner.clean_edge(graph, edge)?;
        }
    } else if !targets.is_empty() {
        for target in targets {
            let node =
                nodeget(graph, target.as_bytes()).ok_or_else(|| ToolError::UnknownTarget {
                    path: target.clone(),
                })?;
            cleaner.clean_target(graph, node)?;
        }
    } else {
        for edge in graph.edge_ids() {
            // A rule-less edge is displayed as phony and cleans like one.
            let rule = graph.edge(edge).rule;
            if rule.is_none() || graph.is_phony_rule(rule) {
                continue;
            }
            if !include_generators
                && edgevar(graph, edge, Names::GENERATOR, PathStyle::Raw).is_some()
            {
                continue;
            }
            cleaner.clean_edge(graph, edge)?;
        }
    }
    Ok(cleaner.removed)
}

#[cfg(test)]
pub(crate) fn clean_dead(
    graph: &Graph,
    logged_outputs: &[BString],
    dry_run: bool,
) -> ToolResult<usize> {
    clean_dead_with_report(graph, logged_outputs, dry_run).map(|removed| removed.len())
}

#[cfg(test)]
pub(crate) fn clean_dead_with_report(
    graph: &Graph,
    logged_outputs: &[BString],
    dry_run: bool,
) -> ToolResult<Vec<BString>> {
    clean_dead_with_report_in(
        graph,
        logged_outputs,
        dry_run,
        crate::os::RealDiskInterface::default(),
    )
}

pub(crate) fn clean_dead_with_report_in(
    graph: &Graph,
    logged_outputs: &[BString],
    dry_run: bool,
    disk: crate::os::RealDiskInterface,
) -> ToolResult<Vec<BString>> {
    let mut cleaner = Cleaner::new(dry_run, disk, graph.edge_count());
    for output in logged_outputs {
        if nodeget(graph, output.as_bytes()).is_some() {
            continue;
        }
        cleaner.remove(Some(output.as_ref()))?;
    }
    Ok(cleaner.removed)
}

// [spec:ronin:def:tool.targetcommands-fn]
// [spec:ronin:sem:tool.targetcommands-fn]
enum CommandWork {
    Visit(NodeId),
    Emit(EdgeId),
}

fn collect_target_commands(
    graph: &Graph,
    node: NodeId,
    output: &mut Vec<BString>,
    visited: &mut EdgeSet,
    work: &mut Vec<CommandWork>,
) {
    work.push(CommandWork::Visit(node));
    while let Some(item) = work.pop() {
        match item {
            CommandWork::Visit(node) => {
                let Some(edge) = graph.node(node).generator else {
                    continue;
                };
                if !visited.insert(edge) {
                    continue;
                }
                work.push(CommandWork::Emit(edge));
                work.extend(
                    graph
                        .edge(edge)
                        .input
                        .iter()
                        .rev()
                        .copied()
                        .map(CommandWork::Visit),
                );
            }
            CommandWork::Emit(edge) => {
                if let Some(command) = edgevar(graph, edge, Names::COMMAND, PathStyle::ShellEscaped)
                    .filter(|command| !command.is_empty())
                {
                    output.push(command);
                }
            }
        }
    }
}

// [spec:ronin:def:tool.commands-fn]
// [spec:ronin:sem:tool.commands-fn]
pub(crate) fn commands(graph: &Graph, targets: &[BString]) -> ToolResult<Vec<BString>> {
    let nodes = if targets.is_empty() {
        graph
            .node_ids()
            .filter(|node| graph.node(*node).uses.is_empty())
            .collect()
    } else {
        targets
            .iter()
            .map(|target| {
                nodeget(graph, target.as_bytes()).ok_or_else(|| ToolError::UnknownTarget {
                    path: target.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut output = Vec::new();
    let mut visited = EdgeSet::new(graph.edge_count());
    let mut work = Vec::new();
    for node in nodes {
        collect_target_commands(graph, node, &mut output, &mut visited, &mut work);
    }
    Ok(output)
}

fn join_byte_strings(values: impl IntoIterator<Item = BString>, separator: u8) -> BString {
    let mut output = Vec::new();
    for value in values {
        if !output.is_empty() {
            output.push(separator);
        }
        output.extend_from_slice(value.as_bytes());
    }
    BString::from(output)
}

pub(crate) fn commands_with_args(graph: &Graph, arguments: &[BString]) -> ToolResult<BString> {
    let mut single = false;
    let mut targets = Vec::new();
    for argument in arguments {
        match argument.as_bytes() {
            b"-s" => single = true,
            b"-h" | b"--help" => {
                return Err(ToolError::Usage {
                    text: "usage: ronin -t commands [options] [targets]\n\noptions:\n  -s     only print the final command to build [target], not the whole chain",
                });
            }
            option if option.starts_with(b"-") => {
                return Err(ToolError::UnknownOption {
                    tool: "commands",
                    option: argument.clone(),
                });
            }
            _ => targets.push(argument.clone()),
        }
    }
    if !single {
        return commands(graph, &targets).map(|commands| join_byte_strings(commands, b'\n'));
    }
    let mut output = Vec::new();
    let mut seen = EdgeSet::new(graph.edge_count());
    for target in targets {
        let node = nodeget(graph, target.as_bytes()).ok_or_else(|| ToolError::UnknownTarget {
            path: target.clone(),
        })?;
        let Some(edge) = graph.node(node).generator else {
            continue;
        };
        if !seen.insert(edge) {
            continue;
        }
        if let Some(command) = edgevar(graph, edge, Names::COMMAND, PathStyle::ShellEscaped)
            .filter(|value| !value.is_empty())
        {
            output.push(command);
        }
    }
    Ok(join_byte_strings(output, b'\n'))
}

// [spec:ronin:def:tool.printquoted-fn]
// [spec:ronin:sem:tool.printquoted-fn]
// [spec:ronin:req:runtime.output-byte-boundaries]
pub(crate) fn printquoted(bytes: &[u8], join: bool) -> BString {
    let mut output = Vec::with_capacity(bytes.len());
    for byte in bytes {
        match byte {
            0 => break,
            b'"' | b'\\' => {
                output.push(b'\\');
                output.push(*byte);
            }
            b'\n' if join => output.push(b' '),
            b'\n' => {}
            _ => output.push(*byte),
        }
    }
    BString::from(output)
}

// [spec:ronin:def:tool.graphnode-fn]
// [spec:ronin:sem:tool.graphnode-fn]
fn graphnode_inner(graph: &Graph, node: NodeId, output: &mut Vec<u8>, visited: &mut EdgeSet) {
    enum Work {
        Visit(NodeId),
        EmitEdge(EdgeId),
    }

    let mut work = vec![Work::Visit(node)];
    while let Some(item) = work.pop() {
        match item {
            Work::Visit(node) => {
                let path = &graph.node_path(node);
                let _ = write!(output, "\"n{}\" [label=\"", node.index());
                output.extend_from_slice(printquoted(path.as_bytes(), false).as_bytes());
                output.extend_from_slice(b"\"]\n");
                let Some(edge) = graph.node(node).generator else {
                    continue;
                };
                if !visited.insert(edge) {
                    continue;
                }
                work.push(Work::EmitEdge(edge));
                work.extend(
                    graph
                        .edge(edge)
                        .input
                        .iter()
                        .rev()
                        .copied()
                        .map(Work::Visit),
                );
            }
            Work::EmitEdge(edge) => emit_graph_edge(graph, edge, output),
        }
    }
}

fn emit_graph_edge(graph: &Graph, edge: EdgeId, output: &mut Vec<u8>) {
    let edge_borrow = graph.edge(edge);
    if edge_borrow.input.len() == 1 && edge_borrow.out.len() == 1 {
        let _ = writeln!(
            output,
            "\"n{}\" -> \"n{}\" [label=\" {}\"]",
            edge_borrow.input[0].index(),
            edge_borrow.out[0].index(),
            edge_name(graph, edge)
        );
    } else {
        let _ = writeln!(
            output,
            "\"e{}\" [label=\"{}\", shape=ellipse]",
            edge.index(),
            edge_name(graph, edge)
        );
        for output_node in &edge_borrow.out {
            let _ = writeln!(
                output,
                "\"e{}\" -> \"n{}\"",
                edge.index(),
                output_node.index()
            );
        }
        for (index, input) in edge_borrow.input.iter().enumerate() {
            let style = if index >= edge_borrow.non_order_only_input_count() {
                " style=dotted"
            } else {
                ""
            };
            let _ = writeln!(
                output,
                "\"n{}\" -> \"e{}\" [arrowhead=none{}]",
                input.index(),
                edge.index(),
                style
            );
        }
    }
}

// [spec:ronin:def:tool.graph-fn]
// [spec:ronin:sem:tool.graph-fn]
pub(crate) fn graph(graph: &Graph, targets: &[BString]) -> ToolResult<BString> {
    let nodes = if targets.is_empty() {
        crate::graph::rootnodes(graph)?
    } else {
        targets
            .iter()
            .map(|target| {
                nodeget(graph, target.as_bytes()).ok_or_else(|| ToolError::UnknownTarget {
                    path: target.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut output = Vec::from(&b"digraph ninja {\nrankdir=\"LR\"\n"[..]);
    output.extend_from_slice(b"node [fontsize=10, shape=box, height=0.25]\n");
    output.extend_from_slice(b"edge [fontsize=10]\n");
    let mut visited = EdgeSet::new(graph.edge_count());
    for node in nodes {
        graphnode_inner(graph, node, &mut output, &mut visited);
    }
    output.push(b'}');
    Ok(BString::from(output))
}

// [spec:ronin:def:tool.query-fn]
// [spec:ronin:sem:tool.query-fn]
pub(crate) fn query(graph: &Graph, targets: &[BString]) -> ToolResult<String> {
    if targets.is_empty() {
        return Err(ToolError::MissingArgument {
            diagnostic: "query expects at least one target",
        });
    }
    let mut output = String::new();
    for target in targets {
        let node = nodeget(graph, target.as_bytes()).ok_or_else(|| ToolError::UnknownTarget {
            path: target.clone(),
        })?;
        let node_borrow = graph.node(node);
        let _ = writeln!(output, "{}:", target.to_str_lossy());
        if let Some(edge) = node_borrow.generator {
            let _ = writeln!(output, "  input: {}", edge_name(graph, edge));
            for (index, input) in graph.edge(edge).input.iter().enumerate() {
                let input_path = graph.node_path(*input);
                let label = if index >= graph.edge(edge).non_order_only_input_count() {
                    "|| "
                } else if index >= graph.edge(edge).explicit_input_count() {
                    "| "
                } else {
                    ""
                };
                let _ = writeln!(
                    output,
                    "    {label}{}",
                    String::from_utf8_lossy(input_path.as_bytes())
                );
            }
            if !graph.edge(edge).validation.is_empty() {
                output.push_str("  validations:\n");
                for validation in &graph.edge(edge).validation {
                    let _ = writeln!(
                        output,
                        "    {}",
                        String::from_utf8_lossy(graph.node_path(*validation).as_bytes())
                    );
                }
            }
        }
        output.push_str("  outputs:\n");
        for edge in &node_borrow.uses {
            for output_node in &graph.edge(*edge).out {
                let path = &graph.node_path(*output_node);
                let _ = writeln!(output, "    {}", String::from_utf8_lossy(path.as_bytes()));
            }
        }
        if !graph.node_validation_uses(node).is_empty() {
            output.push_str("  validation for:\n");
            for edge in graph.node_validation_uses(node) {
                for output_node in &graph.edge(*edge).out {
                    let path = &graph.node_path(*output_node);
                    let _ = writeln!(output, "    {}", String::from_utf8_lossy(path.as_bytes()));
                }
            }
        }
    }
    Ok(output)
}

// [spec:ronin:def:tool.targetsdepth-fn]
// [spec:ronin:sem:tool.targetsdepth-fn]
pub(crate) fn targetsdepth(
    graph: &Graph,
    node: NodeId,
    depth: usize,
    indent: usize,
    output: &mut String,
) {
    let mut work = vec![(node, depth, indent)];
    while let Some((node, depth, indent)) = work.pop() {
        output.push_str(&"  ".repeat(indent));
        let node_borrow = graph.node(node);
        if let Some(edge) = node_borrow.generator {
            let _ = writeln!(
                output,
                "{}: {}",
                String::from_utf8_lossy(graph.node_path(node).as_bytes()),
                edge_name(graph, edge)
            );
            if depth != 1 {
                let next_depth = if depth == 0 { 0 } else { depth - 1 };
                work.extend(
                    graph
                        .edge(edge)
                        .input
                        .iter()
                        .rev()
                        .copied()
                        .map(|input| (input, next_depth, indent + 1)),
                );
            }
        } else {
            let _ = writeln!(
                output,
                "{}",
                String::from_utf8_lossy(graph.node_path(node).as_bytes())
            );
        }
    }
}

// [spec:ronin:def:tool.targetsusage-fn]
// [spec:ronin:sem:tool.targetsusage-fn]
pub(crate) const fn targetsusage() -> &'static str {
    "targets [depth [maxdepth]] | rule [rulename] | all"
}

// [spec:ronin:def:tool.targets-fn]
// [spec:ronin:sem:tool.targets-fn]
pub(crate) fn targets_with_args(graph: &Graph, args: &[String]) -> ToolResult<String> {
    if args.len() > 2 {
        return Err(ToolError::Usage {
            text: targetsusage(),
        });
    }
    match args.first().map(String::as_str) {
        None | Some("depth") => {
            let depth = args
                .get(1)
                .map(|depth| {
                    depth.parse::<usize>().map_err(|_| ToolError::Usage {
                        text: targetsusage(),
                    })
                })
                .transpose()?
                .unwrap_or(1);
            let mut output = String::new();
            for node in crate::graph::rootnodes(graph)? {
                targetsdepth(graph, node, depth, 0, &mut output);
            }
            Ok(output)
        }
        Some("rule") => {
            let mut output = String::new();
            if let Some(rule) = args.get(1) {
                let outputs = graph
                    .edge_ids()
                    .filter(|edge| edge_name(graph, *edge) == *rule)
                    .flat_map(|edge| graph.edge(edge).out.clone())
                    .map(|node| graph.node_path(node).to_str_lossy().into_owned())
                    .collect::<std::collections::BTreeSet<_>>();
                for path in outputs {
                    let _ = writeln!(output, "{path}");
                }
            } else {
                for edge in graph.edge_ids() {
                    for input in &graph.edge(edge).input {
                        if graph.node(*input).generator.is_none() {
                            let input_path = graph.node_path(*input);
                            let _ = writeln!(
                                output,
                                "{}",
                                String::from_utf8_lossy(input_path.as_bytes())
                            );
                        }
                    }
                }
            }
            Ok(output)
        }
        Some("all") if args.len() == 1 => {
            let mut output = String::new();
            for edge in graph.edge_ids() {
                for node in &graph.edge(edge).out {
                    let _ = writeln!(
                        output,
                        "{}: {}",
                        String::from_utf8_lossy(graph.node_path(*node).as_bytes()),
                        edge_name(graph, edge)
                    );
                }
            }
            Ok(output)
        }
        Some(mode) => Err(ToolError::UnknownMode {
            tool: "target tool",
            mode: mode.to_owned(),
        }),
    }
}

pub(crate) fn rules(graph: &Graph, arguments: &[String]) -> ToolResult<String> {
    let mut descriptions = false;
    for argument in arguments {
        match argument.as_str() {
            "-d" => descriptions = true,
            "-h" | "--help" => {
                return Err(ToolError::Usage {
                    text: "usage: ronin -t rules [options]\n\noptions:\n  -d     also print the description of the rule\n  -h     print this message",
                });
            }
            option => {
                return Err(ToolError::UnknownOption {
                    tool: "rules",
                    option: BString::from(option),
                });
            }
        }
    }
    let mut rules = graph
        .rule_ids()
        .map(|rule| (graph.rule(rule).name.clone(), rule))
        .collect::<Vec<_>>();
    rules.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = String::new();
    for (name, rule) in rules {
        output.push_str(&name.to_str_lossy());
        if descriptions && let Some(description) = graph.rule(rule).bindings.get(Names::DESCRIPTION)
        {
            output.push_str(": ");
            for part in &description.parts {
                match part {
                    crate::util::EvalPart::Literal(value) => {
                        output.push_str(&String::from_utf8_lossy(value.as_bytes()));
                    }
                    crate::util::EvalPart::Variable(name) => {
                        let _ = write!(output, "${{{}}}", graph.names().name(*name));
                    }
                }
            }
        }
        output.push('\n');
    }
    Ok(output)
}

// [spec:ronin:def:tool.tool.run-fn]
// [spec:ronin:sem:tool.tool.run-fn]
pub(crate) fn run(
    tool: Tool,
    graph: &Graph,
    args: &[BString],
    working_directory: &std::path::Path,
) -> ToolResult<BString> {
    let utf8_arguments = || {
        args.iter()
            .map(|argument| {
                argument.to_str().map(str::to_owned).map_err(|_| {
                    ToolError::InvalidArgumentsEncoding {
                        context: match tool {
                            Tool::Compdb => "compdb rule",
                            Tool::Targets => "targets mode",
                            Tool::Rules => "rules option",
                            _ => "tool",
                        },
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()
    };
    match tool {
        Tool::Clean => Ok(clean(graph, args, &[], false)?.to_string().into()),
        Tool::Commands => commands_with_args(graph, args),
        Tool::Compdb => Ok(compdb(graph, &utf8_arguments()?, false, working_directory)),
        Tool::CompdbTargets => compdb_for_targets(graph, args, false, working_directory),
        Tool::Graph => self::graph(graph, args),
        Tool::Query => query(graph, args).map(BString::from),
        Tool::Targets => targets_with_args(graph, &utf8_arguments()?).map(BString::from),
        Tool::Rules => rules(graph, &utf8_arguments()?).map(BString::from),
        Tool::Inputs => inputs(graph, args),
        Tool::MultiInputs => multi_inputs(graph, args),
        Tool::List => Ok(tool_list().into()),
        Tool::Browse => Err(ToolError::Availability(ToolAvailability::BrowseUnsupported)),
        Tool::Deps
        | Tool::MissingDeps
        | Tool::Recompact
        | Tool::Restat
        | Tool::CleanDead
        | Tool::Urtle => Err(ToolError::Availability(
            ToolAvailability::RequiresRuntimeState,
        )),
    }
}

// [spec:ronin:def:tool.toolget-fn]
// [spec:ronin:sem:tool.toolget-fn]
pub(crate) fn toolget(name: &str) -> ToolResult<Tool> {
    if name == "list" {
        return Ok(Tool::List);
    }
    if name == "urtle" {
        return Ok(Tool::Urtle);
    }
    if let Some(tool) = TOOLS
        .iter()
        .find_map(|(tool, candidate, _)| (*candidate == name).then_some(*tool))
    {
        return Ok(tool);
    }
    let suggestion = TOOLS
        .iter()
        .map(|(_, candidate, _)| *candidate)
        .chain(std::iter::once("urtle"))
        .filter_map(|candidate| {
            let distance = crate::util::edit_distance(candidate, name, true, Some(3));
            (distance <= 3).then_some((distance, candidate))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate);
    Err(ToolError::UnknownTool {
        name: name.to_owned(),
        suggestion,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_CLEAN_TEST: AtomicUsize = AtomicUsize::new(0);

    struct TempDirectory(std::path::PathBuf);

    impl TempDirectory {
        fn new(name: &str) -> Self {
            for _ in 0..1024 {
                let sequence = NEXT_CLEAN_TEST.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "ronin-ninja-clean-{}-{name}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("could not create clean test directory: {error}"),
                }
            }
            panic!("could not allocate a unique clean test directory")
        }

        fn join(&self, name: impl AsRef<Path>) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn parse_manifest(directory: &TempDirectory, manifest: &str) -> Graph {
        let path = directory.join("build.ninja");
        fs::write(&path, manifest).unwrap();
        crate::parse::load_manifest_in(
            path.to_str().unwrap(),
            crate::os::WorkingDirectory::default(),
            crate::frontend::ManifestOptions::default(),
        )
        .unwrap()
        .graph
        .into_arenas()
    }

    fn ninja_path(path: &Path) -> String {
        path.to_string_lossy().replace(' ', "$ ")
    }

    // [spec:ronin:req:runtime.iterative-tool-traversals/test]
    #[test]
    fn deep_manifest_tools_use_bounded_call_stacks() {
        const DEPTH: usize = 4_000;

        let directory = TempDirectory::new("deep-tools");
        let mut manifest = String::from("rule emit\n  command = echo $out\n");
        let mut input = String::from("source");
        for index in 0..DEPTH {
            let output = format!("node{index}");
            let _ = writeln!(manifest, "build {output}: emit {input}");
            input = output;
        }
        let target = BString::from(input);
        std::thread::Builder::new()
            .name("deep-tool-traversals".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let graph = parse_manifest(&directory, &manifest);

                let commands = commands(&graph, std::slice::from_ref(&target)).unwrap();
                assert_eq!(commands.len(), DEPTH);
                assert_eq!(commands.first().unwrap().as_bytes(), b"echo node0");
                assert_eq!(
                    commands.last().unwrap().as_bytes(),
                    format!("echo node{}", DEPTH - 1).as_bytes()
                );

                let graphviz = super::graph(&graph, std::slice::from_ref(&target)).unwrap();
                assert!(graphviz.as_bytes().contains_str("label=\"source\""));
                assert!(
                    graphviz
                        .as_bytes()
                        .contains_str(format!("label=\"node{}\"", DEPTH - 1))
                );

                let targets = targets_with_args(&graph, &["depth".into(), "0".into()]).unwrap();
                assert_eq!(targets.lines().count(), DEPTH + 1);

                let mut cleaner = Cleaner::new(
                    true,
                    crate::os::RealDiskInterface::default(),
                    graph.edge_count(),
                );
                let target_node = nodeget(&graph, target.as_bytes()).unwrap();
                cleaner.clean_target(&graph, target_node).unwrap();
                assert_eq!(
                    cleaner
                        .visited_edges
                        .0
                        .iter()
                        .filter(|visited| **visited)
                        .count(),
                    DEPTH
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn ninja_clean_dry_run_all_target_and_rule() {
        let directory = TempDirectory::new("dry-run");
        let in1 = directory.join("in1");
        let out1 = directory.join("out1");
        let in2 = directory.join("in2");
        let out2 = directory.join("out2");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\n\
                 rule cat_e\n  command = cat -e $in > $out\n\
                 build {}: cat_e source1\n\
                 build {}: cat {}\n\
                 build {}: cat_e source2\n\
                 build {}: cat {}\n",
                in1.display(),
                out1.display(),
                in1.display(),
                in2.display(),
                out2.display(),
                in2.display(),
            ),
        );
        for path in [&in1, &out1, &in2, &out2] {
            fs::write(path, "").unwrap();
        }
        assert_eq!(clean_with_options(&graph, &[], &[], true, true).unwrap(), 4);
        assert_eq!(
            clean_with_options(
                &graph,
                &[BString::from(out1.to_string_lossy().as_bytes())],
                &[],
                true,
                true,
            )
            .unwrap(),
            2
        );
        assert_eq!(
            clean_with_options(&graph, &[], &["cat_e".into()], true, true).unwrap(),
            2
        );
        assert!([&in1, &out1, &in2, &out2].iter().all(|path| path.exists()));
    }

    #[test]
    fn ninja_clean_loads_dyndep_outputs_and_tolerates_missing_file() {
        let directory = TempDirectory::new("dyndep");
        let input = directory.join("in");
        let dyndep = directory.join("dd");
        let output = directory.join("out");
        let implicit = directory.join("out imp");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\n\
                 build {}: cat {} || {}\n  dyndep = {}\n",
                ninja_path(&output),
                ninja_path(&input),
                ninja_path(&dyndep),
                ninja_path(&dyndep),
            ),
        );
        fs::write(&input, "").unwrap();
        fs::write(
            &dyndep,
            format!(
                "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
                ninja_path(&output),
                ninja_path(&implicit)
            ),
        )
        .unwrap();
        fs::write(&output, "").unwrap();
        fs::write(&implicit, "").unwrap();
        assert_eq!(clean(&graph, &[], &[], true).unwrap(), 2);
        assert!(!output.exists() && !implicit.exists());

        fs::remove_file(&dyndep).unwrap();
        fs::write(&output, "").unwrap();
        fs::write(&implicit, "").unwrap();
        assert_eq!(clean(&graph, &[], &[], true).unwrap(), 1);
        assert!(!output.exists() && implicit.exists());
    }

    // [spec:ronin:req:runtime.dyndep-transaction/test]
    #[test]
    fn ronin_clean_propagates_dyndep_parse_failures_before_removing_outputs() {
        let directory = TempDirectory::new("dyndep-error");
        let input = directory.join("in");
        let dyndep = directory.join("dd");
        let output = directory.join("out");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\n\
                 build {}: cat {} || {}\n  dyndep = {}\n",
                ninja_path(&output),
                ninja_path(&input),
                ninja_path(&dyndep),
                ninja_path(&dyndep),
            ),
        );
        fs::write(&output, "").unwrap();
        fs::write(
            &dyndep,
            format!(
                "ninja_dyndep_version = 1\nbuild {} | invalid$!: dyndep\n",
                ninja_path(&output)
            ),
        )
        .unwrap();

        let error = clean(&graph, &[], &[], true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("bad $-escape (literal $ must be written as $$)")
        );
        assert!(output.exists());
    }

    #[test]
    fn ninja_clean_auxiliary_files_by_target_rule_and_with_spaces() {
        let directory = TempDirectory::new("auxiliary");
        let output1 = directory.join("out 1");
        let output2 = directory.join("out 2");
        let depfile = directory.join("out 1.d");
        let rspfile = directory.join("out 2.rsp");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cc_dep\n  command = cc $in > $out\n  depfile = $out.d\n\
                 rule cc_rsp\n  command = cc $in > $out\n  rspfile = $out.rsp\n  rspfile_content = $in\n\
                 build {}: cc_dep input\n\
                 build {}: cc_rsp input\n",
                ninja_path(&output1),
                ninja_path(&output2),
            ),
        );
        for path in [&output1, &output2, &depfile, &rspfile] {
            fs::write(path, "").unwrap();
        }
        assert_eq!(
            clean(
                &graph,
                &[BString::from(output1.to_string_lossy().as_bytes())],
                &[],
                true,
            )
            .unwrap(),
            2
        );
        assert!(!output1.exists() && !depfile.exists());
        assert_eq!(clean(&graph, &[], &["cc_rsp".into()], true).unwrap(), 2);
        assert!(!output2.exists() && !rspfile.exists());
    }

    #[test]
    fn ninja_clean_reports_directory_removal_failure() {
        let directory = TempDirectory::new("failure");
        let output = directory.join("output-directory");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\nbuild {}: cat input\n",
                output.display()
            ),
        );
        fs::create_dir(&output).unwrap();
        assert!(clean(&graph, &[], &[], true).is_err());
    }

    #[test]
    fn ninja_clean_phony_target_preserves_phony_output() {
        let directory = TempDirectory::new("phony-target");
        let phony = directory.join("phony");
        let target1 = directory.join("t1");
        let target2 = directory.join("t2");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = touch $out\n\
                 build {}: phony {} {}\n\
                 build {}: cat\n\
                 build {}: cat\n",
                phony.display(),
                target1.display(),
                target2.display(),
                target1.display(),
                target2.display(),
            ),
        );
        for path in [&phony, &target1, &target2] {
            fs::write(path, "").unwrap();
        }
        assert_eq!(
            clean(
                &graph,
                &[BString::from(phony.to_string_lossy().as_bytes())],
                &[],
                true,
            )
            .unwrap(),
            2
        );
        assert!(phony.exists());
        assert!(!target1.exists() && !target2.exists());
    }

    #[test]
    fn ninja_clean_dead_removes_only_unreferenced_outputs() {
        let directory = TempDirectory::new("dead");
        let input = directory.join("in");
        let output1 = directory.join("out1");
        let output2 = directory.join("out2");
        for path in [&input, &output1, &output2] {
            fs::write(path, "").unwrap();
        }
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\nbuild {}: cat {}\n",
                output2.display(),
                input.display()
            ),
        );
        let logged = [
            BString::from(output1.to_string_lossy().as_bytes()),
            BString::from(output2.to_string_lossy().as_bytes()),
        ];
        assert_eq!(clean_dead(&graph, &logged, false).unwrap(), 1);
        assert!(!output1.exists() && output2.exists());

        fs::write(&output1, "").unwrap();
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\nbuild {}: cat {} | {}\n",
                output2.display(),
                input.display(),
                output1.display()
            ),
        );
        assert_eq!(clean_dead(&graph, &logged, false).unwrap(), 0);
        assert!(output1.exists() && output2.exists());
    }

    #[test]
    fn ninja_compdb_all_rules_and_rspfile_expansion() {
        let directory = TempDirectory::new("compdb-rsp");
        let input = directory.join("in");
        let output = directory.join("out");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cc\n  command = cc @$rspfile -o $out\n  rspfile = $out.rsp\n  rspfile_content = -DVALUE $in\nbuild {}: cc {}\n",
                output.display(),
                input.display()
            ),
        );
        let regular = compdb(&graph, &[], false, &directory.0);
        let regular = regular.to_str().unwrap();
        assert!(regular.contains(&format!("@{}.rsp", output.display())));
        assert!(regular.contains("\"file\""));
        let expanded = compdb(&graph, &[], true, &directory.0);
        let expanded = expanded.to_str().unwrap();
        assert!(!expanded.contains(&format!("@{}.rsp", output.display())));
        assert!(expanded.contains("-DVALUE"));
        assert!(expanded.contains(&input.to_string_lossy().into_owned()));
        assert_eq!(
            compdb(&graph, &["other".into()], false, &directory.0).as_bytes(),
            b"[\n]\n"
        );
    }

    #[test]
    fn ninja_targets_modes_list_depth_rules_sources_and_all_outputs() {
        let directory = TempDirectory::new("targets-modes");
        let source = directory.join("source");
        let intermediate = directory.join("intermediate");
        let output = directory.join("output");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule first\n  command = touch $out\nrule second\n  command = touch $out\nbuild {}: first {}\nbuild {}: second {}\n",
                intermediate.display(),
                source.display(),
                output.display(),
                intermediate.display()
            ),
        );
        let roots = targets_with_args(&graph, &[]).unwrap();
        assert!(roots.contains(&format!("{}: second", output.display())));
        assert!(!roots.contains(&source.to_string_lossy().into_owned()));

        let depth = targets_with_args(&graph, &["depth".into(), "0".into()]).unwrap();
        assert!(depth.contains(&format!("{}: first", intermediate.display())));
        assert!(depth.contains(&source.to_string_lossy().into_owned()));

        let sources = targets_with_args(&graph, &["rule".into()]).unwrap();
        assert!(sources.contains(&source.to_string_lossy().into_owned()));
        let by_rule = targets_with_args(&graph, &["rule".into(), "first".into()]).unwrap();
        assert_eq!(by_rule.trim(), intermediate.to_string_lossy());
        let all = targets_with_args(&graph, &["all".into()]).unwrap();
        assert!(all.contains(&format!("{}: first", intermediate.display())));
        assert!(all.contains(&format!("{}: second", output.display())));
    }

    #[test]
    fn ninja_graph_emits_multi_edge_shape_and_order_only_style() {
        let directory = TempDirectory::new("graph-multi-edge");
        let build_graph = parse_manifest(
            &directory,
            "rule cat\n  command = touch $out\nbuild out1 out2: cat in1 in2 || order\n",
        );
        let dot = graph(&build_graph, &["out1".into()]).unwrap();
        let dot = dot.to_str().unwrap();
        assert!(dot.contains("shape=ellipse"));
        assert_eq!(dot.matches("arrowhead=none").count(), 3);
        assert_eq!(dot.matches("style=dotted").count(), 1);
    }

    #[test]
    fn ninja_query_prints_generating_inputs_and_dependent_outputs() {
        let directory = TempDirectory::new("query-inputs");
        let graph = parse_manifest(
            &directory,
            "rule cat\n  command = touch $out\nbuild middle: cat in1 in2\nbuild out: cat middle\n",
        );
        let report = query(&graph, &["middle".into()]).unwrap();
        assert!(report.contains("  input: cat\n    in1\n    in2\n"));
        assert!(report.contains("  outputs:\n    out\n"));
        assert!(query(&graph, &[]).is_err());
    }
}
