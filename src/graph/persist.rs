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
#[cfg(test)]
use {
    super::{Edge, Graph, Node},
    crate::env::{EnvState, Environment, Pool, Rule},
    crate::graph::searched::{SettledNameReference, SettledNames},
    crate::graph::withdrawal::Withdrawal,
    crate::util::{BStr, BString, IdVec},
};

const MAGIC: &[u8] = b"ronin-graph\x00";
/// Bumped whenever the bytes change meaning.
const VERSION: u32 = 2;

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
    w.len(arenas.prebuilt.len())?;
    for (edge, rule) in &arenas.prebuilt {
        w.len(edge.index())?;
        w.option(rule.map(RuleId::index), Writer::len)?;
    }
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

/// How many of each arena the file says it holds, against which every
/// identifier in it is checked.
#[cfg(test)]
#[derive(Clone, Copy)]
struct Counts {
    names: usize,
    paths: usize,
    nodes: usize,
    edges: usize,
    environments: usize,
    rules: usize,
    pools: usize,
}

/// The graph `bytes` holds, or `None` for bytes that do not hold one.
#[cfg(test)]
pub(crate) fn read(bytes: &[u8]) -> Option<BuildGraph> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(MAGIC.len())? != MAGIC || r.u32()? != VERSION {
        return None;
    }
    let counts = Counts {
        names: r.len()?,
        paths: r.len()?,
        nodes: r.len()?,
        edges: r.len()?,
        environments: r.len()?,
        rules: r.len()?,
        pools: r.len()?,
    };
    let mut arenas = Graph::default();
    if counts.names < arenas.names.len() {
        return None;
    }
    for index in 0..counts.names {
        let name = BStr::new(r.bytes()?);
        if index < arenas.names.len() {
            if arenas.names.name(VarId::from_index(index)) != name {
                return None;
            }
        } else if arenas.names.intern(name).index() != index {
            return None;
        }
    }
    arenas.paths = r.take(counts.paths)?.to_vec();
    for _ in 0..counts.nodes {
        arenas.nodes.push(Node {
            path: r.span(counts)?,
            shellpath: r.option(|r| r.span(counts))?,
            generator: r.option(|r| r.id(counts.edges).map(EdgeId::from_index))?,
            uses: r
                .ids(counts.edges)?
                .into_iter()
                .map(EdgeId::from_index)
                .collect(),
        });
    }
    for _ in 0..counts.edges {
        let edge = read_edge(&mut r, counts)?;
        arenas.edges.push(edge);
    }
    for _ in 0..counts.environments {
        let parent = r.option(|r| r.id(counts.environments).map(EnvironmentId::from_index))?;
        let bindings = r.bindings(counts, |r| r.bytes().map(BString::from))?;
        let mut rules = std::collections::BTreeMap::new();
        for _ in 0..r.len()? {
            let name = BString::from(r.bytes()?);
            rules.insert(name, RuleId::from_index(r.id(counts.rules)?));
        }
        arenas.environments.push(Environment {
            parent,
            bindings,
            rules,
        });
    }
    for _ in 0..counts.rules {
        let name = BString::from(r.bytes()?);
        let bindings = r.bindings(counts, |r| r.template(counts))?;
        arenas.rules.push(Rule { name, bindings });
    }
    for _ in 0..counts.pools {
        let name = BString::from(r.bytes()?);
        let depth = match r.option(Reader::len)? {
            None => None,
            Some(depth) => Some(std::num::NonZeroUsize::new(depth)?),
        };
        arenas.pools.push(Pool::new(name, depth));
    }
    index_nodes(&mut arenas, r.nodes(counts)?)?;
    read_side_tables(&mut r, counts, &mut arenas)?;

    let root = EnvironmentId::from_index(r.id(counts.environments)?);
    let mut pools = std::collections::BTreeMap::new();
    for _ in 0..r.len()? {
        let name = BString::from(r.bytes()?);
        pools.insert(name, PoolId::from_index(r.id(counts.pools)?));
    }
    let defaults = r.nodes(counts)?.into_iter().collect();
    if r.at != bytes.len() {
        return None;
    }
    Some(BuildGraph {
        arenas,
        state: EnvState::from_parts(root, pools),
        defaults,
        canonical: Vec::new(),
    })
}

