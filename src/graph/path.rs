//! Node path rendering.
//!
//! Command evaluation appends these bytes directly, so the borrowed form must
//! not copy; the owned form stays for the ported C surface.

use super::{Graph, NodeId, PathStyle};
use crate::util::{BString, ByteSlice};

// [spec:samurai:def:graph.nodepath-fn]
// [spec:samurai:sem:graph.nodepath-fn]
/// Borrow a node's path in the requested style.
///
/// Command evaluation appends these directly, so it must not pay for a copy
/// per node occurrence.
pub(crate) fn nodepath_bytes(graph: &Graph, node: NodeId, style: PathStyle) -> &[u8] {
    let node = graph.node(node);
    if style.shell_escaped() {
        node.shellpath.as_bytes()
    } else {
        node.path.as_bytes()
    }
}

pub(crate) fn nodepath(graph: &Graph, node: NodeId, style: PathStyle) -> BString {
    let node = graph.node(node);
    if style.shell_escaped() {
        node.shellpath.clone()
    } else {
        node.path.clone()
    }
}

pub(super) fn shell_escape_path(source: &[u8]) -> BString {
    let quote = source
        .iter()
        .any(|byte| !byte.is_ascii_alphanumeric() && !b"_+-./".contains(byte));
    if !quote {
        return BString::from(source);
    }
    let mut bytes = Vec::with_capacity(source.len() + 2);
    if quote {
        bytes.push(b'\'');
        for byte in source {
            bytes.push(*byte);
            if *byte == b'\'' {
                bytes.extend_from_slice(b"\\''");
            }
        }
        bytes.push(b'\'');
    }
    BString::from(bytes)
}
