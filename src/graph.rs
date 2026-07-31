//! Dense graph arenas and dependency operations.

mod edge;
mod ids;
mod index;
mod marks;
mod path;

use crate::env::{Environment, EnvironmentId, Pool, PoolId, Rule, RuleId};
use crate::error::GraphError;
use crate::htab::rapidhashv1;
use crate::runtime::{CommandHash, FileTime, RuntimeState};
use crate::util::{arena_id, BStr, BString, ByteSlice, IdVec};
use edge::EdgePartitions;
use index::NodeIndex;
pub(crate) use index::{mknode, mknode_bytes, nodeget};
pub(crate) use marks::MarkSet;
use marks::{VisitMarks, VisitState};
pub(crate) use path::nodepath_bytes;
use path::shell_escape_path;
use std::io;
use std::path::Path;

arena_id!(NodeId, pub(in crate::graph));
arena_id!(EdgeId, pub(in crate::graph));

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PathStyle {
    #[default]
    Raw,
    ShellEscaped,
}

impl PathStyle {
    const fn shell_escaped(self) -> bool {
        matches!(self, Self::ShellEscaped)
    }
}

// [spec:samurai:def:graph.node]
pub(crate) struct Node {
    pub(crate) path: BString,
    /// Shell-quoted form, present only when quoting actually changes the path.
    pub(crate) shellpath: Option<BString>,
    pub(crate) gen: Option<EdgeId>,
    pub(crate) uses: IdVec<EdgeId>,
    pub(crate) validation_uses: IdVec<EdgeId>,
}

// [spec:samurai:def:graph.edge]
pub(crate) struct Edge {
    pub(crate) rule: Option<RuleId>,
    pub(crate) pool: Option<PoolId>,
    /// The scope this edge's bindings and variables resolve against.
    ///
    /// This is the enclosing manifest scope directly. Edge-local bindings live
    /// in `bindings`, so giving each edge its own environment only added an
    /// arena entry and one more link to walk on every lookup that missed.
    pub(crate) env: EnvironmentId,
    pub(crate) bindings: crate::names::Bindings<BString>,
    pub(crate) out: IdVec<NodeId>,
    pub(crate) input: IdVec<NodeId>,
    pub(crate) validation: IdVec<NodeId>,
    pub(crate) dyndep: Option<NodeId>,
    partitions: EdgePartitions,
}

// [spec:samurai:def:graph.graphinit-fn]
// [spec:samurai:sem:graph.graphinit-fn]
#[derive(Default)]
pub(crate) struct Graph {
    // Fixed-seed rapidhash follows Ninja and C samurai: manifests are trusted
    // input (executing them runs arbitrary commands), so SipHash DoS
    // hardening buys nothing here. Observable graph order comes from the
    // arenas, never index iteration.
    node_by_path: NodeIndex,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    environments: Vec<Environment>,
    rules: Vec<Rule>,
    pools: Vec<Pool>,
    phony_rule: Option<RuleId>,
    console_pool: Option<PoolId>,
    names: crate::names::Names,
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

    pub(crate) const fn names(&self) -> &crate::names::Names {
        &self.names
    }

    pub(crate) const fn names_mut(&mut self) -> &mut crate::names::Names {
        &mut self.names
    }

    pub(crate) const fn set_phony_rule(&mut self, rule: RuleId) {
        self.phony_rule = Some(rule);
    }

    pub(crate) const fn set_console_pool(&mut self, pool: PoolId) {
        self.console_pool = Some(pool);
    }

    /// Whether `rule` is the built-in phony rule, by identity as in Ninja.
    ///
    /// A manifest-defined rule that shadows the name `phony` in a subninja
    /// scope is an ordinary rule and must not match.
    pub(crate) const fn is_phony_rule(&self, rule: Option<RuleId>) -> bool {
        match (rule, self.phony_rule) {
            (Some(rule), Some(phony)) => rule.index() == phony.index(),
            _ => false,
        }
    }

    /// Whether `pool` is the built-in console pool, by identity as in Ninja.
    pub(crate) const fn is_console_pool(&self, pool: Option<PoolId>) -> bool {
        match (pool, self.console_pool) {
            (Some(pool), Some(console)) => pool.index() == console.index(),
            _ => false,
        }
    }

