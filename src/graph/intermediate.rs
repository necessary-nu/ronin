//! The half of GNU Make's intermediate file that the dirty scan cannot reach.
//!
//! A file the implicit rule search invented is allowed not to be there, and
//! [`super::recompute_edge_dirty_with`] says so where every other timestamp
//! comparison is made: the outputs stand in for the newest thing behind them
//! and the edge is dirty only if an input was. That settles what a consumer
//! sees, which is all the scan can settle, because it evaluates inputs before
//! the edge that reads them.
//!
//! Whether the file is worth creating is the other question, and only the
//! consumer can answer it. So it is asked here, on the way back down.

use super::{DirtyEvaluator, EdgeId, Graph, NodeId};
use crate::runtime::RuntimeState;

impl DirtyEvaluator {
    /// Ask for the intermediates whose absence was excused, wherever something
    /// that reads them has to run anyway.
    ///
    /// Walks down from `target` through the edges that are going to run and
    /// stops at every edge that is not, so an intermediate nothing needs stays
    /// uncreated — which is the whole point of it being intermediate.
    ///
    /// The walk starts wherever GNU Make's `must_make` would be set, which is
    /// one term wider than being out of date: a target that is simply not there
    /// has to be made, whatever its prerequisites say. `remake.c` reads the two
    /// the same way — `this_mtime == NONEXISTENT_MTIME` sets `must_make`, and
    /// `must_make` is what sends `update_file` over the intermediate
    /// prerequisites — and the difference is only visible from here, because
    /// the excusing that made the target look settled is what the scan does.
    /// A bare `.SECONDARY:` makes every file intermediate, so with it the whole
    /// chain under an absent goal is excused and this is the only thing left
    /// that can ask for any of it.
    pub(super) fn push_intermediates(
        &mut self,
        graph: &Graph,
        runtime: &mut RuntimeState,
        target: NodeId,
    ) {
        enum Step {
            Enter(EdgeId),
            Leave(EdgeId),
        }

        self.pushed.begin(graph.edge_count());
        let must_make = runtime.node(target).dirty() || runtime.node(target).absent_on_disk();
        let Some(root) = graph.node(target).generator.filter(|_| must_make) else {
            return;
        };
        let mut work = vec![Step::Enter(root)];
        while let Some(step) = work.pop() {
            match step {
                Step::Enter(edge) => {
                    if self.pushed.replace(edge.index()) {
                        continue;
                    }
                    // Pushed under this edge's inputs, so everything below it —
                    // including the inputs' own `Leave` — is done by the time
                    // this one is reached.
                    work.push(Step::Leave(edge));
                    let inputs: &[NodeId] = &graph.edge(edge).input;
                    for &input in inputs {
                        let Some(generator) = graph.node(input).generator else {
                            continue;
                        };
                        if runtime.edge(generator).absent_intermediate() {
                            ask_for(graph, runtime, generator);
                        } else if !runtime.node(input).dirty() {
                            continue;
                        }
                        work.push(Step::Enter(generator));
                    }
                }
                // A file asked for here is one the scan had already excused, so
                // nothing above it was called out of date on its account. What
                // reads it is going to see it change, and so is everything
                // between that and the goal — the scan's own conclusion, run
                // again over the answers this walk has just altered.
                Step::Leave(edge) => {
                    let inputs: &[NodeId] = &graph.edge(edge).input;
                    if inputs.iter().any(|&input| runtime.node(input).dirty()) {
                        ask_for(graph, runtime, edge);
                    }
                }
            }
        }
    }
}