/// Enter `indexed` into the path index, refusing a path entered twice.
#[cfg(test)]
fn index_nodes(arenas: &mut Graph, indexed: IdVec<NodeId>) -> Option<()> {
    for node in indexed {
        let span = arenas.nodes[node.index()].path;
        let path = &arenas.paths[span.offset as usize..][..span.len as usize];
        let (found, vacancy) = arenas
            .node_by_path
            .locate(&arenas.paths, &arenas.nodes, path);
        if found.is_some() {
            return None;
        }
        arenas
            .node_by_path
            .fill(&arenas.paths, &arenas.nodes, node, vacancy);
    }
    Some(())
}

#[cfg(test)]
fn read_edge(r: &mut Reader<'_>, counts: Counts) -> Option<Edge> {
    let rule = r.option(|r| r.id(counts.rules).map(RuleId::from_index))?;
    let pool = r.option(|r| r.id(counts.pools).map(PoolId::from_index))?;
    let env = EnvironmentId::from_index(r.id(counts.environments)?);
    let bindings = r.bindings(counts, |r| r.bytes().map(BString::from))?;
    let (out, input, validation) = (r.nodes(counts)?, r.nodes(counts)?, r.nodes(counts)?);
    let dyndep = r.option(|r| r.id(counts.nodes).map(NodeId::from_index))?;
    let flags = r.u8()?;
    let bit = |bit: u8| flags & (1 << bit) != 0;
    let mut edge = Edge {
        rule,
        pool,
        env,
        bindings,
        out,
        input,
        validation,
        dyndep,
        always_dirty: bit(0),
        intermediate: bit(1),
        has_touchable_recipe: bit(2),
        outputs_unaliased: bit(3),
        outputs_low_resolution: bit(4),
        outputs_reobserved: bit(5),
        recipe_begun: bit(6),
        freshness_history: if bit(7) {
            FreshnessHistory::FilesystemOnly
        } else {
            FreshnessHistory::BuildLogAware
        },
        partitions: super::EdgePartitions::default(),
    };
    let (explicit_inputs, non_order_only, explicit_outputs) = (r.len()?, r.len()?, r.len()?);
    if explicit_inputs > non_order_only
        || non_order_only > edge.input.len()
        || explicit_outputs > edge.out.len()
    {
        return None;
    }
    edge.set_input_partitions(explicit_inputs, non_order_only);
    edge.set_explicit_output_count(explicit_outputs);
    Some(edge)
}

#[cfg(test)]
fn read_side_tables(r: &mut Reader<'_>, counts: Counts, arenas: &mut Graph) -> Option<()> {
    for _ in 0..r.len()? {
        let node = NodeId::from_index(r.id(counts.nodes)?);
        let edges = r
            .ids(counts.edges)?
            .into_iter()
            .map(EdgeId::from_index)
            .collect();
        arenas.validation_uses.insert(node, edges);
    }
    arenas.dyndep_edges = r
        .ids(counts.edges)?
        .into_iter()
        .map(EdgeId::from_index)
        .collect();
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let freshness = r.freshness(counts)?;
        arenas.deferred_freshness.insert(edge, freshness);
    }
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let node = NodeId::from_index(r.id(counts.nodes)?);
        arenas.completion_joins.insert(edge, node);
    }
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let outputs = r.nodes(counts)?;
        let on_error = r.bool()?;
        arenas
            .withdrawal
            .insert(edge, Withdrawal { outputs, on_error });
    }
    arenas.unmade_makefiles = r.nodes(counts)?.into_iter().collect();
    arenas.questioned_makefiles = r.nodes(counts)?.into_iter().collect();
    arenas.unread_makefiles = r.nodes(counts)?.into_iter().collect();
    arenas.invented_outputs = r.nodes(counts)?.into_iter().collect();
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let node = NodeId::from_index(r.id(counts.nodes)?);
        arenas.forgiven_order.insert((edge, node));
    }
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let nodes = r.nodes(counts)?;
        arenas.peer_outputs.insert(edge, nodes);
    }
    arenas.disposable_outputs = r.nodes(counts)?.into_iter().collect();
    for named in [
        &mut arenas.searched_at,
        &mut arenas.written_as,
        &mut arenas.double_colon_targets,
    ] {
        for _ in 0..r.len()? {
            let node = NodeId::from_index(r.id(counts.nodes)?);
            named.insert(node, BString::from(r.bytes()?));
        }
    }
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let directory = BString::from(r.bytes()?);
        let mut references = Vec::new();
        for _ in 0..r.len()? {
            references.push(SettledNameReference {
                variable: BString::from(r.bytes()?),
                node: NodeId::from_index(r.id(counts.nodes)?),
                view: match r.u8()? {
                    0 => SettledView::Whole,
                    1 => SettledView::Directory,
                    2 => SettledView::Filename,
                    _ => return None,
                },
            });
        }
        arenas.settled_names.insert(
            edge,
            SettledNames {
                directory,
                references,
            },
        );
    }
    for _ in 0..r.len()? {
        let edge = EdgeId::from_index(r.id(counts.edges)?);
        let rule = r.option(|r| r.id(counts.rules).map(RuleId::from_index))?;
        arenas.prebuilt.push((edge, rule));
    }
    arenas.phony_rule = r.option(|r| r.id(counts.rules).map(RuleId::from_index))?;
    arenas.console_pool = r.option(|r| r.id(counts.pools).map(PoolId::from_index))?;
    Some(())
}

