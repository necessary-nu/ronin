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

/// One table slot: an identifier, and the low half of its path's hash.
///
/// Keeping the hash beside the identifier is what keeps a probe run local.
/// Without it, rejecting a slot that merely collided costs two dependent
/// random reads before the comparison can even begin — into `nodes` for the
/// span, then into `paths` for the bytes — and at a three-quarters load factor
/// most probe steps are exactly that rejection. With it, "not this one" is
/// answered from the slot array alone, so the run reads contiguous slots and
/// touches the graph once: when it has actually found the path.
///
/// The low half is the half worth keeping, because the slot index is drawn
/// from it. A stored high half would filter marginally better but would leave
/// [`NodeIndex::grow`] unable to place an entry without hashing its path
/// again, and rehashing is where this table spent most of its misses: the
/// doubling series rehashes about twice as many entries as the table finally
/// holds, each one a random read into `nodes`, a random read into `paths`, and
/// a hash of the bytes. Storing the bits the index needs turns that into a
/// mask of a value already in cache. What filtering the shared bits cost is
/// small — the index consumes the low `log2(slots)` of the thirty-two, and the
/// remainder still rejects all but one collision in several thousand.
#[derive(Clone, Copy, Default)]
struct Slot {
    node: Option<NodeId>,
    hash: u32,
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the low half is deliberately all a slot keeps"
)]
const fn low_half(hash: u64) -> u32 {
    hash as u32
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
    fn probe(slots: &[Slot], paths: &[u8], nodes: &[Node], path: &[u8], hash: u32) -> usize {
        let mask = slots.len() - 1;
        let mut index = hash as usize & mask;
        while let Some(candidate) = slots[index].node {
            if slots[index].hash == hash && node_bytes(paths, nodes, candidate) == path {
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
        let hash = low_half(rapidhashv1(path));
        self.slots[Self::probe(&self.slots, paths, nodes, path, hash)].node
    }

    /// Record `node`, whose path must already be stored in the arena.
    pub(super) fn insert(&mut self, paths: &[u8], nodes: &[Node], node: NodeId) {
        // Grow past a three-quarters load factor to keep probe runs short.
        if (self.occupied + 1) * 4 > self.slots.len() * 3 {
            self.grow();
        }
        let path = node_bytes(paths, nodes, node);
        let hash = low_half(rapidhashv1(path));
        let index = Self::probe(&self.slots, paths, nodes, path, hash);
        if self.slots[index].node.is_none() {
            self.occupied += 1;
        }
        self.slots[index] = Slot {
            node: Some(node),
            hash,
        };
    }

    /// Rebuild at twice the size without consulting the graph.
    ///
    /// A slot already carries the bits the new index is drawn from, so an
    /// entry moves by masking a value the loop just read. Placement order does
    /// not matter: every entry still lands in the first vacancy at or after
    /// its home slot, which is exactly the invariant linear probing needs.
    fn grow(&mut self) {
        let capacity = if self.slots.is_empty() {
            Self::INITIAL_SLOTS
        } else {
            self.slots.len() * 2
        };
        let mask = capacity - 1;
        let previous = std::mem::replace(&mut self.slots, vec![Slot::default(); capacity]);
        for slot in previous {
            if slot.node.is_none() {
                continue;
            }
            let mut index = slot.hash as usize & mask;
            while self.slots[index].node.is_some() {
                index = (index + 1) & mask;
            }
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
