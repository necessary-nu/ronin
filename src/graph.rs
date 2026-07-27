//! Literal graph ownership and dependency operations from `graph.c`.

use crate::env::{Environment, Pool, Rule};
use crate::htab::rapidhashv1;
use crate::os::{osmtime, MTIME_MISSING};
use crate::util::SamuraiString;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::rc::{Rc, Weak};

pub const MTIME_UNKNOWN: i64 = -1;

pub const FLAG_WORK: u32 = 1 << 0;
pub const FLAG_HASH: u32 = 1 << 1;
pub const FLAG_DIRTY_IN: u32 = 1 << 3;
pub const FLAG_DIRTY_OUT: u32 = 1 << 4;
pub const FLAG_DIRTY: u32 = FLAG_DIRTY_IN | FLAG_DIRTY_OUT;
pub const FLAG_CYCLE: u32 = 1 << 5;
pub const FLAG_DEPS: u32 = 1 << 6;

pub type NodeRef = Rc<RefCell<Node>>;
pub type EdgeRef = Rc<RefCell<Edge>>;

// [spec:samurai:def:graph.node]
pub struct Node {
    pub path: SamuraiString,
    pub shellpath: Option<SamuraiString>,
    pub mtime: i64,
    pub logmtime: i64,
    pub gen: Option<Weak<RefCell<Edge>>>,
    pub uses: Vec<Weak<RefCell<Edge>>>,
    pub validation_uses: Vec<Weak<RefCell<Edge>>>,
    pub hash: u64,
    pub id: i32,
    pub dirty: bool,
    pub dyndep_pending: bool,
}

// [spec:samurai:def:graph.edge]
pub struct Edge {
    pub id: usize,
    pub critical_path_weight: i64,
    pub rule: Option<Rc<Rule>>,
    pub pool: Option<Rc<RefCell<Pool>>>,
    pub env: Rc<Environment>,
    pub bindings: BTreeMap<String, SamuraiString>,
    pub out: Vec<NodeRef>,
    pub input: Vec<NodeRef>,
    pub validation: Vec<NodeRef>,
    pub dyndep: Option<NodeRef>,
    pub outimpidx: usize,
    pub inimpidx: usize,
    pub inorderidx: usize,
    pub hash: u64,
    pub nblock: usize,
    pub nprune: usize,
    pub deps_loaded: bool,
    pub deps_missing: bool,
    pub depfile_deps: usize,
    pub command_dirty: bool,
    pub restat_clean: bool,
    pub flags: u32,
}

pub struct Graph {
    nodes: BTreeMap<Vec<u8>, NodeRef>,
    pub edges: Vec<EdgeRef>,
}

impl Graph {
    pub fn nodes(&self) -> Vec<NodeRef> {
        self.nodes.values().cloned().collect()
    }
}

// [spec:samurai:def:graph.delnode-fn]
// [spec:samurai:sem:graph.delnode-fn]
pub fn delnode(_node: NodeRef) {}

// [spec:samurai:def:graph.graphinit-fn]
// [spec:samurai:sem:graph.graphinit-fn]
pub fn graphinit() -> Graph {
    Graph {
        nodes: BTreeMap::new(),
        edges: Vec::new(),
    }
}

// [spec:samurai:def:graph.mknode-fn]
// [spec:samurai:sem:graph.mknode-fn]
pub fn mknode(graph: &mut Graph, path: SamuraiString) -> NodeRef {
    let key = path.s[..path.n].to_vec();
    if let Some(node) = graph.nodes.get(&key) {
        return node.clone();
    }
    let node = Rc::new(RefCell::new(Node {
        path,
        shellpath: None,
        mtime: MTIME_UNKNOWN,
        logmtime: MTIME_MISSING,
        gen: None,
        uses: Vec::new(),
        validation_uses: Vec::new(),
        hash: 0,
        id: -1,
        dirty: false,
        dyndep_pending: false,
    }));
    graph.nodes.insert(key, node.clone());
    node
}

// [spec:samurai:def:graph.nodeget-fn]
// [spec:samurai:sem:graph.nodeget-fn]
pub fn nodeget(graph: &Graph, path: &[u8]) -> Option<NodeRef> {
    graph.nodes.get(path).cloned()
}

