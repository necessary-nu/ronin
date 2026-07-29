//! Dense graph arenas and dependency operations.

use crate::env::{Environment, EnvironmentId, Pool, PoolId, Rule, RuleId};
use crate::htab::rapidhashv1;
use crate::os::{osmtime, MTIME_MISSING};
use crate::util::{BStr, BString, ByteSlice};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::io;
use std::path::Path;

pub const MTIME_UNKNOWN: i64 = -1;

pub const FLAG_WORK: u32 = 1 << 0;
pub const FLAG_HASH: u32 = 1 << 1;
pub const FLAG_DIRTY_IN: u32 = 1 << 3;
pub const FLAG_DIRTY_OUT: u32 = 1 << 4;
pub const FLAG_DIRTY: u32 = FLAG_DIRTY_IN | FLAG_DIRTY_OUT;
pub const FLAG_CYCLE: u32 = 1 << 5;
pub const FLAG_DEPS: u32 = 1 << 6;

macro_rules! arena_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(usize);

        impl $name {
            pub(crate) const fn from_index(index: usize) -> Self {
                Self(index)
            }

            pub const fn index(self) -> usize {
                self.0
            }
        }
    };
}

arena_id!(NodeId);
arena_id!(EdgeId);

// [spec:samurai:def:graph.node]
pub struct Node {
    pub path: BString,
    pub shellpath: BString,
    pub mtime: i64,
    pub logmtime: i64,
    pub gen: Option<EdgeId>,
    pub uses: Vec<EdgeId>,
    pub validation_uses: Vec<EdgeId>,
    pub hash: u64,
    pub id: i32,
    pub dirty: bool,
    pub dyndep_pending: bool,
}

// [spec:samurai:def:graph.edge]
pub struct Edge {
    pub id: EdgeId,
    pub critical_path_weight: i64,
    pub rule: Option<RuleId>,
    pub pool: Option<PoolId>,
    pub env: EnvironmentId,
    pub bindings: BTreeMap<String, BString>,
    pub out: Vec<NodeId>,
    pub input: Vec<NodeId>,
    pub validation: Vec<NodeId>,
    pub dyndep: Option<NodeId>,
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
    node_by_path: BTreeMap<Vec<u8>, NodeId>,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    environments: Vec<Environment>,
    rules: Vec<Rule>,
    pools: Vec<Pool>,
}

