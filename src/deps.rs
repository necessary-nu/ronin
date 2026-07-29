//! Dependency-log support translated from `deps.c`.

use crate::env::edgevar;
use crate::graph::{edgeadddeps, EdgeId, Graph, NodeId};
use crate::util::ByteSlice;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::mem;
use std::path::{Path, PathBuf};

// [spec:samurai:def:deps.nodearray]
#[derive(Clone, Default)]
pub struct NodeArray {
    pub nodes: Vec<NodeId>,
}

// [spec:samurai:def:deps.entry]
#[derive(Clone)]
pub struct Entry {
    pub node: NodeId,
    pub deps: NodeArray,
    pub mtime: i64,
}

pub struct DepsLog {
    writer: BufWriter<File>,
    entries: EntryMap,
    nodes: Vec<NodeId>,
    next_id: i32,
    path: PathBuf,
}

#[derive(Default)]
struct EntryMap {
    slots: Vec<Option<Entry>>,
}

impl EntryMap {
    fn get(&self, node: &NodeId) -> Option<&Entry> {
        self.slots.get(node.index()).and_then(Option::as_ref)
    }

    fn insert(&mut self, node: NodeId, entry: Entry) -> Option<Entry> {
        if self.slots.len() <= node.index() {
            self.slots.resize_with(node.index() + 1, || None);
        }
        self.slots[node.index()].replace(entry)
    }

    #[cfg(test)]
    fn contains_key(&self, node: &NodeId) -> bool {
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

#[derive(Debug, Eq, PartialEq)]
struct ParsedDepfile {
    outputs: Vec<Vec<u8>>,
    inputs: Vec<Vec<u8>>,
}

type DepfileRule = (Vec<Vec<u8>>, Vec<Vec<u8>>);

#[derive(Default)]
struct OrderedPaths {
    indices: HashMap<Vec<u8>, usize>,
}

impl OrderedPaths {
    fn insert(&mut self, path: Vec<u8>) {
        let next = self.indices.len();
        self.indices.entry(path).or_insert(next);
    }

    fn contains(&self, path: &[u8]) -> bool {
        self.indices.contains_key(path)
    }

    fn into_vec(self) -> Vec<Vec<u8>> {
        let mut paths = vec![Vec::new(); self.indices.len()];
        for (path, index) in self.indices {
            paths[index] = path;
        }
        paths
    }
}

fn parse_depfile_rule(line: &[u8]) -> Result<Option<DepfileRule>, String> {
    let mut outputs = OrderedPaths::default();
    let mut inputs = OrderedPaths::default();
    let mut token = Vec::new();
    let mut inputs_started = false;
    let mut index = 0;
    let mut saw_non_whitespace = false;

    let mut finish_token = |token: &mut Vec<u8>, inputs_started: bool| {
        if token.is_empty() {
            return;
        }
        let path = std::mem::take(token);
        if inputs_started {
            inputs.insert(path);
        } else {
            outputs.insert(path);
        }
    };

    while index < line.len() {
        match line[index] {
            b' ' | b'\t' | b'\r' => {
                finish_token(&mut token, inputs_started);
                index += 1;
            }
            b'$' => {
                saw_non_whitespace = true;
                if line.get(index + 1) != Some(&b'$') {
                    return Err("depfile contains a variable reference".into());
                }
                token.push(b'$');
                index += 2;
            }
            b'\\' => {
                saw_non_whitespace = true;
                let start = index;
                while index < line.len() && line[index] == b'\\' {
                    index += 1;
                }
                let slashes = index - start;
                match line.get(index).copied() {
                    Some(b' ' | b'\t') if slashes % 2 == 1 => {
                        token.extend(std::iter::repeat_n(b'\\', slashes / 2));
                        token.push(line[index]);
                        index += 1;
                    }
                    Some(b'#') => {
                        token.extend(std::iter::repeat_n(b'\\', slashes / 2));
                        if slashes % 2 == 1 {
                            token.push(b'#');
                            index += 1;
                        }
                    }
                    _ => token.extend(std::iter::repeat_n(b'\\', slashes)),
                }
            }
            b':' if !inputs_started => {
                saw_non_whitespace = true;
                if token.len() == 1
                    && token[0].is_ascii_alphabetic()
                    && matches!(line.get(index + 1), Some(b'/' | b'\\'))
                {
                    token.push(b':');
                    index += 1;
                    continue;
                }
                if token.ends_with(b"\\") && token.len() == 2 && line.get(index + 1) == Some(&b'\\')
                {
                    token.pop();
                    token.push(b':');
                    index += 1;
                    continue;
                }
                finish_token(&mut token, false);
                inputs_started = true;
                index += 1;
            }
            character => {
                saw_non_whitespace = true;
                token.push(character);
                index += 1;
            }
        }
    }
    finish_token(&mut token, inputs_started);
    if !saw_non_whitespace {
        return Ok(None);
    }
    if !inputs_started {
        return Err("expected ':' in depfile".into());
    }
    Ok(Some((outputs.into_vec(), inputs.into_vec())))
}

fn merge_depfile_rule(
    line: &[u8],
    outputs: &mut OrderedPaths,
    inputs: &mut OrderedPaths,
) -> Result<(), String> {
    let Some((rule_outputs, rule_inputs)) = parse_depfile_rule(line)? else {
        return Ok(());
    };
    let output_is_input = rule_outputs.iter().any(|output| inputs.contains(output));
    if output_is_input && !rule_inputs.is_empty() {
        return Err("inputs may not also have inputs".into());
    }
    if !output_is_input {
        for output in rule_outputs {
            outputs.insert(output);
        }
        for input in rule_inputs {
            inputs.insert(input);
        }
    }
    Ok(())
}

fn parse_depfile(text: &[u8]) -> Result<ParsedDepfile, String> {
    let mut outputs = OrderedPaths::default();
    let mut inputs = OrderedPaths::default();
    let mut line = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if text[index] == b'\\' {
            let start = index;
            while index < text.len() && text[index] == b'\\' {
                index += 1;
            }
            let slashes = index - start;
            let newline = match text.get(index..) {
                Some([b'\r', b'\n', ..]) => Some(2),
                Some([b'\n', ..]) => Some(1),
                _ => None,
            };
            if let Some(newline) = newline.filter(|_| slashes % 2 == 1) {
                line.extend(std::iter::repeat_n(b'\\', slashes / 2));
                line.push(b' ');
                index += newline;
            } else {
                line.extend(std::iter::repeat_n(b'\\', slashes));
            }
            continue;
        }
        if text[index] == b'\n' {
            merge_depfile_rule(&line, &mut outputs, &mut inputs)?;
            line.clear();
            index += 1;
            continue;
        }
        line.push(text[index]);
        index += 1;
    }
    merge_depfile_rule(&line, &mut outputs, &mut inputs)?;
    Ok(ParsedDepfile {
        outputs: outputs.into_vec(),
        inputs: inputs.into_vec(),
    })
}

// [spec:samurai:def:deps.depswrite-fn]
// [spec:samurai:sem:deps.depswrite-fn]
fn depswrite(writer: &mut BufWriter<File>, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)
}

