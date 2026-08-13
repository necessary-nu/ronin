use super::{EdgeId, Graph, NodeId, nodestat_with, nodeuse};
use crate::error::GraphError;
use crate::runtime::{FileTime, RuntimeState};
use crate::util::{BString, IdVec};
use std::io;
use std::path::Path;

/// An edge whose real output freshness cannot be decided until its ordinary
/// and order-only prerequisites have settled.
///
/// The edge itself produces a private virtual completion node. The real
/// outputs are observed before descending into the prerequisites, then the
/// normal inputs are observed again at the scheduling boundary. Keeping the
/// uncommon relation beside the edge arena avoids charging every manifest
/// edge for semantics no Ninja statement can express.
pub(crate) struct DeferredFreshness {
    pub(crate) outputs: IdVec<NodeId>,
    pub(crate) always_dirty_output: bool,
    pub(crate) always_new_inputs: IdVec<NodeId>,
    /// Inputs that affect the late freshness predicate but are omitted from
    /// the value substituted for Make's `$?` automatic variable.
    pub(crate) excluded_new_inputs: IdVec<NodeId>,
    pub(crate) new_inputs_variable: BString,
    /// Roots that become dependencies only when the late predicate succeeds.
    /// Recursive front ends use this to compose conditional child graphs
    /// without starting a nested executor.
    pub(crate) activations: IdVec<NodeId>,
}

impl Graph {
    pub(crate) fn deferred_freshness(&self, edge: EdgeId) -> Option<&DeferredFreshness> {
        self.deferred_freshness.get(&edge)
    }

    pub(crate) fn deferred_freshness_mut(
        &mut self,
        edge: EdgeId,
    ) -> Option<&mut DeferredFreshness> {
        self.deferred_freshness.get_mut(&edge)
    }

    pub(crate) fn set_deferred_freshness(&mut self, edge: EdgeId, freshness: DeferredFreshness) {
        self.deferred_freshness.insert(edge, freshness);
    }

    pub(crate) fn set_completion_join(&mut self, edge: EdgeId, observed_output: NodeId) {
        self.completion_joins.insert(edge, observed_output);
    }

    pub(crate) fn is_completion_join(&self, edge: EdgeId) -> bool {
        self.completion_joins.contains_key(&edge)
    }

    pub(crate) fn completion_join_output(&self, edge: EdgeId) -> Option<NodeId> {
        self.completion_joins.get(&edge).copied()
    }

    pub(crate) fn is_virtual_output(&self, node: NodeId) -> bool {
        self.node(node).generator.is_some_and(|edge| {
            self.deferred_freshness.contains_key(&edge)
                || self
                    .completion_join_output(edge)
                    .is_some_and(|observed| observed != node)
        })
    }

    pub(crate) fn redirect_node_uses(&mut self, from: NodeId, to: NodeId) {
        if from == to {
            return;
        }
        let uses = std::mem::take(&mut self.node_mut(from).uses);
        let mut visited = crate::htab::RapidHashSet::default();
        for edge in uses.iter().copied() {
            if visited.insert(edge) {
                for input in &mut self.edge_mut(edge).input {
                    if *input == from {
                        *input = to;
                    }
                }
            }
        }
        self.node_mut(to).uses.extend(uses);

        if let Some(validation_uses) = self.validation_uses.remove(&from) {
            let mut visited = crate::htab::RapidHashSet::default();
            for edge in validation_uses.iter().copied() {
                if visited.insert(edge) {
                    for validation in &mut self.edge_mut(edge).validation {
                        if *validation == from {
                            *validation = to;
                        }
                    }
                }
            }
            self.validation_uses
                .entry(to)
                .or_default()
                .extend(validation_uses);
        }
    }
}

pub(super) fn capture_deferred_freshness<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    edge: EdgeId,
    stat: &mut F,
) -> Result<(), GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    if runtime.deferred(edge).is_some() {
        return Ok(());
    }
    let freshness = graph
        .deferred_freshness(edge)
        .expect("deferred capture is called for a deferred edge");
    let mut baseline = None;
    let mut missing = freshness.always_dirty_output;
    for output in &freshness.outputs {
        if runtime.node(*output).mtime().is_unobserved() {
            nodestat_with(graph, runtime, *output, stat)?;
        }
        let mtime = runtime.node(*output).mtime();
        missing |= mtime.is_missing();
        if !mtime.is_missing() {
            baseline = Some(baseline.map_or(mtime, |oldest: FileTime| oldest.min(mtime)));
        }
    }
    runtime
        .deferred_mut(edge)
        .capture(baseline.unwrap_or(FileTime::MISSING), missing);
    Ok(())
}

