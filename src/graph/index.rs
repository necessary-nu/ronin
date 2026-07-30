//! Open-addressed node index keyed by each node's own interned path.
//!
//! Storing only node identifiers means a path is never copied into a separate
//! map key: lookups hash the probe bytes and compare against the path the
//! arena already owns. This is the layout `htab.c` uses, and with niche-packed
//! identifiers a slot costs four bytes.

// [spec:samurai:def:htab.hashtablekey]
// [spec:samurai:def:htab.hashtable]
// [spec:samurai:def:htab.htabkey-fn]
// [spec:samurai:sem:htab.htabkey-fn]
// [spec:samurai:def:htab.mkhtab-fn]
// [spec:samurai:sem:htab.mkhtab-fn]
// [spec:samurai:def:htab.keyequal-fn]
// [spec:samurai:sem:htab.keyequal-fn]
// [spec:samurai:def:htab.keyindex-fn]
// [spec:samurai:sem:htab.keyindex-fn]
// [spec:samurai:def:htab.htabput-fn]
// [spec:samurai:sem:htab.htabput-fn]
// [spec:samurai:def:htab.htabget-fn]
// [spec:samurai:sem:htab.htabget-fn]
// [spec:samurai:def:htab.delhtab-fn]
// [spec:samurai:sem:htab.delhtab-fn]
use super::{shell_escape_path, Graph, Node, NodeId};
use crate::htab::rapidhashv1;
use crate::util::{BString, ByteSlice, IdVec};

#[derive(Default)]
pub(super) struct NodeIndex {
    slots: Vec<Option<NodeId>>,
    occupied: usize,
}

impl NodeIndex {
    const INITIAL_SLOTS: usize = 32;

    /// Locate `path`'s slot, which holds either its node or the vacancy where
    /// it belongs. `slots` is always a non-empty power of two here.
    fn probe(slots: &[Option<NodeId>], nodes: &[Node], path: &[u8], hash: u64) -> usize {
        let mask = slots.len() - 1;
        let mut index = usize::try_from(hash & mask as u64).expect("mask keeps the index in range");
        while let Some(candidate) = slots[index] {
            if nodes[candidate.index()].path.as_bytes() == path {
                break;
            }
            index = (index + 1) & mask;
        }
        index
    }

    pub(super) fn get(&self, nodes: &[Node], path: &[u8]) -> Option<NodeId> {
        if self.slots.is_empty() {
            return None;
        }
        self.slots[Self::probe(&self.slots, nodes, path, rapidhashv1(path))]
    }

    /// Record `node`, whose path must already be stored in `nodes`.
    pub(super) fn insert(&mut self, nodes: &[Node], node: NodeId) {
        // Grow past a three-quarters load factor to keep probe runs short.
        if (self.occupied + 1) * 4 > self.slots.len() * 3 {
            self.grow(nodes);
        }
        let path = nodes[node.index()].path.as_bytes();
        let index = Self::probe(&self.slots, nodes, path, rapidhashv1(path));
        if self.slots[index].is_none() {
            self.occupied += 1;
        }
        self.slots[index] = Some(node);
    }

    fn grow(&mut self, nodes: &[Node]) {
        let capacity = if self.slots.is_empty() {
            Self::INITIAL_SLOTS
        } else {
            self.slots.len() * 2
        };
        let previous = std::mem::replace(&mut self.slots, vec![None; capacity]);
        for node in previous.into_iter().flatten() {
            let path = nodes[node.index()].path.as_bytes();
            let index = Self::probe(&self.slots, nodes, path, rapidhashv1(path));
            self.slots[index] = Some(node);
        }
    }
}

// [spec:samurai:def:graph.mknode-fn]
// [spec:samurai:sem:graph.mknode-fn]
// [spec:samurai:def:graph.delnode-fn]
// [spec:samurai:sem:graph.delnode-fn]
pub(crate) fn mknode(graph: &mut Graph, path: BString) -> NodeId {
    if let Some(node) = graph.node_by_path.get(&graph.nodes, path.as_bytes()) {
        return node;
    }
    let shellpath = shell_escape_path(path.as_bytes());
    let node = NodeId::from_index(graph.nodes.len());
    graph.nodes.push(Node {
        path,
        shellpath,
        gen: None,
        uses: IdVec::new(),
        validation_uses: IdVec::new(),
    });
    graph.node_by_path.insert(&graph.nodes, node);
    node
}

/// Intern a path supplied as bytes, allocating only when the node is new.
pub(crate) fn mknode_bytes(graph: &mut Graph, path: &[u8]) -> NodeId {
    if let Some(node) = graph.node_by_path.get(&graph.nodes, path) {
        return node;
    }
    mknode(graph, BString::from(path))
}

// [spec:samurai:def:graph.nodeget-fn]
// [spec:samurai:sem:graph.nodeget-fn]
pub(crate) fn nodeget(graph: &Graph, path: &[u8]) -> Option<NodeId> {
    graph.node_by_path.get(&graph.nodes, path)
}
