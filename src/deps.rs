//! Dependency-log support translated from `deps.c`.

mod depfile;

use crate::env::edgevar;
use crate::error::{DepfileProblem, PersistenceError, PersistenceOperation};
use crate::graph::{edgeadddeps, EdgeId, Graph, NodeId, PathStyle};
use crate::names::Names;
use crate::runtime::RuntimeState;
use crate::util::{BString, ByteSlice};
use depfile::parse_depfile;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

// [spec:samurai:def:deps.nodearray]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NodeArray {
    pub(crate) nodes: Vec<NodeId>,
}

// [spec:samurai:def:deps.entry]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) node: NodeId,
    pub(crate) deps: NodeArray,
    pub(crate) mtime: i64,
}

pub(crate) struct DepsLog {
    writer: BufWriter<File>,
    entries: EntryMap,
    nodes: Vec<NodeId>,
    node_ids: Vec<DependencyId>,
    next_id: i32,
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
struct DependencyId(i32);

impl DependencyId {
    const UNASSIGNED: Self = Self(-1);

    const fn assigned(raw: i32) -> Self {
        debug_assert!(raw >= 0);
        Self(raw)
    }

    const fn get(self) -> Option<i32> {
        if self.0 < 0 {
            None
        } else {
            Some(self.0)
        }
    }
}

impl DepsLog {
    // [spec:samurai:def:deps.depsinit-fn]
    // [spec:samurai:sem:deps.depsinit-fn]
    #[cfg(test)]
    pub(crate) fn open(builddir: Option<&Path>) -> io::Result<Self> {
        let path = builddir.map_or_else(
            || PathBuf::from(".ninja_deps"),
            |directory| directory.join(".ninja_deps"),
        );
        depsinit_path(path)
    }

    // [spec:samurai:def:deps.depsclose-fn]
    // [spec:samurai:sem:deps.depsclose-fn]
    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.writer.flush()
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn dependency_id(&self, node: NodeId) -> Option<i32> {
        self.node_ids
            .get(node.index())
            .copied()
            .and_then(DependencyId::get)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct EntryMap {
    slots: Vec<Option<Entry>>,
}

impl EntryMap {
    fn get(&self, node: NodeId) -> Option<&Entry> {
        self.slots.get(node.index()).and_then(Option::as_ref)
    }

    fn insert(&mut self, node: NodeId, entry: Entry) -> Option<Entry> {
        if self.slots.len() <= node.index() {
            self.slots.resize_with(node.index() + 1, || None);
        }
        self.slots[node.index()].replace(entry)
    }

    #[cfg(test)]
    fn contains_key(&self, node: NodeId) -> bool {
        self.get(node).is_some()
    }

    fn values(&self) -> impl Iterator<Item = &Entry> {
        self.slots.iter().filter_map(Option::as_ref)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.values().next().is_none()
    }
}

struct IdPlan {
    assigned: HashMap<NodeId, i32>,
    new_nodes: Vec<NodeId>,
    next_id: i32,
    mode: IdAssignmentMode,
}

#[derive(Clone, Copy)]
enum IdAssignmentMode {
    Append,
    Compact,
}

impl IdPlan {
    fn append(log: &DepsLog) -> Self {
        Self {
            assigned: HashMap::new(),
            new_nodes: Vec::new(),
            next_id: log.next_id,
            mode: IdAssignmentMode::Append,
        }
    }

    fn compact() -> Self {
        Self {
            assigned: HashMap::new(),
            new_nodes: Vec::new(),
            next_id: 0,
            mode: IdAssignmentMode::Compact,
        }
    }

    fn id(&self, log: &DepsLog, node: NodeId) -> Option<i32> {
        match self.mode {
            IdAssignmentMode::Append => log
                .dependency_id(node)
                .or_else(|| self.assigned.get(&node).copied()),
            IdAssignmentMode::Compact => self.assigned.get(&node).copied(),
        }
    }
}

// `std::io::Write::write_all` replaces the source's `depswrite` forwarding
// wrapper while this staged encoder retains its record semantics.
// [spec:samurai:def:deps.depswrite-fn]
// [spec:samurai:sem:deps.depswrite-fn]
// [spec:samurai:def:deps.recordid-fn]
// [spec:samurai:sem:deps.recordid-fn]
fn stage_record_id(
    writer: &mut dyn Write,
    plan: &mut IdPlan,
    log: &DepsLog,
    graph: &Graph,
    node: NodeId,
) -> io::Result<bool> {
    const MAX_RECORD_SIZE: usize = (1 << 19) - 1;

    if plan.id(log, node).is_some() {
        return Ok(false);
    }
    let id = plan.next_id;
    let path = graph.node(node).path.as_bytes();
    let padding = (4 - path.len() % 4) % 4;
    let size = path.len() + padding + 4;
    if size > MAX_RECORD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dependency path record is too large",
        ));
    }
    let next_id = id.checked_add(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "dependency log contains too many path records",
        )
    })?;
    let encoded_size = u32::try_from(size).expect("bounded dependency path record");
    let encoded_id = u32::try_from(id).expect("dependency IDs are non-negative");
    writer.write_all(&encoded_size.to_ne_bytes())?;
    writer.write_all(path)?;
    writer.write_all(&[0; 3][..padding])?;
    writer.write_all(&(!encoded_id).to_ne_bytes())?;
    plan.assigned.insert(node, id);
    plan.new_nodes.push(node);
    plan.next_id = next_id;
    Ok(true)
}