// [spec:samurai:def:deps.recordid-fn]
// [spec:samurai:sem:deps.recordid-fn]
fn recordid(log: &mut DepsLog, graph: &mut Graph, node: NodeId) -> io::Result<bool> {
    if graph.node(node).id != -1 {
        return Ok(false);
    }
    const MAX_RECORD_SIZE: usize = (1 << 19) - 1;
    let id = log.next_id;
    let path = graph.node(node).path.as_bytes().to_vec();
    let padding = (4 - path.len() % 4) % 4;
    let size = path.len() + padding + 4;
    if size > MAX_RECORD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dependency path record is too large",
        ));
    }
    log.next_id += 1;
    graph.node_mut(node).id = id;
    depswrite(&mut log.writer, &(size as u32).to_ne_bytes())?;
    depswrite(&mut log.writer, &path)?;
    depswrite(&mut log.writer, &[0; 3][..padding])?;
    depswrite(&mut log.writer, &(!(id as u32)).to_ne_bytes())?;
    log.nodes.push(node);
    Ok(true)
}

// [spec:samurai:def:deps.recorddeps-fn]
// [spec:samurai:sem:deps.recorddeps-fn]
fn recorddeps(
    log: &mut DepsLog,
    graph: &Graph,
    output: NodeId,
    deps: &NodeArray,
    mtime: i64,
) -> io::Result<()> {
    const MAX_RECORD_SIZE: usize = (1 << 19) - 1;
    if let Some(existing) = log.entries.get(&output) {
        let unchanged = existing.mtime == mtime
            && existing.deps.nodes.len() == deps.nodes.len()
            && existing
                .deps
                .nodes
                .iter()
                .zip(&deps.nodes)
                .all(|(left, right)| left == right);
        if unchanged {
            return Ok(());
        }
    }
    let size = 12 + deps.nodes.len() * 4;
    if size > MAX_RECORD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dependency record is too large",
        ));
    }
    depswrite(
        &mut log.writer,
        &((size as u32) | 0x8000_0000).to_ne_bytes(),
    )?;
    depswrite(
        &mut log.writer,
        &(graph.node(output).id as u32).to_ne_bytes(),
    )?;
    depswrite(&mut log.writer, &(mtime as u64 as u32).to_ne_bytes())?;
    depswrite(
        &mut log.writer,
        &((mtime as u64 >> 32) as u32).to_ne_bytes(),
    )?;
    for dependency in &deps.nodes {
        depswrite(
            &mut log.writer,
            &(graph.node(*dependency).id as u32).to_ne_bytes(),
        )?;
    }
    log.entries.insert(
        output,
        Entry {
            node: output,
            deps: deps.clone(),
            mtime,
        },
    );
    Ok(())
}

