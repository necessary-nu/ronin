//! One file under two Make target names.
//!
//! GNU Make knows a target by the string the Makefile wrote, and a graph knows
//! a file by its canonical path. So `sub/../config.h` and `config.h` are two
//! targets there and one node here, and something has to say which of the two
//! rules makes the file — a question GNU Make never has to answer, because it
//! never merges them. What arrives at one node from two spellings is almost
//! always a depfile's `gcc -MP` mention beside the rule that really makes the
//! file, and a mention states nothing the rule does not.
//!
//! Everything the emission of one edge needs to decide that lives here,
//! including the two answers it has to have settled first: which nodes the
//! edge publishes, and whether any of them is dated in whole seconds.

use super::GraphSink;
use crate::frontend::Node;
use kati::anyhow;
use kati::build_sink::SinkEdge;
use kati::symtab::Interner;

impl GraphSink {
    /// Whether this Make rule says nothing beyond its output being a target.
    ///
    /// `gcc -MP` writes one for every header a compilation read, so that a
    /// header later deleted leaves a target with nothing to do rather than a
    /// prerequisite nothing can make. It carries no recipe, no prerequisite
    /// and no second output, and none of the other things a rule can say about
    /// its target: not `.PHONY`, not `.INTERMEDIATE`, no `::` record, no
    /// grouped or peer or withdrawable output, no directory search behind the
    /// name, no pool, and no recipe that came to nothing for `-t` to stand in
    /// for. Such a rule adds nothing to a node another rule already generates.
    ///
    /// No name the compiler invented for itself can answer yes to this, and
    /// none of them can reach the other side of the settlement either. The
    /// `::` chain's actions carry the members whose freshness they defer to,
    /// so `deferred_freshness_outputs` alone disqualifies every one of them;
    /// its join carries `completion_join`; the record's own target carries
    /// `declared_by_double_colon`. The two names minted here —
    /// `.ronin_grouped_join/N` and `.ronin_recipe_stage/N` — are stepped over
    /// until they are names no rule holds, so neither can be a node a mention
    /// arrived at first, and `.ronin_grouped_double/N` is chosen the same way
    /// on kati's side. See `super::invented`.
    pub(super) const fn mentions_only_its_name(edge: &SinkEdge<'_>) -> bool {
        edge.rule.is_none()
            && edge.inputs.is_empty()
            && edge.order_only_inputs.is_empty()
            && edge.forgiven_order_only_inputs.is_empty()
            && edge.implicit_outputs.is_empty()
            && edge.deferred_freshness_outputs.is_empty()
            && edge.disposable_outputs.is_empty()
            && edge.withdrawable_outputs.is_empty()
            && edge.peer_outputs.is_empty()
            && edge.settled_names.is_empty()
            && edge.searched_at.is_none()
            && edge.written_as.is_none()
            && edge.declared_by_double_colon.is_none()
            && edge.pool.is_none()
            && !edge.always_dirty
            && !edge.completion_join
            && !edge.intermediate
            && !edge.has_touchable_recipe
    }

    /// The outputs this edge generates in the graph, which for a `::` chain's
    /// completion join is the proxy the compiler invented to sequence it
    /// rather than the file the Makefile wrote.
    ///
    /// Every action in such a chain observes the file — its freshness is
    /// deferred to it — and everything that named the file comes to name the
    /// proxy instead, so that the chain is complete before any dependent runs.
    pub(super) fn published_outputs(
        &mut self,
        edge: &SinkEdge<'_>,
        completion_output: Node,
    ) -> anyhow::Result<Vec<Node>> {
        if !edge.completion_join {
            return Ok(vec![completion_output]);
        }
        self.observed_members.insert(edge.output, completion_output);
        let proxy = self.completion_proxy()?;
        self.graph.redirect_node_uses(completion_output, proxy);
        self.interned.insert(edge.output, proxy);
        Ok(vec![proxy])
    }