impl Graph {
    pub fn nodes(&self) -> Vec<NodeId> {
        self.node_by_path.values().copied().collect()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub fn edge(&self, id: EdgeId) -> &Edge {
        &self.edges[id.index()]
    }

    pub fn edge_mut(&mut self, id: EdgeId) -> &mut Edge {
        &mut self.edges[id.index()]
    }

    pub fn edge_ids(&self) -> Vec<EdgeId> {
        (0..self.edges.len()).map(EdgeId::from_index).collect()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn environment(&self, id: EnvironmentId) -> &Environment {
        &self.environments[id.index()]
    }

    pub fn environment_mut(&mut self, id: EnvironmentId) -> &mut Environment {
        &mut self.environments[id.index()]
    }

    pub(crate) fn push_environment(&mut self, environment: Environment) -> EnvironmentId {
        let id = EnvironmentId::from_index(self.environments.len());
        self.environments.push(environment);
        id
    }

    pub fn rule(&self, id: RuleId) -> &Rule {
        &self.rules[id.index()]
    }

    pub fn rule_mut(&mut self, id: RuleId) -> &mut Rule {
        &mut self.rules[id.index()]
    }

    pub(crate) fn push_rule(&mut self, rule: Rule) -> RuleId {
        let id = RuleId::from_index(self.rules.len());
        self.rules.push(rule);
        id
    }

    pub fn pool(&self, id: PoolId) -> &Pool {
        &self.pools[id.index()]
    }

    pub fn pool_mut(&mut self, id: PoolId) -> &mut Pool {
        &mut self.pools[id.index()]
    }

    pub(crate) fn push_pool(&mut self, pool: Pool) -> PoolId {
        let id = PoolId::from_index(self.pools.len());
        self.pools.push(pool);
        id
    }
}

// [spec:samurai:def:graph.delnode-fn]
// [spec:samurai:sem:graph.delnode-fn]
pub fn delnode(_node: NodeId) {}

// [spec:samurai:def:graph.graphinit-fn]
// [spec:samurai:sem:graph.graphinit-fn]
pub fn graphinit() -> Graph {
    Graph {
        node_by_path: BTreeMap::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        environments: Vec::new(),
        rules: Vec::new(),
        pools: Vec::new(),
    }
}

// [spec:samurai:def:graph.mknode-fn]
// [spec:samurai:sem:graph.mknode-fn]
pub fn mknode(graph: &mut Graph, path: BString) -> NodeId {
    let key = path.as_bytes().to_vec();
    if let Some(node) = graph.node_by_path.get(&key) {
        return *node;
    }
    let shellpath = shell_escape_path(path.as_bytes());
    let node = NodeId::from_index(graph.nodes.len());
    graph.nodes.push(Node {
        path,
        shellpath,
        mtime: MTIME_UNKNOWN,
        logmtime: MTIME_MISSING,
        gen: None,
        uses: Vec::new(),
        validation_uses: Vec::new(),
        hash: 0,
        id: -1,
        dirty: false,
        dyndep_pending: false,
    });
    graph.node_by_path.insert(key, node);
    node
}

// [spec:samurai:def:graph.nodeget-fn]
// [spec:samurai:sem:graph.nodeget-fn]
pub fn nodeget(graph: &Graph, path: &[u8]) -> Option<NodeId> {
    graph.node_by_path.get(path).copied()
}

// [spec:samurai:def:graph.nodestat-fn]
// [spec:samurai:sem:graph.nodestat-fn]
pub fn nodestat(graph: &mut Graph, node: NodeId) -> std::io::Result<()> {
    let mtime = osmtime(
        graph
            .node(node)
            .path
            .to_path()
            .expect("byte paths are valid on Unix"),
    )?;
    graph.node_mut(node).mtime = mtime;
    Ok(())
}

pub fn nodestat_with<F>(graph: &mut Graph, node: NodeId, stat: &mut F) -> io::Result<()>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let mtime = stat(
        graph
            .node(node)
            .path
            .to_path()
            .expect("byte paths are valid on Unix"),
    )?;
    graph.node_mut(node).mtime = mtime;
    Ok(())
}

/// Recompute one edge after all of its inputs have already been evaluated.
pub(crate) fn recompute_edge_dirty_with<F>(
    graph: &mut Graph,
    edge: EdgeId,
    stat: &mut F,
) -> io::Result<bool>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    if graph.edge(edge).restat_clean {
        for index in 0..graph.edge(edge).out.len() {
            let output = graph.edge(edge).out[index];
            graph.node_mut(output).dirty = false;
        }
        return Ok(false);
    }

    for index in 0..graph.edge(edge).out.len() {
        let output = graph.edge(edge).out[index];
        if graph.node(output).mtime == MTIME_UNKNOWN {
            nodestat_with(graph, output, stat)?;
        }
    }

    let edge_data = graph.edge(edge);
    let input_dirty = edge_data
        .input
        .iter()
        .take(edge_data.inorderidx)
        .any(|input| graph.node(*input).dirty);
    let is_phony = edge_data
        .rule
        .is_some_and(|rule| graph.rule(rule).name == "phony");

    let dirty = if is_phony {
        let missing_without_inputs = edge_data.input.is_empty()
            && edge_data.validation.is_empty()
            && edge_data
                .out
                .iter()
                .any(|output| graph.node(*output).mtime == 0);
        let newest_input = edge_data
            .input
            .iter()
            .take(edge_data.inorderidx)
            .map(|input| graph.node(*input).mtime)
            .max()
            .unwrap_or(0);
        let output_count = edge_data.out.len();
        for index in 0..output_count {
            let output = graph.edge(edge).out[index];
            if graph.node(output).mtime == 0 {
                graph.node_mut(output).mtime = newest_input;
            }
        }
        input_dirty || missing_without_inputs
    } else {
        let oldest_output = edge_data
            .out
            .iter()
            .map(|output| graph.node(*output).mtime)
            .min()
            .unwrap_or(0);
        let oldest_recorded_output = edge_data
            .out
            .iter()
            .map(|output| graph.node(*output).logmtime)
            .filter(|mtime| *mtime != MTIME_MISSING)
            .min();
        oldest_output == 0
            || edge_data.deps_missing
            || edge_data.command_dirty
            || input_dirty
            || oldest_recorded_output.is_some_and(|output_mtime| {
                edge_data
                    .input
                    .iter()
                    .take(edge_data.inorderidx)
                    .any(|input| graph.node(*input).mtime > output_mtime)
            })
            || edge_data
                .input
                .iter()
                .take(edge_data.inorderidx)
                .any(|input| graph.node(*input).mtime > oldest_output)
    };

