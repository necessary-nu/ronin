//! Dense graph arenas and dependency operations.

use crate::env::{Environment, EnvironmentId, Pool, PoolId, Rule, RuleId};
use crate::error::GraphError;
use crate::htab::rapidhashv1_parts;
use crate::os::MTIME_MISSING;
use crate::util::{BStr, BString, ByteSlice};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

pub(crate) const MTIME_UNKNOWN: i64 = -1;

pub(crate) const FLAG_HASH: u32 = 1 << 1;

macro_rules! arena_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub(crate) struct $name(usize);

        impl $name {
            pub(crate) const fn from_index(index: usize) -> Self {
                Self(index)
            }

            pub(crate) const fn index(self) -> usize {
                self.0
            }
        }
    };
}

arena_id!(NodeId);
arena_id!(EdgeId);

// [spec:samurai:def:graph.node]
pub(crate) struct Node {
    pub(crate) path: BString,
    pub(crate) shellpath: BString,
    pub(crate) mtime: i64,
    pub(crate) logmtime: i64,
    pub(crate) gen: Option<EdgeId>,
    pub(crate) uses: Vec<EdgeId>,
    pub(crate) validation_uses: Vec<EdgeId>,
    pub(crate) hash: u64,
    pub(crate) id: i32,
    pub(crate) dirty: bool,
    pub(crate) dyndep_pending: bool,
}

// [spec:samurai:def:graph.edge]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these independent cached graph facts avoid repeated evaluation on hot paths"
)]
pub(crate) struct Edge {
    pub(crate) critical_path_weight: i64,
    pub(crate) rule: Option<RuleId>,
    pub(crate) pool: Option<PoolId>,
    pub(crate) env: EnvironmentId,
    pub(crate) bindings: BTreeMap<String, BString>,
    pub(crate) out: Vec<NodeId>,
    pub(crate) input: Vec<NodeId>,
    pub(crate) validation: Vec<NodeId>,
    pub(crate) dyndep: Option<NodeId>,
    pub(crate) outimpidx: usize,
    pub(crate) inimpidx: usize,
    pub(crate) inorderidx: usize,
    pub(crate) hash: u64,
    pub(crate) deps_loaded: bool,
    pub(crate) deps_missing: bool,
    pub(crate) depfile_deps: usize,
    pub(crate) command_dirty: bool,
    pub(crate) restat_clean: bool,
    pub(crate) flags: u32,
}

// [spec:samurai:def:htab.hashtablekey]
// [spec:samurai:def:htab.hashtable]
// [spec:samurai:def:htab.htabkey-fn]
// [spec:samurai:sem:htab.htabkey-fn]
// [spec:samurai:def:htab.mkhtab-fn]
// [spec:samurai:sem:htab.mkhtab-fn]
// [spec:samurai:def:htab.keyequal-fn]
// [spec:samurai:sem:htab.keyequal-fn]
// [spec:samurai:def:htab.keyindex-fn]
// [spec:samurai:sem:htab.keyindex-fn]
// [spec:samurai:def:htab.htabput-fn]
// [spec:samurai:sem:htab.htabput-fn]
// [spec:samurai:def:htab.htabget-fn]
// [spec:samurai:sem:htab.htabget-fn]
// [spec:samurai:def:htab.delhtab-fn]
// [spec:samurai:sem:htab.delhtab-fn]
// [spec:samurai:def:graph.graphinit-fn]
// [spec:samurai:sem:graph.graphinit-fn]
#[derive(Default)]
pub(crate) struct Graph {
    // RandomState protects manifest-controlled paths from collision attacks.
    // Observable graph order comes from the arenas, never map iteration.
    node_by_path: HashMap<Vec<u8>, NodeId>,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    environments: Vec<Environment>,
    rules: Vec<Rule>,
    pools: Vec<Pool>,
}