// [spec:samurai:def:deps.recorddeps-fn]
// [spec:samurai:sem:deps.recorddeps-fn]
#[derive(Clone, Copy)]
enum EntryPolicy<'a> {
    SkipUnchanged(&'a EntryMap),
    Always,
}

fn stage_deps_entry(
    writer: &mut dyn Write,
    policy: EntryPolicy<'_>,
    plan: &IdPlan,
    log: &DepsLog,
    output: NodeId,
    deps: &NodeArray,
    mtime: i64,
) -> io::Result<Option<Entry>> {
    const MAX_RECORD_SIZE: usize = (1 << 19) - 1;
    if let EntryPolicy::SkipUnchanged(existing_entries) = policy {
        if let Some(existing) = existing_entries.get(output) {
            let unchanged = existing.mtime == mtime && existing.deps.nodes == deps.nodes;
            if unchanged {
                return Ok(None);
            }
        }
    }
    let size = 12 + deps.nodes.len() * 4;
    if size > MAX_RECORD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dependency record is too large",
        ));
    }
    let output_id = plan.id(log, output).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "dependency output has no recorded path ID",
        )
    })?;
    let encoded_size = u32::try_from(size).expect("bounded dependency record");
    writer.write_all(&(encoded_size | 0x8000_0000).to_ne_bytes())?;
    writer.write_all(
        &u32::try_from(output_id)
            .expect("recorded output has a non-negative ID")
            .to_ne_bytes(),
    )?;
    let mtime_bits = u64::from_ne_bytes(mtime.to_ne_bytes());
    writer.write_all(
        &u32::try_from(mtime_bits & u64::from(u32::MAX))
            .expect("masked to 32 bits")
            .to_ne_bytes(),
    )?;
    writer.write_all(
        &u32::try_from(mtime_bits >> 32)
            .expect("shifted to 32 bits")
            .to_ne_bytes(),
    )?;
    for dependency in &deps.nodes {
        let dependency_id = plan.id(log, *dependency).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "dependency input has no recorded path ID",
            )
        })?;
        writer.write_all(
            &u32::try_from(dependency_id)
                .expect("recorded dependency has a non-negative ID")
                .to_ne_bytes(),
        )?;
    }
    Ok(Some(Entry {
        node: output,
        deps: deps.clone(),
        mtime,
    }))
}

fn commit_id_plan(log: &mut DepsLog, plan: IdPlan) {
    for node in plan.new_nodes {
        if log.node_ids.len() <= node.index() {
            log.node_ids
                .resize(node.index() + 1, DependencyId::UNASSIGNED);
        }
        log.node_ids[node.index()] = DependencyId::assigned(plan.assigned[&node]);
        log.nodes.push(node);
    }
    log.next_id = plan.next_id;
}

fn deps_log_with_file(path: PathBuf, file: File) -> DepsLog {
    DepsLog {
        writer: BufWriter::new(file),
        entries: EntryMap::default(),
        nodes: Vec::new(),
        node_ids: Vec::new(),
        next_id: 0,
        path,
    }
}

fn reset_deps_path(path: PathBuf) -> io::Result<DepsLog> {
    let file = crate::persistence::atomic_rewrite(&path, |writer| {
        writer.write_all(b"# ninjadeps\n")?;
        writer.write_all(&4u32.to_ne_bytes())
    })?;
    Ok(deps_log_with_file(path, file))
}

fn depsinit_path(path: PathBuf) -> io::Result<DepsLog> {
    let file = match OpenOptions::new().append(true).read(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return reset_deps_path(path),
        Err(error) => return Err(error),
    };
    Ok(deps_log_with_file(path, file))
}

fn node_from_path(graph: &mut Graph, path: &[u8]) -> NodeId {
    // One allocation that copies, rather than a zero-filled buffer overwritten
    // immediately afterwards.
    crate::graph::mknode(graph, BString::from(path))
}

fn native_u32(bytes: &[u8]) -> u32 {
    u32::from_ne_bytes(bytes.try_into().expect("u32 source is four bytes"))
}

