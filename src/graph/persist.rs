//! A build graph written to a file and read back as the same graph.
//!
//! The graph is five arenas of plain data cross-referenced by index, one blob
//! of path bytes, an index over the nodes, and side tables that only a Make
//! compilation fills. Nothing in it points outside itself, so it is written
//! as it stands: every arena in order, every identifier as the index it is,
//! every side table sorted by its key so that two writes of one graph are one
//! sequence of bytes.
//!
//! WHAT A READER TRUSTS, which is nothing. A file is refused, not partly
//! believed, over any byte that does not fit: a length past the end, an
//! identifier past its arena, a path whose span leaves the blob, a node that
//! would be indexed twice. The caller of a refusal composes the graph again,
//! which is what every invocation did before the file existed, so the cost
//! of refusing is time and the cost of believing would be a wrong build.
//!
//! The node index is not written. Which nodes are in it is — an isolated node
//! shares its path with an indexed one and is deliberately absent, and
//! nothing on the node says which it is — and the table is rebuilt from that
//! list. The rebuilt table need not have the same slot layout to answer every
//! lookup the same way.

use super::{EdgeId, FreshnessHistory, NodeId, PathSpan};
use crate::env::{EnvironmentId, PoolId, RuleId};
use crate::frontend::BuildGraph;
use crate::graph::deferred::DeferredFreshness;
use crate::graph::searched::SettledView;
use crate::names::{Bindings, VarId};
use crate::util::{EvalPart, EvalString};
use std::io::{self, Write};

const MAGIC: &[u8] = b"ronin-graph\x00";
/// Bumped whenever the bytes change meaning.
const VERSION: u32 = 1;

pub(crate) fn write(graph: &BuildGraph, out: &mut dyn Write) -> io::Result<()> {
    let (arenas, state, defaults) = (graph.arenas(), &graph.state, &graph.defaults);
    let mut w = Writer { out };
    w.raw(MAGIC)?;
    w.u32(VERSION)?;
    w.len(arenas.names.len())?;
    w.len(arenas.paths.len())?;
    w.len(arenas.nodes.len())?;
    w.len(arenas.edges.len())?;
    w.len(arenas.environments.len())?;
    w.len(arenas.rules.len())?;
    w.len(arenas.pools.len())?;

    for index in 0..arenas.names.len() {
        w.bytes(arenas.names.name(VarId::from_index(index)))?;
    }
    w.raw(&arenas.paths)?;
    for node in &arenas.nodes {
        w.span(node.path)?;
        w.option(node.shellpath, Writer::span)?;
        w.option(node.generator.map(EdgeId::index), Writer::len)?;
        w.ids(node.uses.iter().map(|edge| edge.index()))?;
    }
    for edge in &arenas.edges {
        w.option(edge.rule.map(RuleId::index), Writer::len)?;
        w.option(edge.pool.map(PoolId::index), Writer::len)?;
        w.len(edge.env.index())?;
        w.bindings(&edge.bindings, |w, value| w.bytes(value))?;
        w.ids(edge.out.iter().map(|node| node.index()))?;
        w.ids(edge.input.iter().map(|node| node.index()))?;
        w.ids(edge.validation.iter().map(|node| node.index()))?;
        w.option(edge.dyndep.map(NodeId::index), Writer::len)?;
        let flags = [
            edge.always_dirty,
            edge.intermediate,
            edge.has_touchable_recipe,
            edge.outputs_unaliased,
            edge.outputs_low_resolution,
            edge.outputs_reobserved,
            edge.recipe_begun,
            edge.freshness_history == FreshnessHistory::FilesystemOnly,
        ];
        w.u8(flags
            .iter()
            .enumerate()
            .fold(0, |byte, (bit, set)| byte | (u8::from(*set) << bit)))?;
        w.len(edge.explicit_input_count())?;
        w.len(edge.non_order_only_input_count())?;
        w.len(edge.explicit_output_count())?;
    }
    for environment in &arenas.environments {
        w.option(environment.parent.map(EnvironmentId::index), Writer::len)?;
        w.bindings(&environment.bindings, |w, value| w.bytes(value))?;
        w.len(environment.rules.len())?;
        for (name, rule) in &environment.rules {
            w.bytes(name)?;
            w.len(rule.index())?;
        }
    }
    for rule in &arenas.rules {
        w.bytes(&rule.name)?;
        w.bindings(&rule.bindings, Writer::template)?;
    }
    for pool in &arenas.pools {
        w.bytes(&pool.name)?;
        w.option(pool.depth().map(std::num::NonZeroUsize::get), Writer::len)?;
    }
    w.ids(arenas.node_ids().filter_map(|node| {
        let path = arenas.node_path(node);
        (super::nodeget(arenas, path) == Some(node)).then_some(node.index())
    }))?;
    write_side_tables(&mut w, arenas)?;

    w.len(state.root.index())?;
    w.len(state.pools().len())?;
    for (name, pool) in state.pools() {
        w.bytes(name)?;
        w.len(pool.index())?;
    }
    w.ids(defaults.iter().map(|node| node.index()))
}