#[cfg(test)]
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

#[cfg(test)]
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let taken = self.bytes.get(self.at..self.at.checked_add(count)?)?;
        self.at += count;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|byte| byte[0])
    }

    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }

    fn len(&mut self) -> Option<usize> {
        let value = u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes"));
        usize::try_from(value).ok()
    }

    /// An arena index the file claims, refused past the arena's end.
    fn id(&mut self, count: usize) -> Option<usize> {
        let id = self.len()?;
        (id < count).then_some(id)
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.len()?;
        self.take(len)
    }

    fn span(&mut self, counts: Counts) -> Option<PathSpan> {
        let (offset, len) = (self.u32()?, self.u32()?);
        let end = (offset as usize).checked_add(len as usize)?;
        (end <= counts.paths).then_some(PathSpan { offset, len })
    }

    #[expect(
        clippy::option_option,
        reason = "the outer `None` is a refused file; the inner is the file's own absent value"
    )]
    fn option<T>(&mut self, get: impl FnOnce(&mut Self) -> Option<T>) -> Option<Option<T>> {
        match self.u8()? {
            0 => Some(None),
            1 => get(self).map(Some),
            _ => None,
        }
    }

    fn ids(&mut self, count: usize) -> Option<Vec<usize>> {
        let len = self.len()?;
        // A length no file this small could hold is refused before anything
        // is reserved for it.
        if len > self.bytes.len() {
            return None;
        }
        (0..len).map(|_| self.id(count)).collect()
    }

    fn nodes(&mut self, counts: Counts) -> Option<IdVec<NodeId>> {
        Some(
            self.ids(counts.nodes)?
                .into_iter()
                .map(NodeId::from_index)
                .collect(),
        )
    }

    fn bindings<V>(
        &mut self,
        counts: Counts,
        mut get: impl FnMut(&mut Self) -> Option<V>,
    ) -> Option<Bindings<V>> {
        let mut bindings = Bindings::default();
        for _ in 0..self.len()? {
            let name = VarId::from_index(self.id(counts.names)?);
            bindings.insert(name, get(self)?);
        }
        Some(bindings)
    }

    fn template(&mut self, counts: Counts) -> Option<EvalString> {
        let mut parts = Vec::new();
        for _ in 0..self.len()? {
            parts.push(match self.u8()? {
                0 => EvalPart::Literal(BString::from(self.bytes()?)),
                1 => EvalPart::Variable(VarId::from_index(self.id(counts.names)?)),
                _ => return None,
            });
        }
        Some(EvalString { parts })
    }

    fn freshness(&mut self, counts: Counts) -> Option<DeferredFreshness> {
        let outputs = self.nodes(counts)?;
        let flags = self.u8()?;
        let always_new_inputs = self.nodes(counts)?;
        let excluded_new_inputs = self.nodes(counts)?;
        let mut new_input_names = Vec::new();
        for _ in 0..self.len()? {
            let node = NodeId::from_index(self.id(counts.nodes)?);
            new_input_names.push((node, BString::from(self.bytes()?)));
        }
        Some(DeferredFreshness {
            outputs,
            always_dirty_output: flags & 1 != 0,
            dates_do_not_decide: flags & 2 != 0,
            heads_the_group: flags & 4 != 0,
            always_new_inputs,
            excluded_new_inputs,
            new_input_names,
            new_inputs_variable: BString::from(self.bytes()?),
            new_inputs_directories_variable: BString::from(self.bytes()?),
            new_inputs_filenames_variable: BString::from(self.bytes()?),
            new_inputs_directory: BString::from(self.bytes()?),
            activations: self.nodes(counts)?,
        })
    }
}