    pub(crate) fn pool(&self, id: PoolId) -> &Pool {
        &self.pools[id.index()]
    }

    pub(crate) fn pool_mut(&mut self, id: PoolId) -> &mut Pool {
        &mut self.pools[id.index()]
    }

    pub(crate) const fn pool_count(&self) -> usize {
        self.pools.len()
    }

    pub(crate) fn push_pool(&mut self, pool: Pool) -> PoolId {
        let id = PoolId::from_index(self.pools.len());
        self.pools.push(pool);
        id
    }
}

// [spec:samurai:def:graph.nodestat-fn]
// [spec:samurai:sem:graph.nodestat-fn]
pub(crate) fn nodestat_with<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    node: NodeId,
    stat: &mut F,
) -> Result<(), GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    // Borrow the interned path for the syscall; only the error path needs an
    // owned copy, and scans stat every node.
    let path = &graph.node(node).path;
    let mtime = stat(path.to_path().expect("byte paths are valid on Unix")).map_err(|source| {
        GraphError::Stat {
            node,
            path: path.clone(),
            source,
        }
    })?;
    runtime.node_mut(node).set_mtime(FileTime::observed(mtime));
    Ok(())
}

/// Collect the nodes a dirty scan from `target` is going to want to stat.
///
/// Correctness does not depend on this set being exact, which is what makes it
/// safe to use a plain walk rather than shadowing [`DirtyEvaluator`]'s state
/// machine: a node collected but never reached costs one wasted `stat`, and a
/// node reached but never collected is stat'ed by the scan itself, because
/// `nodestat_with` is still guarded by `is_unobserved`. The scan's behaviour
/// is unchanged either way — this only decides which syscalls happen early.
pub(crate) fn collect_stat_targets(
    graph: &Graph,
    scratch: &mut TraversalScratch,
    target: NodeId,
    out: &mut Vec<NodeId>,
) {
    out.clear();
    scratch.seen_nodes.begin(graph.nodes.len());
    scratch.seen_edges.begin(graph.edges.len());
    let mut work = vec![target];
    while let Some(node) = work.pop() {
        if scratch.seen_nodes.replace(node.index()) {
            continue;
        }
        out.push(node);
        let Some(edge) = graph.node(node).gen else {
            continue;
        };
        if scratch.seen_edges.replace(edge.index()) {
            continue;
        }
        let edge = graph.edge(edge);
        work.extend(edge.out.iter().copied());
        work.extend(edge.input.iter().copied());
        work.extend(edge.validation.iter().copied());
    }
}

/// Recompute one edge after all of its inputs have already been evaluated.
pub(crate) fn recompute_edge_dirty_with<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    edge: EdgeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    if runtime.edge(edge).restat_clean() {
        for output in &graph.edge(edge).out {
            runtime.node_mut(*output).set_dirty(false);
        }
        return Ok(false);
    }

    for output in &graph.edge(edge).out {
        if runtime.node(*output).mtime().is_unobserved() {
            nodestat_with(graph, runtime, *output, stat)?;
        }
    }

    let edge_data = graph.edge(edge);
    let mut input_dirty = false;
    let mut newest_input = FileTime::MISSING;
    for input in edge_data.non_order_only_inputs() {
        let input = runtime.node(*input);
        input_dirty |= input.dirty();
        newest_input = newest_input.max(input.mtime());
    }

    let dirty = if graph.is_phony_rule(edge_data.rule) {
        let mut any_output_missing = false;
        for output in &edge_data.out {
            if runtime.node(*output).mtime().is_missing() {
                any_output_missing = true;
                runtime.node_mut(*output).set_mtime(newest_input);
            }
        }
        let missing_without_inputs =
            edge_data.input.is_empty() && edge_data.validation.is_empty() && any_output_missing;
        input_dirty || missing_without_inputs
    } else {
        let mut oldest_output: Option<FileTime> = None;
        let mut oldest_recorded_output: Option<FileTime> = None;
        for output in &edge_data.out {
            let output = runtime.node(*output);
            oldest_output = Some(
                oldest_output.map_or_else(|| output.mtime(), |oldest| oldest.min(output.mtime())),
            );
            if output.log_mtime().is_observed() {
                oldest_recorded_output = Some(oldest_recorded_output.map_or_else(
                    || output.log_mtime(),
                    |oldest| oldest.min(output.log_mtime()),
                ));
            }
        }
        let oldest_output = oldest_output.unwrap_or(FileTime::MISSING);
        let edge_state = runtime.edge(edge);
        oldest_output.is_missing()
            || edge_state.deps_missing()
            || edge_state.command_dirty()
            || input_dirty
            || oldest_recorded_output.is_some_and(|output_mtime| newest_input > output_mtime)
            || newest_input > oldest_output
    };

    for output in &graph.edge(edge).out {
        runtime.node_mut(*output).set_dirty(dirty);
    }
    Ok(dirty)
}

