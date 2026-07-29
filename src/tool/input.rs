use crate::error::ToolError;
use crate::graph::{nodeget, Graph, InputsCollector, NodeId};
use crate::util::{BString, ByteSlice};

type ToolResult<T> = Result<T, ToolError>;

fn collect_targets(graph: &Graph, targets: &[BString]) -> ToolResult<Vec<NodeId>> {
    targets
        .iter()
        .map(|target| {
            nodeget(graph, target.as_bytes()).ok_or_else(|| {
                ToolError::from(format!("unknown target '{}'", target.to_str_lossy()))
            })
        })
        .collect()
}

pub(crate) fn inputs(graph: &Graph, arguments: &[BString]) -> ToolResult<BString> {
    let mut print0 = false;
    let mut shell_escape = true;
    let mut dependency_order = false;
    let mut targets = Vec::new();
    for argument in arguments {
        match argument.as_bytes() {
            b"-0" | b"--print0" => print0 = true,
            b"-E" | b"--no-shell-escape" => shell_escape = false,
            b"-d" | b"--dependency-order" => dependency_order = true,
            b"-h" | b"--help" => return Err("Usage '-t inputs [options] [targets]".into()),
            option if option.starts_with(b"-") => {
                return Err(format!("unknown inputs option '{}'", argument.to_str_lossy()).into())
            }
            _ => targets.push(argument.clone()),
        }
    }
    let mut collector = InputsCollector::default();
    for node in collect_targets(graph, &targets)? {
        collector.visit_node(graph, node);
    }
    let mut inputs = collector.input_strings(graph, shell_escape);
    if !dependency_order {
        inputs.sort();
    }
    let separator = if print0 { b'\0' } else { b'\n' };
    let mut output = Vec::new();
    for input in inputs {
        output.extend_from_slice(input.as_bytes());
        output.push(separator);
    }
    Ok(BString::from(output))
}

pub(crate) fn multi_inputs(graph: &Graph, arguments: &[BString]) -> ToolResult<BString> {
    let mut print0 = false;
    let mut delimiter = BString::from("\t");
    let mut targets = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_bytes() {
            b"-0" | b"--print0" => print0 = true,
            b"-h" | b"--help" => return Err("Usage '-t multi-inputs [options] [targets]".into()),
            b"-d" | b"--delimiter" => {
                index += 1;
                delimiter = arguments
                    .get(index)
                    .ok_or_else(|| "missing multi-inputs delimiter".to_owned())?
                    .clone();
            }
            option if option.starts_with(b"--delimiter=") => {
                delimiter = BString::from(&option[b"--delimiter=".len()..]);
            }
            option if option.starts_with(b"-") => {
                return Err(format!(
                    "unknown multi-inputs option '{}'",
                    arguments[index].to_str_lossy()
                )
                .into())
            }
            _ => targets.push(arguments[index].clone()),
        }
        index += 1;
    }
    let nodes = collect_targets(graph, &targets)?;
    let terminator = if print0 { b'\0' } else { b'\n' };
    let mut output = Vec::new();
    for (target, node) in targets.iter().zip(nodes) {
        let mut collector = InputsCollector::default();
        collector.visit_node(graph, node);
        for input in collector.input_strings(graph, true) {
            output.extend_from_slice(target.as_bytes());
            output.extend_from_slice(delimiter.as_bytes());
            output.extend_from_slice(input.as_bytes());
            output.push(terminator);
        }
    }
    Ok(BString::from(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::Fixture;

    #[test]
    fn renders_sorted_dependency_and_multi_target_inputs() {
        let fixture = Fixture::parse(
            "inputs",
            concat!(
                "rule cat\n",
                "  command = cat $in > $out\n",
                "build middle: cat source\n",
                "build output: cat middle | implicit || order\n",
                "build all: phony output\n"
            ),
        );
        assert_eq!(
            inputs(&fixture.graph, &["all".into()]).unwrap().as_bytes(),
            b"implicit\nmiddle\norder\noutput\nsource\n"
        );
        assert_eq!(
            inputs(&fixture.graph, &["-d".into(), "-E".into(), "output".into()])
                .unwrap()
                .as_bytes(),
            b"source\nmiddle\nimplicit\norder\n"
        );
        assert_eq!(
            multi_inputs(
                &fixture.graph,
                &["--delimiter=:".into(), "middle".into(), "output".into()]
            )
            .unwrap(),
            b"middle:source\noutput:source\noutput:middle\noutput:implicit\noutput:order\n"
        );
    }
}