/// Every field of a graph as text, for a test to compare two graphs by.
///
/// Side tables are listed in key order, because the maps holding them do not
/// promise an iteration order and a graph read back was filled in a different
/// order from the one that was written.
#[cfg(test)]
pub(crate) fn describe(graph: &BuildGraph) -> String {
    use std::fmt::Write as _;
    let (arenas, state, defaults) = (graph.arenas(), &graph.state, &graph.defaults);
    let mut text = String::new();
    for index in 0..arenas.names.len() {
        let _ = writeln!(
            text,
            "name {:?}",
            arenas.names.name(VarId::from_index(index))
        );
    }
    for (index, node) in arenas.nodes.iter().enumerate() {
        let id = NodeId::from_index(index);
        let indexed = super::nodeget(arenas, arenas.node_path(id)) == Some(id);
        let path = arenas.node_path(id);
        let _ = writeln!(text, "node {index} {path:?} {node:?} indexed={indexed}");
    }
    for (index, edge) in arenas.edges.iter().enumerate() {
        let _ = writeln!(text, "edge {index} {edge:?}");
    }
    for (index, environment) in arenas.environments.iter().enumerate() {
        let _ = writeln!(text, "env {index} {environment:?}");
    }
    for (index, rule) in arenas.rules.iter().enumerate() {
        let _ = writeln!(text, "rule {index} {rule:?}");
    }
    for (index, pool) in arenas.pools.iter().enumerate() {
        let _ = writeln!(text, "pool {index} {pool:?}");
    }
    describe_side_tables(&mut text, arenas);
    let _ = writeln!(
        text,
        "phony={:?} console={:?} root={:?} pools={:?} defaults={defaults:?}",
        arenas.phony_rule,
        arenas.console_pool,
        state.root,
        state.pools(),
    );
    text
}