    /// Whether any of this edge's outputs is dated in whole seconds.
    ///
    /// An archive index dates its members in whole seconds, which the
    /// comparisons that put one on their target side have to read as the end
    /// of that second. Whether an output is one is decided from the name the
    /// Makefile wrote, here, and never from a path the build engine looks at.
    pub(super) fn dates_in_whole_seconds(names: &dyn Interner, edge: &SinkEdge<'_>) -> bool {
        std::iter::once(&edge.output)
            .chain(edge.implicit_outputs)
            .any(|output| kati::archive::split_archive_name(&output.as_bytes(&names)).is_some())
    }

    /// Settle two Make target names that spell one file, answering whether
    /// this edge is the one to leave out of the graph.
    ///
    /// A depfile written with `gcc -MP` states `sub/../config.h:` where the
    /// Makefile states `config.h: stamp-h1`, and a graph knows a file by its
    /// canonical path, so both spellings arrive at one node. GNU Make holds
    /// them apart as two targets and makes the file from the one rule that has
    /// a recipe; the other says only that the name is a target. So the rule
    /// with the recipe takes the node and the mention gives way, whichever of
    /// the two the read reached first — a depfile included ahead of the rule
    /// arrives the other way round, and then the mention is already an edge
    /// and has to hand the node back. What is left of a mention that has
    /// handed its node back generates nothing, and an edge generating nothing
    /// is one no walk of the graph can reach: dropped from the unit here, it
    /// is thereafter as absent as the mentions never made.
    ///
    /// A rule that states anything more than its own name is not a mention,
    /// and two of those over one path are a conflict with no single answer:
    /// they collide as before.
    pub(super) fn settle_one_path_two_spellings(
        &mut self,
        mention: bool,
        outputs: &[Node],
        implicit_outputs: &[Node],
    ) -> bool {
        if mention {
            return outputs
                .first()
                .is_some_and(|output| self.graph.generator(*output).is_some());
        }
        for output in outputs.iter().chain(implicit_outputs) {
            if let Some(held) = self.mentions.remove(output)
                && self.graph.generator(*output) == Some(held)
            {
                self.graph.release_output(held, *output);
                self.unit.edges.retain(|kept| *kept != held);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::GraphSink;
    use kati::build_sink::SinkEdge;
    use kati::symtab::Symbol;

    /// A rule that says nothing but its target's name, which is what a
    /// depfile's `-MP` line is. Every case below takes one of these and puts
    /// exactly one thing back in.
    fn a_bare_mention() -> SinkEdge<'static> {
        SinkEdge {
            rule: None,
            output: Symbol::UNKNOWN_FILENAME,
            implicit_outputs: &[],
            inputs: &[],
            order_only_inputs: &[],
            forgiven_order_only_inputs: &[],
            always_dirty: false,
            deferred_freshness_outputs: &[],
            deferred_freshness_always_dirty: false,
            deferred_freshness_ignores_dates: false,
            deferred_freshness_heads_the_record: false,
            deferred_always_new_inputs: &[],
            deferred_excluded_new_inputs: &[],
            deferred_new_input_names: &[],
            settled_names: &[],
            completion_join: false,
            has_touchable_recipe: false,
            intermediate: false,
            disposable_outputs: &[],
            withdrawable_outputs: &[],
            delete_on_error: false,
            peer_outputs: &[],
            searched_at: None,
            written_as: None,
            declared_by_double_colon: None,
            pool: None,
            loc: None,
        }
    }

    /// One name, for the field being put back in.
    const ONE_NAME: &[Symbol] = &[Symbol::UNKNOWN_FILENAME];

    /// The predicate is what makes giving the node up lossless, so it has to
    /// be the whole of "this rule states nothing else". Every field a rule can
    /// state something in disqualifies it, one field at a time.
    #[test]
    fn a_bare_mention_states_only_its_name() {
        assert!(GraphSink::mentions_only_its_name(&a_bare_mention()));

        let statements: [fn(&mut SinkEdge<'static>); 17] = [
            |edge| edge.rule = Some(0),
            |edge| edge.inputs = ONE_NAME,
            |edge| edge.order_only_inputs = ONE_NAME,
            |edge| edge.forgiven_order_only_inputs = ONE_NAME,
            |edge| edge.implicit_outputs = ONE_NAME,
            |edge| edge.deferred_freshness_outputs = ONE_NAME,
            |edge| edge.disposable_outputs = ONE_NAME,
            |edge| edge.withdrawable_outputs = ONE_NAME,
            |edge| edge.peer_outputs = ONE_NAME,
            |edge| edge.always_dirty = true,
            |edge| edge.completion_join = true,
            |edge| edge.intermediate = true,
            |edge| edge.has_touchable_recipe = true,
            |edge| edge.searched_at = Some(Symbol::UNKNOWN_FILENAME),
            |edge| edge.written_as = Some(Symbol::UNKNOWN_FILENAME),
            |edge| edge.declared_by_double_colon = Some(Symbol::UNKNOWN_FILENAME),
            |edge| edge.pool = Some(b"console"),
        ];
        for (which, state) in statements.into_iter().enumerate() {
            let mut edge = a_bare_mention();
            state(&mut edge);
            assert!(
                !GraphSink::mentions_only_its_name(&edge),
                "field {which} said something and the rule was still a bare mention"
            );
        }
    }

    /// A `::` chain's edges are not mentions, whatever else they leave empty.
    ///
    /// Their outputs are names the compiler invented, and an invented name
    /// that could be dropped or handed back would leave the graph referring to
    /// a node no edge makes. Two fields keep them out and each is enough on
    /// its own: an action defers its freshness to the members of the record it
    /// belongs to, and the join that completes the record says so.
    #[test]
    fn a_double_colon_edge_is_never_a_mention() {
        let mut action = a_bare_mention();
        action.deferred_freshness_outputs = ONE_NAME;
        assert!(!GraphSink::mentions_only_its_name(&action));

        let mut join = a_bare_mention();
        join.completion_join = true;
        assert!(!GraphSink::mentions_only_its_name(&join));
    }

    /// A mention reaching a node something already generates is the edge to
    /// leave out; one reaching a node nothing generates is the edge that makes
    /// the name a target at all, and is kept.
    #[test]
    fn a_mention_gives_way_to_a_generator() {
        let mut sink = GraphSink::new();
        let claimed = sink.graph.node(b"config.h").unwrap();
        let free = sink.graph.node(b"stamp-h1").unwrap();
        let rule = sink.phony;
        sink.graph
            .add_edge(crate::frontend::EdgeSpec {
                scope: sink.graph.root(),
                rule,
                explicit_outputs: &[claimed],
                implicit_outputs: &[],
                explicit_inputs: &[],
                implicit_inputs: &[],
                order_only_inputs: &[],
                validations: &[],
                always_dirty: false,
                intermediate: false,
                has_touchable_recipe: false,
                outputs_unaliased: true,
                outputs_low_resolution: false,
                bindings: Vec::new(),
            })
            .unwrap();

        assert!(sink.settle_one_path_two_spellings(true, &[claimed], &[]));
        assert!(!sink.settle_one_path_two_spellings(true, &[free], &[]));
    }

    /// And the other way round: the mention got there first, so the rule that
    /// makes the file takes the node off it, and what is left of the mention
    /// generates nothing and leaves the unit.
    #[test]
    fn a_rule_takes_its_node_from_a_mention() {
        let mut sink = GraphSink::new();
        let node = sink.graph.node(b"config.h").unwrap();
        let rule = sink.phony;
        let mention = sink
            .graph
            .add_edge(crate::frontend::EdgeSpec {
                scope: sink.graph.root(),
                rule,
                explicit_outputs: &[node],
                implicit_outputs: &[],
                explicit_inputs: &[],
                implicit_inputs: &[],
                order_only_inputs: &[],
                validations: &[],
                always_dirty: false,
                intermediate: false,
                has_touchable_recipe: false,
                outputs_unaliased: true,
                outputs_low_resolution: false,
                bindings: Vec::new(),
            })
            .unwrap();
        sink.mentions.insert(node, mention);
        sink.unit.edges.push(mention);

        assert!(!sink.settle_one_path_two_spellings(false, &[node], &[]));

        assert_eq!(sink.graph.generator(node), None);
        assert!(sink.unit.edges.is_empty());
        assert!(sink.mentions.is_empty());
        // And a second pass over the same node finds nothing left to do,
        // which is what makes the release safe to reach twice.
        assert!(!sink.settle_one_path_two_spellings(false, &[node], &[]));
        assert_eq!(sink.graph.generator(node), None);
    }
}