fn depsinit_path(path: PathBuf) -> io::Result<DepsLog> {
    let new = !path.exists();
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)?;
    let mut log = DepsLog {
        writer: BufWriter::new(file),
        entries: EntryMap::default(),
        nodes: Vec::new(),
        next_id: 0,
        path,
    };
    if new {
        depswrite(&mut log.writer, b"# ninjadeps\n")?;
        depswrite(&mut log.writer, &4u32.to_ne_bytes())?;
    }
    Ok(log)
}

// [spec:samurai:def:deps.depsinit-fn]
// [spec:samurai:sem:deps.depsinit-fn]
pub fn depsinit(builddir: Option<&Path>) -> io::Result<DepsLog> {
    let path = builddir.map_or_else(
        || PathBuf::from(".ninja_deps"),
        |directory| directory.join(".ninja_deps"),
    );
    depsinit_path(path)
}

// [spec:samurai:def:deps.depsclose-fn]
// [spec:samurai:sem:deps.depsclose-fn]
pub fn depsclose(mut log: DepsLog) -> io::Result<()> {
    log.writer.flush()
}

fn node_from_path(graph: &mut Graph, path: &[u8]) -> NodeId {
    let mut value = crate::util::mkstr(path.len());
    value.copy_from_slice(path);
    crate::graph::mknode(graph, value)
}

fn native_u32(bytes: &[u8]) -> u32 {
    u32::from_ne_bytes(bytes.try_into().expect("u32 source is four bytes"))
}

/// Load Ninja's .ninja_deps stream and recover its last partial record.
///
/// The returned warning is non-fatal: just like Ninja, the valid prefix stays
/// usable and the invalid suffix is discarded before future records append.
// [spec:samurai:req:compat.persistent-state]
pub fn depsloadlog(path: &Path, graph: &mut Graph) -> io::Result<(DepsLog, Option<String>)> {
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
        fs::remove_file(path)?;
        let warning = if version == 1 {
            "deps log version change; rebuilding"
        } else {
            "bad deps log signature or version; starting over"
        };
        return Ok((depsinit_path(path.to_path_buf())?, Some(warning.into())));
    }

    let mut nodes: Vec<NodeId> = Vec::new();
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
        let size = (encoded_size & 0x7fff_ffff) as usize;
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
                let output_id = native_u32(&record[..4]) as usize;
                let dependency_ids = record[12..]
                    .chunks_exact(4)
                    .map(|bytes| native_u32(bytes) as usize);
                if output_id >= nodes.len() || dependency_ids.clone().any(|id| id >= nodes.len()) {
                    false
                } else {
                    let low = native_u32(&record[4..8]) as u64;
                    let high = native_u32(&record[8..12]) as u64;
                    let output = nodes[output_id];
                    let deps = NodeArray {
                        nodes: dependency_ids.map(|id| nodes[id]).collect(),
                    };
                    entries.insert(
                        output,
                        Entry {
                            node: output,
                            deps,
                            mtime: ((high << 32) | low) as i64,
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
                let expected_id = !native_u32(&record[size - 4..]) as usize;
                let node = node_from_path(graph, &record[..path_size]);
                if expected_id != nodes.len() || graph.node(node).id >= 0 {
                    false
                } else {
                    graph.node_mut(node).id = expected_id as i32;
                    nodes.push(node);
                    true
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

    if recovery {
        OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(offset as u64)?;
    }
    let mut log = depsinit_path(path.to_path_buf())?;
    log.next_id = nodes.len() as i32;
    log.nodes = nodes;
    log.entries = entries;
    let warning = recovery.then(|| "premature end of file; recovering".into());
    Ok((log, warning))
}

fn deps_entry_is_live(graph: &Graph, entry: &Entry) -> bool {
    graph
        .node(entry.node)
        .gen
        .is_some_and(|edge| edgevar(graph, edge, "deps", false).is_some())
}

/// Rewrite the log with only dependency entries that are still reachable from
/// an edge using Ninja's deps attribute.
pub fn depsrecompact(log: &mut DepsLog, graph: &mut Graph) -> io::Result<()> {
    let mut live_entries = log
        .entries
        .values()
        .filter(|entry| deps_entry_is_live(graph, entry))
        .cloned()
        .collect::<Vec<_>>();
    live_entries.sort_by_key(|entry| graph.node(entry.node).id);

    for node in &log.nodes {
        graph.node_mut(*node).id = -1;
    }
    let mut temp_name = log.path.as_os_str().to_os_string();
    temp_name.push(".recompact");
    let temp_path = PathBuf::from(temp_name);
    if let Err(error) = fs::remove_file(&temp_path) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    let mut compacted = depsinit_path(temp_path.clone())?;
    for entry in &live_entries {
        recordid(&mut compacted, graph, entry.node)?;
        for node in &entry.deps.nodes {
            recordid(&mut compacted, graph, *node)?;
        }
        recorddeps(&mut compacted, graph, entry.node, &entry.deps, entry.mtime)?;
    }
    compacted.writer.flush()?;
    compacted.writer.get_ref().sync_all()?;
    let entries = mem::take(&mut compacted.entries);
    let nodes = mem::take(&mut compacted.nodes);
    let next_id = compacted.next_id;
    drop(compacted);
    log.writer.flush()?;
    fs::rename(&temp_path, &log.path)?;
    let replacement = OpenOptions::new().append(true).read(true).open(&log.path)?;
    log.writer = BufWriter::new(replacement);
    log.entries = entries;
    log.nodes = nodes;
    log.next_id = next_id;
    Ok(())
}

// [spec:samurai:def:deps.depsparse-fn]
// [spec:samurai:sem:deps.depsparse-fn]
pub fn depsparse(graph: &mut Graph, path: &Path, allow_missing: bool) -> io::Result<NodeArray> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
            return Ok(NodeArray::default())
        }
        Err(error) => return Err(error),
    };
    let mut nodes = Vec::new();
    let parsed = parse_depfile(&text)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    for dependency in parsed.inputs {
        let mut path = crate::util::mkstr(dependency.len());
        path.copy_from_slice(&dependency);
        crate::util::canonpath(&mut path);
        nodes.push(crate::graph::mknode(graph, path));
    }
    Ok(NodeArray { nodes })
}

