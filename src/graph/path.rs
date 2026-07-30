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
    match &node.shellpath {
        Some(escaped) if style.shell_escaped() => escaped.as_bytes(),
        _ => node.path.as_bytes(),
    }
}

pub(crate) fn nodepath(graph: &Graph, node: NodeId, style: PathStyle) -> BString {
    BString::from(nodepath_bytes(graph, node, style))
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