/// Everything beside the arenas, after them because every entry names one.
fn write_side_tables(w: &mut Writer<'_>, arenas: &super::Graph) -> io::Result<()> {
    w.table(&arenas.validation_uses, |w, node, edges| {
        w.len(node.index())?;
        w.ids(edges.iter().map(|edge| edge.index()))
    })?;
    w.ids(arenas.dyndep_edges.iter().map(|edge| edge.index()))?;
    w.table(&arenas.deferred_freshness, |w, edge, freshness| {
        w.len(edge.index())?;
        w.freshness(freshness)
    })?;
    w.table(&arenas.completion_joins, |w, edge, node| {
        w.len(edge.index())?;
        w.len(node.index())
    })?;
    w.table(&arenas.withdrawal, |w, edge, withdrawal| {
        w.len(edge.index())?;
        w.ids(withdrawal.outputs.iter().map(|node| node.index()))?;
        w.u8(u8::from(withdrawal.on_error))
    })?;
    w.node_set(&arenas.unmade_makefiles)?;
    w.node_set(&arenas.questioned_makefiles)?;
    w.node_set(&arenas.unread_makefiles)?;
    w.node_set(&arenas.invented_outputs)?;
    let mut forgiven: Vec<_> = arenas.forgiven_order.iter().copied().collect();
    forgiven.sort_unstable();
    w.len(forgiven.len())?;
    for (edge, node) in forgiven {
        w.len(edge.index())?;
        w.len(node.index())?;
    }
    w.table(&arenas.peer_outputs, |w, edge, nodes| {
        w.len(edge.index())?;
        w.ids(nodes.iter().map(|node| node.index()))
    })?;
    w.node_set(&arenas.disposable_outputs)?;
    for named in [
        &arenas.searched_at,
        &arenas.written_as,
        &arenas.double_colon_targets,
    ] {
        w.table(named, |w, node, name| {
            w.len(node.index())?;
            w.bytes(name)
        })?;
    }
    w.table(&arenas.settled_names, |w, edge, settled| {
        w.len(edge.index())?;
        w.bytes(&settled.directory)?;
        w.len(settled.references.len())?;
        for reference in &settled.references {
            w.bytes(&reference.variable)?;
            w.len(reference.node.index())?;
            w.u8(match reference.view {
                SettledView::Whole => 0,
                SettledView::Directory => 1,
                SettledView::Filename => 2,
            })?;
        }
        Ok(())
    })?;
    w.option(arenas.phony_rule.map(RuleId::index), Writer::len)?;
    w.option(arenas.console_pool.map(PoolId::index), Writer::len)
}

struct Writer<'a> {
    out: &'a mut dyn Write,
}

