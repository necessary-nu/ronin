//! What a finished edge does to the edges that were waiting on it.
//!
//! Three answers, and the third is the one Ninja has no need of. A dependent
//! whose last wait settled is either scheduled or taken out of the plan —
//! Ninja's `Plan::CleanNode` — and a dependent of a *failed* edge is, in
//! Ninja, simply never released: its `pending` count never reaches zero.
//!
//! GNU Make needs the third. Under `-k` a double-colon chain runs its later
//! entries after an earlier one failed, so a wait the graph marked forgiven
//! (see [`crate::graph`]'s `forgiven` module) releases its consumer whatever
//! the edge it waited for finished as. Carrying a failure down to the edges
//! that cannot run, so that the ones that can are found, is what this module
//! is for.

use super::{EdgeId, Graph, Plan, ReadyEdge, RuntimeState};

/// What settling a dependent whose last wait just finished did to the plan.
enum Released {
    /// It has a dirty output, so it is now work waiting to be dispatched.
    Scheduled,
    /// Nothing dirtied it, so it left the plan without running. `Some` names it
    /// when the caller owes the progress count an edge it will never see.
    Settled(Option<EdgeId>),
    /// Something had already settled it, so this changed nothing.
    Already,
}

impl Plan {
    /// Carry a failure down to what was waiting on it, releasing the waits that
    /// only wanted the ordering.
    ///
    /// Ninja has one answer here and it is to do nothing: a dependent of a
    /// failed edge never has its `pending` count reach zero, so it is never
    /// ready and never runs. That is still the answer for every ordinary wait.
    /// A wait the graph marked forgiven — see [`crate::graph`]'s `forgiven`
    /// module — asked only to be sequenced behind the edge, so its consumer is
    /// released now that the edge has finished, whatever it finished as.
    ///
    /// The failure propagates past an unforgiving dependent rather than
    /// stopping at it, because that dependent can never run either, and
    /// something further down may be waiting on *it* forgivingly. GNU Make
    /// reaches the same place from the other direction: a double-colon entry
    /// whose own prerequisite failed is not run, and the target's next entry
    /// still is.
    ///
    /// An abandoned edge is not completed and is never scheduled, so the build
    /// still ends with the plan unfinished and reports that it could not make
    /// progress.
    pub(super) fn abandon_dependents(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        failed: EdgeId,
    ) -> Vec<EdgeId> {
        if !graph.has_forgiven_order() {
            return Vec::new();
        }
        self.abandoned.resize(graph.edge_count(), false);
        let mut pruned = Vec::new();
        let mut work = vec![failed];
        while let Some(edge) = work.pop() {
            for index in 0..self.dependents[edge.index()].len() {
                let dependent = self.dependents[edge.index()][index];
                if !graph.order_forgives_generator(dependent, edge) {
                    if !std::mem::replace(&mut self.abandoned[dependent.index()], true) {
                        work.push(dependent);
                    }
                    continue;
                }
                self.pending[dependent.index()] -= 1;
                if self.pending[dependent.index()] != 0 || self.abandoned[dependent.index()] {
                    continue;
                }
                if let Released::Settled(taken) = self.settle_released(graph, runtime, dependent) {
                    pruned.extend(taken);
                }
            }
        }
        pruned
    }

    /// Unblock what the finished edge was holding, and prune what it turned out
    /// not to have dirtied.
    ///
    /// A dependent reached with every input settled and no dirty output is the
    /// case Ninja's `Plan::CleanNode` handles: a `restat` found the input
    /// unchanged, so the consumer is no longer work. Ninja both drops its want
    /// and tells the status printer the plan lost an edge, which is why its
    /// total shrinks mid-build; the pruned command edges are returned so the
    /// caller can do the same. Such an edge had a pending input a moment ago
    /// and so had never started — nothing already counted as finished is ever
    /// taken away.
    pub(super) fn release_dependents(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        finished: EdgeId,
    ) -> Vec<EdgeId> {
        let mut pruned = Vec::new();
        let mut work = vec![finished];
        while let Some(edge) = work.pop() {
            for index in 0..self.dependents[edge.index()].len() {
                let dependent = self.dependents[edge.index()][index];
                self.pending[dependent.index()] -= 1;
                if self.pending[dependent.index()] != 0 {
                    continue;
                }
                if self.abandoned.get(dependent.index()).copied() == Some(true) {
                    continue;
                }
                if let Released::Settled(taken) = self.settle_released(graph, runtime, dependent) {
                    pruned.extend(taken);
                    work.push(dependent);
                }
            }
        }
        pruned
    }

    /// Schedule a dependent whose last wait just settled, or take it out of the
    /// plan when it turns out nothing dirtied it.
    ///
    /// The second case is Ninja's `Plan::CleanNode`: a `restat` found the input
    /// unchanged, so the consumer is no longer work. Ninja both drops its want
    /// and tells the status printer the plan lost an edge, which is why its
    /// total shrinks mid-build; the pruned command edges are answered back so
    /// the caller can do the same. Such an edge had a pending input a moment
    /// ago and so had never started — nothing already counted as finished is
    /// ever taken away.
    fn settle_released(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        dependent: EdgeId,
    ) -> Released {
        let dirty = graph
            .edge(dependent)
            .out
            .iter()
            .any(|output| runtime.node(*output).dirty());
        if dirty {
            self.ready
                .push(ReadyEdge::new(self.weight[dependent.index()], dependent));
            return Released::Scheduled;
        }
        if std::mem::replace(&mut self.completed[dependent.index()], true) {
            return Released::Already;
        }
        self.completed_count += 1;
        let rule = graph.edge(dependent).rule;
        let pruned = self.unwant(dependent) && rule.is_some() && !graph.is_phony_rule(rule);
        Released::Settled(pruned.then_some(dependent))
    }
}
