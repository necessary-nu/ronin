//! Graph-inspection and cleanup tools translated from `tool.c`.

use crate::env::edgevar;
use crate::error::ToolError;
use crate::graph::{nodeget, EdgeId, Graph, NodeId};
use crate::util::{BString, ByteSlice};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;

type ToolResult<T> = Result<T, ToolError>;

mod compdb;
mod input;
mod state;
#[cfg(test)]
mod test_support;
mod urtle;

pub(crate) use urtle::decode as urtle;

pub(crate) use compdb::{compdb, compdb_for_targets};
pub(crate) use input::{inputs, multi_inputs};
pub(crate) use state::{deps, missing_deps};

// [spec:samurai:def:tool.tool]
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
    pub(crate) fn stage(self) -> ToolStage {
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
    graph
        .edge(edge)
        .rule
        .map(|rule| graph.rule(rule).name.clone())
        .unwrap_or_else(|| "phony".into())
}

// [spec:samurai:def:tool.cleanpath-fn]
// [spec:samurai:sem:tool.cleanpath-fn]
#[cfg(test)]
pub(crate) fn cleanpath(path: Option<&BString>) -> io::Result<bool> {
    cleanpath_mode(path, false)
}

fn cleanpath_mode(path: Option<&BString>, dry_run: bool) -> io::Result<bool> {
    let Some(path) = path else { return Ok(false) };
    if dry_run {
        return match fs::symlink_metadata(path.to_path().expect("byte paths are valid on Unix")) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        };
    }
    match fs::remove_file(path.to_path().expect("byte paths are valid on Unix")) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[derive(Default)]
struct Cleaner {
    dry_run: bool,
    seen_paths: BTreeSet<BString>,
    visited_edges: BTreeSet<EdgeId>,
    removed: Vec<BString>,
}

impl Cleaner {
    fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            ..Self::default()
        }
    }

    fn remove(&mut self, path: Option<&BString>) -> io::Result<()> {
        let Some(path) = path else { return Ok(()) };
        if self.seen_paths.insert(path.clone()) && cleanpath_mode(Some(path), self.dry_run)? {
            self.removed.push(path.clone());
        }
        Ok(())
    }

    // [spec:samurai:def:tool.cleanedge-fn]
    // [spec:samurai:sem:tool.cleanedge-fn]
    fn clean_edge(&mut self, graph: &Graph, edge: EdgeId) -> io::Result<()> {
        if !self.visited_edges.insert(edge) {
            return Ok(());
        }
        for output in graph.edge(edge).out.clone() {
            self.remove(Some(&graph.node(output).path))?;
        }
        for output in dyndep_outputs(graph, edge) {
            self.remove(Some(&output))?;
        }
        for variable in ["rspfile", "depfile"] {
            self.remove(edgevar(graph, edge, variable, false).as_ref())?;
        }
        Ok(())
    }

    // [spec:samurai:def:tool.cleantarget-fn]
    // [spec:samurai:sem:tool.cleantarget-fn]
    fn clean_target(&mut self, graph: &Graph, node: NodeId) -> io::Result<()> {
        let Some(edge) = graph.node(node).gen else {
            return Ok(());
        };
        if !self.visited_edges.insert(edge) {
            return Ok(());
        }
        if edge_name(graph, edge) == "phony" {
            for input in graph.edge(edge).input.clone() {
                self.clean_target(graph, input)?;
            }
            return Ok(());
        }
        for output in graph.edge(edge).out.clone() {
            self.remove(Some(&graph.node(output).path))?;
        }
        for output in dyndep_outputs(graph, edge) {
            self.remove(Some(&output))?;
        }
        for variable in ["rspfile", "depfile"] {
            self.remove(edgevar(graph, edge, variable, false).as_ref())?;
        }
        for input in graph.edge(edge).input.clone() {
            self.clean_target(graph, input)?;
        }
        Ok(())
    }
}