// [spec:samurai:def:graph.nodestat-fn]
// [spec:samurai:sem:graph.nodestat-fn]
pub fn nodestat(node: &NodeRef) -> std::io::Result<()> {
    let bytes = {
        let node = node.borrow();
        node.path.s[..node.path.n].to_vec()
    };
    let path = String::from_utf8_lossy(&bytes).into_owned();
    let mtime = osmtime(Path::new(&path))?;
    node.borrow_mut().mtime = mtime;
    Ok(())
}

pub fn nodestat_with<F>(node: &NodeRef, stat: &mut F) -> io::Result<()>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let bytes = {
        let node = node.borrow();
        node.path.s[..node.path.n].to_vec()
    };
    let path = String::from_utf8_lossy(&bytes).into_owned();
    node.borrow_mut().mtime = stat(Path::new(&path))?;
    Ok(())
}

/// Stat a dependency graph depth-first and update each node's dirty bit.
pub fn recompute_dirty_with<F>(node: &NodeRef, stat: &mut F) -> io::Result<bool>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    fn visit<F>(node: &NodeRef, stat: &mut F, visiting: &mut BTreeSet<usize>) -> io::Result<bool>
    where
        F: FnMut(&Path) -> io::Result<i64>,
    {
        let identity = Rc::as_ptr(node) as usize;
        if !visiting.insert(identity) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "dependency cycle",
            ));
        }
        let edge = node.borrow().gen.as_ref().and_then(Weak::upgrade);
        let dirty = if let Some(edge) = edge {
            if edge.borrow().restat_clean {
                for output in edge.borrow().out.clone() {
                    output.borrow_mut().dirty = false;
                }
                visiting.remove(&identity);
                return Ok(false);
            }
            let (
                inputs,
                outputs,
                inorderidx,
                is_phony,
                has_validations,
                deps_missing,
                command_dirty,
            ) = {
                let edge = edge.borrow();
                (
                    edge.input.clone(),
                    edge.out.clone(),
                    edge.inorderidx,
                    edge.rule.as_ref().is_some_and(|rule| rule.name == "phony"),
                    !edge.validation.is_empty(),
                    edge.deps_missing,
                    edge.command_dirty,
                )
            };
            for output in &outputs {
                if output.borrow().mtime == MTIME_UNKNOWN {
                    nodestat_with(output, stat)?;
                }
            }
            let mut input_dirty = false;
            for (index, input) in inputs.iter().enumerate() {
                let dirty = visit(input, stat, visiting)?;
                if index < inorderidx {
                    input_dirty |= dirty;
                }
            }
            let dirty = if is_phony {
                let missing_without_inputs = inputs.is_empty()
                    && !has_validations
                    && outputs.iter().any(|output| output.borrow().mtime == 0);
                let newest_input = inputs
                    .iter()
                    .take(inorderidx)
                    .map(|input| input.borrow().mtime)
                    .max()
                    .unwrap_or(0);
                for output in &outputs {
                    let mut output = output.borrow_mut();
                    if output.mtime == 0 {
                        output.mtime = newest_input;
                    }
                }
                input_dirty || missing_without_inputs
            } else {
                let oldest_output = outputs
                    .iter()
                    .map(|output| output.borrow().mtime)
                    .min()
                    .unwrap_or(0);
                let recorded_output_older = inputs.iter().take(inorderidx).any(|input| {
                    outputs.iter().any(|output| {
                        let output = output.borrow();
                        output.logmtime != MTIME_MISSING && input.borrow().mtime > output.logmtime
                    })
                });
                oldest_output == 0
                    || deps_missing
                    || command_dirty
                    || input_dirty
                    || recorded_output_older
                    || inputs
                        .iter()
                        .take(inorderidx)
                        .any(|input| input.borrow().mtime > oldest_output)
            };
            for output in outputs {
                output.borrow_mut().dirty = dirty;
            }
            dirty
        } else {
            if node.borrow().mtime == MTIME_UNKNOWN {
                nodestat_with(node, stat)?;
            }
            node.borrow().mtime == 0
        };
        node.borrow_mut().dirty = dirty;
        visiting.remove(&identity);
        Ok(dirty)
    }

    visit(node, stat, &mut BTreeSet::new())
}