fn canonical_dep_path(path: &[u8]) -> Vec<u8> {
    let mut canonical = crate::util::mkstr(path.len());
    canonical.copy_from_slice(path);
    crate::util::canonpath(&mut canonical);
    canonical.as_bytes().to_vec()
}

pub fn depsparse_for_edge(
    graph: &mut Graph,
    path: &Path,
    edge: EdgeId,
) -> io::Result<Option<NodeArray>> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if text.is_empty() {
        return Ok(None);
    }
    let parsed = parse_depfile(&text)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    if parsed.outputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no outputs declared",
        ));
    }
    let outputs = graph.edge(edge).out.clone();
    let expected = outputs
        .first()
        .map(|output| graph.node(*output).path.as_bytes().to_vec())
        .unwrap_or_default();
    if canonical_dep_path(&parsed.outputs[0]) != expected {
        return Ok(None);
    }
    for output in &parsed.outputs {
        let output = canonical_dep_path(output);
        if !outputs
            .iter()
            .any(|expected| graph.node(*expected).path.as_bytes() == output)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "depfile mentions '{}' as an output, but no such output was declared",
                    String::from_utf8_lossy(&output)
                ),
            ));
        }
    }
    let mut nodes = Vec::new();
    for dependency in parsed.inputs {
        let dependency = canonical_dep_path(&dependency);
        nodes.push(node_from_path(graph, &dependency));
    }
    Ok(Some(NodeArray { nodes }))
}

// [spec:samurai:def:deps.depsload-fn]
// [spec:samurai:sem:deps.depsload-fn]
pub fn depsload(graph: &mut Graph, edge: EdgeId, log: &DepsLog) {
    let output = graph.edge(edge).out.first().copied();
    let Some(output) = output else { return };
    if let Some(entry) = log.entries.get(&output) {
        edgeadddeps(graph, edge, &entry.deps.nodes);
    }
}

pub fn depsentry(log: &DepsLog, output: NodeId) -> Option<&Entry> {
    log.entries.get(&output)
}