#[derive(Default)]
struct DirtyEvaluator {
    nodes: VisitMarks,
    edges: VisitMarks,
}

impl DirtyEvaluator {
    fn begin(&mut self, graph: &Graph) {
        self.nodes.begin(graph.nodes.len());
        self.edges.begin(graph.edges.len());
    }
}

/// Traversal buffers reused across every scan of one build.
#[derive(Default)]
pub(crate) struct TraversalScratch {
    evaluator: DirtyEvaluator,
    seen_nodes: MarkSet,
    seen_edges: MarkSet,
}

impl DirtyEvaluator {
    fn evaluate<F>(
        &mut self,
        graph: &Graph,
        runtime: &mut RuntimeState,
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
                Work::Enter(node) => match self.nodes.get(node.index()) {
                    VisitState::Done => {}
                    VisitState::Active => {
                        return Err(GraphError::DependencyCycle { node: Some(node) });
                    }
                    VisitState::New => {
                        let Some(edge) = graph.node(node).gen else {
                            if runtime.node(node).mtime().is_unobserved() {
                                nodestat_with(graph, runtime, node, stat)?;
                            }
                            let dirty = runtime.node(node).mtime().is_missing();
                            runtime.node_mut(node).set_dirty(dirty);
                            self.nodes.set(node.index(), VisitState::Done);
                            continue;
                        };

                        match self.edges.get(edge.index()) {
                            VisitState::Done => {
                                self.nodes.set(node.index(), VisitState::Done);
                                continue;
                            }
                            VisitState::Active => {
                                return Err(GraphError::DependencyCycle { node: Some(node) });
                            }
                            VisitState::New => {}
                        }

                        self.edges.set(edge.index(), VisitState::Active);
                        let outputs: &[NodeId] = &graph.edge(edge).out;
                        if runtime.edge(edge).restat_clean() {
                            for &output in outputs {
                                runtime.node_mut(output).set_dirty(false);
                                self.nodes.set(output.index(), VisitState::Done);
                            }
                            self.edges.set(edge.index(), VisitState::Done);
                            continue;
                        }

                        for &output in outputs {
                            if runtime.node(output).mtime().is_unobserved() {
                                nodestat_with(graph, runtime, output, stat)?;
                            }
                            self.nodes.set(output.index(), VisitState::Active);
                        }
                        work.push(Work::Finish(edge));
                        let inputs: &[NodeId] = &graph.edge(edge).input;
                        for &input in inputs.iter().rev() {
                            work.push(Work::Enter(input));
                        }
                    }
                },
                Work::Finish(edge) => {
                    recompute_edge_dirty_with(graph, runtime, edge, stat)?;
                    let outputs: &[NodeId] = &graph.edge(edge).out;
                    for &output in outputs {
                        self.nodes.set(output.index(), VisitState::Done);
                    }
                    self.edges.set(edge.index(), VisitState::Done);
                }
            }
        }
        Ok(runtime.node(target).dirty())
    }
}

