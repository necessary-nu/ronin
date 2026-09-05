//! What a child compilation sees made around it.
//!
//! The files the units enclosing a child make, by canonical path and the node
//! each is made at, are what the child's names for those paths resolve to —
//! see [`GraphSink::begin_subninja`]. This is the arithmetic of the set: what
//! a unit hands the children it composes, and what one recipe's own child is
//! not shown of it.

use super::sink;
use crate::frontend::Node;

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

/// `enclosing` without the files that come OF this recursion, for the child
/// that recursion composes.
///
/// A child's name for an enclosing unit's file resolves to that unit's node
/// because GNU Make's child process finds the file made — the phase that made
/// it ran before the recipe that started the child. That is a claim about
/// ORDER, and where it is false the child is a child of nothing: it must keep
/// its own rule, because its own rule is what makes the file.
///
/// It is false for anything the enclosing unit makes only by running THIS
/// recursion. The kernel writes the shape plainly: `vmlinux_o` carries
/// `$(MAKE) -f $(srctree)/scripts/Makefile.vmlinux_o`, and the rule after it
/// reads `vmlinux.o modules.builtin.modinfo modules.builtin: vmlinux_o` with
/// `@:` for a recipe.
///
/// That rule writes nothing; `scripts/Makefile.vmlinux_o` links
/// `vmlinux.o` and the parent's `@:` runs afterwards to say so. Handed
/// `vmlinux.o` as an enclosing file, the child gives up the rule that is the
/// only producer there is and points at a node that waits for the child — a
/// cycle, and the recipe that would have made the file is gone from the graph
/// either way.
///
/// The wrapper's own outputs are the near case of the same thing — a child
/// whose goal named its own wrapper's node would be a child making its
/// parent's target — so the walk starts from them and they narrow themselves.
///
/// What is NOT narrowed is a file made beside the recursion. zsh's every
/// module subdirectory writes `$(dir_top)/Src/zsh.export: ; false` for a file
/// `Src` makes, and `Src` makes it before the subdirectory it is handed to
/// runs: nothing on the way to it waits for that subdirectory, so it survives
/// here and the stub stays unread.
// [spec:ronin:req:make.recursive-invocation+4]
pub(super) fn for_the_child_of(
    enclosing: &std::sync::Arc<sink::Enclosing>,
    pending: &sink::PendingSubninja,
    sink: &sink::GraphSink,
) -> std::sync::Arc<sink::Enclosing> {
    if enclosing.is_empty() {
        return std::sync::Arc::clone(enclosing);
    }
    let wrapper_outputs = pending.outputs().collect::<Vec<Node>>();
    let of_the_recursion = sink.waiting_on(enclosing.values().copied(), &wrapper_outputs);
    if of_the_recursion.is_empty() {
        return std::sync::Arc::clone(enclosing);
    }
    let mut narrowed = (**enclosing).clone();
    narrowed.retain(|_, node| !of_the_recursion.contains(node));
    std::sync::Arc::new(narrowed)
}