pub(super) fn recompute_deferred_freshness<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    edge: EdgeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    capture_deferred_freshness(graph, runtime, edge, stat)?;
    if runtime
        .deferred(edge)
        .expect("deferred capture populated runtime state")
        .settled()
    {
        for output in &graph.edge(edge).out {
            runtime.node_mut(*output).set_dirty(false);
        }
        return Ok(false);
    }
    if runtime.edge(edge).restat_clean() {
        for output in &graph.edge(edge).out {
            runtime.node_mut(*output).set_dirty(false);
        }
        return Ok(false);
    }

    let freshness = graph
        .deferred_freshness(edge)
        .expect("deferred recomputation has deferred metadata");
    let edge_data = graph.edge(edge);
    let state = runtime
        .deferred(edge)
        .expect("deferred capture populated runtime state");
    let baseline = state.baseline();
    let all_inputs_new = state.all_inputs_new();
    // An empty ordinary prerequisite list only forces a run for rules Make
    // never considers current: phony targets and double-colon rules that
    // declared no prerequisites. A single-colon rule with no prerequisites is
    // up to date as soon as its target exists, so it must fall through to the
    // timestamp comparison below rather than run on every invocation.
    let mut timestamp_dirty =
        all_inputs_new || (edge_data.always_dirty && edge_data.non_order_only_inputs().is_empty());
    for input in edge_data.non_order_only_inputs() {
        let input_state = runtime.node(*input);
        timestamp_dirty |= freshness.always_new_inputs.contains(input)
            || input_state.mtime().is_missing()
            || input_state.mtime() > baseline;
    }
    let semantic_dirty =
        timestamp_dirty || runtime.edge(edge).deps_missing() || runtime.edge(edge).command_dirty();
    let dependency_dirty = edge_data
        .input
        .iter()
        .any(|input| runtime.node(*input).dirty());
    let dirty = semantic_dirty || dependency_dirty;
    if !runtime
        .deferred(edge)
        .expect("deferred runtime state remains present")
        .initial_decided()
    {
        runtime
            .deferred_mut(edge)
            .decide_initial(semantic_dirty, dirty && !semantic_dirty);
    }
    for output in &edge_data.out {
        let output_state = runtime.node_mut(*output);
        output_state.set_mtime(baseline);
        output_state.set_dirty(dirty);
    }
    Ok(dirty)
}

pub(super) fn recompute_completion_join<F>(
    graph: &Graph,
    runtime: &mut RuntimeState,
    edge: EdgeId,
    stat: &mut F,
) -> Result<bool, GraphError>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let edge_data = graph.edge(edge);
    let observed_output = graph
        .completion_join_output(edge)
        .expect("completion joins name the real output they observe");
    if runtime.node(observed_output).mtime().is_unobserved() {
        nodestat_with(graph, runtime, observed_output, stat)?;
    }
    let observed_mtime = runtime.node(observed_output).mtime();
    let actions_pending = edge_data
        .input
        .iter()
        .any(|input| runtime.node(*input).dirty());
    let dirty = actions_pending || edge_data.always_dirty || observed_mtime.is_missing();
    for output in &edge_data.out {
        let output = runtime.node_mut(*output);
        output.set_mtime(observed_mtime);
        output.set_dirty(dirty);
    }
    Ok(dirty)
}

/// Append dependencies that gate execution without entering the edge's
/// timestamp comparison. Deferred graph activation uses this after its late
/// predicate succeeds.
pub(crate) fn edgeaddorderonly(graph: &mut Graph, edge: EdgeId, deps: &[NodeId]) {
    for node in deps {
        if graph.edge(edge).input.contains(node) {
            continue;
        }
        nodeuse(graph, *node, edge);
        graph.edge_mut(edge).input.push(*node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::graph::{mkedge, mknode};

    #[test]
    fn redirect_moves_inputs_and_validations() {
        let mut graph = Graph::default();
        let scope = mkenv(&mut graph, None);
        let from = mknode(&mut graph, b"from");
        let to = mknode(&mut graph, b"to");
        let consumer = mkedge(&mut graph, scope);
        nodeuse(&mut graph, from, consumer);
        graph.edge_mut(consumer).input.push(from);
        graph.add_validation_use(from, consumer);
        graph.edge_mut(consumer).validation.push(from);

        graph.redirect_node_uses(from, to);

        assert!(graph.node(from).uses.is_empty());
        assert_eq!(graph.node(to).uses.as_slice(), &[consumer]);
        assert_eq!(graph.edge(consumer).input.as_slice(), &[to]);
        assert_eq!(graph.edge(consumer).validation.as_slice(), &[to]);
        assert!(graph.node_validation_uses(from).is_empty());
        assert_eq!(graph.node_validation_uses(to), &[consumer]);
    }

    #[test]
    fn activation_keeps_order_only_partition() {
        let mut graph = Graph::default();
        let scope = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, scope);
        let normal = mknode(&mut graph, b"normal");
        let first = mknode(&mut graph, b"first");
        let second = mknode(&mut graph, b"second");
        nodeuse(&mut graph, normal, edge);
        graph.edge_mut(edge).input.push(normal);
        graph.edge_mut(edge).set_input_partitions(1, 1);

        edgeaddorderonly(&mut graph, edge, &[normal, first, first]);
        edgeaddorderonly(&mut graph, edge, &[first, second]);

        assert_eq!(graph.edge(edge).non_order_only_inputs(), &[normal]);
        assert_eq!(graph.edge(edge).input.as_slice(), &[normal, first, second]);
        assert_eq!(graph.node(first).uses.as_slice(), &[edge]);
        assert_eq!(graph.node(second).uses.as_slice(), &[edge]);
    }
}