impl Writer<'_> {
    fn raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)
    }

    fn u8(&mut self, value: u8) -> io::Result<()> {
        self.raw(&[value])
    }

    fn u32(&mut self, value: u32) -> io::Result<()> {
        self.raw(&value.to_le_bytes())
    }

    fn len(&mut self, value: usize) -> io::Result<()> {
        self.raw(&(value as u64).to_le_bytes())
    }

    fn bytes(&mut self, value: &[u8]) -> io::Result<()> {
        self.len(value.len())?;
        self.raw(value)
    }

    fn span(&mut self, span: PathSpan) -> io::Result<()> {
        self.u32(span.offset)?;
        self.u32(span.len)
    }

    fn option<T>(
        &mut self,
        value: Option<T>,
        put: impl FnOnce(&mut Self, T) -> io::Result<()>,
    ) -> io::Result<()> {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1)?;
                put(self, value)
            }
        }
    }

    fn ids(&mut self, ids: impl Iterator<Item = usize>) -> io::Result<()> {
        let ids: Vec<usize> = ids.collect();
        self.len(ids.len())?;
        ids.into_iter().try_for_each(|id| self.len(id))
    }

    fn bindings<V>(
        &mut self,
        bindings: &Bindings<V>,
        mut put: impl FnMut(&mut Self, &V) -> io::Result<()>,
    ) -> io::Result<()> {
        self.len(bindings.iter().count())?;
        for (name, value) in bindings.iter() {
            self.len(name.index())?;
            put(self, value)?;
        }
        Ok(())
    }

    fn template(&mut self, template: &EvalString) -> io::Result<()> {
        self.len(template.parts.len())?;
        for part in &template.parts {
            match part {
                EvalPart::Literal(text) => {
                    self.u8(0)?;
                    self.bytes(text)?;
                }
                EvalPart::Variable(name) => {
                    self.u8(1)?;
                    self.len(name.index())?;
                }
            }
        }
        Ok(())
    }

    /// A map written in key order, so one graph is one sequence of bytes.
    fn table<K: Copy + Ord, V>(
        &mut self,
        table: &crate::htab::RapidHashMap<K, V>,
        mut put: impl FnMut(&mut Self, K, &V) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut entries: Vec<(K, &V)> = table.iter().map(|(key, value)| (*key, value)).collect();
        entries.sort_unstable_by_key(|(key, _)| *key);
        self.len(entries.len())?;
        entries
            .into_iter()
            .try_for_each(|(key, value)| put(self, key, value))
    }

    fn node_set(&mut self, set: &crate::htab::RapidHashSet<NodeId>) -> io::Result<()> {
        let mut nodes: Vec<usize> = set.iter().map(|node| node.index()).collect();
        nodes.sort_unstable();
        self.ids(nodes.into_iter())
    }

    fn freshness(&mut self, freshness: &DeferredFreshness) -> io::Result<()> {
        self.ids(freshness.outputs.iter().map(|node| node.index()))?;
        self.u8(u8::from(freshness.always_dirty_output)
            | u8::from(freshness.dates_do_not_decide) << 1
            | u8::from(freshness.heads_the_group) << 2)?;
        self.ids(freshness.always_new_inputs.iter().map(|node| node.index()))?;
        self.ids(
            freshness
                .excluded_new_inputs
                .iter()
                .map(|node| node.index()),
        )?;
        self.len(freshness.new_input_names.len())?;
        for (node, name) in &freshness.new_input_names {
            self.len(node.index())?;
            self.bytes(name)?;
        }
        self.bytes(&freshness.new_inputs_variable)?;
        self.bytes(&freshness.new_inputs_directories_variable)?;
        self.bytes(&freshness.new_inputs_filenames_variable)?;
        self.bytes(&freshness.new_inputs_directory)?;
        self.ids(freshness.activations.iter().map(|node| node.index()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ManifestOptions;

    pub(crate) fn graph_of(manifest: &str) -> BuildGraph {
        let directory = tempfile::tempdir().expect("a scratch directory");
        std::fs::write(directory.path().join("build.ninja"), manifest)
            .expect("the scratch directory is writable");
        crate::parse::load_manifest(directory.path(), "build.ninja", ManifestOptions::default())
            .expect("the manifest describes a graph")
            .graph
    }

    pub(crate) fn written(graph: &BuildGraph) -> Vec<u8> {
        let mut bytes = Vec::new();
        write(graph, &mut bytes).expect("writing to memory cannot fail");
        bytes
    }

    pub(crate) const MANIFEST: &str = "\
pool link
  depth = 2
cflags = -O2
rule cc
  command = cc $cflags -c $in -o $out
  description = CC $out
  depfile = $out.d
  deps = gcc
rule ld
  command = ld $in -o $out
  pool = link
build a.o: cc a.c | gen.h || order.stamp
  cflags = -O0
build b.o: cc b.c |@ check.txt
build prog: ld a.o b.o
build all: phony prog
default all
";

    #[test]
    fn writing_one_graph_twice_gives_one_sequence() {
        let graph = graph_of(MANIFEST);
        assert_eq!(written(&graph), written(&graph));
    }

    #[test]
    fn two_graphs_write_two_sequences() {
        let graph = graph_of(MANIFEST);
        let other = graph_of(&MANIFEST.replace("default all", "default prog"));
        assert_ne!(written(&graph), written(&other));
    }
}