pub fn recompute_dirty_with_validations<F>(node: &NodeRef, stat: &mut F) -> io::Result<Vec<NodeRef>>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    fn collect<F>(
        node: &NodeRef,
        stat: &mut F,
        validations: &mut Vec<NodeRef>,
        seen_nodes: &mut BTreeSet<usize>,
        seen_edges: &mut BTreeSet<usize>,
    ) -> io::Result<()>
    where
        F: FnMut(&Path) -> io::Result<i64>,
    {
        let Some(edge) = node.borrow().gen.as_ref().and_then(Weak::upgrade) else {
            return Ok(());
        };
        if !seen_edges.insert(Rc::as_ptr(&edge) as usize) {
            return Ok(());
        }
        for input in edge.borrow().input.clone() {
            collect(&input, stat, validations, seen_nodes, seen_edges)?;
        }
        for validation in edge.borrow().validation.clone() {
            let identity = Rc::as_ptr(&validation) as usize;
            if !seen_nodes.insert(identity) {
                continue;
            }
            recompute_dirty_with(&validation, stat)?;
            collect(&validation, stat, validations, seen_nodes, seen_edges)?;
            validations.push(validation);
        }
        Ok(())
    }

    recompute_dirty_with(node, stat)?;
    let mut validations = Vec::new();
    collect(
        node,
        stat,
        &mut validations,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
    )?;
    Ok(validations)
}

// [spec:samurai:def:graph.nodepath-fn]
// [spec:samurai:sem:graph.nodepath-fn]
pub fn nodepath(node: &NodeRef, escape: bool) -> SamuraiString {
    let mut node = node.borrow_mut();
    if !escape {
        return node.path.clone();
    }
    if let Some(path) = &node.shellpath {
        return path.clone();
    }
    let source = &node.path.s[..node.path.n];
    let quote = source
        .iter()
        .any(|byte| !byte.is_ascii_alphanumeric() && !b"_+-./".contains(byte));
    let mut bytes = Vec::new();
    if quote {
        bytes.push(b'\'');
        for byte in source {
            bytes.push(*byte);
            if *byte == b'\'' {
                bytes.extend_from_slice(b"\\''");
            }
        }
        bytes.push(b'\'');
    } else {
        bytes.extend_from_slice(source);
    }
    let n = bytes.len();
    bytes.push(0);
    let escaped = SamuraiString { n, s: bytes };
    node.shellpath = Some(escaped.clone());
    escaped
}

// [spec:samurai:def:graph.nodeuse-fn]
// [spec:samurai:sem:graph.nodeuse-fn]
pub fn nodeuse(node: &NodeRef, edge: &EdgeRef) {
    node.borrow_mut().uses.push(Rc::downgrade(edge));
}

// [spec:samurai:def:graph.mkedge-fn]
// [spec:samurai:sem:graph.mkedge-fn]
pub fn mkedge(graph: &mut Graph, parent: Rc<Environment>) -> EdgeRef {
    let id = graph.edges.len();
    let edge = Rc::new(RefCell::new(Edge {
        id,
        critical_path_weight: -1,
        rule: None,
        pool: None,
        env: crate::env::mkenv(Some(parent)),
        bindings: BTreeMap::new(),
        out: Vec::new(),
        input: Vec::new(),
        validation: Vec::new(),
        dyndep: None,
        outimpidx: 0,
        inimpidx: 0,
        inorderidx: 0,
        hash: 0,
        nblock: 0,
        nprune: 0,
        deps_loaded: false,
        deps_missing: false,
        depfile_deps: 0,
        command_dirty: false,
        restat_clean: false,
        flags: 0,
    }));
    graph.edges.push(edge.clone());
    edge
}

// [spec:samurai:def:graph.edgehash-fn]
// [spec:samurai:sem:graph.edgehash-fn]
pub fn edgehash(edge: &EdgeRef, command: &SamuraiString, rspfile_content: Option<&SamuraiString>) {
    let mut edge = edge.borrow_mut();
    if edge.flags & FLAG_HASH != 0 {
        return;
    }
    edge.flags |= FLAG_HASH;
    let hash = if let Some(rsp) = rspfile_content.filter(|rsp| rsp.n != 0) {
        let mut bytes = command.s[..command.n].to_vec();
        bytes.extend_from_slice(b";rspfile=");
        bytes.extend_from_slice(&rsp.s[..rsp.n]);
        rapidhashv1(&bytes)
    } else {
        rapidhashv1(&command.s[..command.n])
    };
    edge.hash = hash;
}

