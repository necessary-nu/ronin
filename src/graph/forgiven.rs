//! Order-only inputs whose failure their consumer is willing to outlive.
//!
//! Ninja's graph has two answers to "when may this edge start": an ordinary
//! input, whose timestamp also decides whether the edge is dirty, and an
//! order-only input, whose timestamp does not. Both mean "after it succeeded" —
//! a failed input never releases its consumer, and there is no third spelling.
//!
//! GNU Make needs one. Under `-k` a double-colon target whose first entry fails
//! still runs its later entries: `update_file` walks the chain and abandons the
//! rest only when `keep_going_flag` is clear
//! (reference/gnumake/src/remake.c). The entries are strictly ordered — the
//! second recipe runs after the first — so the ordering edge between them has
//! to mean "after it finished, whatever it did".
//!
//! That is what an entry here says: the wait holds and the status waited for is
//! discarded. It is recorded per `(consumer, input)` pair rather than on either
//! end, because forgiveness belongs to the relation: the same action is an
//! ordinary dependency of the target's completion join, which is not forgiven
//! and is why the target is still not remade.
//!
//! Without `-k` this changes nothing observable. The failure exhausts the
//! allowed-failure budget, the build stops dispatching, and the released
//! successor never starts — which is GNU Make's answer for the same makefile.
//!
//! Beside the edge arena for the reason `withdrawal` and `unmade_makefiles`
//! are beside theirs: no edge of a Ninja manifest is ever in it, and almost no
//! edge of a Makefile's graph either.

use super::{EdgeId, Graph, NodeId};

impl Graph {
    /// Whether any edge in this graph forgives an input at all.
    ///
    /// The scheduler reads this before it does any work on a failure, so a
    /// graph without a double-colon chain pays one emptiness test.
    pub(crate) fn has_forgiven_order(&self) -> bool {
        !self.forgiven_order.is_empty()
    }

    /// Record that `edge` waits for `input` only to be sequenced behind it.
    pub(crate) fn forgive_order(&mut self, edge: EdgeId, input: NodeId) {
        self.forgiven_order.insert((edge, input));
    }

    /// Whether this exact wait is one its consumer outlives a failure of.
    pub(crate) fn order_is_forgiven(&self, edge: EdgeId, input: NodeId) -> bool {
        self.forgiven_order.contains(&(edge, input))
    }

    /// Whether every way `dependent` waits for `generator` is forgiven.
    ///
    /// Asked of a generator rather than of a node because that is the shape the
    /// plan holds: one edge finished, and each consumer has to answer whether
    /// that finishing releases it. A consumer that names two of the generator's
    /// outputs, one forgiven and one not, is blocked — the unforgiven wait is
    /// still a wait for success.
    pub(crate) fn order_forgives_generator(&self, dependent: EdgeId, generator: EdgeId) -> bool {
        if self.forgiven_order.is_empty() {
            return false;
        }
        let mut waited = false;
        for input in &self.edge(dependent).input {
            if self.node(*input).generator != Some(generator) {
                continue;
            }
            if !self.order_is_forgiven(dependent, *input) {
                return false;
            }
            waited = true;
        }
        waited
    }
}
