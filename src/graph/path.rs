//! Node path rendering.
//!
//! Command evaluation appends these bytes directly, so the borrowed form must
//! not copy; the owned form stays for the ported C surface.

use super::PathSpan;
use super::{Graph, NodeId, PathStyle};
use crate::util::{BStr, BString, ByteSlice};

/// Borrow a node's path in the requested style.
///
/// Command evaluation appends these directly, so it must not pay for a copy
/// per node occurrence.
pub(crate) fn nodepath_bytes(graph: &Graph, node: NodeId, style: PathStyle) -> &[u8] {
    if style.shell_escaped() {
        graph.node_shellpath(node).as_bytes()
    } else {
        graph.node_path(node).as_bytes()
    }
}

/// Shell-quote `source`, or report that quoting would not change it.
///
/// Almost every real path needs no quoting, so the unquoted case must not
/// copy: the node keeps only its plain path and renders that for both styles.
pub(super) fn shell_escape_path(source: &[u8]) -> Option<BString> {
    let quote = source
        .iter()
        .any(|byte| !byte.is_ascii_alphanumeric() && !b"_+-./".contains(byte));
    if !quote {
        return None;
    }
    let mut bytes = Vec::with_capacity(source.len() + 2);
    bytes.push(b'\'');
    for byte in source {
        bytes.push(*byte);
        if *byte == b'\'' {
            bytes.extend_from_slice(b"\\''");
        }
    }
    bytes.push(b'\'');
    Some(BString::from(bytes))
}

impl Graph {
    pub(super) fn span(&self, span: PathSpan) -> &BStr {
        let start = span.offset as usize;
        BStr::new(&self.paths[start..start + span.len as usize])
    }

    /// Append to the arena and return the span naming what was appended.
    pub(super) fn intern_bytes(&mut self, bytes: &[u8]) -> PathSpan {
        let offset = u32::try_from(self.paths.len()).expect("path arena stays within u32");
        let len = u32::try_from(bytes.len()).expect("a path stays within u32");
        self.paths.extend_from_slice(bytes);
        PathSpan { offset, len }
    }

    pub(crate) fn node_path(&self, id: NodeId) -> &BStr {
        self.span(self.nodes[id.index()].path)
    }

    /// The shell-quoted form, or the path itself when quoting changes nothing.
    pub(crate) fn node_shellpath(&self, id: NodeId) -> &BStr {
        let node = &self.nodes[id.index()];
        self.span(node.shellpath.unwrap_or(node.path))
    }
}

#[cfg(test)]
mod tests {
    use super::super::mknode;
    use super::{Graph, PathStyle};
    use crate::util::ByteSlice;

    /// Re-interning a path must not append to the arena a second time.
    ///
    /// The arena is append-only and never compacted, so a path that grew it on
    /// every reference would turn a manifest's repeated inputs into unbounded
    /// growth — a build statement naming the same header a thousand times is
    /// ordinary. Interning is also the only writer, so this is the one place
    /// the invariant can be checked.
    #[test]
    fn interning_a_known_path_leaves_the_arena_untouched() {
        let mut graph = Graph::default();
        let first = mknode(&mut graph, b"src/a.c".as_slice());
        let quoted = mknode(&mut graph, b"src/a b.c".as_slice());
        let grown = graph.paths.len();

        assert_eq!(mknode(&mut graph, b"src/a.c".as_slice()), first);
        assert_eq!(mknode(&mut graph, b"src/a b.c".as_slice()), quoted);
        assert_eq!(graph.paths.len(), grown, "a known path must not re-append");

        // Spans keep resolving after the arena has grown past them.
        assert_eq!(graph.node_path(first), "src/a.c");
        assert_eq!(graph.node_shellpath(first), "src/a.c");
        assert_eq!(graph.node_shellpath(quoted), "'src/a b.c'");
        assert_eq!(
            super::nodepath_bytes(&graph, quoted, PathStyle::Raw).as_bstr(),
            "src/a b.c"
        );
    }
}