    for index in 0..graph.edge(edge).out.len() {
        let output = graph.edge(edge).out[index];
        graph.node_mut(output).dirty = dirty;
    }
    Ok(dirty)
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum VisitState {
    #[default]
    New,
    Active,
    Done,
}

struct DirtyEvaluator {
    nodes: Vec<VisitState>,
    edges: Vec<VisitState>,
}

impl DirtyEvaluator {
    fn evaluate<F>(&mut self, graph: &mut Graph, target: NodeId, stat: &mut F) -> io::Result<bool>
    where
        F: FnMut(&Path) -> io::Result<i64>,
    {
        enum Work {
            Enter(NodeId),
            Finish(EdgeId),
        }

        let mut work = vec![Work::Enter(target)];
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => match self.nodes[node.index()] {
                    VisitState::Done => continue,
                    VisitState::Active => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "dependency cycle",
                        ));
                    }
                    VisitState::New => {
                        let Some(edge) = graph.node(node).gen else {
                            if graph.node(node).mtime == MTIME_UNKNOWN {
                                nodestat_with(graph, node, stat)?;
                            }
                            let dirty = graph.node(node).mtime == 0;
                            graph.node_mut(node).dirty = dirty;
                            self.nodes[node.index()] = VisitState::Done;
                            continue;
                        };

                        match self.edges[edge.index()] {
                            VisitState::Done => {
                                self.nodes[node.index()] = VisitState::Done;
                                continue;
                            }
                            VisitState::Active => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "dependency cycle",
                                ));
                            }
                            VisitState::New => {}
                        }

                        self.edges[edge.index()] = VisitState::Active;
                        let output_count = graph.edge(edge).out.len();
                        if graph.edge(edge).restat_clean {
                            for index in 0..output_count {
                                let output = graph.edge(edge).out[index];
                                graph.node_mut(output).dirty = false;
                                self.nodes[output.index()] = VisitState::Done;
                            }
                            self.edges[edge.index()] = VisitState::Done;
                            continue;
                        }

                        for index in 0..output_count {
                            let output = graph.edge(edge).out[index];
                            if graph.node(output).mtime == MTIME_UNKNOWN {
                                nodestat_with(graph, output, stat)?;
                            }
                            self.nodes[output.index()] = VisitState::Active;
                        }
                        work.push(Work::Finish(edge));
                        for index in (0..graph.edge(edge).input.len()).rev() {
                            work.push(Work::Enter(graph.edge(edge).input[index]));
                        }
                    }
                },
                Work::Finish(edge) => {
                    recompute_edge_dirty_with(graph, edge, stat)?;
                    let output_count = graph.edge(edge).out.len();
                    for index in 0..output_count {
                        let output = graph.edge(edge).out[index];
                        self.nodes[output.index()] = VisitState::Done;
                    }
                    self.edges[edge.index()] = VisitState::Done;
                }
            }
        }
        Ok(graph.node(target).dirty)
    }
}

/// Stat a dependency graph in one iterative pass and update each node's dirty bit.
// [spec:samurai:req:compat.graph-semantics]
pub fn recompute_dirty_with<F>(graph: &mut Graph, node: NodeId, stat: &mut F) -> io::Result<bool>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let mut evaluator = DirtyEvaluator {
        nodes: vec![VisitState::New; graph.nodes.len()],
        edges: vec![VisitState::New; graph.edges.len()],
    };
    evaluator.evaluate(graph, node, stat)
}

pub fn recompute_dirty_with_validations<F>(
    graph: &mut Graph,
    node: NodeId,
    stat: &mut F,
) -> io::Result<Vec<NodeId>>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    enum Work {
        Enter(NodeId),
        EvaluateValidation(NodeId),
        RecordValidation(NodeId),
    }

    let mut evaluator = DirtyEvaluator {
        nodes: vec![VisitState::New; graph.nodes.len()],
        edges: vec![VisitState::New; graph.edges.len()],
    };
    evaluator.evaluate(graph, node, stat)?;
    let mut validations = Vec::new();
    let mut seen_nodes = vec![false; graph.nodes.len()];
    let mut seen_edges = vec![false; graph.edges.len()];
    let mut work = vec![Work::Enter(node)];
    while let Some(item) = work.pop() {
        match item {
            Work::Enter(node) => {
                let Some(edge) = graph.node(node).gen else {
                    continue;
                };
                if std::mem::replace(&mut seen_edges[edge.index()], true) {
                    continue;
                }
                for index in (0..graph.edge(edge).validation.len()).rev() {
                    work.push(Work::EvaluateValidation(graph.edge(edge).validation[index]));
                }
                for index in (0..graph.edge(edge).input.len()).rev() {
                    work.push(Work::Enter(graph.edge(edge).input[index]));
                }
            }
            Work::EvaluateValidation(validation) => {
                if std::mem::replace(&mut seen_nodes[validation.index()], true) {
                    continue;
                }
                evaluator.evaluate(graph, validation, stat)?;
                work.push(Work::RecordValidation(validation));
                work.push(Work::Enter(validation));
            }
            Work::RecordValidation(validation) => validations.push(validation),
        }
    }
    Ok(validations)
}

