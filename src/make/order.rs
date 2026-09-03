//! The order one unit's recursive recipes compose in, what a pass could not
//! carry, and the chain `.NOTPARALLEL` runs them in.
//!
//! One subject in three parts. [`dependency_ordered`] settles the order, and
//! the relation it settles it from answers two more questions the composition
//! asks about the same recipes: which of them a pass has to leave for the pass
//! after the staged work ([`Holds`]), and which of them wait for each other
//! where the makefiles said not to run two at once ([`SerialJobs`]). All three
//! live here rather than in the loop that walks them, so that loop reads as the
//! composition it is.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::sink::{self, ChildGroup, GraphSink, PendingSubninja};
use crate::frontend::{Edge, Node};

/// Put held recursive edges before any held edge that needs their outputs.
///
/// Kati emits recursive edges in target walk order, which is not necessarily
/// prerequisite order. A provisional compiler graph must nevertheless be able
/// to build a recursive target used as another recursive target's evaluation
/// input. Stable topological order makes that producer available first; Make's
/// ordinary cycle diagnostics remain responsible for a cyclic remainder.
///
/// A wrapper's prerequisite is not necessarily another wrapper's output, so
/// the producer is searched for through whatever ordinary targets stand
/// between the two. zsh's generated `Src/Makemod` is the shape that shows it:
/// `X.mdh` re-invokes the makefile and needs `X.mdhi`, which has an ordinary
/// recipe and needs `X.mdhs`, which re-invokes the makefile too. Comparing
/// only what each wrapper directly reads finds `X.mdh` no producer at all and
/// composes it first, against a provisional graph that has not been given the
/// edge which makes what it asks for.
///
/// The relation the sort consumed is reported beside the order rather than
/// thrown away, because the composition asks the same question again. See
/// [`Holds`].
pub(super) fn dependency_ordered(
    subninjas: Vec<PendingSubninja>,
    sink: &GraphSink,
) -> (Vec<PendingSubninja>, Holds) {
    let mut producers = HashMap::new();
    for (index, pending) in subninjas.iter().enumerate() {
        for output in pending.outputs() {
            producers.insert(output, index);
        }
    }

    let mut predecessors_of = vec![Vec::new(); subninjas.len()];
    let mut predecessor_counts = vec![0usize; subninjas.len()];
    let mut successors = vec![Vec::new(); subninjas.len()];
    for (consumer, pending) in subninjas.iter().enumerate() {
        let mut predecessors = HashSet::new();
        let mut walked = HashSet::new();
        let mut frontier = pending.evaluation_inputs();
        while let Some(input) = frontier.pop() {
            if !walked.insert(input) {
                continue;
            }
            let Some(&producer) = producers.get(&input) else {
                // Nothing held makes this one, so what makes it is an
                // ordinary edge and the wrapper being looked for is behind
                // that edge rather than at it.
                frontier.extend(sink.prerequisites_of(input));
                continue;
            };
            if producer != consumer && predecessors.insert(producer) {
                predecessors_of[consumer].push(producer);
                predecessor_counts[consumer] += 1;
                successors[producer].push(consumer);
            }
        }
    }

    let mut ready = predecessor_counts
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<BTreeSet<_>>();
    let mut sorted = vec![false; subninjas.len()];
    let mut order = Vec::with_capacity(subninjas.len());
    while let Some(index) = ready.pop_first() {
        order.push(index);
        sorted[index] = true;
        for successor in &successors[index] {
            predecessor_counts[*successor] -= 1;
            if predecessor_counts[*successor] == 0 {
                ready.insert(*successor);
            }
        }
    }
    let settled = order.len();
    order.extend((0..subninjas.len()).filter(|index| !sorted[*index]));

    // Where each recipe ended up, so the relation can be reported in the order
    // the composition walks rather than the order kati emitted.
    let mut place_of = vec![0usize; order.len()];
    for (place, index) in order.iter().enumerate() {
        place_of[*index] = place;
    }
    let read_off = order
        .iter()
        .map(|index| {
            predecessors_of[*index]
                .iter()
                .map(|producer| place_of[*producer])
                .collect()
        })
        .collect();
    let mut pending = subninjas.into_iter().map(Some).collect::<Vec<_>>();
    let ordered = order
        .iter()
        .map(|index| pending[*index].take().expect("each recipe ordered once"))
        .collect();
    (
        ordered,
        Holds {
            read_off,
            held: vec![false; order.len()],
            any: false,
            stopped: false,
            batched: settled == order.len(),
            settled,
        },
    )
}

