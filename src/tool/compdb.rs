use crate::env::edgevar;
use crate::error::ToolError;
use crate::graph::{nodeget, CommandCollector, EdgeId, Graph};
use crate::util::{BString, ByteSlice};

fn push_json_string(output: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        match byte {
            b'"' => output.extend_from_slice(b"\\\""),
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\n' => output.extend_from_slice(b"\\n"),
            b'\r' => output.extend_from_slice(b"\\r"),
            b'\t' => output.extend_from_slice(b"\\t"),
            0x00..=0x1f => {
                output.extend_from_slice(b"\\u00");
                output.push(HEX[usize::from(*byte >> 4)]);
                output.push(HEX[usize::from(*byte & 0x0f)]);
            }
            _ => output.push(*byte),
        }
    }
}

fn expanded_command(graph: &Graph, edge: EdgeId, expand_rsp: bool) -> Vec<u8> {
    let mut command = edgevar(graph, edge, "command", true)
        .map(Vec::from)
        .unwrap_or_default();
    if !expand_rsp {
        return command;
    }
    let Some(rspfile) = edgevar(graph, edge, "rspfile", false).filter(|path| !path.is_empty())
    else {
        return command;
    };
    let Some(index) = command
        .windows(rspfile.len())
        .position(|window| window == rspfile.as_bytes())
    else {
        return command;
    };
    let prefix = if index != 0 && command[index - 1] == b'@' {
        index - 1
    } else if index >= 3 && &command[index - 3..index] == b"-f " {
        index - 3
    } else if index >= 14 && &command[index - 14..index] == b"--option-file=" {
        index - 14
    } else {
        return command;
    };
    let mut content = edgevar(graph, edge, "rspfile_content", false)
        .map(Vec::from)
        .unwrap_or_default();
    for byte in &mut content {
        if *byte == b'\n' {
            *byte = b' ';
        }
    }
    command.splice(prefix..index + rspfile.len(), content);
    command
}

fn validation_only(graph: &Graph, edge: EdgeId) -> bool {
    let outputs = &graph.edge(edge).out;
    !outputs.is_empty()
        && outputs.iter().all(|output| {
            let output = graph.node(*output);
            output.uses.is_empty() && !output.validation_uses.is_empty()
        })
}

// [spec:samurai:def:os.osgetcwd-fn]
// [spec:samurai:sem:os.osgetcwd-fn]
// [spec:samurai:def:os-posix.osgetcwd-fn]
// [spec:samurai:sem:os-posix.osgetcwd-fn]
fn render(
    graph: &Graph,
    edges: impl IntoIterator<Item = EdgeId>,
    expand_rsp: bool,
    skip_phony: bool,
) -> Result<BString, ToolError> {
    render_with_current_directory(graph, edges, expand_rsp, skip_phony, std::env::current_dir)
}

fn render_with_current_directory(
    graph: &Graph,
    edges: impl IntoIterator<Item = EdgeId>,
    expand_rsp: bool,
    skip_phony: bool,
    current_directory: impl FnOnce() -> std::io::Result<std::path::PathBuf>,
) -> Result<BString, ToolError> {
    let directory = current_directory()?.into_os_string().into_encoded_bytes();
    let mut output = Vec::from(&b"[\n"[..]);
    let mut first = true;
    for edge in edges {
        let edge_ref = graph.edge(edge);
        if edge_ref.input.is_empty()
            || validation_only(graph, edge)
            || (skip_phony
                && edge_ref
                    .rule
                    .is_some_and(|rule| graph.rule(rule).name == "phony"))
        {
            continue;
        }
        let command = expanded_command(graph, edge, expand_rsp);
        for input in &edge_ref.input {
            if !first {
                output.extend_from_slice(b",\n");
            }
            first = false;
            output.extend_from_slice(b"  {\n    \"directory\": \"");
            push_json_string(&mut output, &directory);
            output.extend_from_slice(b"\",\n    \"command\": \"");
            push_json_string(&mut output, &command);
            output.extend_from_slice(b"\",\n    \"file\": \"");
            push_json_string(&mut output, graph.node(*input).path.as_bytes());
            output.extend_from_slice(b"\",\n    \"output\": \"");
            if let Some(output_node) = edge_ref.out.first() {
                push_json_string(&mut output, graph.node(*output_node).path.as_bytes());
            }
            output.extend_from_slice(b"\"\n  }");
        }
    }
    output.extend_from_slice(if first { b"]\n" } else { b"\n]\n" });
    Ok(BString::from(output))
}