/// Load Ninja's `.ninja_deps` stream and recover its last partial record.
///
/// The returned warning is non-fatal: just like Ninja, the valid prefix stays
/// usable and the invalid suffix is discarded before future records append.
// [spec:samurai:req:compat.persistent-state]
#[allow(
    clippy::too_many_lines,
    reason = "the byte-exact Ninja deps-log decoder is one record-validation state machine"
)]
pub(crate) fn depsloadlog(path: &Path, graph: &mut Graph) -> io::Result<(DepsLog, Option<String>)> {
    const SIGNATURE: &[u8] = b"# ninjadeps\n";
    const HEADER_LEN: usize = SIGNATURE.len() + 4;
    const MAX_RECORD_SIZE: usize = (1 << 19) - 1;

    let content = match fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((depsinit_path(path.to_path_buf())?, None))
        }
        Err(error) => return Err(error),
    };
    let version = content
        .get(SIGNATURE.len()..HEADER_LEN)
        .map(native_u32)
        .unwrap_or_default();
    if content.get(..SIGNATURE.len()) != Some(SIGNATURE) || version != 4 {
        let warning = if version == 1 {
            "deps log version change; rebuilding"
        } else {
            "bad deps log signature or version; starting over"
        };
        return Ok((reset_deps_path(path.to_path_buf())?, Some(warning.into())));
    }

    let mut nodes: Vec<NodeId> = Vec::new();
    let mut seen_nodes = HashSet::new();
    let mut entries = EntryMap::default();
    let mut offset = HEADER_LEN;
    let mut recovery = false;
    while offset < content.len() {
        // Ninja treats a trailing partial size field as EOF. A complete size
        // field followed by a partial record, however, is recoverable damage.
        if content.len() - offset < 4 {
            break;
        }
        let record_offset = offset;
        let encoded_size = native_u32(&content[offset..offset + 4]);
        offset += 4;
        let is_deps = encoded_size & 0x8000_0000 != 0;
        let size = usize::try_from(encoded_size & 0x7fff_ffff).unwrap_or(usize::MAX);
        if size > MAX_RECORD_SIZE || content.len() - offset < size {
            offset = record_offset;
            recovery = true;
            break;
        }
        let record = &content[offset..offset + size];

        let valid = if is_deps {
            if size < 12 || !size.is_multiple_of(4) {
                false
            } else {
                let output_id = usize::try_from(native_u32(&record[..4])).unwrap_or(usize::MAX);
                let dependency_ids = record[12..]
                    .chunks_exact(4)
                    .map(|bytes| usize::try_from(native_u32(bytes)).unwrap_or(usize::MAX));
                if output_id >= nodes.len() || dependency_ids.clone().any(|id| id >= nodes.len()) {
                    false
                } else {
                    let low = u64::from(native_u32(&record[4..8]));
                    let high = u64::from(native_u32(&record[8..12]));
                    let output = nodes[output_id];
                    let deps = NodeArray {
                        nodes: dependency_ids.map(|id| nodes[id]).collect(),
                    };
                    entries.insert(
                        output,
                        Entry {
                            node: output,
                            deps,
                            mtime: i64::from_ne_bytes(((high << 32) | low).to_ne_bytes()),
                        },
                    );
                    true
                }
            }
        } else if size < 5 {
            false
        } else {
            let mut path_size = size - 4;
            for _ in 0..3 {
                if path_size != 0 && record[path_size - 1] == 0 {
                    path_size -= 1;
                }
            }
            if path_size == 0 {
                false
            } else {
                let expected_id =
                    usize::try_from(!native_u32(&record[size - 4..])).unwrap_or(usize::MAX);
                let node = node_from_path(graph, &record[..path_size]);
                match i32::try_from(expected_id) {
                    Ok(_) if expected_id == nodes.len() && seen_nodes.insert(node) => {
                        nodes.push(node);
                        true
                    }
                    _ => false,
                }
            }
        };
        if !valid {
            offset = record_offset;
            recovery = true;
            break;
        }
        offset += size;
    }

    let next_id = i32::try_from(nodes.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "dependency log contains too many path records",
        )
    })?;
    let file = if recovery {
        crate::persistence::atomic_rewrite(path, |writer| writer.write_all(&content[..offset]))?
    } else {
        OpenOptions::new().append(true).read(true).open(path)?
    };
    let mut log = deps_log_with_file(path.to_path_buf(), file);
    log.node_ids
        .resize(graph.node_ids().len(), DependencyId::UNASSIGNED);
    for (id, node) in nodes.iter().copied().enumerate() {
        log.node_ids[node.index()] =
            DependencyId::assigned(i32::try_from(id).expect("validated dependency IDs fit in i32"));
    }
    log.next_id = next_id;
    log.nodes = nodes;
    log.entries = entries;
    let warning = recovery.then(|| "premature end of file; recovering".into());
    Ok((log, warning))
}

fn deps_entry_is_live(graph: &Graph, entry: &Entry) -> bool {
    graph
        .node(entry.node)
        .gen
        .is_some_and(|edge| edgevar(graph, edge, Names::DEPS, PathStyle::Raw).is_some())
}

