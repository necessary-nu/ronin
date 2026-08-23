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
    /// Whether this edge's freshness is decided without comparing dates.
    ///
    /// GNU Make overrules the comparison outright for a target that exists and
    /// has no recipe (`update_file_1`'s `!noexist && file->is_target &&
    /// !deps_changed && file->cmds == 0` clause), so a prerequisite that is
    /// merely newer decides nothing and only one this run actually remade
    /// does. `--always-make` withholds the overruling and the comparison
    /// stands again, which is the same clause read the other way round.
    ///
    /// A `::` chain's recipe-less entry is what asks for this, and it asks
    /// because it still answers for the chain's name.
    pub(crate) dates_do_not_decide: bool,
    pub(crate) always_new_inputs: IdVec<NodeId>,
    /// Inputs that affect the late freshness predicate but are omitted from
    /// the value substituted for Make's `$?` automatic variable.
    pub(crate) excluded_new_inputs: IdVec<NodeId>,
    /// Inputs the published value spells differently from the name the graph
    /// knows them by, paired with the spelling to publish.
    ///
    /// A front end may know a prerequisite by one name and have the command
    /// read another: GNU Make's `$?` names an archive member `m.o` where the
    /// graph node is `lib.a(m.o)`. Nothing here reads either spelling — the
    /// pair is carried, not interpreted — which is what keeps the executor free
    /// of the front end's naming rules. Empty for every edge with nothing to
    /// respell, which is nearly all of them.
    pub(crate) new_input_names: Vec<(NodeId, BString)>,
    pub(crate) new_inputs_variable: BString,
    /// Two further names the same value is substituted for, in the two forms
    /// a path splits into: the directory each name carries, and the name with
    /// that directory taken off. One word out for every word in, in the same
    /// order, so a front end that publishes a list of paths can have the
    /// halves of it without a value to halve existing before the scheduler
    /// picks the list. Empty for a front end that asked for neither, which is
    /// nearly every edge. The scheduler assigns no meaning to either name.
    pub(crate) new_inputs_directories_variable: BString,
    pub(crate) new_inputs_filenames_variable: BString,
    /// The directory the command reads that value from, and so the directory
    /// the names in it are spelt relative to.
    ///
    /// A graph holds one namespace of paths, so a compilation unit that was
    /// read somewhere else contributes its nodes under that path. The command
    /// it produced runs there rather than here, which is what makes the
    /// qualified name the wrong one to hand it: GNU Make's recursive child
    /// answers `$?` with the names its own Makefile wrote. Empty for a unit
    /// that was read where the build runs, which is the common case and costs
    /// nothing.
    pub(crate) new_inputs_directory: BString,
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

    /// A name a front end invented rather than one a build file wrote, which
    /// stands for work rather than for a file.
    ///
    /// Two of these are read off the edge that makes them: a deferred-freshness
    /// rule and a `::` completion join both write to a name the graph made to
    /// sequence them, and the file the Makefile named is reached through the
    /// edge instead. The third is said outright, because nothing about the edge
    /// gives it away — a recipe segment staged for its effects has an output
    /// that is a handle and nothing else.
    pub(crate) fn is_virtual_output(&self, node: NodeId) -> bool {
        self.invented_outputs.contains(&node)
            || self.node(node).generator.is_some_and(|edge| {
                self.deferred_freshness.contains_key(&edge)
                    || self
                        .completion_join_output(edge)
                        .is_some_and(|observed| observed != node)
            })
    }

    /// The file a node stands for, which for a `::` chain's completion proxy is
    /// the target the Makefile wrote rather than the name that sequences it.
    ///
    /// The chain's target is redirected onto the proxy so everything naming it
    /// waits for every entry, which leaves the dependents holding a name no
    /// search ever answered about. A question about the FILE — where it was
    /// found, which of its two names the build settled on — has to be asked of
    /// the target instead. Every other node is its own file and answers itself.
    pub(crate) fn observed_file(&self, node: NodeId) -> NodeId {
        self.node(node)
            .generator
            .and_then(|edge| self.completion_join_output(edge))
            .filter(|observed| *observed != node)
            .unwrap_or(node)
    }

    /// Say that `node` is a name and not a file, so the build stops treating it
    /// as one.
    pub(crate) fn mark_invented_output(&mut self, node: NodeId) {
        self.invented_outputs.insert(node);
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
    // A scan answering `-B` treats the outputs as though they were not there:
    // the edge runs, and GNU Make's `$?` holds every prerequisite because
    // `d->changed || always_make_flag` is what fills it (commands.c). Both
    // fall out of the one flag the absent-output case already sets.
    //
    // Withheld from an edge no date decides, because `-B` does not force one
    // of those either: `always_make_flag` reaches a target through
    // `file->cmds != 0` (remake.c), and what it does to an edge with no recipe
    // is give the ordinary comparison back rather than answer for it.
    let mut missing =
        freshness.always_dirty_output || (runtime.always_make && !freshness.dates_do_not_decide);
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
    // Whether a prerequisite being newer than the outputs is a reason to run.
    // For nearly every edge it is the question. For an edge the front end said
    // no date decides, it is not asked at all unless `-B` gives it back — see
    // `DeferredFreshness::dates_do_not_decide`, and the `-B` clause in the
    // capture above, which is the same withholding from the other side.
    let dates_decide = !freshness.dates_do_not_decide || runtime.always_make;
    // The comparison has the outputs on the target side, where GNU Make reads a
    // whole-second record as the end of its second. What is published below is
    // the plain baseline, because what reads these outputs as prerequisites
    // must see the date they actually have.
    let target_baseline = edge_data.target_mtime(baseline);
    if dates_decide {
        for input in edge_data.non_order_only_inputs() {
            let input_state = runtime.node(*input);
            timestamp_dirty |= freshness.always_new_inputs.contains(input)
                || input_state.mtime().is_missing()
                || input_state.mtime() > target_baseline;
        }
    }
    let semantic_dirty =
        timestamp_dirty || runtime.edge(edge).deps_missing() || runtime.edge(edge).command_dirty();
    // For an edge no date decides, an out-of-date ordinary prerequisite is the
    // whole answer, and it is remembered rather than re-asked: GNU Make's
    // `deps_changed |= d->changed` gathers a bit `update_file` set when it
    // remade the prerequisite and nothing clears, while the prerequisite's own
    // out-of-dateness ends the moment it is made.
    //
    // Order-only prerequisites are left out of it because GNU Make gathers
    // that sum under `if (! d->ignore_mtime)`. The names the compiler invented
    // to sequence a `::` chain are order-only too and are NOT left out: they
    // are not prerequisites anybody wrote, they are how this entry waits its
    // turn, and an entry that settled the chain's name before its turn would
    // answer for the entries in front of it.
    if freshness.dates_do_not_decide
        && edge_data
            .non_order_only_inputs()
            .iter()
            .any(|input| runtime.node(*input).dirty())
    {
        runtime.deferred_mut(edge).note_deps_changed();
    }
    let edge_data = graph.edge(edge);
    let dependency_dirty = if freshness.dates_do_not_decide {
        runtime
            .deferred(edge)
            .expect("deferred capture populated runtime state")
            .deps_changed()
            || edge_data.input[edge_data.non_order_only_input_count()..]
                .iter()
                .any(|input| graph.is_virtual_output(*input) && runtime.node(*input).dirty())
    } else {
        edge_data
            .input
            .iter()
            .any(|input| runtime.node(*input).dirty())
    };
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
    super::settle_searched_outputs(graph, runtime, edge, dirty);
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
    super::settle_searched_outputs(graph, runtime, edge, dirty);
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
