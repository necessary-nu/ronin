use super::{Edge, NodeId};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct EdgePartitions {
    explicit_inputs: usize,
    non_order_only_inputs: usize,
    explicit_outputs: usize,
}

impl Edge {
    pub(crate) fn explicit_inputs(&self) -> &[NodeId] {
        &self.input[..self.partitions.explicit_inputs]
    }

    pub(crate) fn non_order_only_inputs(&self) -> &[NodeId] {
        &self.input[..self.partitions.non_order_only_inputs]
    }

    pub(crate) fn explicit_outputs(&self) -> &[NodeId] {
        &self.out[..self.partitions.explicit_outputs]
    }

    pub(crate) const fn explicit_input_count(&self) -> usize {
        self.partitions.explicit_inputs
    }

    pub(crate) const fn non_order_only_input_count(&self) -> usize {
        self.partitions.non_order_only_inputs
    }

    pub(crate) const fn explicit_output_count(&self) -> usize {
        self.partitions.explicit_outputs
    }

    pub(crate) fn set_input_partitions(
        &mut self,
        explicit_inputs: usize,
        non_order_only_inputs: usize,
    ) {
        debug_assert!(explicit_inputs <= non_order_only_inputs);
        debug_assert!(non_order_only_inputs <= self.input.len());
        self.partitions.explicit_inputs = explicit_inputs;
        self.partitions.non_order_only_inputs = non_order_only_inputs;
    }

    pub(crate) fn set_explicit_output_count(&mut self, count: usize) {
        debug_assert!(count <= self.out.len());
        self.partitions.explicit_outputs = count;
    }

    pub(crate) fn remove_input(&mut self, index: usize) -> NodeId {
        if index < self.partitions.explicit_inputs {
            self.partitions.explicit_inputs -= 1;
        }
        if index < self.partitions.non_order_only_inputs {
            self.partitions.non_order_only_inputs -= 1;
        }
        self.input.remove(index)
    }

    pub(crate) fn drain_discovered_inputs(&mut self, count: usize) {
        let end = self.partitions.non_order_only_inputs;
        let start = end.saturating_sub(count);
        self.input.drain(start..end);
        self.partitions.non_order_only_inputs -= count;
    }

    pub(crate) fn insert_implicit_inputs(&mut self, deps: &[NodeId]) {
        let index = self.partitions.non_order_only_inputs;
        self.input.insert_many(index, deps.iter().copied());
        self.partitions.non_order_only_inputs += deps.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::graph::{mkedge, Graph};

    #[test]
    fn partitions_follow_insertions_removals_and_discovered_input_replacement() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        let edge = graph.edge_mut(edge);
        edge.input = (0..5).map(NodeId::from_index).collect();
        edge.out = (5..8).map(NodeId::from_index).collect();
        edge.set_input_partitions(2, 4);
        edge.set_explicit_output_count(2);

        assert_eq!(edge.explicit_inputs().len(), 2);
        assert_eq!(edge.non_order_only_inputs().len(), 4);
        assert_eq!(edge.explicit_outputs().len(), 2);

        assert_eq!(edge.remove_input(1), NodeId::from_index(1));
        edge.drain_discovered_inputs(1);
        edge.insert_implicit_inputs(&[NodeId::from_index(9), NodeId::from_index(10)]);

        assert_eq!(edge.explicit_input_count(), 1);
        assert_eq!(edge.non_order_only_input_count(), 4);
        assert_eq!(edge.explicit_output_count(), 2);
        assert_eq!(
            edge.non_order_only_inputs(),
            &[
                NodeId::from_index(0),
                NodeId::from_index(2),
                NodeId::from_index(9),
                NodeId::from_index(10),
            ]
        );
    }
}