/// Rewrite the log with only dependency entries that are still reachable from
/// an edge using Ninja's deps attribute.
fn depsrecompact_inner(
    log: &mut DepsLog,
    graph: &Graph,
    #[cfg(test)] fault: Option<crate::persistence::RewriteStage>,
) -> io::Result<()> {
    let mut live_entries = log
        .entries
        .values()
        .filter(|entry| deps_entry_is_live(graph, entry))
        .cloned()
        .collect::<Vec<_>>();
    live_entries.sort_by_key(|entry| log.dependency_id(entry.node));

    log.writer.flush()?;
    let mut plan = IdPlan::compact();
    let mut entries = EntryMap::default();
    let write_contents = |writer: &mut dyn Write| {
        writer.write_all(b"# ninjadeps\n")?;
        writer.write_all(&4u32.to_ne_bytes())?;
        for entry in &live_entries {
            stage_record_id(writer, &mut plan, log, graph, entry.node)?;
            for node in &entry.deps.nodes {
                stage_record_id(writer, &mut plan, log, graph, *node)?;
            }
            if let Some(entry) = stage_deps_entry(
                writer,
                EntryPolicy::Always,
                &plan,
                log,
                entry.node,
                &entry.deps,
                entry.mtime,
            )? {
                entries.insert(entry.node, entry);
            }
        }
        Ok(())
    };
    #[cfg(test)]
    let replacement = if let Some(stage) = fault {
        crate::persistence::atomic_rewrite_with_fault(&log.path, stage, write_contents)?
    } else {
        crate::persistence::atomic_rewrite(&log.path, write_contents)?
    };
    #[cfg(not(test))]
    let replacement = crate::persistence::atomic_rewrite(&log.path, write_contents)?;

    let mut node_ids = vec![DependencyId::UNASSIGNED; graph.node_ids().len()];
    for node in &plan.new_nodes {
        node_ids[node.index()] = DependencyId::assigned(plan.assigned[node]);
    }
    log.writer = BufWriter::new(replacement);
    log.entries = entries;
    log.nodes = plan.new_nodes;
    log.node_ids = node_ids;
    log.next_id = plan.next_id;
    Ok(())
}

pub(crate) fn depsrecompact(log: &mut DepsLog, graph: &Graph) -> io::Result<()> {
    #[cfg(test)]
    {
        depsrecompact_inner(log, graph, None)
    }
    #[cfg(not(test))]
    {
        depsrecompact_inner(log, graph)
    }
}

#[cfg(test)]
fn depsrecompact_with_fault(
    log: &mut DepsLog,
    graph: &Graph,
    stage: crate::persistence::RewriteStage,
) -> io::Result<()> {
    depsrecompact_inner(log, graph, Some(stage))
}

// [spec:samurai:def:deps.depsparse-fn]
// [spec:samurai:sem:deps.depsparse-fn]
pub(crate) fn depsparse(
    graph: &mut Graph,
    path: &Path,
    allow_missing: bool,
) -> Result<NodeArray, PersistenceError> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
            return Ok(NodeArray::default())
        }
        Err(source) => {
            return Err(PersistenceError::io(
                PersistenceOperation::ReadDepfile,
                path,
                source,
            ))
        }
    };
    let mut nodes = Vec::new();
    let display_path = BString::from(path.as_os_str().as_encoded_bytes());
    let parsed = parse_depfile(&text).map_err(|error| match error {
        PersistenceError::Depfile { problem, .. } => {
            PersistenceError::depfile_at(display_path, problem)
        }
        PersistenceError::Io { .. } => unreachable!("depfile parsing performs no I/O"),
    })?;
    for dependency in parsed.inputs {
        nodes.push(crate::graph::mknode(graph, canonical_dep_path(dependency)));
    }
    Ok(NodeArray { nodes })
}

/// Canonicalize a parsed dependency path, consuming its parsed buffer.
///
/// Taking ownership keeps ingestion to the one copy `canonpath` makes
/// internally, and returning a `BString` lets `mknode` adopt the result
/// without a further copy.
fn canonical_dep_path(path: Vec<u8>) -> BString {
    let mut canonical = BString::from(path);
    crate::util::canonpath(&mut canonical);
    canonical
}

pub(crate) fn depsparse_for_edge(
    graph: &mut Graph,
    path: &Path,
    edge: EdgeId,
) -> Result<Option<NodeArray>, PersistenceError> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(PersistenceError::io(
                PersistenceOperation::ReadDepfile,
                path,
                source,
            ))
        }
    };
    if text.is_empty() {
        return Ok(None);
    }
    let display_path = BString::from(path.as_os_str().as_encoded_bytes());
    let parsed = parse_depfile(&text).map_err(|error| match error {
        PersistenceError::Depfile { problem, .. } => {
            PersistenceError::depfile_at(display_path.clone(), problem)
        }
        PersistenceError::Io { .. } => unreachable!("depfile parsing performs no I/O"),
    })?;
    if parsed.outputs.is_empty() {
        return Err(PersistenceError::depfile_at(
            display_path,
            DepfileProblem::NoOutputs,
        ));
    }
    let outputs = graph.edge(edge).out.clone();
    let mut parsed_outputs = parsed.outputs.into_iter().map(canonical_dep_path);
    let first = parsed_outputs.next().expect("outputs were checked above");
    let matches_first = outputs
        .first()
        .is_some_and(|output| graph.node(*output).path == first);
    if !matches_first {
        return Ok(None);
    }
    for output in std::iter::once(first).chain(parsed_outputs) {
        if !outputs
            .iter()
            .any(|expected| graph.node(*expected).path == output)
        {
            return Err(PersistenceError::depfile_at(
                display_path,
                DepfileProblem::UndeclaredOutput(output),
            ));
        }
    }
    let mut nodes = Vec::new();
    for dependency in parsed.inputs {
        nodes.push(crate::graph::mknode(graph, canonical_dep_path(dependency)));
    }
    Ok(Some(NodeArray { nodes }))
}

