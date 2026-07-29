use crate::error::ToolError;
use crate::graph::{nodeget, Graph, InputsCollector, NodeId};

type ToolResult<T> = Result<T, ToolError>;

fn collect_targets(graph: &Graph, targets: &[String]) -> ToolResult<Vec<NodeId>> {
    targets
        .iter()
        .map(|target| {
            nodeget(graph, target.as_bytes())
                .ok_or_else(|| ToolError::from(format!("unknown target '{target}'")))
        })
        .collect()
}

pub(crate) fn inputs(graph: &Graph, arguments: &[String]) -> ToolResult<String> {
    let mut print0 = false;
    let mut shell_escape = true;
    let mut dependency_order = false;
    let mut targets = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-0" | "--print0" => print0 = true,
            "-E" | "--no-shell-escape" => shell_escape = false,
            "-d" | "--dependency-order" => dependency_order = true,
            "-h" | "--help" => return Err("Usage '-t inputs [options] [targets]".into()),
            option if option.starts_with('-') => {
                return Err(format!("unknown inputs option '{option}'").into())
            }
            target => targets.push(target.to_owned()),
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
    let separator = if print0 { "\0" } else { "\n" };
    let mut output = inputs.join(separator);
    if !output.is_empty() {
        output.push_str(separator);
    }
    Ok(output)
}

pub(crate) fn multi_inputs(graph: &Graph, arguments: &[String]) -> ToolResult<String> {
    let mut print0 = false;
    let mut delimiter = "\t".to_owned();
    let mut targets = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "-0" | "--print0" => print0 = true,
            "-h" | "--help" => return Err("Usage '-t multi-inputs [options] [targets]".into()),
            "-d" | "--delimiter" => {
                index += 1;
                delimiter.clone_from(
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing multi-inputs delimiter".to_owned())?,
                );
            }
            option if option.starts_with("--delimiter=") => {
                delimiter.clear();
                delimiter.push_str(&option["--delimiter=".len()..]);
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown multi-inputs option '{option}'").into())
            }
            target => targets.push(target.to_owned()),
        }
        index += 1;
    }
    let nodes = collect_targets(graph, &targets)?;
    let terminator = if print0 { '\0' } else { '\n' };
    let mut output = String::new();
    for (target, node) in targets.iter().zip(nodes) {
        let mut collector = InputsCollector::default();
        collector.visit_node(graph, node);
        for input in collector.input_strings(graph, true) {
            output.push_str(target);
            output.push_str(&delimiter);
            output.push_str(&input);
            output.push(terminator);
        }
    }
    Ok(output)
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
            inputs(&fixture.graph, &["all".into()]).unwrap(),
            "implicit\nmiddle\norder\noutput\nsource\n"
        );
        assert_eq!(
            inputs(&fixture.graph, &["-d".into(), "-E".into(), "output".into()]).unwrap(),
            "source\nmiddle\nimplicit\norder\n"
        );
        assert_eq!(
            multi_inputs(
                &fixture.graph,
                &["--delimiter=:".into(), "middle".into(), "output".into()]
            )
            .unwrap(),
            "middle:source\noutput:source\noutput:middle\noutput:implicit\noutput:order\n"
        );
    }
}