// [spec:samurai:def:graph.mkphony-fn]
// [spec:samurai:sem:graph.mkphony-fn]
pub fn mkphony(
    graph: &mut Graph,
    root: Rc<Environment>,
    phony: Rc<Rule>,
    node: NodeRef,
) -> EdgeRef {
    let edge = mkedge(graph, root);
    {
        let mut edge_mut = edge.borrow_mut();
        edge_mut.rule = Some(phony);
        edge_mut.outimpidx = 1;
        edge_mut.out.push(node);
    }
    edge
}

// [spec:samurai:def:graph.edgeadddeps-fn]
// [spec:samurai:sem:graph.edgeadddeps-fn]
pub fn edgeadddeps(edge: &EdgeRef, deps: &[NodeRef]) {
    for node in deps {
        nodeuse(node, edge);
    }
    let mut edge = edge.borrow_mut();
    let index = edge.inorderidx;
    edge.input.splice(index..index, deps.iter().cloned());
    edge.inorderidx += deps.len();
}

/// Return generated outputs that are not consumed by another build edge.
pub fn rootnodes(graph: &Graph) -> Result<Vec<NodeRef>, String> {
    let roots = graph
        .nodes()
        .into_iter()
        .filter(|node| {
            let node = node.borrow();
            node.gen.is_some() && node.uses.is_empty()
        })
        .collect::<Vec<_>>();
    if roots.is_empty() && !graph.edges.is_empty() {
        Err("could not determine root nodes of build graph".into())
    } else {
        Ok(roots)
    }
}

#[derive(Default)]
pub struct InputsCollector {
    inputs: Vec<NodeRef>,
    visited_nodes: BTreeSet<usize>,
}

impl InputsCollector {
    pub fn visit_node(&mut self, node: &NodeRef) {
        let Some(edge) = node.borrow().gen.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let inputs = edge.borrow().input.clone();
        for input in inputs {
            let identity = Rc::as_ptr(&input) as usize;
            if !self.visited_nodes.insert(identity) {
                continue;
            }
            self.visit_node(&input);
            let generated_by_phony = input
                .borrow()
                .gen
                .as_ref()
                .and_then(Weak::upgrade)
                .and_then(|edge| edge.borrow().rule.clone())
                .is_some_and(|rule| rule.name == "phony");
            if !generated_by_phony {
                self.inputs.push(input);
            }
        }
    }

    pub fn inputs(&self) -> &[NodeRef] {
        &self.inputs
    }

    pub fn input_strings(&self, shell_escape: bool) -> Vec<String> {
        self.inputs
            .iter()
            .map(|node| {
                let path = nodepath(node, shell_escape);
                String::from_utf8_lossy(&path.s[..path.n]).into_owned()
            })
            .collect()
    }

    pub fn reset(&mut self) {
        self.inputs.clear();
        self.visited_nodes.clear();
    }
}

#[derive(Default)]
pub struct CommandCollector {
    pub edges: Vec<EdgeRef>,
    visited_nodes: BTreeSet<usize>,
    visited_edges: BTreeSet<usize>,
}

impl CommandCollector {
    pub fn collect_from(&mut self, node: &NodeRef) {
        let node_identity = Rc::as_ptr(node) as usize;
        if !self.visited_nodes.insert(node_identity) {
            return;
        }
        let Some(edge) = node.borrow().gen.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let edge_identity = Rc::as_ptr(&edge) as usize;
        if !self.visited_edges.insert(edge_identity) {
            return;
        }
        for input in edge.borrow().input.clone() {
            self.collect_from(&input);
        }
        let is_phony = edge
            .borrow()
            .rule
            .as_ref()
            .is_some_and(|rule| rule.name == "phony");
        if !is_phony {
            self.edges.push(edge);
        }
    }
}

#[derive(Default)]
pub struct EdgePriorityQueue {
    edges: Vec<EdgeRef>,
}

impl EdgePriorityQueue {
    pub fn push(&mut self, edge: EdgeRef) {
        self.edges.push(edge);
    }