// [spec:samurai:def:graph.nodepath-fn]
// [spec:samurai:sem:graph.nodepath-fn]
pub fn nodepath(graph: &Graph, node: NodeId, escape: bool) -> BString {
    let node = graph.node(node);
    if escape {
        node.shellpath.clone()
    } else {
        node.path.clone()
    }
}

fn shell_escape_path(source: &[u8]) -> BString {
    let quote = source
        .iter()
        .any(|byte| !byte.is_ascii_alphanumeric() && !b"_+-./".contains(byte));
    if !quote {
        return BString::from(source);
    }
    let mut bytes = Vec::with_capacity(source.len() + 2);
    if quote {
        bytes.push(b'\'');
        for byte in source {
            bytes.push(*byte);
            if *byte == b'\'' {
                bytes.extend_from_slice(b"\\''");
            }
        }
        bytes.push(b'\'');
    }
    BString::from(bytes)
}

// [spec:samurai:def:graph.nodeuse-fn]
// [spec:samurai:sem:graph.nodeuse-fn]
pub fn nodeuse(graph: &mut Graph, node: NodeId, edge: EdgeId) {
    graph.node_mut(node).uses.push(edge);
}

// [spec:samurai:def:graph.mkedge-fn]
// [spec:samurai:sem:graph.mkedge-fn]
pub fn mkedge(graph: &mut Graph, parent: EnvironmentId) -> EdgeId {
    let id = EdgeId::from_index(graph.edges.len());
    let environment = crate::env::mkenv(graph, Some(parent));
    graph.edges.push(Edge {
        id,
        critical_path_weight: -1,
        rule: None,
        pool: None,
        env: environment,
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
    });
    id
}

// [spec:samurai:def:graph.edgehash-fn]
// [spec:samurai:sem:graph.edgehash-fn]
pub fn edgehash(graph: &mut Graph, edge: EdgeId, command: &BStr, rspfile_content: Option<&BStr>) {
    let edge = graph.edge_mut(edge);
    if edge.flags & FLAG_HASH != 0 {
        return;
    }
    edge.flags |= FLAG_HASH;
    let hash = if let Some(rsp) = rspfile_content.filter(|rsp| !rsp.is_empty()) {
        let mut bytes = command.as_bytes().to_vec();
        bytes.extend_from_slice(b";rspfile=");
        bytes.extend_from_slice(rsp.as_bytes());
        rapidhashv1(&bytes)
    } else {
        rapidhashv1(command.as_bytes())
    };
    edge.hash = hash;
}

// [spec:samurai:def:graph.mkphony-fn]
// [spec:samurai:sem:graph.mkphony-fn]
pub fn mkphony(graph: &mut Graph, root: EnvironmentId, phony: RuleId, node: NodeId) -> EdgeId {
    let edge = mkedge(graph, root);
    let edge_mut = graph.edge_mut(edge);
    edge_mut.rule = Some(phony);
    edge_mut.outimpidx = 1;
    edge_mut.out.push(node);
    edge
}

// [spec:samurai:def:graph.edgeadddeps-fn]
// [spec:samurai:sem:graph.edgeadddeps-fn]
pub fn edgeadddeps(graph: &mut Graph, edge: EdgeId, deps: &[NodeId]) {
    for node in deps {
        nodeuse(graph, *node, edge);
    }
    let edge = graph.edge_mut(edge);
    let index = edge.inorderidx;
    edge.input.splice(index..index, deps.iter().copied());
    edge.inorderidx += deps.len();
}

/// Return generated outputs that are not consumed by another build edge.
pub fn rootnodes(graph: &Graph) -> Result<Vec<NodeId>, String> {
    let roots = graph
        .nodes()
        .into_iter()
        .filter(|node| {
            let node = graph.node(*node);
            node.gen.is_some() && node.uses.is_empty()
        })
        .collect::<Vec<_>>();
    if roots.is_empty() && graph.edge_count() != 0 {
        Err("could not determine root nodes of build graph".into())
    } else {
        Ok(roots)
    }
}

#[derive(Default)]
pub struct InputsCollector {
    inputs: Vec<NodeId>,
    visited_nodes: Vec<bool>,
}