fn dyndep_outputs(graph: &Graph, edge: EdgeId) -> Vec<BString> {
    let Some(path) = edgevar(graph, edge, "dyndep", false) else {
        return Vec::new();
    };
    let Ok(contents) = fs::read_to_string(path.to_path().expect("byte paths are valid on Unix"))
    else {
        return Vec::new();
    };
    let explicit_outputs = graph
        .edge(edge)
        .out
        .iter()
        .map(|output| {
            let output = graph.node(*output);
            String::from_utf8_lossy(output.path.as_bytes()).into_owned()
        })
        .collect::<Vec<_>>();
    let mut outputs = Vec::new();
    for line in contents.lines() {
        let Some(statement) = line.strip_prefix("build ") else {
            continue;
        };
        let Some((outputs_text, _)) = statement.split_once(':') else {
            continue;
        };
        let Some((explicit, implicit)) = outputs_text.split_once('|') else {
            continue;
        };
        if !explicit_outputs
            .iter()
            .any(|output| explicit.split_whitespace().any(|path| path == output))
        {
            continue;
        }
        for output in implicit.split_whitespace() {
            outputs.push(crate::util::xasprintf(format_args!("{output}")));
        }
    }
    outputs
}

// [spec:samurai:def:tool.clean-fn]
// [spec:samurai:sem:tool.clean-fn]
pub(crate) fn clean(
    graph: &Graph,
    targets: &[String],
    rules: &[String],
    include_generators: bool,
) -> io::Result<usize> {
    clean_with_options(graph, targets, rules, include_generators, false)
}

pub(crate) fn clean_with_options(
    graph: &Graph,
    targets: &[String],
    rules: &[String],
    include_generators: bool,
    dry_run: bool,
) -> io::Result<usize> {
    clean_with_report(graph, targets, rules, include_generators, dry_run)
        .map(|removed| removed.len())
}