/// Stat a dependency graph in one iterative pass and update each node's dirty bit.
// [spec:samurai:req:compat.graph-semantics]
#[cfg(test)]
pub(crate) fn recompute_dirty_with<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    node: NodeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let mut evaluator = DirtyEvaluator::default();
    evaluator.begin(graph);
    evaluator.evaluate(graph, runtime, node, stat)
}

pub(crate) fn recompute_dirty_with_validations<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    scratch: &mut TraversalScratch,
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

    scratch.evaluator.begin(graph);
    scratch.evaluator.evaluate(graph, runtime, node, stat)?;
    scratch.seen_nodes.begin(graph.nodes.len());
    scratch.seen_edges.begin(graph.edges.len());
    let mut validations = Vec::new();
    let mut work = vec![Work::Enter(node)];
    while let Some(item) = work.pop() {
        match item {
            Work::Enter(node) => {
                let Some(edge) = graph.node(node).gen else {
                    continue;
                };
                if scratch.seen_edges.replace(edge.index()) {
                    continue;
                }
                let edge_validations: &[NodeId] = &graph.edge(edge).validation;
                for &validation in edge_validations.iter().rev() {
                    work.push(Work::EvaluateValidation(validation));
                }
                let inputs: &[NodeId] = &graph.edge(edge).input;
                for &input in inputs.iter().rev() {
                    work.push(Work::Enter(input));
                }
            }
            Work::EvaluateValidation(validation) => {
                if scratch.seen_nodes.replace(validation.index()) {
                    continue;
                }
                scratch
                    .evaluator
                    .evaluate(graph, runtime, validation, stat)?;
                work.push(Work::RecordValidation(validation));
                work.push(Work::Enter(validation));
            }
            Work::RecordValidation(validation) => validations.push(validation),
        }
    }
    Ok(validations)
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
pub(crate) fn mkedge(graph: &mut Graph, scope: EnvironmentId) -> EdgeId {
    let id = EdgeId::from_index(graph.edges.len());
    graph.edges.push(Edge {
        rule: None,
        pool: None,
        env: scope,
        bindings: crate::names::Bindings::default(),
        out: IdVec::new(),
        input: IdVec::new(),
        validation: IdVec::new(),
        dyndep: None,
        partitions: EdgePartitions::default(),
    });
    id
}

// [spec:samurai:def:graph.edgehash-fn]
// [spec:samurai:sem:graph.edgehash-fn]
pub(crate) fn edgehash(
    runtime: &mut RuntimeState,
    edge: EdgeId,
    command: &BStr,
    rspfile_content: Option<&BStr>,
) -> CommandHash {
    if let Some(cached) = runtime.edge(edge).command_hash() {
        return cached;
    }
    let hash = rspfile_content.filter(|rsp| !rsp.is_empty()).map_or_else(
        || rapidhashv1(command.as_bytes()),
        |rsp| rapidhashv1(&[command.as_bytes(), b";rspfile=", rsp.as_bytes()][..]),
    );
    let hash = CommandHash::from_raw(hash);
    runtime.edge_mut(edge).set_command_hash(hash);
    hash
}

