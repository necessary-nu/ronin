use crate::env::edgevar;
use crate::error::ToolError;
use crate::graph::{nodeget, CommandCollector, EdgeId, Graph};
use crate::util::ByteSlice;
use std::fmt::Write;

fn json_string(bytes: &[u8]) -> String {
    let mut output = String::new();
    for byte in bytes {
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x00..=0x1f => {
                let _ = write!(output, "\\u{byte:04x}");
            }
            _ => output.push(char::from(*byte)),
        }
    }
    output
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
) -> String {
    let directory = std::env::current_dir()
        .unwrap_or_default()
        .into_os_string()
        .into_encoded_bytes();
    let mut entries = Vec::new();
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
            entries.push(format!(
                "  {{\n    \"directory\": \"{}\",\n    \"command\": \"{}\",\n    \"file\": \"{}\",\n    \"output\": \"{}\"\n  }}",
                json_string(&directory),
                json_string(&command),
                json_string(graph.node(*input).path.as_bytes()),
                edge_ref
                    .out
                    .first()
                    .map(|output| json_string(graph.node(*output).path.as_bytes()))
                    .unwrap_or_default(),
            ));
        }
    }
    if entries.is_empty() {
        "[]\n".into()
    } else {
        format!("[\n{}\n]\n", entries.join(",\n"))
    }
}

// [spec:samurai:def:tool.compdb-fn]
// [spec:samurai:sem:tool.compdb-fn]
pub(crate) fn compdb(graph: &Graph, rules: &[String], expand_rsp: bool) -> String {
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
    targets: &[String],
    expand_rsp: bool,
) -> Result<String, ToolError> {
    if targets.is_empty() {
        return Err("compdb-targets expects the name of at least one target".into());
    }
    let mut collector = CommandCollector::default();
    for target in targets {
        let node = nodeget(graph, target.as_bytes())
            .ok_or_else(|| format!("unknown target '{target}'"))?;
        if graph.node(node).gen.is_none() {
            return Err(format!(
                "'{target}' is not a target (i.e. it is not an output of any `build` statement)"
            )
            .into());
        }
        collector.collect_from(graph, node);
    }
    Ok(render(graph, collector.edges, expand_rsp, true))
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
        let regular = compdb(&fixture.graph, &[], false);
        assert!(regular.contains("\"command\": \"cc @object.rsp -o object\""));
        assert!(regular.contains("\"file\": \"source\""));
        assert!(regular.contains("\"output\": \"object\""));

        let expanded = compdb_for_targets(&fixture.graph, &["all".into()], true).unwrap();
        assert!(expanded.contains("\"command\": \"cc -DVALUE source -o object\""));
        assert!(!expanded.contains("@object.rsp"));
        assert_eq!(
            compdb_for_targets(&fixture.graph, &["source".into()], false).unwrap_err(),
            "'source' is not a target (i.e. it is not an output of any `build` statement)"
        );
    }
}
