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
use crate::util::{ByteSlice, IdVec};

/// One table slot: an identifier, and a fingerprint of its path's hash.
///
/// The fingerprint is what keeps a probe run local. Without it, rejecting a
/// slot that merely collided costs two dependent random reads before the
/// comparison can even begin — into `nodes` for the span, then into `paths`
/// for the bytes — and at a three-quarters load factor most probe steps are
/// exactly that rejection. With it, "not this one" is answered from the slot
/// array alone, so the run reads contiguous slots and touches the graph once:
/// when it has actually found the path.
#[derive(Clone, Copy, Default)]
struct Slot {
    node: Option<NodeId>,
    fingerprint: u32,
}

/// Bits of the hash the slot index does not already use.
///
/// The index takes the low bits, so a fingerprint drawn from them would agree
/// on every slot in a probe run and filter nothing.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a fingerprint is deliberately a lossy digest of the high half"
)]
const fn fingerprint(hash: u64) -> u32 {
    (hash >> 32) as u32
}

#[derive(Default)]
pub(super) struct NodeIndex {
    slots: Vec<Slot>,
    occupied: usize,
}

impl NodeIndex {
    const INITIAL_SLOTS: usize = 32;

    /// Locate `path`'s slot, which holds either its node or the vacancy where
    /// it belongs. `slots` is always a non-empty power of two here.
    fn probe(slots: &[Slot], paths: &[u8], nodes: &[Node], path: &[u8], hash: u64) -> usize {
        let mask = slots.len() - 1;
        let fingerprint = fingerprint(hash);
        let mut index = usize::try_from(hash & mask as u64).expect("mask keeps the index in range");
        while let Some(candidate) = slots[index].node {
            if slots[index].fingerprint == fingerprint
                && node_bytes(paths, nodes, candidate) == path
            {
                break;
            }
            index = (index + 1) & mask;
        }
        index
    }

    pub(super) fn get(&self, paths: &[u8], nodes: &[Node], path: &[u8]) -> Option<NodeId> {
        if self.slots.is_empty() {
            return None;
        }
        self.slots[Self::probe(&self.slots, paths, nodes, path, rapidhashv1(path))].node
    }

    /// Record `node`, whose path must already be stored in the arena.
    pub(super) fn insert(&mut self, paths: &[u8], nodes: &[Node], node: NodeId) {
        // Grow past a three-quarters load factor to keep probe runs short.
        if (self.occupied + 1) * 4 > self.slots.len() * 3 {
            self.grow(paths, nodes);
        }
        let path = node_bytes(paths, nodes, node);
        let hash = rapidhashv1(path);
        let index = Self::probe(&self.slots, paths, nodes, path, hash);
        if self.slots[index].node.is_none() {
            self.occupied += 1;
        }
        self.slots[index] = Slot {
            node: Some(node),
            fingerprint: fingerprint(hash),
        };
    }

    fn grow(&mut self, paths: &[u8], nodes: &[Node]) {
        let capacity = if self.slots.is_empty() {
            Self::INITIAL_SLOTS
        } else {
            self.slots.len() * 2
        };
        let previous = std::mem::replace(&mut self.slots, vec![Slot::default(); capacity]);
        for slot in previous {
            let Some(node) = slot.node else { continue };
            let path = node_bytes(paths, nodes, node);
            let index = Self::probe(&self.slots, paths, nodes, path, rapidhashv1(path));
            self.slots[index] = slot;
        }
    }
}

/// Read one node's path out of the arena.
fn node_bytes<'arena>(paths: &'arena [u8], nodes: &[Node], node: NodeId) -> &'arena [u8] {
    let span = nodes[node.index()].path;
    &paths[span.offset as usize..][..span.len as usize]
}

// [spec:samurai:def:graph.mknode-fn]
// [spec:samurai:sem:graph.mknode-fn]
// [spec:samurai:def:graph.delnode-fn]
// [spec:samurai:sem:graph.delnode-fn]
/// Intern a path, allocating nothing when the node already exists.
///
/// A new node appends into the arena rather than taking ownership of a
/// buffer, so interning never allocates per path at all.
pub(crate) fn mknode(graph: &mut Graph, path: impl AsRef<[u8]>) -> NodeId {
    let path = path.as_ref();
    if let Some(node) = graph.node_by_path.get(&graph.paths, &graph.nodes, path) {
        return node;
    }
    let quoted = shell_escape_path(path);
    let path = graph.intern_bytes(path);
    let shellpath = quoted.map(|quoted| graph.intern_bytes(quoted.as_bytes()));
    let node = NodeId::from_index(graph.nodes.len());
    graph.nodes.push(Node {
        path,
        shellpath,
        gen: None,
        uses: IdVec::new(),
        validation_uses: IdVec::new(),
    });
    graph.node_by_path.insert(&graph.paths, &graph.nodes, node);
    node
}

// [spec:samurai:def:graph.nodeget-fn]
// [spec:samurai:sem:graph.nodeget-fn]
pub(crate) fn nodeget(graph: &Graph, path: &[u8]) -> Option<NodeId> {
    graph.node_by_path.get(&graph.paths, &graph.nodes, path)
}
