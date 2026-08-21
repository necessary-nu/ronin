use super::{BuildGraph, Edge, Node};
use crate::graph::DeferredFreshness;
use crate::util::{BString, IdVec};

/// What a front end says about an edge whose real-output freshness it wants
/// decided late.
pub struct DeferredSpec<'a> {
    /// Observed before prerequisite traversal, and not graph outputs of the
    /// edge: its declared output is a private virtual completion identity.
    pub outputs: &'a [Node],
    /// Whether those outputs count as dirty however their timestamps compare.
    pub always_dirty_output: bool,
    /// Normal inputs that always enter the late new-input set.
    pub always_new_inputs: &'a [Node],
    /// Inputs that still affect freshness but stay out of the published value.
    pub excluded_new_inputs: &'a [Node],
    /// Inputs the published value spells differently from the graph's own name
    /// for them, paired with the spelling to publish. The scheduler carries the
    /// pair and reads neither spelling.
    pub new_input_names: &'a [(Node, &'a [u8])],
    /// The name the scheduler substitutes the published value for, to which it
    /// assigns no meaning of its own.
    pub new_inputs_variable: &'a [u8],
    /// Two further names for the same value, split the way a path splits: the
    /// directory each published name carries, and the name with that
    /// directory taken off. One word out per word in, in order.
    ///
    /// A front end needs these because the value does not exist before the
    /// scheduler picks it, so it cannot split the value itself and cannot
    /// split the name it substitutes for one either — a reference is one word
    /// with no directory in it, and halving it answers about the reference.
    /// Empty for a front end that wants neither. The scheduler assigns no
    /// meaning to either name.
    pub new_inputs_directories_variable: &'a [u8],
    /// The file half, under the same terms as the directory half above.
    pub new_inputs_filenames_variable: &'a [u8],
    /// Where the command that reads the value runs, and so what the names in
    /// it are spelt relative to. A front end that reads every unit where the
    /// build runs passes nothing and gets the graph's own names.
    pub new_inputs_directory: &'a [u8],
}

impl BuildGraph {
    #[cfg(test)]
    pub(crate) fn completion_join_observed_output(&self, node: Node) -> Option<Node> {
        self.arenas
            .node(node.0)
            .generator
            .and_then(|edge| self.arenas.completion_join_output(edge))
            .map(Node)
    }

    /// Defer an edge's real-output freshness decision until all prerequisites
    /// have settled.
    ///
    /// `outputs` are observed before prerequisite traversal and are not graph
    /// outputs of `edge`; its declared output is a private virtual completion
    /// identity. `always_new_inputs` are normal inputs that always enter the
    /// late new-input set. `excluded_new_inputs` still affect freshness but do
    /// not enter the published value. The scheduler substitutes that value for
    /// `new_inputs_variable` without assigning any meaning to the name, and
    /// spells the names in it relative to `new_inputs_directory`.
    pub fn set_deferred_freshness(&mut self, edge: Edge, deferred: &DeferredSpec<'_>) {
        self.arenas.set_deferred_freshness(
            edge.0,
            DeferredFreshness {
                outputs: deferred.outputs.iter().map(|node| node.0).collect(),
                always_dirty_output: deferred.always_dirty_output,
                always_new_inputs: deferred
                    .always_new_inputs
                    .iter()
                    .map(|node| node.0)
                    .collect(),
                excluded_new_inputs: deferred
                    .excluded_new_inputs
                    .iter()
                    .map(|node| node.0)
                    .collect(),
                new_input_names: deferred
                    .new_input_names
                    .iter()
                    .map(|(node, name)| (node.0, BString::from(*name)))
                    .collect(),
                new_inputs_variable: BString::from(deferred.new_inputs_variable),
                new_inputs_directories_variable: BString::from(
                    deferred.new_inputs_directories_variable,
                ),
                new_inputs_filenames_variable: BString::from(
                    deferred.new_inputs_filenames_variable,
                ),
                new_inputs_directory: BString::from(deferred.new_inputs_directory),
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

    /// Say that a node is a name this front end invented rather than a file,
    /// so the build neither stats it nor creates the directory it appears to
    /// sit in.
    ///
    /// The other two kinds of invented name are read off the edge that makes
    /// them — a deferred-freshness rule and a completion join both point at the
    /// real output through the edge. A staged recipe segment has no such
    /// indirection to be recognised by, so it says so.
    pub(crate) fn mark_invented_output(&mut self, node: Node) {
        self.arenas.mark_invented_output(node.0);
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