// [spec:samurai:def:graph.edgeadddeps-fn]
// [spec:samurai:sem:graph.edgeadddeps-fn]
pub(crate) fn edgeadddeps(graph: &mut Graph, edge: EdgeId, deps: &[NodeId]) {
    for node in deps {
        nodeuse(graph, *node, edge);
    }
    graph.edge_mut(edge).insert_implicit_inputs(deps);
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
                        .is_some_and(|edge| graph.is_phony_rule(graph.edge(edge).rule));
                    if !generated_by_phony {
                        self.inputs.push(input);
                    }
                }
            }
        }
    }

    pub(crate) fn input_strings(&self, graph: &Graph, style: PathStyle) -> Vec<BString> {
        self.inputs
            .iter()
            .map(|node| BString::from(nodepath_bytes(graph, *node, style)))
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
                    if !graph.is_phony_rule(graph.edge(edge).rule) {
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
    use crate::names::Names;
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
    fn arena_identifiers_are_niche_packed_and_index_ordered() {
        use std::mem::size_of;

        assert_eq!(size_of::<NodeId>(), 4);
        assert_eq!(size_of::<EdgeId>(), 4);
        // The niche is what shrinks Node.gen, Edge.rule, Edge.pool, and
        // Edge.dyndep from sixteen bytes to four.
        assert_eq!(size_of::<Option<NodeId>>(), 4);
        assert_eq!(size_of::<Option<EdgeId>>(), 4);

        assert_eq!(NodeId::from_index(0).index(), 0);
        assert_eq!(
            NodeId::from_index(u32::MAX as usize - 1).index(),
            u32::MAX as usize - 1
        );

        // The scheduler's ready heap orders edges by Reverse(EdgeId), so the
        // shifted encoding must keep comparing by index.
        let ids = (0..8).map(EdgeId::from_index).collect::<Vec<_>>();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn node_index_interns_across_growth_and_collisions() {
        const PATHS: usize = 2_000;

        let mut graph = Graph::default();
        let mut ids = Vec::new();
        for index in 0..PATHS {
            ids.push(mknode(&mut graph, xasprintf(format_args!("out/{index}.o"))));
        }
        // A byte path that is not valid UTF-8 must intern like any other.
        let raw = mknode(&mut graph, BString::from(b"out/\xff.o".as_slice()));

        // Growth rehashes every occupied slot, so every path must still map to
        // its original node and re-interning must not allocate a new one.
        assert_eq!(graph.node_ids().len(), PATHS + 1);
        for (index, id) in ids.iter().enumerate() {
            let path = xasprintf(format_args!("out/{index}.o"));
            assert_eq!(nodeget(&graph, path.as_bytes()), Some(*id));
            assert_eq!(mknode(&mut graph, path), *id);
        }
        assert_eq!(nodeget(&graph, b"out/\xff.o"), Some(raw));
        assert_eq!(nodeget(&graph, b"absent"), None);
        assert_eq!(graph.node_ids().len(), PATHS + 1);
    }

    #[test]
    fn unquoted_paths_do_not_store_a_second_copy() {
        let mut graph = Graph::default();
        let plain = mknode(&mut graph, xasprintf(format_args!("src/main.c")));
        let quoted = mknode(&mut graph, xasprintf(format_args!("src/a b.c")));

        // The common case renders identically in both styles from one buffer.
        assert!(graph.node(plain).shellpath.is_none());
        assert_eq!(nodepath_bytes(&graph, plain, PathStyle::Raw), b"src/main.c");
        assert_eq!(
            nodepath_bytes(&graph, plain, PathStyle::ShellEscaped),
            b"src/main.c"
        );
        assert!(graph.node(quoted).shellpath.is_some());
        assert_eq!(
            nodepath_bytes(&graph, quoted, PathStyle::ShellEscaped),
            b"'src/a b.c'"
        );
    }

    #[test]
    fn interns_nodes_and_quotes_shell_paths() {
        let mut graph = Graph::default();
        let first = mknode(&mut graph, xasprintf(format_args!("a b")));
        let second = mknode(&mut graph, xasprintf(format_args!("a b")));
        assert_eq!(first, second);
        assert_eq!(
            nodepath_bytes(&graph, first, PathStyle::ShellEscaped),
            b"'a b'"
        );
    }

    #[test]
    fn ninja_shell_path_escaping_torture_case() {
        let mut graph = Graph::default();
        let node = mknode(
            &mut graph,
            xasprintf(format_args!("foo bar\"/'$@d!st!c'/path'")),
        );
        let path = nodepath_bytes(&graph, node, PathStyle::ShellEscaped);
        assert_eq!(
            std::str::from_utf8(path).unwrap(),
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
        graph
            .edge_mut(edge)
            .set_input_partitions(input_count, input_count);
        graph.node_mut(output).gen = Some(edge);
        output
    }

    fn scan_graph(
        graph: &Graph,
        node: NodeId,
        mtimes: &[(&str, i64)],
        stats: &mut Vec<String>,
    ) -> Result<RuntimeState, GraphError> {
        let mut runtime = RuntimeState::new(graph);
        let mtimes = mtimes
            .iter()
            .map(|(path, mtime)| (path.to_string(), *mtime))
            .collect::<BTreeMap<_, _>>();
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy().into_owned();
            stats.push(path.clone());
            Ok(*mtimes.get(&path).unwrap_or(&0))
        };
        nodestat_with(graph, &mut runtime, node, &mut stat)?;
        recompute_dirty_with(graph, &mut runtime, node, &mut stat)?;
        Ok(runtime)
    }

    #[test]
    fn ninja_stat_scan_simple() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["in"]);
        let mut stats = Vec::new();
        scan_graph(&graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "in"]);
    }

    #[test]
    fn ninja_stat_scan_two_step() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid"]);
        let middle = generated_node(&mut graph, root, "mid", &["in"]);
        let mut stats = Vec::new();
        let runtime = scan_graph(&graph, output, &[], &mut stats).unwrap();
        assert_eq!(stats, ["out", "mid", "in"]);
        assert!(runtime.node(output).dirty());
        assert!(runtime.node(middle).dirty());
    }

    #[test]
    fn ninja_stat_scan_tree() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let output = generated_node(&mut graph, root, "out", &["mid1", "mid2"]);
        let middle1 = generated_node(&mut graph, root, "mid1", &["in11", "in12"]);
        generated_node(&mut graph, root, "mid2", &["in21", "in22"]);
        let mut stats = Vec::new();
        let runtime = scan_graph(&graph, output, &[], &mut stats).unwrap();
        assert_eq!(
            stats,
            ["out", "mid1", "in11", "in12", "mid2", "in21", "in22"]
        );
        assert!(runtime.node(middle1).dirty());
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
        let mut runtime = RuntimeState::new(&graph);
        assert!(recompute_dirty_with(&graph, &mut runtime, target.unwrap(), &mut stat).unwrap());
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
        let runtime = scan_graph(
            &graph,
            output,
            &[("in", 1), ("mid", 0), ("out", 1)],
            &mut stats,
        )
        .unwrap();
        assert!(!runtime.node(input).dirty());
        assert!(runtime.node(middle).dirty());
        assert!(runtime.node(output).dirty());
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
            name: crate::names::VarId,
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
                crate::names::Names::IN,
                Some(Box::new(text(
                    " > ",
                    Some(Box::new(variable(crate::names::Names::OUT, None))),
                ))),
            ))),
        );
        let command_name = graph.names_mut().intern(BStr::new("command"));
        crate::env::ruleaddvar(&mut graph, rule, command_name, command);

        let edge = mkedge(&mut graph, state.root);
        graph.edge_mut(edge).rule = Some(rule);
        let input1 = mknode(&mut graph, xasprintf(format_args!("in1")));
        let input2 = mknode(&mut graph, xasprintf(format_args!("in2")));
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        {
            let edge = graph.edge_mut(edge);
            edge.input.extend([input1, input2]);
            edge.set_input_partitions(2, 2);
            edge.out.push(output);
            edge.set_explicit_output_count(1);
        }
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(command.as_bytes(), b"cat in1 in2 > out");
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
        assert_eq!(collector.input_strings(&graph, PathStyle::Raw), ["in1"]);
        collector.visit_node(&graph, nodeget(&graph, b"out2").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "mid1"]
        );
        collector.visit_node(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "mid1", "out1", "out2", "out3"]
        );

        collector.reset();
        collector.visit_node(&graph, nodeget(&graph, b"all").unwrap());
        assert_eq!(
            collector.input_strings(&graph, PathStyle::Raw),
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
            collector.input_strings(&graph, PathStyle::Raw),
            ["in1", "in2", "in with space", "implicit", "order_only"]
        );
        assert_eq!(
            collector.input_strings(&graph, PathStyle::ShellEscaped),
            ["in1", "in2", "'in with space'", "implicit", "order_only"]
        );
    }

    fn commands(graph: &Graph, collector: &CommandCollector) -> Vec<String> {
        collector
            .edges
            .iter()
            .map(|edge| {
                let command =
                    crate::env::edgevar(graph, *edge, Names::COMMAND, PathStyle::Raw).unwrap();
                String::from_utf8_lossy(command.as_bytes()).into_owned()
            })
            .collect()
    }

    fn recompute_state_with_mtimes(
        graph: &Graph,
        target: &[u8],
        mtimes: &[(&str, i64)],
    ) -> Result<(bool, RuntimeState), GraphError> {
        let mut runtime = RuntimeState::new(graph);
        let mtimes = mtimes
            .iter()
            .map(|(path, mtime)| (path.to_string(), *mtime))
            .collect::<BTreeMap<_, _>>();
        let mut stat = |path: &Path| {
            let path = path.to_string_lossy();
            Ok(*mtimes.get(path.as_ref()).unwrap_or(&0))
        };
        let dirty = recompute_dirty_with(
            graph,
            &mut runtime,
            nodeget(graph, target).unwrap(),
            &mut stat,
        )?;
        Ok((dirty, runtime))
    }

    fn recompute_with_mtimes(
        graph: &Graph,
        target: &[u8],
        mtimes: &[(&str, i64)],
    ) -> Result<bool, GraphError> {
        recompute_state_with_mtimes(graph, target, mtimes).map(|(dirty, _)| dirty)
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
        let command =
            crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::ShellEscaped).unwrap();
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
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(command.as_bytes(), b"depfile is x");
    }

    #[test]
    fn ninja_graph_edge_binding_overrides_rule_binding() {
        let graph = parse_graph(
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n  depfile = y\n",
        );
        let edge = nodeget(&graph, b"out").unwrap();
        let edge = graph.node(edge).gen.unwrap();
        let depfile = crate::env::edgevar(&graph, edge, Names::DEPFILE, PathStyle::Raw).unwrap();
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(depfile.as_bytes(), b"y");
        assert_eq!(command.as_bytes(), b"depfile is y");
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
        let (dirty, runtime) =
            recompute_state_with_mtimes(&graph, b"out", &[("in", 1), ("out", 1)]).unwrap();
        assert!(dirty);
        assert!(runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert!(runtime.node(nodeget(&graph, b"out.imp").unwrap()).dirty());
    }

    #[test]
    fn ninja_graph_old_implicit_output_dirties_all_outputs() {
        let graph = parse_graph("build out | out.imp: cat in\n");
        let (dirty, runtime) =
            recompute_state_with_mtimes(&graph, b"out", &[("out.imp", 1), ("in", 2), ("out", 2)])
                .unwrap();
        assert!(dirty);
        assert!(runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert!(runtime.node(nodeget(&graph, b"out.imp").unwrap()).dirty());
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
        let output = nodeget(&graph, b"out").unwrap();
        let mut runtime = RuntimeState::new(&graph);
        let validations = recompute_dirty_with_validations(
            &graph,
            &mut runtime,
            &mut TraversalScratch::default(),
            output,
            &mut stat,
        )
        .unwrap();
        assert_eq!(validations.len(), 1);
        assert!(runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert!(runtime.node(nodeget(&graph, b"validate").unwrap()).dirty());
    }

    #[test]
    fn ninja_graph_phony_dependency_propagates_mtime() {
        let graph = parse_graph("build in_ph: phony in1\nbuild out1: cat in_ph\n");
        assert!(!recompute_with_mtimes(&graph, b"out1", &[("in1", 1), ("out1", 2)]).unwrap());
        assert!(recompute_with_mtimes(&graph, b"out1", &[("in1", 3), ("out1", 2)]).unwrap());
    }

    #[test]
    fn ninja_graph_phony_output_with_validation_is_clean() {
        let graph = parse_graph("build valid: phony\nbuild out: phony |@ valid\n");
        let mut stat = |_path: &Path| Ok(0);
        let output = nodeget(&graph, b"out").unwrap();
        let mut runtime = RuntimeState::new(&graph);
        let validations = recompute_dirty_with_validations(
            &graph,
            &mut runtime,
            &mut TraversalScratch::default(),
            output,
            &mut stat,
        )
        .unwrap();
        assert!(!runtime.node(nodeget(&graph, b"out").unwrap()).dirty());
        assert_eq!(validations.len(), 1);
        assert_eq!(graph.node(validations[0]).path.as_bytes(), b"valid");
    }
}
