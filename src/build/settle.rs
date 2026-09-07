//! Settling what a finished edge pruned, and carrying it to everything that
//! reads what it made.
//!
//! A `restat` that finds its outputs unmoved has to say so to everything above
//! them, and on a graph the size of a composed kernel build that walk is the
//! largest single thing an up-to-date run does. It sits here rather than beside
//! the scheduler because what bounds it is a question about the graph, not
//! about the loop that asks it, and because the loop's own file is at the
//! function count its language allows.
//!
//! A recipe-less edge — a `phony`, or a Make target whose recipe expanded to
//! nothing — finishes the moment the scheduler reaches it, and the consumers of
//! any two such edges the plan offers at one moment overlap almost entirely.
//! [`Wave`] is what may therefore be settled together, and
//! [`Builder::settle_wave`] says why that is the same answer as settling them
//! in turn.

use super::{
    BuildError, BuildResult, Builder, EdgeId, EdgeResult, NodeId, Reconsidered,
    recompute_dirty_with_validations, status,
};
use std::path::Path;

/// The recipe-less edges the plan had ready together, each with what finishing
/// it came to.
///
/// Held rather than settled one at a time so that what they prune is propagated
/// in one walk over the union of their consumers. Nothing in a wave waits on
/// anything else in it — an edge that did would still have been pending when
/// the wave was taken — which is what makes settling them together the same
/// answer as settling them in turn. See [`Builder::settle_wave`].
pub(crate) type Wave = Vec<(EdgeId, BuildResult<(bool, Vec<NodeId>)>)>;

impl Builder<'_> {
    /// Settle a whole wave of recipe-less edges, propagating what they pruned
    /// in ONE walk over the union of their consumers.
    ///
    /// See [`Wave`] for what may be in one.
    ///
    /// WHY ONE WALK IS THE SAME ANSWER as one per edge. The wave is the edges
    /// the plan had ready at one moment, so no edge in it waits on another: an
    /// edge that did would still have been pending when the wave was taken.
    /// What the plan then DECIDES about a consumer is decided once, when the
    /// last of its own prerequisites finishes, and every edge of the wave that
    /// the consumer can read is one of those — so the consumer is never decided
    /// before a propagation that could reach it has run, whichever order the
    /// wave is settled in. An edge of the wave the consumer cannot read cannot
    /// move it. See `release`'s `settle_released`, which is the only reader of
    /// what the propagation writes.
    ///
    /// The saving is most of what a settled tree spends here. A no-op over the
    /// Linux kernel at `allnoconfig` prunes 7,413 edges with nothing to run,
    /// and settling each on its own collected about 1,272 consumers for it:
    /// 9.4 million consumer settlements, 84 million edge visits collecting
    /// them, and 1.13 billion prerequisites read asking for intermediates — to
    /// reach an answer that moved 41 dirty bits in the whole invocation. The
    /// same edges arrive in 432 waves, and a wave costs one walk.
    pub(super) fn settle_wave(
        &mut self,
        wave: Wave,
        failures: &mut usize,
        last_error: &mut Option<BuildError>,
    ) {
        if wave.is_empty() {
            return;
        }
        let pruned = wave
            .iter()
            .filter(|(_, result)| matches!(result, Ok((true, _))))
            .map(|(edge, _)| *edge)
            .collect::<Vec<_>>();
        if !pruned.is_empty()
            && let Err(error) = self.recompute_consumers_after_restat(&pruned)
        {
            *failures += 1;
            *last_error = Some(error);
            return;
        }
        for (edge, result) in wave {
            if let Err(error) = self.finish_settled_edge(edge, result) {
                *failures += 1;
                *last_error = Some(error);
            }
        }
    }

    /// What settling an edge does once whatever it pruned has been propagated.
    pub(super) fn finish_settled_edge(
        &mut self,
        edge: EdgeId,
        result: BuildResult<(bool, Vec<NodeId>)>,
    ) -> BuildResult<()> {
        match result {
            Ok((_, loaded_dyndeps)) => {
                if !loaded_dyndeps.is_empty() {
                    self.recompute_planned_after_dyndep(&loaded_dyndeps)?;
                    self.plan.refresh_dependencies(self.graph, &self.runtime)?;
                }
                let pruned = self.plan.edge_finished(
                    self.graph,
                    &self.runtime,
                    edge,
                    EdgeResult::Succeeded,
                )?;
                status::forget_pruned_work(
                    &mut self.progress,
                    self.graph,
                    self.build_log.as_deref(),
                    &pruned,
                );
                Ok(())
            }
            Err(error) => {
                self.failed_edges.insert(edge);
                self.plan
                    .edge_finished(self.graph, &self.runtime, edge, EdgeResult::Failed)?;
                Err(error)
            }
        }
    }

    /// Settle everything above a batch of `restat`s again, in one walk over the
    /// union of their consumers rather than one walk per restat, per consumer,
    /// or over the whole graph.
    ///
    /// What the restats changed is beneath all of the consumers, so the set is
    /// asked together: see [`recompute_dirty_with_validations`] for why one
    /// walk is the same answer, and for what asking them one at a time costs on
    /// a graph the size of a composed kernel build.
    ///
    /// The consumers are also the whole of what can have changed, and the walk
    /// is told so. Descending past them re-settles a graph nothing has written
    /// to since it was last settled, and reaches the answer already standing
    /// there.
    pub(super) fn recompute_consumers_after_restat(&mut self, edges: &[EdgeId]) -> BuildResult<()> {
        let mut frontier = std::mem::take(&mut self.restat_queue);
        let mut consumers = std::mem::take(&mut self.restat_consumers);
        frontier.clear();
        for edge in edges.iter().copied() {
            for output in &self.graph.edge(edge).out {
                frontier.extend(self.graph.node(*output).uses.iter().copied());
                frontier.extend(self.graph.node_validation_uses(*output).iter().copied());
            }
        }
        self.visited_edges.begin(self.graph.edge_count());
        let result = self.settle_whole_closure(&mut frontier, &mut consumers);
        self.restat_queue = frontier;
        self.restat_consumers = consumers;
        result
    }

    fn settle_whole_closure(
        &mut self,
        frontier: &mut Vec<EdgeId>,
        consumers: &mut Vec<NodeId>,
    ) -> BuildResult<()> {
        consumers.clear();
        while let Some(dependent) = frontier.pop() {
            if self.visited_edges.replace(dependent.index()) {
                continue;
            }
            let outputs: &[NodeId] = &self.graph.edge(dependent).out;
            consumers.extend_from_slice(outputs);
            for &output in outputs {
                frontier.extend(self.graph.node(output).uses.iter().copied());
                frontier.extend(self.graph.node_validation_uses(output).iter().copied());
            }
        }
        let disk = self.disk.clone();
        let mut stat = |path: &Path| disk.stat(path);
        recompute_dirty_with_validations(
            self.graph,
            &mut self.runtime,
            &mut self.scratch,
            consumers,
            Some(&Reconsidered::new(&self.visited_edges)),
            &mut stat,
        )?;
        Ok(())
    }
}