// [spec:samurai:def:tool.compdb-fn]
// [spec:samurai:sem:tool.compdb-fn]
// [spec:samurai:req:runtime.output-byte-boundaries]
pub(crate) fn compdb(
    graph: &Graph,
    rules: &[String],
    expand_rsp: bool,
) -> Result<BString, ToolError> {
    let edges = graph.edge_ids().filter(|edge| {
        rules.is_empty()
            || graph
                .edge(*edge)
                .rule
                .is_some_and(|rule| rules.iter().any(|name| name == &graph.rule(rule).name))
    });
    render(graph, edges, expand_rsp, false)
}

pub(crate) fn compdb_for_targets(
    graph: &Graph,
    targets: &[BString],
    expand_rsp: bool,
) -> Result<BString, ToolError> {
    if targets.is_empty() {
        return Err("compdb-targets expects the name of at least one target".into());
    }
    let mut collector = CommandCollector::default();
    for target in targets {
        let node = nodeget(graph, target.as_bytes())
            .ok_or_else(|| format!("unknown target '{}'", target.to_str_lossy()))?;
        if graph.node(node).gen.is_none() {
            return Err(format!(
                "'{}' is not a target (i.e. it is not an output of any `build` statement)",
                target.to_str_lossy()
            )
            .into());
        }
        collector.collect_from(graph, node);
    }
    render(graph, collector.edges, expand_rsp, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::Fixture;

    #[test]
    fn renders_all_rules_targets_and_expanded_response_files() {
        let fixture = Fixture::parse(
            "compdb",
            concat!(
                "rule cc\n",
                "  command = cc @$rspfile -o $out\n",
                "  rspfile = $out.rsp\n",
                "  rspfile_content = -DVALUE $in\n",
                "build object: cc source\n",
                "build all: phony object\n"
            ),
        );
        let regular = compdb(&fixture.graph, &[], false).unwrap();
        assert!(regular
            .as_bytes()
            .contains_str("\"command\": \"cc @object.rsp -o object\""));
        assert!(regular.as_bytes().contains_str("\"file\": \"source\""));
        assert!(regular.as_bytes().contains_str("\"output\": \"object\""));

        let expanded = compdb_for_targets(&fixture.graph, &["all".into()], true).unwrap();
        assert!(expanded
            .as_bytes()
            .contains_str("\"command\": \"cc -DVALUE source -o object\""));
        assert!(!expanded.as_bytes().contains_str("@object.rsp"));
        assert_eq!(
            compdb_for_targets(&fixture.graph, &["source".into()], false).unwrap_err(),
            "'source' is not a target (i.e. it is not an output of any `build` statement)"
        );
    }

    // [spec:samurai:req:runtime.output-byte-boundaries/test]
    #[test]
    fn json_encoding_preserves_utf8_and_non_utf8_bytes() {
        let mut encoded = Vec::new();
        push_json_string(&mut encoded, b"r\xc3\xa9sum\xc3\xa9-\xff-\n-\"");
        assert_eq!(encoded, b"r\xc3\xa9sum\xc3\xa9-\xff-\\n-\\\"");
    }

    #[test]
    fn current_directory_errors_are_propagated() {
        let fixture = Fixture::parse("compdb-cwd-error", "");
        let error =
            render_with_current_directory(&fixture.graph, std::iter::empty(), false, false, || {
                Err(std::io::Error::other("cwd unavailable"))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "cwd unavailable");
    }
}
