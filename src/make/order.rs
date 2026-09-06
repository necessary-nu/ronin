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
// [spec:ronin:req:make.compiler-input-staging+2]
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

/// Every edge one unit's compilation has taken into its closure, in the order
/// it took them and each of them once.
///
/// The order is the answer and the set is only how the question is asked: a
/// unit's closure is handed to its parent to be adopted in turn, and what the
/// graph is built from is the sequence. The set is what keeps the membership
/// question off that sequence — a unit's closure reaches the size of the whole
/// graph at the root of a kernel tree, and asking a list of that length per
/// edge adopted is quadratic in the subtree.
///
/// Only adoption deduplicates. [`Self::push`] takes an edge whether or not the
/// closure holds it, because its callers hand over a unit's own emission and
/// its wrappers, which arrive distinct and are the closure's first entries.
pub(super) struct EdgeClosure {
    edges: Vec<Edge>,
    seen: crate::htab::RapidHashSet<Edge>,
}

impl EdgeClosure {
    /// The closure a unit starts with: the edges its own Makefiles emitted.
    pub(super) fn of(edges: Vec<Edge>) -> Self {
        let seen = edges.iter().copied().collect();
        Self { edges, seen }
    }

    /// Take one edge this unit made itself.
    pub(super) fn push(&mut self, edge: Edge) {
        self.seen.insert(edge);
        self.edges.push(edge);
    }

    /// Take one edge a child contributed, if the closure has not got it.
    fn adopt(&mut self, edge: Edge) {
        if self.seen.insert(edge) {
            self.edges.push(edge);
        }
    }

    /// The closure, in the order it was taken.
    pub(super) fn into_edges(self) -> Vec<Edge> {
        self.edges
    }
}

/// Take what one recipe's children contribute into the unit's closure, and
/// what the ones this recipe composed made into what the unit made.
pub(super) fn adopt_child_groups(
    child_groups: Vec<ChildGroup>,
    subtree_edges: &mut EdgeClosure,
    fresh_edges: &mut Vec<Edge>,
) {
    for group in child_groups {
        if group.fresh {
            fresh_edges.extend(group.subgraph.fresh_edges.iter().copied());
        }
        for edge in group.subgraph.edges {
            subtree_edges.adopt(edge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeClosure, adopt_child_groups};
    use crate::frontend::{BuildGraph, Edge, EdgeSpec, Template};
    use crate::make::sink::{ChildGroup, UnitSubgraph};

    /// As many edges as a test wants, to stand for what a unit and its children
    /// contribute to one closure.
    fn edges(graph: &mut BuildGraph, count: usize) -> Vec<Edge> {
        let root = graph.root();
        let command = graph.binding(b"command");
        let rule = graph
            .define_rule(root, b"touch", vec![(command, Template::literal(b"true"))])
            .expect("rule");
        (0..count)
            .map(|at| {
                let output = graph.node(format!("out{at}").as_bytes()).expect("node");
                graph
                    .add_edge(EdgeSpec {
                        scope: root,
                        rule,
                        explicit_outputs: &[output],
                        implicit_outputs: &[],
                        explicit_inputs: &[],
                        implicit_inputs: &[],
                        order_only_inputs: &[],
                        validations: &[],
                        always_dirty: false,
                        intermediate: false,
                        has_touchable_recipe: false,
                        outputs_unaliased: false,
                        outputs_low_resolution: false,
                        bindings: Vec::new(),
                    })
                    .expect("edge")
            })
            .collect()
    }

    fn group(edges: &[Edge]) -> ChildGroup {
        ChildGroup {
            subgraph: UnitSubgraph {
                targets: Vec::new(),
                edges: edges.to_vec(),
                fresh_edges: Vec::new(),
            },
            fresh: false,
        }
    }

    /// A child two recipes both reach contributes its edges once, and the
    /// closure stays in the order the compilation took them. The order is what
    /// the graph is built from, so answering the membership question with a set
    /// must not answer the ordering one differently.
    #[test]
    fn ronin_make_a_shared_child_lands_once_ordered() {
        let mut graph = BuildGraph::new();
        let made = edges(&mut graph, 5);
        let mut closure = EdgeClosure::of(vec![made[0]]);
        let mut fresh = Vec::new();
        closure.push(made[1]);
        adopt_child_groups(vec![group(&[made[3], made[2]])], &mut closure, &mut fresh);
        // A second recipe reaching the same child, and one edge more.
        adopt_child_groups(
            vec![group(&[made[2], made[4], made[3]])],
            &mut closure,
            &mut fresh,
        );
        assert_eq!(
            closure.into_edges(),
            vec![made[0], made[1], made[3], made[2], made[4]]
        );
    }

    /// An edge the unit already holds is not adopted a second time, whether it
    /// came from the unit's own emission or from a wrapper it pushed.
    #[test]
    fn ronin_make_a_closure_keeps_own_edges_once() {
        let mut graph = BuildGraph::new();
        let made = edges(&mut graph, 3);
        let mut closure = EdgeClosure::of(vec![made[0], made[1]]);
        let mut fresh = Vec::new();
        closure.push(made[2]);
        adopt_child_groups(
            vec![group(&[made[1], made[2], made[0]])],
            &mut closure,
            &mut fresh,
        );
        assert_eq!(closure.into_edges(), vec![made[0], made[1], made[2]]);
    }
}