// [spec:samurai:def:deps.depsload-fn]
// [spec:samurai:sem:deps.depsload-fn]
pub(crate) fn depsload(graph: &mut Graph, edge: EdgeId, log: &DepsLog) {
    let output = graph.edge(edge).out.first().copied();
    let Some(output) = output else { return };
    if let Some(entry) = log.entries.get(output) {
        edgeadddeps(graph, edge, &entry.deps.nodes);
    }
}

pub(crate) fn depsentry(log: &DepsLog, output: NodeId) -> Option<&Entry> {
    log.entries.get(output)
}

pub(crate) fn depsnodes(log: &DepsLog) -> impl Iterator<Item = NodeId> + '_ {
    log.nodes
        .iter()
        .copied()
        .filter(|node| log.entries.get(*node).is_some())
}

pub(crate) fn visit_dependencies(log: &DepsLog, mut visit: impl FnMut(NodeId, NodeId)) {
    for entry in log.entries.values() {
        for dependency in &entry.deps.nodes {
            visit(entry.node, *dependency);
        }
    }
}

// [spec:samurai:def:deps.depsrecord-fn]
// [spec:samurai:sem:deps.depsrecord-fn]
pub(crate) fn depsrecord(
    log: &mut DepsLog,
    edge: EdgeId,
    graph: &mut Graph,
    runtime: &RuntimeState,
    disk: &crate::os::RealDiskInterface,
) -> Result<(), PersistenceError> {
    let Some(depfile) = edgevar(graph, edge, Names::DEPFILE, PathStyle::Raw) else {
        return Ok(());
    };
    let deps = depsparse(
        graph,
        &disk.resolve(depfile.to_path().expect("byte paths are valid on Unix")),
        true,
    )?;
    depsrecordnodes(log, graph, runtime, edge, &deps.nodes)
}

fn record_nodes(
    log: &mut DepsLog,
    graph: &Graph,
    outputs: &[NodeId],
    deps: &[NodeId],
    mtimes: &[i64],
) -> io::Result<()> {
    debug_assert_eq!(outputs.len(), mtimes.len());
    let deps = NodeArray {
        nodes: deps.to_vec(),
    };
    let mut plan = IdPlan::append(log);
    let mut encoded = Vec::new();
    for dependency in &deps.nodes {
        stage_record_id(&mut encoded, &mut plan, log, graph, *dependency)?;
    }
    let mut entries = Vec::new();
    for (output, mtime) in outputs.iter().copied().zip(mtimes.iter().copied()) {
        stage_record_id(&mut encoded, &mut plan, log, graph, output)?;
        if let Some(entry) = stage_deps_entry(
            &mut encoded,
            EntryPolicy::SkipUnchanged(&log.entries),
            &plan,
            log,
            output,
            &deps,
            mtime,
        )? {
            entries.push(entry);
        }
    }
    log.writer.write_all(&encoded)?;
    log.writer.flush()?;
    commit_id_plan(log, plan);
    for entry in entries {
        log.entries.insert(entry.node, entry);
    }
    Ok(())
}

pub(crate) fn depsrecordnodes(
    log: &mut DepsLog,
    graph: &Graph,
    runtime: &RuntimeState,
    edge: EdgeId,
    deps: &[NodeId],
) -> Result<(), PersistenceError> {
    let outputs = graph.edge(edge).out.clone();
    let mtimes = outputs
        .iter()
        .map(|output| runtime.node(*output).mtime().raw())
        .collect::<Vec<_>>();
    record_nodes(log, graph, &outputs, deps, &mtimes).map_err(|source| {
        PersistenceError::io(
            PersistenceOperation::RecordDepsLog,
            log.path.clone(),
            source,
        )
    })
}

#[cfg(test)]
mod ninja_depfile_tests {
    use super::*;
    use crate::graph::nodeget;

    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_LOG: AtomicUsize = AtomicUsize::new(0);