#[cfg(test)]
fn describe_side_tables(text: &mut String, arenas: &Graph) {
    use std::fmt::Write as _;
    fn sorted<K: Copy + Ord + std::fmt::Debug, V: std::fmt::Debug>(
        text: &mut String,
        label: &str,
        table: impl IntoIterator<Item = (K, V)>,
    ) {
        let mut entries: Vec<(K, V)> = table.into_iter().collect();
        entries.sort_unstable_by_key(|(key, _)| *key);
        for (key, value) in entries {
            let _ = writeln!(text, "{label} {key:?} {value:?}");
        }
    }
    let by_key =
        |set: &crate::htab::RapidHashSet<NodeId>| set.iter().map(|k| (*k, ())).collect::<Vec<_>>();
    sorted(
        text,
        "validation",
        arenas.validation_uses.iter().map(|(k, v)| (*k, v)),
    );
    let _ = writeln!(text, "dyndep {:?}", arenas.dyndep_edges);
    sorted(
        text,
        "deferred",
        arenas.deferred_freshness.iter().map(|(k, v)| (*k, v)),
    );
    sorted(
        text,
        "join",
        arenas.completion_joins.iter().map(|(k, v)| (*k, v)),
    );
    sorted(
        text,
        "withdrawal",
        arenas.withdrawal.iter().map(|(k, v)| (*k, v)),
    );
    sorted(text, "unmade", by_key(&arenas.unmade_makefiles));
    sorted(text, "questioned", by_key(&arenas.questioned_makefiles));
    sorted(text, "unread", by_key(&arenas.unread_makefiles));
    sorted(text, "invented", by_key(&arenas.invented_outputs));
    sorted(
        text,
        "forgiven",
        arenas.forgiven_order.iter().map(|k| (*k, ())),
    );
    sorted(
        text,
        "peers",
        arenas.peer_outputs.iter().map(|(k, v)| (*k, v)),
    );
    sorted(text, "disposable", by_key(&arenas.disposable_outputs));
    sorted(
        text,
        "searched",
        arenas.searched_at.iter().map(|(k, v)| (*k, v)),
    );
    sorted(
        text,
        "written",
        arenas.written_as.iter().map(|(k, v)| (*k, v)),
    );
    sorted(
        text,
        "double-colon",
        arenas.double_colon_targets.iter().map(|(k, v)| (*k, v)),
    );
    sorted(
        text,
        "settled",
        arenas.settled_names.iter().map(|(k, v)| (*k, v)),
    );
    let _ = writeln!(text, "prebuilt {:?}", arenas.prebuilt);
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

    #[test]
    fn a_graph_read_back_is_the_graph_written() {
        let graph = graph_of(MANIFEST);
        let read_back = read(&written(&graph)).expect("the bytes hold a graph");
        assert_eq!(describe(&read_back), describe(&graph));
    }

    #[test]
    fn every_truncation_is_refused() {
        let bytes = written(&graph_of(MANIFEST));
        for end in 0..bytes.len() {
            assert!(
                read(&bytes[..end]).is_none(),
                "a file cut at byte {end} of {} read as a graph",
                bytes.len()
            );
        }
        let mut longer = bytes;
        longer.push(0);
        assert!(read(&longer).is_none(), "trailing bytes are not a graph");
    }

    #[test]
    fn another_version_is_not_opened() {
        let mut bytes = written(&graph_of(MANIFEST));
        bytes[MAGIC.len()] ^= 1;
        assert!(read(&bytes).is_none());
    }

    #[test]
    fn an_identifier_past_its_arena_is_refused() {
        let mut bytes = written(&graph_of(MANIFEST));
        // The edge count sits fourth among the header's seven counts. With it
        // zero, the first node that names an edge names one past the arena.
        let edges_at = MAGIC.len() + 4 + 3 * 8;
        bytes[edges_at..edges_at + 8].copy_from_slice(&0_u64.to_le_bytes());
        assert!(read(&bytes).is_none());
    }

    #[test]
    fn a_prebuilt_mark_remembers_the_rule_it_replaced() {
        let mut graph = graph_of(MANIFEST);
        let phony = graph.rule(graph.root(), b"phony").expect("the phony rule");
        let prog = graph.lookup(b"prog").expect("prog");
        let settled = graph.mark_subgraphs_prebuilt(&[prog], phony);
        assert_eq!(settled.len(), 3, "prog and the two objects behind it");
        let marked = graph.arenas().prebuilt.clone();
        assert_eq!(marked.len(), 3);
        assert!(marked.iter().all(|(_, rule)| rule.is_some()));
        assert_eq!(
            graph.mark_subgraphs_prebuilt(&[prog], phony).len(),
            3,
            "marking again settles the same nodes"
        );
        assert_eq!(
            graph.arenas().prebuilt.len(),
            3,
            "and remembers nothing new, because they were already phony"
        );
        let read_back = read(&written(&graph)).expect("the bytes hold a graph");
        assert_eq!(describe(&read_back), describe(&graph));
        assert_eq!(read_back.arenas().prebuilt, marked);
    }

    #[test]
    fn an_isolated_node_stays_out_of_the_index() {
        let mut graph = graph_of(MANIFEST);
        let indexed =
            super::super::nodeget(graph.arenas(), crate::util::BStr::new(b"a.c")).expect("a.c");
        let isolated = super::super::allocate_node(graph.arenas_mut(), b"a.c");
        assert_ne!(indexed, isolated);
        let read_back = read(&written(&graph)).expect("the bytes hold a graph");
        assert_eq!(describe(&read_back), describe(&graph));
        assert_eq!(
            super::super::nodeget(read_back.arenas(), crate::util::BStr::new(b"a.c")),
            Some(indexed)
        );
    }
}
