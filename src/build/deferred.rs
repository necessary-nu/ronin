use super::{
    BuildError, BuildResult, Builder, EdgeId, FileTime, Graph, NodeId, Plan, RuntimeState,
};
use crate::graph::{edgeaddorderonly, nodestat_with};
use crate::util::{BString, ByteSlice};
use std::collections::BTreeSet;
use std::path::Path;

enum DeferredWork {
    Ordinary,
    Skip,
    Activate(Vec<NodeId>),
    Run,
}

impl Plan {
    pub(super) fn reportable_work_count(&self, graph: &Graph, runtime: &RuntimeState) -> usize {
        self.wanted
            .iter()
            .zip(graph.edge_ids())
            .filter(|(wanted, edge)| {
                if !**wanted {
                    return false;
                }
                if graph.deferred_freshness(*edge).is_some() {
                    return runtime
                        .deferred(*edge)
                        .is_some_and(crate::runtime::DeferredRuntime::initial_run);
                }
                let rule = graph.edge(*edge).rule;
                rule.is_some() && !graph.is_phony_rule(rule)
            })
            .count()
    }
}

impl Builder<'_> {
    /// Re-evaluate normal inputs after every prerequisite has completed but
    /// before the command starts. The real-output baseline was captured by the
    /// dirty walk before it descended into those prerequisites.
    fn deferred_work(&mut self, edge: EdgeId) -> DeferredWork {
        let Some(freshness) = self.graph.deferred_freshness(edge) else {
            return DeferredWork::Ordinary;
        };
        let always_new = freshness.always_new_inputs.clone();
        let activations = freshness.activations.to_vec();
        let normal_inputs = self.graph.edge(edge).non_order_only_inputs().to_vec();
        let state = self
            .runtime
            .deferred(edge)
            .expect("the initial dirty walk captured deferred freshness");
        if state.activation_attached() {
            return DeferredWork::Run;
        }
        let baseline = state.baseline();
        let all_inputs_new = state.all_inputs_new();
        let mut should_run = state.initial_run();
        let mut seen = BTreeSet::new();
        let mut new_inputs = Vec::new();
        for input in normal_inputs {
            let input_state = self.runtime.node(input);
            let is_new = all_inputs_new
                || always_new.contains(&input)
                || input_state.mtime().is_missing()
                || input_state.mtime() > baseline;
            if is_new && seen.insert(input) {
                new_inputs.push(input);
            }
            should_run |= is_new;
        }
        // A dry run cannot observe the changes prerequisite commands would
        // have made. Reaching a candidate means such work was planned, so the
        // deferred contract carries that hypothetical update across the same
        // boundary.
        if self.options.dryrun
            && self
                .runtime
                .deferred(edge)
                .is_some_and(crate::runtime::DeferredRuntime::candidate_only)
        {
            should_run = true;
        }
        self.runtime.deferred_mut(edge).set_new_inputs(new_inputs);
        if !should_run {
            return DeferredWork::Skip;
        }
        if !activations.is_empty() {
            edgeaddorderonly(self.graph, edge, &activations);
            self.runtime.deferred_mut(edge).attach_activations();
            return DeferredWork::Activate(activations);
        }
        DeferredWork::Run
    }

    pub(super) fn deferred_launch_command(&self, edge: EdgeId, command: &BString) -> BString {
        let Some(freshness) = self.graph.deferred_freshness(edge) else {
            return command.clone();
        };
        if freshness.new_inputs_environment.is_empty() {
            return command.clone();
        }
        let mut value = Vec::new();
        if let Some(state) = self.runtime.deferred(edge) {
            for input in state.new_inputs() {
                if !value.is_empty() {
                    value.push(b' ');
                }
                value.extend_from_slice(self.graph.node_path(*input).as_bytes());
            }
        }
        let mut launch = Vec::with_capacity(
            freshness.new_inputs_environment.len() + value.len() + command.len() + 4,
        );
        launch.extend_from_slice(&freshness.new_inputs_environment);
        launch.extend_from_slice(b"='");
        for byte in value {
            if byte == b'\'' {
                launch.extend_from_slice(b"'\\''");
            } else {
                launch.push(byte);
            }
        }
        launch.extend_from_slice(b"' ");
        launch.extend_from_slice(command);
        BString::from(launch)
    }

    fn finish_deferred_without_command(
        &mut self,
        edge: EdgeId,
        executed: bool,
    ) -> BuildResult<(bool, Vec<NodeId>)> {
        let outputs = self
            .graph
            .deferred_freshness(edge)
            .expect("deferred completion has metadata")
            .outputs
            .clone();
        let disk = self.disk.clone();
        let mut logical_mtime = FileTime::MISSING;
        for output in outputs {
            let mut stat = |path: &Path| disk.stat(path);
            nodestat_with(self.graph, &mut self.runtime, output, &mut stat)?;
            logical_mtime = logical_mtime.max(self.runtime.node(output).mtime());
        }
        for output in &self.graph.edge(edge).out {
            let state = self.runtime.node_mut(*output);
            state.set_mtime(logical_mtime);
            state.set_dirty(false);
        }
        self.runtime.deferred_mut(edge).settle();
        self.runtime.edge_mut(edge).set_command_dirty(false);
        self.runtime.edge_mut(edge).set_restat_clean(!executed);
        Ok((!self.options.dryrun, Vec::new()))
    }

    /// Resolve late work before the ordinary scheduler dispatches an edge.
    /// Returns whether the caller should continue with normal edge handling.
    pub(super) fn advance_deferred(
        &mut self,
        edge: EdgeId,
        failures: &mut usize,
        failure_limit: usize,
        last_error: &mut Option<BuildError>,
    ) -> bool {
        match self.deferred_work(edge) {
            DeferredWork::Skip => {
                let rule = self.graph.edge(edge).rule;
                if rule.is_some() && !self.graph.is_phony_rule(rule) {
                    self.progress.total = self.progress.total.saturating_sub(1);
                }
                let result = self.finish_deferred_without_command(edge, false);
                if let Err(error) = self.settle_edge(edge, result) {
                    *failures += 1;
                    *last_error = Some(error);
                }
                false
            }
            DeferredWork::Activate(roots) => {
                self.plan.defer_work(self.graph, edge);
                let activated = (|| -> BuildResult<()> {
                    for root in roots {
                        self.add_target_node(root)?;
                    }
                    self.plan.refresh_dependencies(self.graph, &self.runtime)?;
                    self.progress.total = self.plan.command_edge_count(self.graph);
                    Ok(())
                })();
                if let Err(error) = activated {
                    *failures = failure_limit;
                    *last_error = Some(error);
                }
                false
            }
            DeferredWork::Run if self.graph.is_phony_rule(self.graph.edge(edge).rule) => {
                let result = self.finish_deferred_without_command(edge, true);
                if let Err(error) = self.settle_edge(edge, result) {
                    *failures += 1;
                    *last_error = Some(error);
                }
                false
            }
            DeferredWork::Ordinary | DeferredWork::Run => true,
        }
    }
}
