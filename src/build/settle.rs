//! Settling what a finished edge pruned, and carrying it to everything that
//! reads what it made.
//!
//! A `restat` that finds its outputs unmoved has to say so to everything above
//! them, and on a graph the size of a composed kernel build that walk is the
//! largest single thing an up-to-date run does. It sits here rather than beside
//! the scheduler because what bounds it is a question about the graph, not
//! about the loop that asks it, and because the loop's own file is at the
//! function count its language allows.

use super::{BuildResult, Builder, EdgeId, NodeId, Reconsidered, recompute_dirty_with_validations};
use std::path::Path;

impl Builder<'_> {
    /// Settle everything above a `restat` again, in one walk over the consumers
    /// rather than one walk per consumer or one walk over the whole graph.
    ///
    /// What the restat changed is beneath all of them, so the set is asked
    /// together: see [`recompute_dirty_with_validations`] for why one walk is the
    /// same answer, and for what asking them one at a time costs on a graph
    /// the size of a composed kernel build.
    ///
    /// The consumers are also the whole of what can have changed, and the walk
    /// is told so. Descending past them re-settles a graph nothing has written
    /// to since it was last settled, and reaches the answer already standing
    /// there: over a finished Linux kernel tree that descent recomputed 42,400
    /// of the graph's 50,346 edges on each of 7,413 restats — 314 million
    /// recomputations to reach 9.4 million consumers.
    pub(super) fn recompute_consumers_after_restat(&mut self, edge: EdgeId) -> BuildResult<()> {
        self.restat_queue.clear();
        self.restat_consumers.clear();
        for output in &self.graph.edge(edge).out {
            self.restat_queue
                .extend(self.graph.node(*output).uses.iter().copied());
            self.restat_queue
                .extend(self.graph.node_validation_uses(*output).iter().copied());
        }
        self.visited_edges.begin(self.graph.edge_count());
        while let Some(dependent) = self.restat_queue.pop() {
            if self.visited_edges.replace(dependent.index()) {
                continue;
            }
            let outputs: &[NodeId] = &self.graph.edge(dependent).out;
            self.restat_consumers.extend_from_slice(outputs);
            for &output in outputs {
                self.restat_queue
                    .extend(self.graph.node(output).uses.iter().copied());
                self.restat_queue
                    .extend(self.graph.node_validation_uses(output).iter().copied());
            }
        }
        let disk = self.disk.clone();
        let mut stat = |path: &Path| disk.stat(path);
        recompute_dirty_with_validations(
            self.graph,
            &mut self.runtime,
            &mut self.scratch,
            &self.restat_consumers,
            Some(&Reconsidered::new(&self.visited_edges)),
            &mut stat,
        )?;
        Ok(())
    }
}