// [spec:samurai:def:deps.depsrecord-fn]
// [spec:samurai:sem:deps.depsrecord-fn]
pub fn depsrecord(log: &mut DepsLog, edge: EdgeId, graph: &mut Graph) -> io::Result<()> {
    let Some(depfile) = edgevar(graph, edge, "depfile", false) else {
        return Ok(());
    };
    let deps = depsparse(
        graph,
        depfile.to_path().expect("byte paths are valid on Unix"),
        true,
    )?;
    depsrecordnodes(log, graph, edge, &deps.nodes)
}

pub fn depsrecordnodes(
    log: &mut DepsLog,
    graph: &mut Graph,
    edge: EdgeId,
    deps: &[NodeId],
) -> io::Result<()> {
    let outputs = graph.edge(edge).out.clone();
    let deps = NodeArray {
        nodes: deps.to_vec(),
    };
    for dependency in &deps.nodes {
        recordid(log, graph, *dependency)?;
    }
    for output in outputs {
        recordid(log, graph, output)?;
        let mtime = graph.node(output).mtime;
        recorddeps(log, graph, output, &deps, mtime)?;
    }
    log.writer.flush()
}

#[cfg(test)]
mod ninja_depfile_tests {
    use super::*;
    use crate::graph::nodeget;

    fn assert_depfile(input: &str, outputs: &[&str], inputs: &[&str]) {
        let parsed = parse_depfile(input.as_bytes()).unwrap();
        assert_eq!(
            parsed.outputs,
            outputs
                .iter()
                .map(|path| path.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parsed.inputs,
            inputs
                .iter()
                .map(|path| path.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
    }

    // Cases adapted from Ninja's src/depfile_parser_test.cc.
    #[test]
    fn ninja_depfile_parser_core_cases() {
        assert_depfile(
            "build/ninja.o: ninja.cc ninja.h eval_env.h manifest_parser.h\n",
            &["build/ninja.o"],
            &["ninja.cc", "ninja.h", "eval_env.h", "manifest_parser.h"],
        );
        assert_depfile(" \\\n  out: in\n", &["out"], &["in"]);
        assert_depfile(
            "foo.o: \\\n  bar.h baz.h\n",
            &["foo.o"],
            &["bar.h", "baz.h"],
        );
        assert_depfile("foo.o: //?/c:/bar.h\n", &["foo.o"], &["//?/c:/bar.h"]);
        assert_depfile(
            "foo&bar.o foo'bar.o foo\"bar.o: foo&bar.h foo'bar.h foo\"bar.h\n",
            &["foo&bar.o", "foo'bar.o", "foo\"bar.o"],
            &["foo&bar.h", "foo'bar.h", "foo\"bar.h"],
        );
        assert_depfile(
            "foo.o: \\\r\n  bar.h baz.h\r\n",
            &["foo.o"],
            &["bar.h", "baz.h"],
        );
        assert_depfile(
            "Project\\Dir\\Build\\Release8\\Foo\\Foo.res : \\\n  Dir\\Library\\Foo.rc \\\n  Dir\\Library\\Version\\Bar.h \\\n  Dir\\Library\\Foo.ico \\\n  Project\\Thing\\Bar.tlb \\\n",
            &["Project\\Dir\\Build\\Release8\\Foo\\Foo.res"],
            &[
                "Dir\\Library\\Foo.rc",
                "Dir\\Library\\Version\\Bar.h",
                "Dir\\Library\\Foo.ico",
                "Project\\Thing\\Bar.tlb",
            ],
        );
        assert_depfile(
            "a\\ bc\\ def:   a\\ b c d",
            &["a bc def"],
            &["a b", "c", "d"],
        );
        assert_depfile(
            "a\\ b\\#c.h: \\\\\\\\\\  \\\\\\\\ \\\\share\\info\\\\#1",
            &["a b#c.h"],
            &["\\\\ ", "\\\\\\\\", "\\\\share\\info\\#1"],
        );
        assert_depfile(
            "\\!\\@\\#$$\\%\\^\\&\\[\\]\\\\:",
            &["\\!\\@#$\\%\\^\\&\\[\\]\\\\"],
            &[],
        );
        assert_depfile(
            "c\\:\\gcc\\x86_64-w64-mingw32\\include\\stddef.o: \\\n c:\\gcc\\x86_64-w64-mingw32\\include\\stddef.h \n",
            &["c:\\gcc\\x86_64-w64-mingw32\\include\\stddef.o"],
            &["c:\\gcc\\x86_64-w64-mingw32\\include\\stddef.h"],
        );
        assert_depfile(
            "foo1\\: x\nfoo1\\:\nfoo1\\:\r\nfoo1\\:\t\nfoo1\\:",
            &["foo1\\"],
            &["x"],
        );
        assert_depfile(
            "C:/Program\\ Files\\ (x86)/Microsoft\\ crtdefs.h: \\\n en@quot.header~ t+t-x!=1 \\\n openldap/slapd.d/cn=config/cn=schema/cn={0}core.ldif\\\n Fußball\\\n a[1]b@2%c",
            &["C:/Program Files (x86)/Microsoft crtdefs.h"],
            &[
                "en@quot.header~",
                "t+t-x!=1",
                "openldap/slapd.d/cn=config/cn=schema/cn={0}core.ldif",
                "Fußball",
                "a[1]b@2%c",
            ],
        );
    }

    #[test]
    fn ninja_depfile_parser_multi_rule_cases() {
        assert_depfile("foo foo: x y z", &["foo"], &["x", "y", "z"]);
        assert_depfile("foo bar: x y z", &["foo", "bar"], &["x", "y", "z"]);
        assert_depfile("foo: x\nfoo: \nfoo:\n", &["foo"], &["x"]);
        assert_depfile(
            "foo: x\nfoo: y\nfoo \\\nfoo: z\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(
            "foo: x\r\nfoo: y\r\nfoo \\\r\nfoo: z\r\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(
            "foo: x\\\n     y\nfoo \\\nfoo: z\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(
            "foo: x\\\r\n     y\r\nfoo \\\r\nfoo: z\r\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(" foo: x\n foo: y\n foo: z\n", &["foo"], &["x", "y", "z"]);
        assert_depfile(
            " foo: x\r\n foo: y\r\n foo: z\r\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile("foo: x y z\nx:\ny:\nz:\n", &["foo"], &["x", "y", "z"]);
        assert_depfile(
            "foo: x\nx:\nfoo: y\ny:\nfoo: z\nz:\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile("foo: x y\nbar: y z\n", &["foo", "bar"], &["x", "y", "z"]);
        assert_depfile("", &[], &[]);
        assert_depfile("\n\n", &[], &[]);
    }

    #[test]
    fn ninja_depfile_parser_rejects_invalid_rules() {
        assert_eq!(
            parse_depfile(b"foo: x y z\nx: alsoin\ny:\nz:\n").unwrap_err(),
            "inputs may not also have inputs"
        );
        assert_eq!(
            parse_depfile(b"foo.o foo.c\n").unwrap_err(),
            "expected ':' in depfile"
        );
    }

    #[test]
    fn ninja_depfile_parser_preserves_non_utf8_paths() {
        let parsed = parse_depfile(b"out: in-\xff.h in-\xff.h\n").unwrap();
        assert_eq!(parsed.outputs, [b"out".to_vec()]);
        assert_eq!(parsed.inputs, [b"in-\xff.h".to_vec()]);
    }

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
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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

    fn record(log: &mut DepsLog, graph: &mut Graph, output: NodeId, paths: &[NodeId], mtime: i64) {
        let deps = NodeArray {
            nodes: paths.to_vec(),
        };
        recordid(log, graph, output).unwrap();
        for dependency in paths {
            recordid(log, graph, *dependency).unwrap();
        }
        recorddeps(log, graph, output, &deps, mtime).unwrap();
        log.writer.flush().unwrap();
    }

    fn paths(graph: &Graph, nodes: &[NodeId]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| String::from_utf8(graph.node(*node).path.as_bytes().to_vec()).unwrap())
            .collect()
    }

    fn make_deps_edge(graph: &mut Graph, output: NodeId) {
        let state = crate::env::envinit(graph);
        let rule = crate::env::mkrule(graph, "cc".into());
        crate::env::ruleaddvar(
            graph,
            rule,
            "deps".into(),
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
        let mut source = crate::graph::graphinit();
        let output = make_node(&mut source, "out.o");
        let output2 = make_node(&mut source, "out2.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let bar2 = make_node(&mut source, "bar2.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut source, output, &[foo, bar], 1);
        record(&mut log, &mut source, output2, &[foo, bar2], 2);
        depsclose(log).unwrap();

        let mut loaded_graph = crate::graph::graphinit();
        let (loaded, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(warning, None);
        assert_eq!(loaded.nodes.len(), 5);
        let output2 = make_node(&mut loaded_graph, "out2.o");
        let entry = loaded.entries.get(&output2).unwrap();
        assert_eq!(entry.mtime, 2);
        assert_eq!(paths(&loaded_graph, &entry.deps.nodes), ["foo.h", "bar2.h"]);
        drop(loaded);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_lots_of_dependencies() {
        let (directory, path) = test_log_path("many");
        let mut source = crate::graph::graphinit();
        let output = make_node(&mut source, "out.o");
        let dependencies = (0..100_000)
            .map(|index| make_node(&mut source, &format!("file{index}.h")))
            .collect::<Vec<_>>();
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut source, output, &dependencies, 1);
        depsclose(log).unwrap();

        let mut loaded_graph = crate::graph::graphinit();
        let (loaded, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(warning, None);
        let output = make_node(&mut loaded_graph, "out.o");
        assert_eq!(
            loaded.entries.get(&output).unwrap().deps.nodes.len(),
            100_000
        );
        drop(loaded);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_avoids_duplicate_entries() {
        let (directory, path) = test_log_path("duplicate");
        let mut graph = crate::graph::graphinit();
        let output = make_node(&mut graph, "out.o");
        let foo = make_node(&mut graph, "foo.h");
        let bar = make_node(&mut graph, "bar.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut graph, output, &[foo, bar], 1);
        depsclose(log).unwrap();
        let original_size = fs::metadata(&path).unwrap().len();

        let mut reloaded_graph = crate::graph::graphinit();
        let (mut log, warning) = depsloadlog(&path, &mut reloaded_graph).unwrap();
        assert_eq!(warning, None);
        let output = make_node(&mut reloaded_graph, "out.o");
        let foo = make_node(&mut reloaded_graph, "foo.h");
        let bar = make_node(&mut reloaded_graph, "bar.h");
        record(&mut log, &mut reloaded_graph, output, &[foo, bar], 1);
        depsclose(log).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), original_size);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_recompacts_live_entries() {
        let (directory, path) = test_log_path("recompact");
        let mut source = crate::graph::graphinit();
        let output = make_node(&mut source, "out.o");
        let other_output = make_node(&mut source, "other_out.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let baz = make_node(&mut source, "baz.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut source, output, &[foo, bar], 1);
        record(&mut log, &mut source, other_output, &[foo, baz], 1);
        depsclose(log).unwrap();

        let mut graph = crate::graph::graphinit();
        let (mut log, warning) = depsloadlog(&path, &mut graph).unwrap();
        assert_eq!(warning, None);
        let output = make_node(&mut graph, "out.o");
        let other_output = make_node(&mut graph, "other_out.o");
        let foo = make_node(&mut graph, "foo.h");
        make_deps_edge(&mut graph, output);
        make_deps_edge(&mut graph, other_output);
        record(&mut log, &mut graph, output, &[foo], 1);
        let grown_size = fs::metadata(&path).unwrap().len();
        depsrecompact(&mut log, &mut graph).unwrap();
        assert_eq!(
            paths(&graph, &log.entries.get(&output).unwrap().deps.nodes),
            ["foo.h"]
        );
        assert_eq!(
            paths(&graph, &log.entries.get(&other_output).unwrap().deps.nodes),
            ["foo.h", "baz.h"]
        );
        assert!(fs::metadata(&path).unwrap().len() < grown_size);
        for entry in log.entries.values() {
            assert_eq!(
                graph.node(log.nodes[graph.node(entry.node).id as usize]).id,
                graph.node(entry.node).id
            );
        }

        let mut dead_graph = crate::graph::graphinit();
        let (mut dead_log, warning) = depsloadlog(&path, &mut dead_graph).unwrap();
        assert_eq!(warning, None);
        let foo = make_node(&mut dead_graph, "foo.h");
        depsrecompact(&mut dead_log, &mut dead_graph).unwrap();
        assert!(dead_log.entries.is_empty());
        assert_eq!(dead_graph.node(foo).id, -1);
        drop(dead_log);
        drop(log);
        remove_test_log(directory);
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
            let mut graph = crate::graph::graphinit();
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
        let mut source = crate::graph::graphinit();
        let output = make_node(&mut source, "out.o");
        let output2 = make_node(&mut source, "out2.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let bar2 = make_node(&mut source, "bar2.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut source, output, &[foo, bar], 1);
        record(&mut log, &mut source, output2, &[foo, bar2], 2);
        depsclose(log).unwrap();
        let original_size = fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(original_size - 2)
            .unwrap();

        let mut graph = crate::graph::graphinit();
        let (log, warning) = depsloadlog(&path, &mut graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        let out = nodeget(&graph, b"out.o").unwrap();
        let out2 = nodeget(&graph, b"out2.o").unwrap();
        assert!(log.entries.contains_key(&out));
        assert!(!log.entries.contains_key(&out2));
        drop(log);
        let mut reloaded_graph = crate::graph::graphinit();
        let (_log, warning) = depsloadlog(&path, &mut reloaded_graph).unwrap();
        assert_eq!(warning, None);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_appends_after_truncation_recovery() {
        let (directory, path) = test_log_path("truncated-append");
        let mut source = crate::graph::graphinit();
        let output = make_node(&mut source, "out.o");
        let output2 = make_node(&mut source, "out2.o");
        let foo = make_node(&mut source, "foo.h");
        let bar = make_node(&mut source, "bar.h");
        let bar2 = make_node(&mut source, "bar2.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut source, output, &[foo, bar], 1);
        record(&mut log, &mut source, output2, &[foo, bar2], 2);
        depsclose(log).unwrap();

        let original_size = fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(original_size - 2)
            .unwrap();

        let mut recovered_graph = crate::graph::graphinit();
        let (mut recovered, warning) = depsloadlog(&path, &mut recovered_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        let output2 = make_node(&mut recovered_graph, "out2.o");
        let foo = make_node(&mut recovered_graph, "foo.h");
        let bar2 = make_node(&mut recovered_graph, "bar2.h");
        record(
            &mut recovered,
            &mut recovered_graph,
            output2,
            &[foo, bar2],
            3,
        );
        depsclose(recovered).unwrap();

        let mut final_graph = crate::graph::graphinit();
        let (final_log, warning) = depsloadlog(&path, &mut final_graph).unwrap();
        assert_eq!(warning, None);
        let output2 = make_node(&mut final_graph, "out2.o");
        let entry = final_log.entries.get(&output2).unwrap();
        assert_eq!(entry.mtime, 3);
        assert_eq!(paths(&final_graph, &entry.deps.nodes), ["foo.h", "bar2.h"]);
        drop(final_log);
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_finds_reverse_dependencies() {
        let (directory, path) = test_log_path("reverse");
        let mut graph = crate::graph::graphinit();
        let output = make_node(&mut graph, "out.o");
        let output2 = make_node(&mut graph, "out2.o");
        let foo = make_node(&mut graph, "foo.h");
        let bar = make_node(&mut graph, "bar.h");
        let bar2 = make_node(&mut graph, "bar2.h");
        let mut log = depsinit_path(path).unwrap();
        record(&mut log, &mut graph, output, &[foo, bar], 1);
        record(&mut log, &mut graph, output2, &[foo, bar2], 2);
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
        depsclose(log).unwrap();
        remove_test_log(directory);
    }

    #[test]
    fn ninja_deps_log_recovers_malformed_records() {
        let (directory, path) = test_log_path("malformed");
        let mut graph = crate::graph::graphinit();
        let output = make_node(&mut graph, "out.o");
        let foo = make_node(&mut graph, "foo.hh");
        let bar = make_node(&mut graph, "bar.hpp");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut graph, output, &[foo, bar], 1);
        depsclose(log).unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(&original[..12], b"# ninjadeps\n");

        let first_record = 16;
        let mut bad = original.clone();
        bad[first_record..first_record + 4].copy_from_slice(&0x7fff_aa55u32.to_ne_bytes());
        fs::write(&path, bad).unwrap();
        let mut loaded_graph = crate::graph::graphinit();
        let (_log, warning) = depsloadlog(&path, &mut loaded_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );

        fs::write(&path, &original[..first_record + 4 + 1]).unwrap();
        let mut loaded_graph = crate::graph::graphinit();
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
        let mut graph = crate::graph::graphinit();
        let output = make_node(&mut graph, "out.o");
        let foo = make_node(&mut graph, "foo.h");
        let bar = make_node(&mut graph, "bar.h");
        let mut log = depsinit_path(path.clone()).unwrap();
        record(&mut log, &mut graph, output, &[foo, bar], 1);
        depsclose(log).unwrap();

        let mut duplicate = Vec::new();
        duplicate.extend_from_slice(&12u32.to_ne_bytes());
        duplicate.extend_from_slice(b"foo.h\0\0\0");
        duplicate.extend_from_slice(&(!1u32).to_ne_bytes());
        let mut content = fs::read(&path).unwrap();
        content.extend_from_slice(&duplicate);
        fs::write(&path, content).unwrap();

        let mut first_graph = crate::graph::graphinit();
        let (first, warning) = depsloadlog(&path, &mut first_graph).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("premature end of file; recovering")
        );
        let out = nodeget(&first_graph, b"out.o").unwrap();
        assert!(first.entries.contains_key(&out));
        drop(first);
        let mut second_graph = crate::graph::graphinit();
        let (second, warning) = depsloadlog(&path, &mut second_graph).unwrap();
        assert_eq!(warning, None);
        let out = nodeget(&second_graph, b"out.o").unwrap();
        assert!(second.entries.contains_key(&out));
        drop(second);
        remove_test_log(directory);
    }
}