impl Graph {
    pub(crate) fn node_ids(&self) -> impl ExactSizeIterator<Item = NodeId> {
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

    pub(crate) fn edge_ids(&self) -> impl ExactSizeIterator<Item = EdgeId> {
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

    pub(crate) fn pool(&self, id: PoolId) -> &Pool {
        &self.pools[id.index()]
    }

    pub(crate) fn pool_mut(&mut self, id: PoolId) -> &mut Pool {
        &mut self.pools[id.index()]
    }

    pub(crate) fn push_pool(&mut self, pool: Pool) -> PoolId {
        let id = PoolId::from_index(self.pools.len());
        self.pools.push(pool);
        id
    }
}

// [spec:samurai:def:graph.mknode-fn]
// [spec:samurai:sem:graph.mknode-fn]
// [spec:samurai:def:graph.delnode-fn]
// [spec:samurai:sem:graph.delnode-fn]
pub(crate) fn mknode(graph: &mut Graph, path: BString) -> NodeId {
    if let Some(node) = graph.node_by_path.get(path.as_bytes()) {
        return *node;
    }
    let key = path.as_bytes().to_vec();
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
pub(crate) fn nodeget(graph: &Graph, path: &[u8]) -> Option<NodeId> {
    graph.node_by_path.get(path).copied()
}

// [spec:samurai:def:graph.nodestat-fn]
// [spec:samurai:sem:graph.nodestat-fn]
pub(crate) fn nodestat_with<F>(
    graph: &mut Graph,
    node: NodeId,
    stat: &mut F,
) -> Result<(), GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let path = graph.node(node).path.clone();
    let mtime = stat(path.to_path().expect("byte paths are valid on Unix"))
        .map_err(|source| GraphError::Stat { node, path, source })?;
    graph.node_mut(node).mtime = mtime;
    Ok(())
}

/// Recompute one edge after all of its inputs have already been evaluated.
pub(crate) fn recompute_edge_dirty_with<F>(
    graph: &mut Graph,
    edge: EdgeId,
    stat: &mut F,
) -> Result<bool, GraphError>
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
    fn evaluate<F>(
        &mut self,
        graph: &mut Graph,
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
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => match self.nodes[node.index()] {
                    VisitState::Done => {}
                    VisitState::Active => {
                        return Err(GraphError::DependencyCycle { node: Some(node) });
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
                                return Err(GraphError::DependencyCycle { node: Some(node) });
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
#[cfg(test)]
pub(crate) fn recompute_dirty_with<F>(
    graph: &mut Graph,
    node: NodeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let mut evaluator = DirtyEvaluator {
        nodes: vec![VisitState::New; graph.nodes.len()],
        edges: vec![VisitState::New; graph.edges.len()],
    };
    evaluator.evaluate(graph, node, stat)
}

pub(crate) fn recompute_dirty_with_validations<F>(
    graph: &mut Graph,
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
pub(crate) fn nodepath(graph: &Graph, node: NodeId, escape: bool) -> BString {
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
pub(crate) fn nodeuse(graph: &mut Graph, node: NodeId, edge: EdgeId) {
    graph.node_mut(node).uses.push(edge);
}

// [spec:samurai:def:graph.mkedge-fn]
// [spec:samurai:sem:graph.mkedge-fn]
// [spec:samurai:def:graph.mkphony-fn]
// [spec:samurai:sem:graph.mkphony-fn]
pub(crate) fn mkedge(graph: &mut Graph, parent: EnvironmentId) -> EdgeId {
    let id = EdgeId::from_index(graph.edges.len());
    let environment = crate::env::mkenv(graph, Some(parent));
    graph.edges.push(Edge {
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
pub(crate) fn edgehash(
    graph: &mut Graph,
    edge: EdgeId,
    command: &BStr,
    rspfile_content: Option<&BStr>,
) {
    let edge = graph.edge_mut(edge);
    if edge.flags & FLAG_HASH != 0 {
        return;
    }
    edge.flags |= FLAG_HASH;
    let hash = rspfile_content.filter(|rsp| !rsp.is_empty()).map_or_else(
        || rapidhashv1_parts(&[command.as_bytes()]),
        |rsp| rapidhashv1_parts(&[command.as_bytes(), b";rspfile=", rsp.as_bytes()]),
    );
    edge.hash = hash;
}

pub(crate) fn invalidate_edge_hash(graph: &mut Graph, edge: EdgeId) {
    let edge = graph.edge_mut(edge);
    edge.flags &= !FLAG_HASH;
    edge.hash = 0;
}

// [spec:samurai:def:graph.edgeadddeps-fn]
// [spec:samurai:sem:graph.edgeadddeps-fn]
pub(crate) fn edgeadddeps(graph: &mut Graph, edge: EdgeId, deps: &[NodeId]) {
    for node in deps {
        nodeuse(graph, *node, edge);
    }
    let edge = graph.edge_mut(edge);
    let index = edge.inorderidx;
    edge.input.splice(index..index, deps.iter().copied());
    edge.inorderidx += deps.len();
}

/// Return generated outputs that are not consumed by another build edge.
pub(crate) fn rootnodes(graph: &Graph) -> Result<Vec<NodeId>, GraphError> {
    let roots = graph
        .node_ids()
        .filter(|node| {
            let node = graph.node(*node);
            node.gen.is_some() && node.uses.is_empty()
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

    pub(crate) fn input_strings(&self, graph: &Graph, shell_escape: bool) -> Vec<BString> {
        self.inputs
            .iter()
            .map(|node| nodepath(graph, *node, shell_escape))
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
        let mut graph = Graph::default();
        let mut parser = crate::parse::Parser::default();
        let mut state = crate::env::EnvState::new(&mut graph);
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
        let mut graph = Graph::default();
        let first = mknode(&mut graph, xasprintf(format_args!("a b")));
        let second = mknode(&mut graph, xasprintf(format_args!("a b")));
        assert_eq!(first, second);
        assert_eq!(nodepath(&graph, first, true).as_bytes(), b"'a b'");
    }

    #[test]
    fn ninja_shell_path_escaping_torture_case() {
        let mut graph = Graph::default();
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
    ) -> Result<(), GraphError> {
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
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&mut graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "in"]);
    }

    #[test]
    fn ninja_stat_scan_two_step() {
        let mut graph = Graph::default();
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
        let mut graph = Graph::default();
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
        assert!(recompute_dirty_with(&mut graph, target.unwrap(), &mut stat).unwrap());
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

        let mut graph = Graph::default();
        let state = crate::env::EnvState::new(&mut graph);
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
    ) -> Result<bool, GraphError> {
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
        for node in graph.node_ids() {
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