impl InputsCollector {
    pub fn visit_node(&mut self, graph: &Graph, node: NodeId) {
        enum Work {
            Enter(NodeId),
            Record(NodeId),
        }

        self.visited_nodes.resize(graph.nodes.len(), false);
        let mut work = Vec::new();
        if let Some(edge) = graph.node(node).gen {
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
                    if let Some(edge) = graph.node(input).gen {
                        for child in graph.edge(edge).input.iter().rev() {
                            work.push(Work::Enter(*child));
                        }
                    }
                }
                Work::Record(input) => {
                    let generated_by_phony = graph
                        .node(input)
                        .gen
                        .and_then(|edge| graph.edge(edge).rule)
                        .is_some_and(|rule| graph.rule(rule).name == "phony");
                    if !generated_by_phony {
                        self.inputs.push(input);
                    }
                }
            }
        }
    }

    pub fn inputs(&self) -> &[NodeId] {
        &self.inputs
    }

    pub fn input_strings(&self, graph: &Graph, shell_escape: bool) -> Vec<String> {
        self.inputs
            .iter()
            .map(|node| {
                let path = nodepath(graph, *node, shell_escape);
                String::from_utf8_lossy(path.as_bytes()).into_owned()
            })
            .collect()
    }

    pub fn reset(&mut self) {
        self.inputs.clear();
        self.visited_nodes.fill(false);
    }
}

#[derive(Default)]
pub struct CommandCollector {
    pub edges: Vec<EdgeId>,
    visited_nodes: Vec<bool>,
    visited_edges: Vec<bool>,
}

