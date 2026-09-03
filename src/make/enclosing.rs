//! What a child compilation sees made around it.
//!
//! The files the units enclosing a child make, by canonical path and the node
//! each is made at, are what the child's names for those paths resolve to —
//! see [`GraphSink::begin_subninja`]. This is the arithmetic of the set: what
//! a unit hands the children it composes, and what one recipe's own child is
//! not shown of it.

use super::sink;
use crate::htab::RapidHashSet;

/// What the children of a unit see made around them: what encloses the unit,
/// and the files the unit itself makes. See [`GraphSink::begin_subninja`].
pub(super) fn for_children(
    enclosing: &std::sync::Arc<sink::Enclosing>,
    generated: sink::Enclosing,
) -> std::sync::Arc<sink::Enclosing> {
    if generated.is_empty() {
        return std::sync::Arc::clone(enclosing);
    }
    let mut merged = (**enclosing).clone();
    merged.extend(generated);
    std::sync::Arc::new(merged)
}

/// `enclosing` without a recipe's own targets, for the child that recipe
/// composes: those targets are what the child's goals replace, and a child
/// whose goal named its own wrapper's node would be a child making its
/// parent's target.
pub(super) fn for_the_child_of(
    enclosing: &std::sync::Arc<sink::Enclosing>,
    pending: &sink::PendingSubninja,
) -> std::sync::Arc<sink::Enclosing> {
    let wrapper_outputs = pending.outputs().collect::<RapidHashSet<_>>();
    if !enclosing
        .values()
        .any(|node| wrapper_outputs.contains(node))
    {
        return std::sync::Arc::clone(enclosing);
    }
    let mut narrowed = (**enclosing).clone();
    narrowed.retain(|_, node| !wrapper_outputs.contains(node));
    std::sync::Arc::new(narrowed)
}
