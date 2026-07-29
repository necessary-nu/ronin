//! Graph-inspection and cleanup tools translated from `tool.c`.

use crate::env::edgevar;
use crate::graph::{nodeget, EdgeRef, Graph, NodeRef, FLAG_WORK};
use crate::util::{BString, ByteSlice};
use std::fmt::Write as _;
use std::fs;
use std::io;

// [spec:samurai:def:tool.tool]
pub enum Tool {
    Clean,
    Commands,
    Compdb,
    Graph,
    Query,
    Targets,
}

fn edge_name(edge: &EdgeRef) -> String {
    edge.borrow()
        .rule
        .as_ref()
        .map(|rule| rule.name.clone())
        .unwrap_or_else(|| "phony".into())
}

// [spec:samurai:def:tool.cleanpath-fn]
// [spec:samurai:sem:tool.cleanpath-fn]
pub fn cleanpath(path: Option<&BString>) -> io::Result<bool> {
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

// [spec:samurai:def:tool.cleanedge-fn]
// [spec:samurai:sem:tool.cleanedge-fn]
pub fn cleanedge(edge: &EdgeRef) -> io::Result<usize> {
    cleanedge_mode(edge, false)
}

fn dyndep_outputs(edge: &EdgeRef) -> Vec<BString> {
    let Some(path) = edgevar(edge, "dyndep", false) else {
        return Vec::new();
    };
    let Ok(contents) = fs::read_to_string(path.to_path().expect("byte paths are valid on Unix"))
    else {
        return Vec::new();
    };
    let explicit_outputs = edge
        .borrow()
        .out
        .iter()
        .map(|output| {
            let output = output.borrow();
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

fn cleanedge_mode(edge: &EdgeRef, dry_run: bool) -> io::Result<usize> {
    let outputs = edge.borrow().out.clone();
    let mut removed = 0;
    for output in outputs {
        if cleanpath_mode(Some(&output.borrow().path), dry_run)? {
            removed += 1;
        }
    }
    for output in dyndep_outputs(edge) {
        if cleanpath_mode(Some(&output), dry_run)? {
            removed += 1;
        }
    }
    for variable in ["rspfile", "depfile"] {
        if cleanpath_mode(edgevar(edge, variable, false).as_ref(), dry_run)? {
            removed += 1;
        }
    }
    Ok(removed)
}

// [spec:samurai:def:tool.cleantarget-fn]
// [spec:samurai:sem:tool.cleantarget-fn]
pub fn cleantarget(node: &NodeRef) -> io::Result<usize> {
    cleantarget_mode(node, false)
}

fn cleantarget_mode(node: &NodeRef, dry_run: bool) -> io::Result<usize> {
    let edge = node.borrow().gen.as_ref().and_then(|edge| edge.upgrade());
    let Some(edge) = edge else { return Ok(0) };
    if edge_name(&edge) == "phony" {
        return edge
            .borrow()
            .input
            .clone()
            .iter()
            .try_fold(0, |count, input| {
                Ok(count + cleantarget_mode(input, dry_run)?)
            });
    }
    let mut removed = cleanedge_mode(&edge, dry_run)?;
    for input in edge.borrow().input.clone() {
        removed += cleantarget_mode(&input, dry_run)?;
    }
    Ok(removed)
}

// [spec:samurai:def:tool.clean-fn]
// [spec:samurai:sem:tool.clean-fn]
pub fn clean(
    graph: &Graph,
    targets: &[String],
    rules: &[String],
    include_generators: bool,
) -> io::Result<usize> {
    clean_with_options(graph, targets, rules, include_generators, false)
}

pub fn clean_with_options(
    graph: &Graph,
    targets: &[String],
    rules: &[String],
    include_generators: bool,
    dry_run: bool,
) -> io::Result<usize> {
    if !rules.is_empty() {
        return graph
            .edges
            .iter()
            .filter(|edge| rules.iter().any(|rule| rule == &edge_name(edge)))
            .try_fold(0, |count, edge| Ok(count + cleanedge_mode(edge, dry_run)?));
    }
    if !targets.is_empty() {
        return targets.iter().try_fold(0, |count, target| {
            let node = nodeget(graph, target.as_bytes())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, target.clone()))?;
            Ok(count + cleantarget_mode(&node, dry_run)?)
        });
    }
    graph.edges.iter().try_fold(0, |count, edge| {
        if edge_name(edge) == "phony" {
            return Ok(count);
        }
        if !include_generators && edgevar(edge, "generator", false).is_some() {
            return Ok(count);
        }
        Ok(count + cleanedge_mode(edge, dry_run)?)
    })
}

pub fn clean_dead(graph: &Graph, logged_outputs: &[String], dry_run: bool) -> io::Result<usize> {
    logged_outputs.iter().try_fold(0, |count, output| {
        if nodeget(graph, output.as_bytes()).is_some() {
            return Ok(count);
        }
        let path = crate::util::xasprintf(format_args!("{output}"));
        Ok(count + usize::from(cleanpath_mode(Some(&path), dry_run)?))
    })
}

// [spec:samurai:def:tool.targetcommands-fn]
// [spec:samurai:sem:tool.targetcommands-fn]
pub fn targetcommands(node: &NodeRef, output: &mut Vec<String>) {
    let edge = node.borrow().gen.as_ref().and_then(|edge| edge.upgrade());
    let Some(edge) = edge else { return };
    {
        let mut edge_mut = edge.borrow_mut();
        if edge_mut.flags & FLAG_WORK != 0 {
            return;
        }
        edge_mut.flags |= FLAG_WORK;
    }
    for input in edge.borrow().input.clone() {
        targetcommands(&input, output);
    }
    if let Some(command) = edgevar(&edge, "command", true) {
        if !command.is_empty() {
            output.push(String::from_utf8_lossy(command.as_bytes()).into_owned());
        }
    }
}

// [spec:samurai:def:tool.commands-fn]
// [spec:samurai:sem:tool.commands-fn]
pub fn commands(graph: &Graph, targets: &[String]) -> Result<Vec<String>, String> {
    let nodes = if targets.is_empty() {
        graph
            .nodes()
            .into_iter()
            .filter(|node| node.borrow().uses.is_empty())
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
    for node in nodes {
        targetcommands(&node, &mut output);
    }
    Ok(output)
}

// [spec:samurai:def:tool.printquoted-fn]
// [spec:samurai:sem:tool.printquoted-fn]
pub fn printquoted(bytes: &[u8], join: bool) -> String {
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

// [spec:samurai:def:tool.compdb-fn]
// [spec:samurai:sem:tool.compdb-fn]
pub fn compdb(graph: &Graph, rules: &[String], expand_rsp: bool) -> String {
    let directory = std::env::current_dir()
        .unwrap_or_default()
        .into_os_string()
        .into_encoded_bytes();
    let mut entries = Vec::new();
    for edge in &graph.edges {
        let edge_ref = edge.borrow();
        if edge_ref.input.is_empty()
            || (!rules.is_empty() && !rules.iter().any(|rule| rule == &edge_name(edge)))
        {
            continue;
        }
        let command = edgevar(edge, "command", true)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .unwrap_or_default();
        let command = if expand_rsp {
            let rspfile = edgevar(edge, "rspfile", true)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
            let content = edgevar(edge, "rspfile_content", true)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).replace(['\r', '\n'], " "))
                .unwrap_or_default();
            rspfile.map_or(command.clone(), |rspfile| {
                command.replace(&format!("@{rspfile}"), &content)
            })
        } else {
            command
        };
        let command = printquoted(command.as_bytes(), false);
        entries.push(format!(
            "{{\"directory\":\"{}\",\"command\":\"{}\",\"file\":\"{}\",\"output\":\"{}\"}}",
            printquoted(&directory, false),
            command,
            printquoted(edge_ref.input[0].borrow().path.as_bytes(), false),
            printquoted(edge_ref.out[0].borrow().path.as_bytes(), false),
        ));
    }
    format!("[{}]", entries.join(","))
}

// [spec:samurai:def:tool.graphnode-fn]
// [spec:samurai:sem:tool.graphnode-fn]
pub fn graphnode(node: &NodeRef, output: &mut String) {
    let path = node.borrow().path.clone();
    let _ = writeln!(
        output,
        "\"{:p}\" [label=\"{}\"]",
        Rc::as_ptr(node),
        printquoted(path.as_bytes(), false)
    );
    let edge = node.borrow().gen.as_ref().and_then(|edge| edge.upgrade());
    let Some(edge) = edge else { return };
    {
        let mut edge_mut = edge.borrow_mut();
        if edge_mut.flags & FLAG_WORK != 0 {
            return;
        }
        edge_mut.flags |= FLAG_WORK;
    }
    for input in edge.borrow().input.clone() {
        graphnode(&input, output);
    }
    let edge_borrow = edge.borrow();
    if edge_borrow.input.len() == 1 && edge_borrow.out.len() == 1 {
        let _ = writeln!(
            output,
            "\"{:p}\" -> \"{:p}\" [label=\"{}\"]",
            Rc::as_ptr(&edge_borrow.input[0]),
            Rc::as_ptr(&edge_borrow.out[0]),
            edge_name(&edge)
        );
    } else {
        let _ = writeln!(
            output,
            "\"{:p}\" [label=\"{}\", shape=ellipse]",
            Rc::as_ptr(&edge),
            edge_name(&edge)
        );
        for output_node in &edge_borrow.out {
            let _ = writeln!(
                output,
                "\"{:p}\" -> \"{:p}\"",
                Rc::as_ptr(&edge),
                Rc::as_ptr(output_node)
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
                "\"{:p}\" -> \"{:p}\" [arrowhead=none{}]",
                Rc::as_ptr(input),
                Rc::as_ptr(&edge),
                style
            );
        }
    }
}

// [spec:samurai:def:tool.graph-fn]
// [spec:samurai:sem:tool.graph-fn]
pub fn graph(graph: &Graph, targets: &[String]) -> Result<String, String> {
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
    for node in nodes {
        graphnode(&node, &mut output);
    }
    output.push('}');
    Ok(output)
}

// [spec:samurai:def:tool.query-fn]
// [spec:samurai:sem:tool.query-fn]
pub fn query(graph: &Graph, targets: &[String]) -> Result<String, String> {
    if targets.is_empty() {
        return Err("query expects at least one target".into());
    }
    let mut output = String::new();
    for target in targets {
        let node = nodeget(graph, target.as_bytes())
            .ok_or_else(|| format!("unknown target '{target}'"))?;
        let node_borrow = node.borrow();
        let _ = writeln!(output, "{}:", target);
        if let Some(edge) = node_borrow.gen.as_ref().and_then(|edge| edge.upgrade()) {
            let _ = writeln!(output, "  input: {}", edge_name(&edge));
            for input in &edge.borrow().input {
                let input = input.borrow();
                let _ = writeln!(
                    output,
                    "    {}",
                    String::from_utf8_lossy(input.path.as_bytes())
                );
            }
        }
        output.push_str("  outputs:\n");
        for edge in &node_borrow.uses {
            if let Some(edge) = edge.upgrade() {
                for output_node in &edge.borrow().out {
                    let path = &output_node.borrow().path;
                    let _ = writeln!(output, "    {}", String::from_utf8_lossy(path.as_bytes()));
                }
            }
        }
    }
    Ok(output)
}

// [spec:samurai:def:tool.targetsdepth-fn]
// [spec:samurai:sem:tool.targetsdepth-fn]
pub fn targetsdepth(node: &NodeRef, depth: usize, indent: usize, output: &mut String) {
    output.push_str(&"  ".repeat(indent));
    let node_borrow = node.borrow();
    if let Some(edge) = node_borrow.gen.as_ref().and_then(|edge| edge.upgrade()) {
        let _ = writeln!(
            output,
            "{}: {}",
            String::from_utf8_lossy(node_borrow.path.as_bytes()),
            edge_name(&edge)
        );
        if depth != 1 {
            let next_depth = if depth == 0 { 0 } else { depth - 1 };
            for input in edge.borrow().input.clone() {
                targetsdepth(&input, next_depth, indent + 1, output);
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
pub fn targetsusage() -> &'static str {
    "targets [depth [maxdepth]] | rule [rulename] | all"
}

// [spec:samurai:def:tool.targets-fn]
// [spec:samurai:sem:tool.targets-fn]
pub fn targets(graph: &Graph, depth: usize) -> String {
    let mut output = String::new();
    for node in graph
        .nodes()
        .into_iter()
        .filter(|node| node.borrow().uses.is_empty())
    {
        targetsdepth(&node, depth, 0, &mut output);
    }
    output
}

pub fn targets_with_args(graph: &Graph, args: &[String]) -> Result<String, String> {
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
                targetsdepth(&node, depth, 0, &mut output);
            }
            Ok(output)
        }
        Some("rule") => {
            let mut output = String::new();
            if let Some(rule) = args.get(1) {
                let outputs = graph
                    .edges
                    .iter()
                    .filter(|edge| edge_name(edge) == *rule)
                    .flat_map(|edge| edge.borrow().out.clone())
                    .map(|node| {
                        let node = node.borrow();
                        String::from_utf8_lossy(node.path.as_bytes()).into_owned()
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                for path in outputs {
                    let _ = writeln!(output, "{path}");
                }
            } else {
                for edge in &graph.edges {
                    for input in &edge.borrow().input {
                        if input.borrow().gen.is_none() {
                            let input = input.borrow();
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
            for edge in &graph.edges {
                for node in &edge.borrow().out {
                    let node = node.borrow();
                    let _ = writeln!(
                        output,
                        "{}: {}",
                        String::from_utf8_lossy(node.path.as_bytes()),
                        edge_name(edge)
                    );
                }
            }
            Ok(output)
        }
        Some(mode) => Err(format!("unknown target tool mode '{mode}'")),
    }
}

// [spec:samurai:def:tool.tool.run-fn]
// [spec:samurai:sem:tool.tool.run-fn]
pub fn run(tool: Tool, graph: &Graph, args: &[String]) -> Result<String, String> {
    match tool {
        Tool::Clean => clean(graph, args, &[], false)
            .map(|removed| removed.to_string())
            .map_err(|error| error.to_string()),
        Tool::Commands => commands(graph, args).map(|lines| lines.join("\n")),
        Tool::Compdb => Ok(compdb(graph, args, false)),
        Tool::Graph => self::graph(graph, args),
        Tool::Query => query(graph, args),
        Tool::Targets => targets_with_args(graph, args),
    }
}

// [spec:samurai:def:tool.toolget-fn]
// [spec:samurai:sem:tool.toolget-fn]
pub fn toolget(name: &str) -> Result<Tool, String> {
    match name {
        "clean" => Ok(Tool::Clean),
        "commands" => Ok(Tool::Commands),
        "compdb" => Ok(Tool::Compdb),
        "graph" => Ok(Tool::Graph),
        "query" => Ok(Tool::Query),
        "targets" => Ok(Tool::Targets),
        _ => Err(format!("unknown tool '{name}'")),
    }
}

use std::rc::Rc;

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
        let mut state = env::envinit();
        parse::parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root.clone(),
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
            output1.to_string_lossy().into_owned(),
            output2.to_string_lossy().into_owned(),
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
        assert_eq!(compdb(&graph, &["other".into()], false), "[]");
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