    fn test_log_path(name: &str) -> (PathBuf, PathBuf) {
        for _ in 0..1024 {
            let sequence = NEXT_TEST_LOG.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "ronin-ninja-deps-{}-{name}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => return (directory.clone(), directory.join(".ninja_deps")),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("could not create test log directory: {error}"),
            }
        }
        panic!("could not allocate a unique dependency log test directory")
    }

    fn remove_test_log(directory: PathBuf) {
        let _ = fs::remove_dir_all(directory);
    }

    fn make_node(graph: &mut Graph, path: &str) -> NodeId {
        node_from_path(graph, path.as_bytes())
    }

    fn record(log: &mut DepsLog, graph: &Graph, output: NodeId, paths: &[NodeId], mtime: i64) {
        record_nodes(log, graph, &[output], paths, &[mtime]).unwrap();
    }

    fn paths(graph: &Graph, nodes: &[NodeId]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| String::from_utf8(graph.node(*node).path.as_bytes().to_vec()).unwrap())
            .collect()
    }

    fn make_deps_edge(graph: &mut Graph, output: NodeId) {
        let state = crate::env::EnvState::new(graph);
        let rule = crate::env::mkrule(graph, "cc".into());
        crate::env::ruleaddvar(
            graph,
            rule,
            Names::DEPS,
            crate::util::EvalString::literal("gcc"),
        );
        let edge = crate::graph::mkedge(graph, state.root);
        graph.edge_mut(edge).rule = Some(rule);
        graph.edge_mut(edge).out.push(output);
        graph.node_mut(output).gen = Some(edge);
    }

    // [spec:samurai:req:compat.persistent-state/test]
    #[test]
    fn ninja_deps_log_write_read() {
        let (directory, path) = test_log_path("write-read");
        let mut source = crate::graph::Graph::default();
        let output = make_node(&mut source, "out.o");
        let output2 = make_node(&mut source, "out2.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let bar2 = make_node(&mut source, "bar2.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &source, output, &[foo, bar], 1);
        record(&mut log, &source, output2, &[foo, bar2], 2);
        log.finish().unwrap();

        let mut loaded_graph = crate::graph::Graph::default();
        let (loaded, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(warning, None);
        assert_eq!(loaded.nodes.len(), 5);
        let output2 = make_node(&mut loaded_graph, "out2.o");
        let entry = loaded.entries.get(output2).unwrap();
        assert_eq!(entry.mtime, 2);
        assert_eq!(paths(&loaded_graph, &entry.deps.nodes), ["foo.h", "bar2.h"]);
        drop(loaded);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_lots_of_dependencies() {
        let (directory, path) = test_log_path("many");
        let mut source = crate::graph::Graph::default();
        let output = make_node(&mut source, "out.o");
        let dependencies = (0..100_000)
            .map(|index| make_node(&mut source, &format!("file{index}.h")))
            .collect::<Vec<_>>();
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &source, output, &dependencies, 1);
        log.finish().unwrap();

        let mut loaded_graph = crate::graph::Graph::default();
        let (loaded, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(warning, None);
        let output = make_node(&mut loaded_graph, "out.o");
        assert_eq!(
            loaded.entries.get(output).unwrap().deps.nodes.len(),
            100_000
        );
        drop(loaded);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_avoids_duplicate_entries() {
        let (directory, path) = test_log_path("duplicate");
        let mut graph = crate::graph::Graph::default();
        let output = make_node(&mut graph, "out.o");
        let foo = make_node(&mut graph, "foo.h");
        let bar = make_node(&mut graph, "bar.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &graph, output, &[foo, bar], 1);
        log.finish().unwrap();
        let original_size = fs::metadata(&path).unwrap().len();

        let mut reloaded_graph = crate::graph::Graph::default();
        let (mut log, warning) = depsloadlog(&path, &mut reloaded_graph).unwrap();
        assert_eq!(warning, None);
        let output = make_node(&mut reloaded_graph, "out.o");
        let foo = make_node(&mut reloaded_graph, "foo.h");
        let bar = make_node(&mut reloaded_graph, "bar.h");
        record(&mut log, &reloaded_graph, output, &[foo, bar], 1);
        log.finish().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), original_size);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_recompacts_live_entries() {
        let (directory, path) = test_log_path("recompact");
        let mut source = crate::graph::Graph::default();
        let output = make_node(&mut source, "out.o");
        let other_output = make_node(&mut source, "other_out.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let baz = make_node(&mut source, "baz.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &source, output, &[foo, bar], 1);
        record(&mut log, &source, other_output, &[foo, baz], 1);
        log.finish().unwrap();

        let mut graph = crate::graph::Graph::default();
        let (mut log, warning) = depsloadlog(&path, &mut graph).unwrap();
        assert_eq!(warning, None);
        let output = make_node(&mut graph, "out.o");
        let other_output = make_node(&mut graph, "other_out.o");
        let foo = make_node(&mut graph, "foo.h");
        make_deps_edge(&mut graph, output);
        make_deps_edge(&mut graph, other_output);
        record(&mut log, &graph, output, &[foo], 1);
        let grown_size = fs::metadata(&path).unwrap().len();
        depsrecompact(&mut log, &graph).unwrap();
        assert_eq!(
            paths(&graph, &log.entries.get(output).unwrap().deps.nodes),
            ["foo.h"]
        );
        assert_eq!(
            paths(&graph, &log.entries.get(other_output).unwrap().deps.nodes),
            ["foo.h", "baz.h"]
        );
        assert!(fs::metadata(&path).unwrap().len() < grown_size);
        for entry in log.entries.values() {
            let id = usize::try_from(log.dependency_id(entry.node).unwrap()).unwrap();
            assert_eq!(
                log.dependency_id(log.nodes[id]),
                log.dependency_id(entry.node)
            );
        }

        let mut dead_graph = crate::graph::Graph::default();
        let (mut dead_log, warning) = depsloadlog(&path, &mut dead_graph).unwrap();
        assert_eq!(warning, None);
        let foo = make_node(&mut dead_graph, "foo.h");
        depsrecompact(&mut dead_log, &dead_graph).unwrap();
        assert!(dead_log.entries.is_empty());
        assert_eq!(dead_log.dependency_id(foo), None);
        drop(dead_log);
        drop(log);
        remove_test_log(directory);
    }

    // [spec:samurai:req:runtime.persistence-transactions/test]
    #[test]
    fn deps_log_rewrite_failures_preserve_ids_state_and_writer() {
        for stage in crate::persistence::RewriteStage::ALL {
            let (directory, path) = test_log_path("transaction-failure");
            let mut source = crate::graph::Graph::default();
            let output = make_node(&mut source, "out.o");
            let input = make_node(&mut source, "input.h");
            let mut source_log = depsinit_path(path.clone()).unwrap();
            record(&mut source_log, &source, output, &[input], 1);
            source_log.finish().unwrap();

            let mut graph = crate::graph::Graph::default();
            let (mut log, warning) = depsloadlog(&path, &mut graph).unwrap();
            assert_eq!(warning, None);
            let output = make_node(&mut graph, "out.o");
            let input = make_node(&mut graph, "input.h");
            make_deps_edge(&mut graph, output);
            let original_file = fs::read(&path).unwrap();
            let original_entries = log.entries.clone();
            let original_nodes = log.nodes.clone();
            let original_next_id = log.next_id;
            let original_ids = original_nodes
                .iter()
                .map(|node| (*node, log.dependency_id(*node)))
                .collect::<Vec<_>>();

            let error = depsrecompact_with_fault(&mut log, &graph, stage).unwrap_err();
            assert!(error
                .to_string()
                .contains("injected atomic rewrite failure"));
            assert_eq!(fs::read(&path).unwrap(), original_file);
            assert_eq!(log.entries, original_entries);
            assert_eq!(log.nodes, original_nodes);
            assert_eq!(log.next_id, original_next_id);
            for (node, id) in original_ids {
                assert_eq!(log.dependency_id(node), id);
            }

            record(&mut log, &graph, output, &[input], 2);
            log.finish().unwrap();
            let mut reloaded_graph = crate::graph::Graph::default();
            let (reloaded, warning) = depsloadlog(&path, &mut reloaded_graph).unwrap();
            assert_eq!(warning, None);
            let output = make_node(&mut reloaded_graph, "out.o");
            assert_eq!(reloaded.entries.get(output).unwrap().mtime, 2);
            drop(reloaded);
            remove_test_log(directory);
        }
    }

    #[test]
    fn ninja_deps_log_restarts_for_invalid_headers() {
        let (directory, path) = test_log_path("invalid-header");
        for content in [
            Vec::new(),
            b"# ninjad".to_vec(),
            b"# ninjadeps\n".to_vec(),
            b"# ninjadeps\n\x01\x02".to_vec(),
            b"# ninjadeps\n\x01\x02\x03\x04".to_vec(),
        ] {
            fs::write(&path, content).unwrap();
            let mut graph = crate::graph::Graph::default();
            let (log, warning) = depsloadlog(&path, &mut graph).unwrap();
            assert_eq!(
                warning.as_deref(),
                Some("bad deps log signature or version; starting over")
            );
            drop(log);
        }
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_recovers_truncated_records() {
        let (directory, path) = test_log_path("truncated");
        let mut source = crate::graph::Graph::default();
        let output = make_node(&mut source, "out.o");
        let output2 = make_node(&mut source, "out2.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let bar2 = make_node(&mut source, "bar2.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &source, output, &[foo, bar], 1);
        record(&mut log, &source, output2, &[foo, bar2], 2);
        log.finish().unwrap();
        let original_size = fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(original_size - 2)
            .unwrap();

        let mut graph = crate::graph::Graph::default();
        let (log, warning) = depsloadlog(&path, &mut graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        let out = nodeget(&graph, b"out.o").unwrap();
        let out2 = nodeget(&graph, b"out2.o").unwrap();
        assert!(log.entries.contains_key(out));
        assert!(!log.entries.contains_key(out2));
        drop(log);
        let mut reloaded_graph = crate::graph::Graph::default();
        let (_log, warning) = depsloadlog(&path, &mut reloaded_graph).unwrap();
        assert_eq!(warning, None);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_appends_after_truncation_recovery() {
        let (directory, path) = test_log_path("truncated-append");
        let mut source = crate::graph::Graph::default();
        let output = make_node(&mut source, "out.o");
        let output2 = make_node(&mut source, "out2.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let bar2 = make_node(&mut source, "bar2.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &source, output, &[foo, bar], 1);
        record(&mut log, &source, output2, &[foo, bar2], 2);
        log.finish().unwrap();

        let original_size = fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(original_size - 2)
            .unwrap();

        let mut recovered_graph = crate::graph::Graph::default();
        let (mut recovered, warning) = depsloadlog(&path, &mut recovered_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        let output2 = make_node(&mut recovered_graph, "out2.o");
        let foo = make_node(&mut recovered_graph, "foo.h");
        let bar2 = make_node(&mut recovered_graph, "bar2.h");
        record(&mut recovered, &recovered_graph, output2, &[foo, bar2], 3);
        recovered.finish().unwrap();

        let mut final_graph = crate::graph::Graph::default();
        let (final_log, warning) = depsloadlog(&path, &mut final_graph).unwrap();
        assert_eq!(warning, None);
        let output2 = make_node(&mut final_graph, "out2.o");
        let entry = final_log.entries.get(output2).unwrap();
        assert_eq!(entry.mtime, 3);
        assert_eq!(paths(&final_graph, &entry.deps.nodes), ["foo.h", "bar2.h"]);
        drop(final_log);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_finds_reverse_dependencies() {
        let (directory, path) = test_log_path("reverse");
        let mut graph = crate::graph::Graph::default();
        let output = make_node(&mut graph, "out.o");
        let output2 = make_node(&mut graph, "out2.o");
        let foo = make_node(&mut graph, "foo.h");
        let bar = make_node(&mut graph, "bar.h");
        let bar2 = make_node(&mut graph, "bar2.h");
        let mut log = depsinit_path(path).unwrap();
        record(&mut log, &graph, output, &[foo, bar], 1);
        record(&mut log, &graph, output2, &[foo, bar2], 2);
        let reverse = log
            .entries
            .values()
            .find(|entry| entry.deps.nodes.contains(&foo))
            .unwrap();
        assert!(reverse.node == output || reverse.node == output2);
        let reverse = log
            .entries
            .values()
            .find(|entry| entry.deps.nodes.contains(&bar))
            .unwrap();
        assert_eq!(reverse.node, output);
        log.finish().unwrap();
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_recovers_malformed_records() {
        let (directory, path) = test_log_path("malformed");
        let mut graph = crate::graph::Graph::default();
        let output = make_node(&mut graph, "out.o");
        let foo = make_node(&mut graph, "foo.hh");
        let bar = make_node(&mut graph, "bar.hpp");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &graph, output, &[foo, bar], 1);
        log.finish().unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(&original[..12], b"# ninjadeps\n");

        let first_record = 16;
        let mut bad = original.clone();
        bad[first_record..first_record + 4].copy_from_slice(&0x7fff_aa55u32.to_ne_bytes());
        fs::write(&path, bad).unwrap();
        let mut loaded_graph = crate::graph::Graph::default();
        let (_log, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );

        fs::write(&path, &original[..=first_record + 4]).unwrap();
        let mut loaded_graph = crate::graph::Graph::default();
        let (_log, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_removes_duplicate_path_record_during_recovery() {
        let (directory, path) = test_log_path("duplicate-path");
        let mut graph = crate::graph::Graph::default();
        let output = make_node(&mut graph, "out.o");
        let foo = make_node(&mut graph, "foo.h");
        let bar = make_node(&mut graph, "bar.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &graph, output, &[foo, bar], 1);
        log.finish().unwrap();

        let mut duplicate = Vec::new();
        duplicate.extend_from_slice(&12u32.to_ne_bytes());
        duplicate.extend_from_slice(b"foo.h\0\0\0");
        duplicate.extend_from_slice(&(!1u32).to_ne_bytes());
        let mut content = fs::read(&path).unwrap();
        content.extend_from_slice(&duplicate);
        fs::write(&path, content).unwrap();

        let mut first_graph = crate::graph::Graph::default();
        let (first, warning) = depsloadlog(&path, &mut first_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        let out = nodeget(&first_graph, b"out.o").unwrap();
        assert!(first.entries.contains_key(out));
        drop(first);
        let mut second_graph = crate::graph::Graph::default();
        let (second, warning) = depsloadlog(&path, &mut second_graph).unwrap();
        assert_eq!(warning, None);
        let out = nodeget(&second_graph, b"out.o").unwrap();
        assert!(second.entries.contains_key(out));
        drop(second);
        remove_test_log(directory);
    }
}