/// Which of a unit's recursive recipes this pass cannot carry to their end.
///
/// A pass does not stop at the first of them. It records the boundary and walks
/// on, so every recipe that does not READ a held one stages its own boundary in
/// the same pass and the whole pass's staged work goes out together — where a
/// unit holding N independent recursive recipes used to cost N reads of itself,
/// and the read grows with every pass, so the cost was quadratic in N.
///
/// What may be walked on to is decided by the relation
/// [`dependency_ordered`] already built and nothing else. Composition order
/// puts a producer before anything that reads it, so holding each reader as it
/// arrives holds everything downstream of a boundary too, however long the
/// chain.
// [spec:ronin:req:make.compiler-input-staging]
pub(super) struct Holds {
    /// For each recipe, by its place in the composed order, the places of the
    /// recipes its evaluation inputs are read off.
    read_off: Vec<Vec<usize>>,
    held: Vec<bool>,
    any: bool,
    /// False where the sort left a cyclic remainder. The relation then says
    /// nothing about anything, so the first held recipe holds every recipe
    /// after it — which is exactly the read batching replaces.
    batched: bool,
    /// Whether a child composition has stopped inside this pass.
    stopped: bool,
    /// How many recipes the sort settled. A cyclic remainder stands in no
    /// relation to anything, which is what `.NOTPARALLEL` chaining has to know
    /// so it does not turn that remainder into a wait that cannot be satisfied.
    settled: usize,
}

impl Holds {
    /// Leave this recipe for the pass that follows the staged work.
    pub(super) fn hold(&mut self, place: usize) {
        self.held[place] = true;
        self.any = true;
    }

    /// Whether this recipe has to be left alone before anything is asked of it.
    ///
    /// A recipe waiting on work this pass has not built is left exactly where
    /// the read used to leave every recipe past the first boundary: unprobed,
    /// unstaged, with no wrapper of its own in the graph the staged build runs
    /// from. Holding it here rather than after its wrapper is probed is what
    /// keeps the freshness question being asked in the pass that can answer it.
    pub(super) fn reads_a_held_recipe(&self, place: usize) -> bool {
        self.any && (!self.batched || self.read_off[place].iter().any(|read| self.held[*read]))
    }

    /// Record that a child composition stopped at a boundary of its own.
    pub(super) const fn stopped_inside(&mut self) {
        self.stopped = true;
    }

    /// Whether this pass may still compose a child.
    pub(super) const fn composing(&self) -> bool {
        !self.stopped
    }

    /// Whether this pass left anything for the next one.
    pub(super) const fn any(&self) -> bool {
        self.any
    }

    /// Whether this recipe is one the sort placed, rather than part of a cyclic
    /// remainder that stands in no relation to anything.
    pub(super) const fn sorted(&self, place: usize) -> bool {
        place < self.settled
    }
}

/// The recursive recipes of a `.NOTPARALLEL` unit, in the order GNU Make would
/// have run them, each as the job it blocks in.
///
/// Left empty for a unit whose makefiles did not declare it, and wired into the
/// graph only once the whole unit has composed — see
/// [`GraphSink::chain_serial_jobs`] and [`Self::chain`].
#[derive(Default)]
pub(super) struct SerialJobs(Vec<sink::SerialJob>);

impl SerialJobs {
    /// Record one recipe that is going to run as one job.
    ///
    /// The job is the wrapper, and every edge of the children THIS recipe
    /// composed. A child it only reached is one copy of one piece of work,
    /// already held by the recipe that composed it — see [`ChildGroup::fresh`].
    ///
    /// `completion` is what finishing the job means, read before the wrapper is
    /// completed because completing it consumes the recipe.
    pub(super) fn push(&mut self, wrapper: Edge, children: &[ChildGroup], completion: Vec<Node>) {
        let fresh = children.iter().filter(|group| group.fresh);
        let edges = std::iter::once(wrapper)
            .chain(fresh.flat_map(|group| group.subgraph.fresh_edges.iter().copied()))
            .collect();
        self.0.push(sink::SerialJob { completion, edges });
    }

    /// Wire the chain, over a composition that completed.
    ///
    /// The last moment at which no staging pass can see what the chain adds,
    /// and the only one at which the edges recorded are the edges the build
    /// will actually schedule. A pass that held anything never reaches here:
    /// what it left is half a unit, and the chain has to be over the whole of
    /// one. That is also why batching cannot move the chain's order — the pass
    /// that wires it is the pass in which every recipe composed whole, in
    /// [`dependency_ordered`]'s order, exactly as before.
    pub(super) fn chain(&self, sink: &mut GraphSink) {
        sink.chain_serial_jobs(&self.0);
    }
}

/// Take what one recipe's children contribute into the unit's closure, and
/// what the ones this recipe composed made into what the unit made.
pub(super) fn adopt_child_groups(
    child_groups: Vec<ChildGroup>,
    subtree_edges: &mut Vec<Edge>,
    fresh_edges: &mut Vec<Edge>,
) {
    for group in child_groups {
        if group.fresh {
            fresh_edges.extend(group.subgraph.fresh_edges.iter().copied());
        }
        for edge in group.subgraph.edges {
            if !subtree_edges.contains(&edge) {
                subtree_edges.push(edge);
            }
        }
    }
}