    pub fn pop(&mut self) -> Option<EdgeRef> {
        let index = self
            .edges
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                let left = left.borrow();
                let right = right.borrow();
                left.critical_path_weight
                    .cmp(&right.critical_path_weight)
                    .then_with(|| right.id.cmp(&left.id))
            })
            .map(|(index, _)| index)?;
        Some(self.edges.swap_remove(index))
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
}

pub fn verify_dag(node: &NodeRef) -> Result<(), String> {
    fn visit(
        node: &NodeRef,
        visiting: &mut Vec<NodeRef>,
        finished: &mut BTreeSet<usize>,
    ) -> Result<(), String> {
        let identity = Rc::as_ptr(node) as usize;
        if let Some(index) = visiting
            .iter()
            .position(|candidate| Rc::ptr_eq(candidate, node))
        {
            let mut paths = visiting[index..]
                .iter()
                .map(|node| {
                    let node = node.borrow();
                    String::from_utf8_lossy(&node.path.s[..node.path.n]).into_owned()
                })
                .collect::<Vec<_>>();
            let node = node.borrow();
            paths.push(String::from_utf8_lossy(&node.path.s[..node.path.n]).into_owned());
            return Err(format!("dependency cycle: {}", paths.join(" -> ")));
        }
        if finished.contains(&identity) {
            return Ok(());
        }
        visiting.push(node.clone());
        if let Some(edge) = node.borrow().gen.as_ref().and_then(Weak::upgrade) {
            for input in edge.borrow().input.clone() {
                visit(&input, visiting, finished)?;
            }
        }
        visiting.pop();
        finished.insert(identity);
        Ok(())
    }

    visit(node, &mut Vec::new(), &mut BTreeSet::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::util::xasprintf;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_GRAPH: AtomicUsize = AtomicUsize::new(0);

    fn parse_graph(source: &str) -> Graph {
        let path = std::env::temp_dir().join(format!(
            "samurai-graph-test-{}-{}.ninja",
            std::process::id(),
            NEXT_GRAPH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            format!("rule cat\n  command = cat $in > $out\n{source}"),
        )
        .unwrap();
        let mut graph = graphinit();
        let mut parser = crate::parse::parseinit();
        let mut state = crate::env::envinit();
        crate::parse::parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root.clone(),
            &mut state,
        )
        .unwrap();
        fs::remove_file(path).unwrap();
        graph
    }

    #[test]
    fn interns_nodes_and_quotes_shell_paths() {
        let mut graph = graphinit();
        let first = mknode(&mut graph, xasprintf(format_args!("a b")));
        let second = mknode(&mut graph, xasprintf(format_args!("a b")));
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(&nodepath(&first, true).s[..5], b"'a b'");
    }

    #[test]
    fn ninja_shell_path_escaping_torture_case() {
        let mut graph = graphinit();
        let node = mknode(
            &mut graph,
            xasprintf(format_args!("foo bar\"/'$@d!st!c'/path'")),
        );
        let path = nodepath(&node, true);
        assert_eq!(
            std::str::from_utf8(&path.s[..path.n]).unwrap(),
            "'foo bar\"/'\\''$@d!st!c'\\''/path'\\'''"
        );
    }

    fn generated_node(
        graph: &mut Graph,
        root: &Rc<Environment>,
        output: &str,
        inputs: &[&str],
    ) -> NodeRef {
        let output = mknode(graph, xasprintf(format_args!("{output}")));
        let edge = mkedge(graph, root.clone());
        {
            let mut edge = edge.borrow_mut();
            edge.out.push(output.clone());
            for input in inputs {
                let input = mknode(graph, xasprintf(format_args!("{input}")));
                nodeuse(&input, &graph.edges.last().unwrap().clone());
                edge.input.push(input);
            }
            edge.inimpidx = edge.input.len();
            edge.inorderidx = edge.input.len();
        }
        output.borrow_mut().gen = Some(Rc::downgrade(&edge));
        output
    }

    fn scan_graph(
        node: &NodeRef,
        mtimes: &[(&str, i64)],
        stats: &mut Vec<String>,
    ) -> io::Result<()> {
        let mtimes = mtimes
            .iter()
            .map(|(path, mtime)| (path.to_string(), *mtime))
            .collect::<BTreeMap<_, _>>();
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy().into_owned();
            stats.push(path.clone());
            Ok(*mtimes.get(&path).unwrap_or(&0))
        };
        nodestat_with(node, &mut stat)?;
        recompute_dirty_with(node, &mut stat)?;
        Ok(())
    }

    #[test]
    fn ninja_stat_scan_simple() {
        let mut graph = graphinit();
        let root = mkenv(None);
        let output = generated_node(&mut graph, &root, "out", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "in"]);
    }

    #[test]
    fn ninja_stat_scan_two_step() {
        let mut graph = graphinit();
        let root = mkenv(None);
        let output = generated_node(&mut graph, &root, "out", &["mid"]);
        let middle = generated_node(&mut graph, &root, "mid", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "mid", "in"]);
        assert!(output.borrow().dirty);
        assert!(middle.borrow().dirty);
    }

    #[test]
    fn ninja_stat_scan_tree() {
        let mut graph = graphinit();
        let root = mkenv(None);
        let output = generated_node(&mut graph, &root, "out", &["mid1", "mid2"]);
        let middle1 = generated_node(&mut graph, &root, "mid1", &["in11", "in12"]);
        generated_node(&mut graph, &root, "mid2", &["in21", "in22"]);
        let mut stats = Vec::new();
        scan_graph(&output, &[], &mut stats).unwrap();
        assert_eq!(
            stats,
            ["out", "mid1", "in11", "in12", "mid2", "in21", "in22"]
        );
        assert!(middle1.borrow().dirty);
    }

    #[test]
    fn ninja_stat_scan_middle_missing() {
        let mut graph = graphinit();
        let root = mkenv(None);
        let output = generated_node(&mut graph, &root, "out", &["mid"]);
        let middle = generated_node(&mut graph, &root, "mid", &["in"]);
        let input = nodeget(&graph, b"in").unwrap();
        let mut stats = Vec::new();
        scan_graph(&output, &[("in", 1), ("mid", 0), ("out", 1)], &mut stats).unwrap();
        assert!(!input.borrow().dirty);
        assert!(middle.borrow().dirty);
        assert!(output.borrow().dirty);
    }

    #[test]
    fn ninja_state_basic_command_evaluation() {
        fn text(
            value: &str,
            next: Option<Box<crate::util::EvalString>>,
        ) -> crate::util::EvalString {
            crate::util::EvalString {
                var: None,
                string: Some(xasprintf(format_args!("{value}"))),
                next,
            }
        }

        fn variable(
            name: &str,
            next: Option<Box<crate::util::EvalString>>,
        ) -> crate::util::EvalString {
            crate::util::EvalString {
                var: Some(name.as_bytes().to_vec()),
                string: None,
                next,
            }
        }

        let state = crate::env::envinit();
        let rule = crate::env::mkrule("cat".into());
        let command = text(
            "cat ",
            Some(Box::new(variable(
                "in",
                Some(Box::new(text(" > ", Some(Box::new(variable("out", None)))))),
            ))),
        );
        crate::env::ruleaddvar(&rule, "command".into(), command);

        let mut graph = graphinit();
        let edge = mkedge(&mut graph, state.root);
        edge.borrow_mut().rule = Some(rule);
        let input1 = mknode(&mut graph, xasprintf(format_args!("in1")));
        let input2 = mknode(&mut graph, xasprintf(format_args!("in2")));
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        {
            let mut edge = edge.borrow_mut();
            edge.input.extend([input1.clone(), input2.clone()]);
            edge.inimpidx = 2;
            edge.inorderidx = 2;
            edge.out.push(output.clone());
            edge.outimpidx = 1;
        }
        let command = crate::env::edgevar(&edge, "command", false).unwrap();
        assert_eq!(&command.s[..command.n], b"cat in1 in2 > out");
        assert!(!input1.borrow().dirty);
        assert!(!input2.borrow().dirty);
        assert!(!output.borrow().dirty);
    }

    #[test]
    fn ninja_graph_root_nodes() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\n",
        );
        let roots = rootnodes(&graph).unwrap();
        assert_eq!(roots.len(), 4);
        assert!(roots.iter().all(|node| {
            let node = node.borrow();
            node.path.s[..node.path.n].starts_with(b"out")
        }));
    }

    #[test]
    fn ninja_graph_inputs_collector() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\nbuild all: phony out1 out2 out3\n",
        );
        let mut collector = InputsCollector::default();
        collector.visit_node(&nodeget(&graph, b"out1").unwrap());
        assert_eq!(collector.input_strings(false), ["in1"]);
        collector.visit_node(&nodeget(&graph, b"out2").unwrap());
        assert_eq!(collector.input_strings(false), ["in1", "mid1"]);
        collector.visit_node(&nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(false),
            ["in1", "mid1", "out1", "out2", "out3"]
        );

        collector.reset();
        collector.visit_node(&nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(false),
            ["in1", "out1", "mid1", "out2", "out3"]
        );
    }

    #[test]
    fn ninja_graph_inputs_collector_with_escapes() {
        let graph =
            parse_graph("build out$ 1: cat in1 in2 in$ with$ space | implicit || order_only\n");
        let mut collector = InputsCollector::default();
        collector.visit_node(&nodeget(&graph, b"out 1").unwrap());
        assert_eq!(
            collector.input_strings(false),
            ["in1", "in2", "in with space", "implicit", "order_only"]
        );
        assert_eq!(
            collector.input_strings(true),
            ["in1", "in2", "'in with space'", "implicit", "order_only"]
        );
    }

    fn commands(collector: &CommandCollector) -> Vec<String> {
        collector
            .edges
            .iter()
            .map(|edge| {
                let command = crate::env::edgevar(edge, "command", false).unwrap();
                String::from_utf8_lossy(&command.s[..command.n]).into_owned()
            })
            .collect()
    }

    fn recompute_with_mtimes(
        graph: &Graph,
        target: &[u8],
        mtimes: &[(&str, i64)],
    ) -> io::Result<bool> {
        let mtimes = mtimes
            .iter()
            .map(|(path, mtime)| (path.to_string(), *mtime))
            .collect::<BTreeMap<_, _>>();
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy();
            Ok(*mtimes.get(path.as_ref()).unwrap_or(&0))
        };
        recompute_dirty_with(&nodeget(graph, target).unwrap(), &mut stat)
    }

    #[test]
    fn ninja_graph_command_collector() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\nbuild all: phony out1 out2 out3\n",
        );
        let mut collector = CommandCollector::default();
        collector.collect_from(&nodeget(&graph, b"out2").unwrap());
        assert_eq!(commands(&collector), ["cat in1 > mid1", "cat mid1 > out2"]);
        collector.collect_from(&nodeget(&graph, b"out1").unwrap());
        assert_eq!(
            commands(&collector),
            ["cat in1 > mid1", "cat mid1 > out2", "cat in1 > out1"]
        );
        collector.collect_from(&nodeget(&graph, b"all").unwrap());
        assert_eq!(
            commands(&collector),
            [
                "cat in1 > mid1",
                "cat mid1 > out2",
                "cat in1 > out1",
                "cat mid1 > out3 out4"
            ]
        );

        let mut collector = CommandCollector::default();
        collector.collect_from(&nodeget(&graph, b"all").unwrap());
        assert_eq!(
            commands(&collector),
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
        let edge = nodeget(&graph, b"a b")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        let command = crate::env::edgevar(&edge, "command", true).unwrap();
        assert_eq!(
            &command.s[..command.n],
            b"cat 'no'\\''space' 'with space$' 'no\"space2' > 'a b'"
        );
    }

    #[test]
    fn ninja_graph_rule_variables_are_in_scope() {
        let graph = parse_graph(
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n",
        );
        let edge = nodeget(&graph, b"out")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        let command = crate::env::edgevar(&edge, "command", false).unwrap();
        assert_eq!(&command.s[..command.n], b"depfile is x");
    }

    #[test]
    fn ninja_graph_edge_binding_overrides_rule_binding() {
        let graph = parse_graph(
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n  depfile = y\n",
        );
        let edge = nodeget(&graph, b"out")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        let depfile = crate::env::edgevar(&edge, "depfile", false).unwrap();
        let command = crate::env::edgevar(&edge, "command", false).unwrap();
        assert_eq!(&depfile.s[..depfile.n], b"y");
        assert_eq!(&command.s[..command.n], b"depfile is y");
    }

    #[test]
    fn ninja_graph_dependency_cycle() {
        let graph = parse_graph(
            "build out: cat mid\nbuild mid: cat in\nbuild in: cat pre\nbuild pre: cat out\n",
        );
        assert_eq!(
            verify_dag(&nodeget(&graph, b"out").unwrap()),
            Err("dependency cycle: out -> mid -> in -> pre -> out".into())
        );
    }

    #[test]
    fn ninja_graph_cycle_in_multi_output_edge() {
        let graph = parse_graph("build a b: cat a\n");
        assert_eq!(
            verify_dag(&nodeget(&graph, b"b").unwrap()),
            Err("dependency cycle: a -> a".into())
        );
    }

    #[test]
    fn ninja_graph_edge_queue_priority() {
        let graph = parse_graph("build out1: cat in1\nbuild out2: cat in2\nbuild out3: cat in3\n");
        let edges = ["out1", "out2", "out3"].map(|output| {
            nodeget(&graph, output.as_bytes())
                .unwrap()
                .borrow()
                .gen
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap()
        });
        for (index, edge) in edges.iter().enumerate() {
            edge.borrow_mut().critical_path_weight = index as i64 * 10;
        }
        let mut queue = EdgePriorityQueue::default();
        for edge in &edges {
            queue.push(edge.clone());
        }
        assert_eq!(queue.len(), 3);
        for expected in edges.iter().rev() {
            assert!(Rc::ptr_eq(&queue.pop().unwrap(), expected));
        }
        assert!(queue.is_empty());

        for edge in &edges {
            edge.borrow_mut().critical_path_weight = 0;
        }
        queue.push(edges[1].clone());
        queue.push(edges[2].clone());
        queue.push(edges[0].clone());
        for expected in &edges {
            assert!(Rc::ptr_eq(&queue.pop().unwrap(), expected));
        }
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
        assert!(!recompute_with_mtimes(
            &graph,
            b"out",
            &[("in", 1), ("out", 1), ("order_only", 2)]
        )
        .unwrap());
    }

    #[test]
    fn ninja_graph_missing_implicit_output_dirties_all_outputs() {
        let graph = parse_graph("build out | out.imp: cat in\n");
        assert!(recompute_with_mtimes(&graph, b"out", &[("in", 1), ("out", 1)]).unwrap());
        assert!(nodeget(&graph, b"out").unwrap().borrow().dirty);
        assert!(nodeget(&graph, b"out.imp").unwrap().borrow().dirty);
    }

    #[test]
    fn ninja_graph_old_implicit_output_dirties_all_outputs() {
        let graph = parse_graph("build out | out.imp: cat in\n");
        assert!(
            recompute_with_mtimes(&graph, b"out", &[("out.imp", 1), ("in", 2), ("out", 2)])
                .unwrap()
        );
        assert!(nodeget(&graph, b"out").unwrap().borrow().dirty);
        assert!(nodeget(&graph, b"out.imp").unwrap().borrow().dirty);
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
        let validations =
            recompute_dirty_with_validations(&nodeget(&graph, b"out").unwrap(), &mut stat).unwrap();
        assert_eq!(validations.len(), 1);
        assert!(nodeget(&graph, b"out").unwrap().borrow().dirty);
        assert!(nodeget(&graph, b"validate").unwrap().borrow().dirty);
    }

    #[test]
    fn ninja_graph_phony_dependency_propagates_mtime() {
        let graph = parse_graph("build in_ph: phony in1\nbuild out1: cat in_ph\n");
        assert!(!recompute_with_mtimes(&graph, b"out1", &[("in1", 1), ("out1", 2)]).unwrap());
        for node in graph.nodes() {
            let mut node = node.borrow_mut();
            node.mtime = MTIME_UNKNOWN;
            node.dirty = false;
        }
        assert!(recompute_with_mtimes(&graph, b"out1", &[("in1", 3), ("out1", 2)]).unwrap());
    }

    #[test]
    fn ninja_graph_phony_output_with_validation_is_clean() {
        let graph = parse_graph("build valid: phony\nbuild out: phony |@ valid\n");
        let mut stat = |_path: &Path| Ok(0);
        let validations =
            recompute_dirty_with_validations(&nodeget(&graph, b"out").unwrap(), &mut stat).unwrap();
        assert!(!nodeget(&graph, b"out").unwrap().borrow().dirty);
        assert_eq!(validations.len(), 1);
        assert_eq!(
            &validations[0].borrow().path.s[..validations[0].borrow().path.n],
            b"valid"
        );
    }
}