pub(crate) fn clean_with_report(
    graph: &Graph,
    targets: &[String],
    rules: &[String],
    include_generators: bool,
    dry_run: bool,
) -> io::Result<Vec<BString>> {
    let mut cleaner = Cleaner::new(dry_run);
    if !rules.is_empty() {
        for edge in graph
            .edge_ids()
            .into_iter()
            .filter(|edge| rules.iter().any(|rule| rule == &edge_name(graph, *edge)))
        {
            cleaner.clean_edge(graph, edge)?;
        }
    } else if !targets.is_empty() {
        for target in targets {
            let node = nodeget(graph, target.as_bytes())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, target.clone()))?;
            cleaner.clean_target(graph, node)?;
        }
    } else {
        for edge in graph.edge_ids() {
            if edge_name(graph, edge) == "phony" {
                continue;
            }
            if !include_generators && edgevar(graph, edge, "generator", false).is_some() {
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
) -> io::Result<usize> {
    clean_dead_with_report(graph, logged_outputs, dry_run).map(|removed| removed.len())
}

pub(crate) fn clean_dead_with_report(
    graph: &Graph,
    logged_outputs: &[BString],
    dry_run: bool,
) -> io::Result<Vec<BString>> {
    let mut cleaner = Cleaner::new(dry_run);
    for output in logged_outputs {
        if nodeget(graph, output.as_bytes()).is_some() {
            continue;
        }
        cleaner.remove(Some(output))?;
    }
    Ok(cleaner.removed)
}

// [spec:samurai:def:tool.targetcommands-fn]
// [spec:samurai:sem:tool.targetcommands-fn]
fn collect_target_commands(
    graph: &Graph,
    node: NodeId,
    output: &mut Vec<String>,
    visited: &mut BTreeSet<EdgeId>,
) {
    let Some(edge) = graph.node(node).gen else {
        return;
    };
    if !visited.insert(edge) {
        return;
    }
    for input in graph.edge(edge).input.iter().copied() {
        collect_target_commands(graph, input, output, visited);
    }
    if let Some(command) = edgevar(graph, edge, "command", true) {
        if !command.is_empty() {
            output.push(String::from_utf8_lossy(command.as_bytes()).into_owned());
        }
    }
}

// [spec:samurai:def:tool.commands-fn]
// [spec:samurai:sem:tool.commands-fn]
pub(crate) fn commands(graph: &Graph, targets: &[String]) -> ToolResult<Vec<String>> {
    let nodes = if targets.is_empty() {
        graph
            .nodes()
            .into_iter()
            .filter(|node| graph.node(*node).uses.is_empty())
            .collect()
    } else {
        targets
            .iter()
            .map(|target| {
                nodeget(graph, target.as_bytes())
                    .ok_or_else(|| format!("unknown target '{target}'"))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut output = Vec::new();
    let mut visited = BTreeSet::new();
    for node in nodes {
        collect_target_commands(graph, node, &mut output, &mut visited);
    }
    Ok(output)
}

pub(crate) fn commands_with_args(graph: &Graph, arguments: &[String]) -> ToolResult<String> {
    let mut single = false;
    let mut targets = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-s" => single = true,
            "-h" | "--help" => {
                return Err(
                    "usage: ronin -t commands [options] [targets]\n\noptions:\n  -s     only print the final command to build [target], not the whole chain"
                        .into(),
                )
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown commands option '{option}'").into())
            }
            target => targets.push(target.to_owned()),
        }
    }
    if !single {
        return commands(graph, &targets).map(|commands| commands.join("\n"));
    }
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for target in targets {
        let node = nodeget(graph, target.as_bytes())
            .ok_or_else(|| format!("unknown target '{target}'"))?;
        let Some(edge) = graph.node(node).gen else {
            continue;
        };
        if !seen.insert(edge) {
            continue;
        }
        if let Some(command) =
            edgevar(graph, edge, "command", true).filter(|value| !value.is_empty())
        {
            output.push(String::from_utf8_lossy(command.as_bytes()).into_owned());
        }
    }
    Ok(output.join("\n"))
}

// [spec:samurai:def:tool.printquoted-fn]
// [spec:samurai:sem:tool.printquoted-fn]
pub(crate) fn printquoted(bytes: &[u8], join: bool) -> String {
    let mut output = String::new();
    for byte in bytes {
        match byte {
            0 => break,
            b'"' | b'\\' => {
                output.push('\\');
                output.push(*byte as char);
            }
            b'\n' if join => output.push(' '),
            b'\n' => {}
            _ => output.push(*byte as char),
        }
    }
    output
}

// [spec:samurai:def:tool.graphnode-fn]
// [spec:samurai:sem:tool.graphnode-fn]
fn graphnode_inner(
    graph: &Graph,
    node: NodeId,
    output: &mut String,
    visited: &mut BTreeSet<EdgeId>,
) {
    let path = &graph.node(node).path;
    let _ = writeln!(
        output,
        "\"n{}\" [label=\"{}\"]",
        node.index(),
        printquoted(path.as_bytes(), false)
    );
    let Some(edge) = graph.node(node).gen else {
        return;
    };
    if !visited.insert(edge) {
        return;
    }
    for input in graph.edge(edge).input.iter().copied() {
        graphnode_inner(graph, input, output, visited);
    }
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
            let style = if index >= edge_borrow.inorderidx {
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

// [spec:samurai:def:tool.graph-fn]
// [spec:samurai:sem:tool.graph-fn]
pub(crate) fn graph(graph: &Graph, targets: &[String]) -> ToolResult<String> {
    let nodes = if targets.is_empty() {
        crate::graph::rootnodes(graph)?
    } else {
        targets
            .iter()
            .map(|target| {
                nodeget(graph, target.as_bytes())
                    .ok_or_else(|| format!("unknown target '{target}'"))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut output = "digraph ninja {\nrankdir=\"LR\"\n".to_owned();
    output.push_str("node [fontsize=10, shape=box, height=0.25]\n");
    output.push_str("edge [fontsize=10]\n");
    let mut visited = BTreeSet::new();
    for node in nodes {
        graphnode_inner(graph, node, &mut output, &mut visited);
    }
    output.push('}');
    Ok(output)
}

// [spec:samurai:def:tool.query-fn]
// [spec:samurai:sem:tool.query-fn]
pub(crate) fn query(graph: &Graph, targets: &[String]) -> ToolResult<String> {
    if targets.is_empty() {
        return Err("query expects at least one target".into());
    }
    let mut output = String::new();
    for target in targets {
        let node = nodeget(graph, target.as_bytes())
            .ok_or_else(|| format!("unknown target '{target}'"))?;
        let node_borrow = graph.node(node);
        let _ = writeln!(output, "{}:", target);
        if let Some(edge) = node_borrow.gen {
            let _ = writeln!(output, "  input: {}", edge_name(graph, edge));
            for (index, input) in graph.edge(edge).input.iter().enumerate() {
                let input = graph.node(*input);
                let label = if index >= graph.edge(edge).inorderidx {
                    "|| "
                } else if index >= graph.edge(edge).inimpidx {
                    "| "
                } else {
                    ""
                };
                let _ = writeln!(
                    output,
                    "    {label}{}",
                    String::from_utf8_lossy(input.path.as_bytes())
                );
            }
            if !graph.edge(edge).validation.is_empty() {
                output.push_str("  validations:\n");
                for validation in &graph.edge(edge).validation {
                    let _ = writeln!(
                        output,
                        "    {}",
                        String::from_utf8_lossy(graph.node(*validation).path.as_bytes())
                    );
                }
            }
        }
        output.push_str("  outputs:\n");
        for edge in &node_borrow.uses {
            for output_node in &graph.edge(*edge).out {
                let path = &graph.node(*output_node).path;
                let _ = writeln!(output, "    {}", String::from_utf8_lossy(path.as_bytes()));
            }
        }
        if !node_borrow.validation_uses.is_empty() {
            output.push_str("  validation for:\n");
            for edge in &node_borrow.validation_uses {
                for output_node in &graph.edge(*edge).out {
                    let path = &graph.node(*output_node).path;
                    let _ = writeln!(output, "    {}", String::from_utf8_lossy(path.as_bytes()));
                }
            }
        }
    }
    Ok(output)
}

// [spec:samurai:def:tool.targetsdepth-fn]
// [spec:samurai:sem:tool.targetsdepth-fn]
pub(crate) fn targetsdepth(
    graph: &Graph,
    node: NodeId,
    depth: usize,
    indent: usize,
    output: &mut String,
) {
    output.push_str(&"  ".repeat(indent));
    let node_borrow = graph.node(node);
    if let Some(edge) = node_borrow.gen {
        let _ = writeln!(
            output,
            "{}: {}",
            String::from_utf8_lossy(node_borrow.path.as_bytes()),
            edge_name(graph, edge)
        );
        if depth != 1 {
            let next_depth = if depth == 0 { 0 } else { depth - 1 };
            for input in graph.edge(edge).input.iter().copied() {
                targetsdepth(graph, input, next_depth, indent + 1, output);
            }
        }
    } else {
        let _ = writeln!(
            output,
            "{}",
            String::from_utf8_lossy(node_borrow.path.as_bytes())
        );
    }
}

// [spec:samurai:def:tool.targetsusage-fn]
// [spec:samurai:sem:tool.targetsusage-fn]
pub(crate) fn targetsusage() -> &'static str {
    "targets [depth [maxdepth]] | rule [rulename] | all"
}

// [spec:samurai:def:tool.targets-fn]
// [spec:samurai:sem:tool.targets-fn]
pub(crate) fn targets_with_args(graph: &Graph, args: &[String]) -> ToolResult<String> {
    if args.len() > 2 {
        return Err(targetsusage().into());
    }
    match args.first().map(String::as_str) {
        None | Some("depth") => {
            let depth = args
                .get(1)
                .map(|depth| {
                    depth
                        .parse::<usize>()
                        .map_err(|_| targetsusage().to_owned())
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
                    .into_iter()
                    .filter(|edge| edge_name(graph, *edge) == *rule)
                    .flat_map(|edge| graph.edge(edge).out.clone())
                    .map(|node| {
                        let node = graph.node(node);
                        String::from_utf8_lossy(node.path.as_bytes()).into_owned()
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                for path in outputs {
                    let _ = writeln!(output, "{path}");
                }
            } else {
                for edge in graph.edge_ids() {
                    for input in &graph.edge(edge).input {
                        if graph.node(*input).gen.is_none() {
                            let input = graph.node(*input);
                            let _ = writeln!(
                                output,
                                "{}",
                                String::from_utf8_lossy(input.path.as_bytes())
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
                    let node = graph.node(*node);
                    let _ = writeln!(
                        output,
                        "{}: {}",
                        String::from_utf8_lossy(node.path.as_bytes()),
                        edge_name(graph, edge)
                    );
                }
            }
            Ok(output)
        }
        Some(mode) => Err(format!("unknown target tool mode '{mode}'").into()),
    }
}

pub(crate) fn rules(graph: &Graph, arguments: &[String]) -> ToolResult<String> {
    let mut descriptions = false;
    for argument in arguments {
        match argument.as_str() {
            "-d" => descriptions = true,
            "-h" | "--help" => {
                return Err(
                    "usage: ronin -t rules [options]\n\noptions:\n  -d     also print the description of the rule\n  -h     print this message"
                        .into(),
                )
            }
            option => return Err(format!("unknown rules option '{option}'").into()),
        }
    }
    let mut rules = graph
        .rule_ids()
        .map(|rule| (graph.rule(rule).name.clone(), rule))
        .collect::<Vec<_>>();
    rules.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = String::new();
    for (name, rule) in rules {
        output.push_str(&name);
        if descriptions {
            if let Some(description) = graph.rule(rule).bindings.get("description") {
                output.push_str(": ");
                for part in &description.parts {
                    match part {
                        crate::util::EvalPart::Literal(value) => {
                            output.push_str(&String::from_utf8_lossy(value.as_bytes()));
                        }
                        crate::util::EvalPart::Variable(name) => {
                            let _ = write!(output, "${{{name}}}");
                        }
                    }
                }
            }
        }
        output.push('\n');
    }
    Ok(output)
}

// [spec:samurai:def:tool.tool.run-fn]
// [spec:samurai:sem:tool.tool.run-fn]
pub(crate) fn run(tool: Tool, graph: &Graph, args: &[String]) -> ToolResult<String> {
    match tool {
        Tool::Clean => Ok(clean(graph, args, &[], false)?.to_string()),
        Tool::Commands => commands_with_args(graph, args),
        Tool::Compdb => Ok(compdb(graph, args, false)),
        Tool::CompdbTargets => compdb_for_targets(graph, args, false),
        Tool::Graph => self::graph(graph, args),
        Tool::Query => query(graph, args),
        Tool::Targets => targets_with_args(graph, args),
        Tool::Rules => rules(graph, args),
        Tool::Inputs => inputs(graph, args),
        Tool::MultiInputs => multi_inputs(graph, args),
        Tool::List => Ok(tool_list()),
        Tool::Browse => Err("browse tool not supported on this platform".into()),
        Tool::Deps
        | Tool::MissingDeps
        | Tool::Recompact
        | Tool::Restat
        | Tool::CleanDead
        | Tool::Urtle => Err("tool requires runtime state".into()),
    }
}

// [spec:samurai:def:tool.toolget-fn]
// [spec:samurai:sem:tool.toolget-fn]
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
    Err(suggestion
        .map_or_else(
            || format!("fatal: unknown tool '{name}'"),
            |suggestion| format!("fatal: unknown tool '{name}', did you mean '{suggestion}'?"),
        )
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{env, graph, parse};
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
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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
        let mut graph = graph::graphinit();
        let mut parser = parse::parseinit();
        let mut state = env::envinit(&mut graph);
        parse::parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        )
        .unwrap();
        graph
    }

    fn ninja_path(path: &Path) -> String {
        path.to_string_lossy().replace(' ', "$ ")
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
                &[out1.to_string_lossy().into_owned()],
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
        let implicit = directory.join("out.imp");
        let graph = parse_manifest(
            &directory,
            &format!(
                "rule cat\n  command = cat $in > $out\n\
                 build {}: cat {} || {}\n  dyndep = {}\n",
                output.display(),
                input.display(),
                dyndep.display(),
                dyndep.display(),
            ),
        );
        fs::write(&input, "").unwrap();
        fs::write(
            &dyndep,
            format!(
                "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
                output.display(),
                implicit.display()
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
            clean(&graph, &[output1.to_string_lossy().into_owned()], &[], true,).unwrap(),
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
            clean(&graph, &[phony.to_string_lossy().into_owned()], &[], true,).unwrap(),
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
        let regular = compdb(&graph, &[], false);
        assert!(regular.contains(&format!("@{}.rsp", output.display())));
        assert!(regular.contains("\"file\""));
        let expanded = compdb(&graph, &[], true);
        assert!(!expanded.contains(&format!("@{}.rsp", output.display())));
        assert!(expanded.contains("-DVALUE"));
        assert!(expanded.contains(&input.to_string_lossy().into_owned()));
        assert_eq!(compdb(&graph, &["other".into()], false), "[]\n");
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