fn ask_for(graph: &Graph, runtime: &mut RuntimeState, edge: EdgeId) {
    let outputs: &[NodeId] = &graph.edge(edge).out;
    for &output in outputs {
        runtime.node_mut(output).set_dirty(true);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Graph, NodeId, mkedge, mknode, nodeuse, recompute_dirty_with};
    use crate::env::mkenv;
    use crate::runtime::RuntimeState;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn generated(graph: &mut Graph, output: &str, input: &str) -> NodeId {
        let root = mkenv(graph, None);
        let output = mknode(graph, output);
        let input = mknode(graph, input);
        let edge = mkedge(graph, root);
        graph.edge_mut(edge).out.push(output);
        graph.edge_mut(edge).input.push(input);
        graph.edge_mut(edge).set_input_partitions(1, 1);
        graph.edge_mut(edge).intermediate = true;
        nodeuse(graph, input, edge);
        graph.node_mut(output).generator = Some(edge);
        output
    }

    /// A chain of them: asking for one intermediate has to ask for the ones
    /// underneath it, which is the part the consumer alone cannot say. Nothing
    /// under `mid2` is dirty on its own account — both files are equally
    /// absent whichever way the timestamps fall.
    #[test]
    fn ronin_graph_asking_for_an_intermediate_asks_for_the_chain_below_it() {
        let mut graph = Graph::default();
        let first = generated(&mut graph, "mid1", "src");
        let second = generated(&mut graph, "mid2", "mid1");
        let out = generated(&mut graph, "out", "mid2");
        graph
            .edge_mut(graph.node(out).generator.unwrap())
            .intermediate = false;

        let settled = |graph: &Graph, source: i64| {
            let mtimes = BTreeMap::from([("src".to_owned(), source), ("out".to_owned(), 2)]);
            let mut stat = |path: &Path| Ok(*mtimes.get(&*path.to_string_lossy()).unwrap_or(&0));
            let mut runtime = RuntimeState::new(graph);
            let dirty = recompute_dirty_with(graph, &mut runtime, out, &mut stat).unwrap();
            (dirty, runtime)
        };

        let (dirty, runtime) = settled(&graph, 1);
        assert!(!dirty);
        assert!(!runtime.node(first).dirty());
        assert!(!runtime.node(second).dirty());

        let (dirty, runtime) = settled(&graph, 3);
        assert!(dirty);
        assert!(runtime.node(first).dirty());
        assert!(runtime.node(second).dirty());
    }

    /// A bare `.SECONDARY:` makes every file intermediate, so the goal reads a
    /// chain no timestamp can call out of date: every file in it is equally
    /// absent. What decides is the goal itself — GNU Make must make a target
    /// that is not there, and making it is what asks for the chain.
    ///
    /// The goal's own edge is phony here because a Makefile target with no
    /// recipe compiles to one, which is exactly the case where the scan has
    /// nothing else to go on.
    #[test]
    fn ronin_graph_absent_goal_asks_intermediates() {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let phony = crate::env::mkrule(&mut graph, "phony".into());
        graph.set_phony_rule(phony);

        // Nothing behind the chain, so no timestamp anywhere can call any of
        // it out of date — the absent goal is the only thing left that can.
        let source = mknode(&mut graph, "src");
        let seeded = mkedge(&mut graph, root);
        graph.edge_mut(seeded).out.push(source);
        graph.edge_mut(seeded).intermediate = true;
        graph.node_mut(source).generator = Some(seeded);
        let middle = generated(&mut graph, "mid", "src");
        let goal = mknode(&mut graph, "goal");
        let alias = mkedge(&mut graph, root);
        graph.edge_mut(alias).rule = Some(phony);
        graph.edge_mut(alias).out.push(goal);
        graph.edge_mut(alias).input.push(middle);
        graph.edge_mut(alias).set_input_partitions(1, 1);
        nodeuse(&mut graph, middle, alias);
        graph.node_mut(goal).generator = Some(alias);

        let settled = |present: &[&str]| {
            let present = present
                .iter()
                .map(|path| ((*path).to_owned(), 1))
                .collect::<BTreeMap<_, _>>();
            let mut stat = |path: &Path| Ok(*present.get(&*path.to_string_lossy()).unwrap_or(&0));
            let mut runtime = RuntimeState::new(&graph);
            let dirty = recompute_dirty_with(&graph, &mut runtime, goal, &mut stat).unwrap();
            (dirty, runtime)
        };

        let (dirty, runtime) = settled(&[]);
        assert!(dirty);
        assert!(runtime.node(source).dirty());
        assert!(runtime.node(middle).dirty());

        // The control, and the whole reason the goal is what is asked about:
        // with the goal on disk there is nothing that must be made, and the
        // chain under it stays as absent as it was.
        let (dirty, runtime) = settled(&["goal"]);
        assert!(!dirty);
        assert!(!runtime.node(source).dirty());
        assert!(!runtime.node(middle).dirty());
    }
}
