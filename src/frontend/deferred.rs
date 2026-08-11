use super::{BuildGraph, Edge, Node};
use crate::graph::DeferredFreshness;
use crate::util::{BString, IdVec};

impl BuildGraph {
    #[cfg(test)]
    pub(crate) fn completion_join_observed_output(&self, node: Node) -> Option<Node> {
        self.arenas
            .node(node.0)
            .gen
            .and_then(|edge| self.arenas.completion_join_output(edge))
            .map(Node)
    }

    /// Defer an edge's real-output freshness decision until all prerequisites
    /// have settled.
    ///
    /// `outputs` are observed before prerequisite traversal and are not graph
    /// outputs of `edge`; its declared output is a private virtual completion
    /// identity. `always_new_inputs` are normal inputs that always enter the
    /// late new-input set. The scheduler exposes that set to the command in
    /// `new_inputs_environment` without assigning any meaning to the name.
    pub fn set_deferred_freshness(
        &mut self,
        edge: Edge,
        outputs: &[Node],
        always_dirty_output: bool,
        always_new_inputs: &[Node],
        new_inputs_environment: &[u8],
    ) {
        self.arenas.set_deferred_freshness(
            edge.0,
            DeferredFreshness {
                outputs: outputs.iter().map(|node| node.0).collect(),
                always_dirty_output,
                always_new_inputs: always_new_inputs.iter().map(|node| node.0).collect(),
                new_inputs_environment: BString::from(new_inputs_environment),
                activations: IdVec::new(),
            },
        );
    }

    /// Mark a commandless edge as the public completion point for private
    /// deferred actions. Once those actions settle, an existing real output is
    /// clean and downstream freshness is decided from its final timestamp.
    pub fn set_completion_join(&mut self, edge: Edge, observed_output: Node) {
        self.arenas.set_completion_join(edge.0, observed_output.0);
    }

    /// Add graph roots that become order-only dependencies only after a
    /// deferred freshness predicate succeeds.
    pub(crate) fn add_deferred_activations(&mut self, edge: Edge, roots: &[Node]) {
        if let Some(freshness) = self.arenas.deferred_freshness_mut(edge.0) {
            for root in roots {
                if !freshness.activations.contains(&root.0) {
                    freshness.activations.push(root.0);
                }
            }
        }
    }

    /// Redirect dependencies already attached to `from` onto `to`, preserving
    /// their edge partitions. Used when a late-declared completion proxy takes
    /// ownership of a logical front-end name while `from` remains the real
    /// filesystem node it observes.
    pub(crate) fn redirect_node_uses(&mut self, from: Node, to: Node) {
        self.arenas.redirect_node_uses(from.0, to.0);
        for target in &mut self.defaults {
            if *target == from.0 {
                *target = to.0;
            }
        }
    }
}