impl CommandCollector {
    pub fn collect_from(&mut self, graph: &Graph, node: NodeId) {
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
                    let Some(edge) = graph.node(node).gen else {
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
                    let is_phony = graph
                        .edge(edge)
                        .rule
                        .is_some_and(|rule| graph.rule(rule).name == "phony");
                    if !is_phony {
                        self.edges.push(edge);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct QueuedEdge {
    weight: i64,
    edge: Reverse<EdgeId>,
}

#[derive(Default)]
pub struct EdgePriorityQueue {
    pending: Vec<EdgeId>,
    heap: BinaryHeap<QueuedEdge>,
}

impl EdgePriorityQueue {
    pub fn push(&mut self, edge: EdgeId) {
        self.pending.push(edge);
    }

    pub fn pop(&mut self, graph: &Graph) -> Option<EdgeId> {
        self.heap
            .extend(self.pending.drain(..).map(|edge| QueuedEdge {
                weight: graph.edge(edge).critical_path_weight,
                edge: Reverse(edge),
            }));
        self.heap.pop().map(|queued| queued.edge.0)
    }
}

pub fn verify_dag(graph: &Graph, node: NodeId) -> Result<(), String> {
    struct Frame {
        node: NodeId,
        next_input: usize,
    }

    let mut state = vec![VisitState::New; graph.nodes.len()];
    let mut positions = vec![None; graph.nodes.len()];
    let mut path = vec![node];
    let mut stack = vec![Frame {
        node,
        next_input: 0,
    }];
    state[node.index()] = VisitState::Active;
    positions[node.index()] = Some(0);

    while let Some(frame) = stack.last_mut() {
        let input = graph
            .node(frame.node)
            .gen
            .and_then(|edge| graph.edge(edge).input.get(frame.next_input))
            .copied();
        if let Some(input) = input {
            frame.next_input += 1;
            match state[input.index()] {
                VisitState::Done => {}
                VisitState::New => {
                    positions[input.index()] = Some(path.len());
                    state[input.index()] = VisitState::Active;
                    path.push(input);
                    stack.push(Frame {
                        node: input,
                        next_input: 0,
                    });
                }
                VisitState::Active => {
                    let start =
                        positions[input.index()].expect("active graph nodes have a path position");
                    let mut paths = path[start..]
                        .iter()
                        .map(|node| String::from_utf8_lossy(&graph.node(*node).path).into_owned())
                        .collect::<Vec<_>>();
                    paths.push(String::from_utf8_lossy(&graph.node(input).path).into_owned());
                    return Err(format!("dependency cycle: {}", paths.join(" -> ")));
                }
            }
            continue;
        }

        let finished = stack.pop().expect("the traversal stack is nonempty").node;
        path.pop();
        positions[finished.index()] = None;
        state[finished.index()] = VisitState::Done;
    }

    Ok(())
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
            "ronin-graph-test-{}-{}.ninja",
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
        let mut state = crate::env::envinit(&mut graph);
        crate::parse::parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root,
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
        assert_eq!(first, second);
        assert_eq!(nodepath(&graph, first, true).as_bytes(), b"'a b'");
    }

    #[test]
    fn ninja_shell_path_escaping_torture_case() {
        let mut graph = graphinit();
        let node = mknode(
            &mut graph,
            xasprintf(format_args!("foo bar\"/'$@d!st!c'/path'")),
        );
        let path = nodepath(&graph, node, true);
        assert_eq!(
            std::str::from_utf8(path.as_bytes()).unwrap(),
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
        graph.edge_mut(edge).inimpidx = input_count;
        graph.edge_mut(edge).inorderidx = input_count;
        graph.node_mut(output).gen = Some(edge);
        output
    }

    fn scan_graph(
        graph: &mut Graph,
        node: NodeId,
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
        nodestat_with(graph, node, &mut stat)?;
        recompute_dirty_with(graph, node, &mut stat)?;
        Ok(())
    }

    #[test]
    fn ninja_stat_scan_simple() {
        let mut graph = graphinit();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&mut graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "in"]);
    }

    #[test]
    fn ninja_stat_scan_two_step() {
        let mut graph = graphinit();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid"]);
        let middle = generated_node(&mut graph, root, "mid", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&mut graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "mid", "in"]);
        assert!(graph.node(output).dirty);
        assert!(graph.node(middle).dirty);
    }

    #[test]
    fn ninja_stat_scan_tree() {
        let mut graph = graphinit();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid1", "mid2"]);
        let middle1 = generated_node(&mut graph, root, "mid1", &["in11", "in12"]);
        generated_node(&mut graph, root, "mid2", &["in21", "in22"]);
        let mut stats = Vec::new();
        scan_graph(&mut graph, output, &[], &mut stats).unwrap();
        assert_eq!(
            stats,
            ["out", "mid1", "in11", "in12", "mid2", "in21", "in22"]
        );
        assert!(graph.node(middle1).dirty);
    }

    #[test]
    // [spec:samurai:req:compat.graph-semantics/test]
    fn ronin_deep_graph_evaluation_uses_an_iterative_worklist() {
        const DEPTH: usize = 20_000;

        let mut graph = graphinit();
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
        assert!(recompute_dirty_with(&mut graph, target.unwrap(), &mut stat).unwrap());
        assert_eq!(stat_count, DEPTH + 1);
    }

    #[test]
    fn ninja_stat_scan_middle_missing() {
        let mut graph = graphinit();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid"]);
        let middle = generated_node(&mut graph, root, "mid", &["in"]);
        let input = nodeget(&graph, b"in").unwrap();
        let mut stats = Vec::new();
        scan_graph(
            &mut graph,
            output,
            &[("in", 1), ("mid", 0), ("out", 1)],
            &mut stats,
        )
        .unwrap();
        assert!(!graph.node(input).dirty);
        assert!(graph.node(middle).dirty);
        assert!(graph.node(output).dirty);
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
            name: &str,
            next: Option<Box<crate::util::EvalString>>,
        ) -> crate::util::EvalString {
            let mut result = crate::util::EvalString::variable(name);
            if let Some(next) = next {
                result.parts.extend(next.parts);
            }
            result
        }

        let mut graph = graphinit();
        let state = crate::env::envinit(&mut graph);
        let rule = crate::env::mkrule(&mut graph, "cat".into());
        let command = text(
            "cat ",
            Some(Box::new(variable(
                "in",
                Some(Box::new(text(" > ", Some(Box::new(variable("out", None)))))),
            ))),
        );
        crate::env::ruleaddvar(&mut graph, rule, "command".into(), command);

        let edge = mkedge(&mut graph, state.root);
        graph.edge_mut(edge).rule = Some(rule);
        let input1 = mknode(&mut graph, xasprintf(format_args!("in1")));
        let input2 = mknode(&mut graph, xasprintf(format_args!("in2")));
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        {
            let edge = graph.edge_mut(edge);
            edge.input.extend([input1, input2]);
            edge.inimpidx = 2;
            edge.inorderidx = 2;
            edge.out.push(output);
            edge.outimpidx = 1;
        }
        let command = crate::env::edgevar(&graph, edge, "command", false).unwrap();
        assert_eq!(command.as_bytes(), b"cat in1 in2 > out");
        assert!(!graph.node(input1).dirty);
        assert!(!graph.node(input2).dirty);
        assert!(!graph.node(output).dirty);
    }

    #[test]
    fn ninja_graph_root_nodes() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\n",
        );
        let roots = rootnodes(&graph).unwrap();
        assert_eq!(roots.len(), 4);
        assert!(roots
            .iter()
            .all(|node| graph.node(*node).path.as_bytes().starts_with(b"out")));
    }

    #[test]
    fn ninja_graph_inputs_collector() {
        let graph = parse_graph(
            "build out1: cat in1\nbuild mid1: cat in1\nbuild out2: cat mid1\nbuild out3 out4: cat mid1\nbuild all: phony out1 out2 out3\n",
        );
        let mut collector = InputsCollector::default();
        collector.visit_node(&graph, nodeget(&graph, b"out1").unwrap());
        assert_eq!(collector.input_strings(&graph, false), ["in1"]);
        collector.visit_node(&graph, nodeget(&graph, b"out2").unwrap());
        assert_eq!(collector.input_strings(&graph, false), ["in1", "mid1"]);
        collector.visit_node(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(&graph, false),
            ["in1", "mid1", "out1", "out2", "out3"]
        );

        collector.reset();
        collector.visit_node(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(&graph, false),
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
            collector.input_strings(&graph, false),
            ["in1", "in2", "in with space", "implicit", "order_only"]
        );
        assert_eq!(
            collector.input_strings(&graph, true),
            ["in1", "in2", "'in with space'", "implicit", "order_only"]
        );
    }

    fn commands(graph: &Graph, collector: &CommandCollector) -> Vec<String> {
        collector
            .edges
            .iter()
            .map(|edge| {
                let command = crate::env::edgevar(graph, *edge, "command", false).unwrap();
                String::from_utf8_lossy(command.as_bytes()).into_owned()
            })
            .collect()
    }

    fn recompute_with_mtimes(
        graph: &mut Graph,
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
        recompute_dirty_with(graph, nodeget(graph, target).unwrap(), &mut stat)
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
        let edge = graph.node(edge).gen.unwrap();
        let command = crate::env::edgevar(&graph, edge, "command", true).unwrap();
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
        let edge = graph.node(edge).gen.unwrap();
        let command = crate::env::edgevar(&graph, edge, "command", false).unwrap();
        assert_eq!(command.as_bytes(), b"depfile is x");
    }

    #[test]
    fn ninja_graph_edge_binding_overrides_rule_binding() {
        let graph = parse_graph(
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n  depfile = y\n",
        );
        let edge = nodeget(&graph, b"out").unwrap();
        let edge = graph.node(edge).gen.unwrap();
        let depfile = crate::env::edgevar(&graph, edge, "depfile", false).unwrap();
        let command = crate::env::edgevar(&graph, edge, "command", false).unwrap();
        assert_eq!(depfile.as_bytes(), b"y");
        assert_eq!(command.as_bytes(), b"depfile is y");
    }

    #[test]
    fn ninja_graph_dependency_cycle() {
        let graph = parse_graph(
            "build out: cat mid\nbuild mid: cat in\nbuild in: cat pre\nbuild pre: cat out\n",
        );
        assert_eq!(
            verify_dag(&graph, nodeget(&graph, b"out").unwrap()),
            Err("dependency cycle: out -> mid -> in -> pre -> out".into())
        );
    }

    #[test]
    fn ninja_graph_cycle_in_multi_output_edge() {
        let graph = parse_graph("build a b: cat a\n");
        assert_eq!(
            verify_dag(&graph, nodeget(&graph, b"b").unwrap()),
            Err("dependency cycle: a -> a".into())
        );
    }

    #[test]
    fn ninja_graph_edge_queue_priority() {
        let mut graph =
            parse_graph("build out1: cat in1\nbuild out2: cat in2\nbuild out3: cat in3\n");
        let edges = ["out1", "out2", "out3"].map(|output| {
            graph
                .node(nodeget(&graph, output.as_bytes()).unwrap())
                .gen
                .unwrap()
        });
        for (index, edge) in edges.iter().copied().enumerate() {
            graph.edge_mut(edge).critical_path_weight = index as i64 * 10;
        }
        let mut queue = EdgePriorityQueue::default();
        for edge in edges {
            queue.push(edge);
        }
        assert_eq!(queue.pending.len() + queue.heap.len(), 3);
        for expected in edges.into_iter().rev() {
            assert_eq!(queue.pop(&graph).unwrap(), expected);
        }
        assert!(queue.pending.is_empty() && queue.heap.is_empty());

        for edge in edges {
            graph.edge_mut(edge).critical_path_weight = 0;
        }
        queue.push(edges[1]);
        queue.push(edges[2]);
        queue.push(edges[0]);
        for expected in edges {
            assert_eq!(queue.pop(&graph).unwrap(), expected);
        }
    }

    #[test]
    fn ninja_graph_missing_implicit_input_is_dirty() {
        let mut graph = parse_graph("build out: cat in | implicit\n");
        assert!(recompute_with_mtimes(&mut graph, b"out", &[("in", 1), ("out", 1)]).unwrap());
    }

    #[test]
    fn ninja_graph_modified_implicit_input_is_dirty() {
        let mut graph = parse_graph("build out: cat in | implicit\n");
        assert!(recompute_with_mtimes(
            &mut graph,
            b"out",
            &[("in", 1), ("out", 1), ("implicit", 2)]
        )
        .unwrap());
    }

    #[test]
    fn ninja_graph_newer_order_only_input_is_clean() {
        let mut graph = parse_graph("build out: cat in || order_only\n");
        assert!(!recompute_with_mtimes(
            &mut graph,
            b"out",
            &[("in", 1), ("out", 1), ("order_only", 2)]
        )
        .unwrap());
    }

    #[test]
    fn ninja_graph_missing_implicit_output_dirties_all_outputs() {
        let mut graph = parse_graph("build out | out.imp: cat in\n");
        assert!(recompute_with_mtimes(&mut graph, b"out", &[("in", 1), ("out", 1)]).unwrap());
        assert!(graph.node(nodeget(&graph, b"out").unwrap()).dirty);
        assert!(graph.node(nodeget(&graph, b"out.imp").unwrap()).dirty);
    }

    #[test]
    fn ninja_graph_old_implicit_output_dirties_all_outputs() {
        let mut graph = parse_graph("build out | out.imp: cat in\n");
        assert!(recompute_with_mtimes(
            &mut graph,
            b"out",
            &[("out.imp", 1), ("in", 2), ("out", 2)]
        )
        .unwrap());
        assert!(graph.node(nodeget(&graph, b"out").unwrap()).dirty);
        assert!(graph.node(nodeget(&graph, b"out.imp").unwrap()).dirty);
    }

    #[test]
    fn ninja_graph_implicit_only_output_missing() {
        let mut graph = parse_graph("build | out.imp: cat in\n");
        assert!(recompute_with_mtimes(&mut graph, b"out.imp", &[("in", 1)]).unwrap());
    }

    #[test]
    fn ninja_graph_implicit_only_output_outdated() {
        let mut graph = parse_graph("build | out.imp: cat in\n");
        assert!(
            recompute_with_mtimes(&mut graph, b"out.imp", &[("out.imp", 1), ("in", 2)]).unwrap()
        );
    }

    #[test]
    fn ninja_graph_validation_is_scanned_separately() {
        let mut graph = parse_graph("build out: cat in |@ validate\nbuild validate: cat in\n");
        let mtimes = BTreeMap::from([("in".to_owned(), 1)]);
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy();
            Ok(*mtimes.get(path.as_ref()).unwrap_or(&0))
        };
        let output = nodeget(&graph, b"out").unwrap();
        let validations = recompute_dirty_with_validations(&mut graph, output, &mut stat).unwrap();
        assert_eq!(validations.len(), 1);
        assert!(graph.node(nodeget(&graph, b"out").unwrap()).dirty);
        assert!(graph.node(nodeget(&graph, b"validate").unwrap()).dirty);
    }

    #[test]
    fn ninja_graph_phony_dependency_propagates_mtime() {
        let mut graph = parse_graph("build in_ph: phony in1\nbuild out1: cat in_ph\n");
        assert!(!recompute_with_mtimes(&mut graph, b"out1", &[("in1", 1), ("out1", 2)]).unwrap());
        for node in graph.nodes() {
            let node = graph.node_mut(node);
            node.mtime = MTIME_UNKNOWN;
            node.dirty = false;
        }
        assert!(recompute_with_mtimes(&mut graph, b"out1", &[("in1", 3), ("out1", 2)]).unwrap());
    }

    #[test]
    fn ninja_graph_phony_output_with_validation_is_clean() {
        let mut graph = parse_graph("build valid: phony\nbuild out: phony |@ valid\n");
        let mut stat = |_path: &Path| Ok(0);
        let output = nodeget(&graph, b"out").unwrap();
        let validations = recompute_dirty_with_validations(&mut graph, output, &mut stat).unwrap();
        assert!(!graph.node(nodeget(&graph, b"out").unwrap()).dirty);
        assert_eq!(validations.len(), 1);
        assert_eq!(graph.node(validations[0]).path.as_bytes(), b"valid");
    }
}
