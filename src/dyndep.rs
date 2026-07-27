//! Ninja version-1 dynamic dependency file parser.

use crate::graph::{edgeadddeps, mknode, nodeget, EdgeRef, Graph, NodeRef};
use crate::util::{canonpath, xasprintf};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::rc::Rc;

#[derive(Clone, Default)]
pub struct Dyndeps {
    pub used: bool,
    pub restat: bool,
    pub implicit_inputs: Vec<NodeRef>,
    pub implicit_outputs: Vec<NodeRef>,
}

pub type DyndepFile = BTreeMap<usize, Dyndeps>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DyndepError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for DyndepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "input:{}: {}", self.line, self.message)
    }
}

impl std::error::Error for DyndepError {}

fn error(line: usize, message: impl Into<String>) -> DyndepError {
    DyndepError {
        line,
        message: message.into(),
    }
}

fn edge_identity(edge: &EdgeRef) -> usize {
    Rc::as_ptr(edge) as usize
}

fn tokens(line: &str) -> Vec<String> {
    let mut result = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index] == b':' {
            result.push(":".into());
            index += 1;
            continue;
        }
        if bytes[index] == b'|' {
            match bytes.get(index + 1) {
                Some(b'|') => {
                    result.push("||".into());
                    index += 2;
                }
                Some(b'@') => {
                    result.push("|@".into());
                    index += 2;
                }
                _ => {
                    result.push("|".into());
                    index += 1;
                }
            }
            continue;
        }
        let start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b':' | b'|')
        {
            index += 1;
        }
        result.push(line[start..index].into());
    }
    result
}

fn parse_version(line_number: usize, line: &str, terminated: bool) -> Result<(), DyndepError> {
    if line.starts_with(char::is_whitespace) {
        return Err(error(line_number, "unexpected indent"));
    }
    if line.starts_with('=') {
        return Err(error(line_number, "unexpected '='"));
    }
    let Some((name, value)) = line.split_once('=') else {
        return Err(error(line_number, "expected 'ninja_dyndep_version = ...'"));
    };
    if name.trim() != "ninja_dyndep_version" {
        return Err(error(line_number, "expected 'ninja_dyndep_version = ...'"));
    }
    if !terminated {
        return Err(error(line_number, "unexpected EOF"));
    }
    let value = value.trim();
    let numeric = value.split('-').next().unwrap_or_default();
    let mut components = numeric.split('.');
    let major = components.next().and_then(|part| part.parse::<u32>().ok());
    let minor = components
        .next()
        .map(|part| part.parse::<u32>().ok())
        .unwrap_or(Some(0));
    if major != Some(1) || minor != Some(0) {
        return Err(error(
            line_number,
            format!("unsupported 'ninja_dyndep_version = {value}'"),
        ));
    }
    Ok(())
}

fn dynamic_node(graph: &mut Graph, path: &str) -> NodeRef {
    let mut path = xasprintf(format_args!("{path}"));
    canonpath(&mut path);
    mknode(graph, path)
}

fn parse_build(
    graph: &mut Graph,
    file: &mut DyndepFile,
    line_number: usize,
    line: &str,
    terminated: bool,
) -> Result<Dyndeps, DyndepError> {
    let fields = tokens(line);
    if fields.len() == 1 && !terminated {
        return Err(error(line_number, "unexpected EOF"));
    }
    let mut index = 1;
    let Some(output) = fields.get(index) else {
        return Err(error(line_number, "expected path"));
    };
    if output == ":" {
        return Err(error(line_number, "expected path"));
    }
    index += 1;
    let Some(node) = nodeget(graph, output.as_bytes()) else {
        return Err(error(
            line_number,
            format!("no build statement exists for '{output}'"),
        ));
    };
    let Some(edge) = node.borrow().gen.as_ref().and_then(|edge| edge.upgrade()) else {
        return Err(error(
            line_number,
            format!("no build statement exists for '{output}'"),
        ));
    };
    let identity = edge_identity(&edge);
    if file.contains_key(&identity) {
        return Err(error(
            line_number,
            format!("multiple statements for '{output}'"),
        ));
    }

    if index >= fields.len() && !terminated {
        return Err(error(line_number, "unexpected EOF"));
    }
    if fields
        .get(index)
        .is_some_and(|field| field != "|" && field != ":")
    {
        return Err(error(line_number, "explicit outputs not supported"));
    }
    let mut implicit_outputs = Vec::new();
    if fields.get(index).is_some_and(|field| field == "|") {
        index += 1;
        while fields.get(index).is_some_and(|field| field != ":") {
            implicit_outputs.push(fields[index].clone());
            index += 1;
        }
    }
    if fields.get(index).is_none() {
        return Err(error(line_number, "unexpected EOF"));
    }
    index += 1;

    if fields.get(index).is_none() || fields[index] != "dyndep" {
        return Err(error(line_number, "expected build command name 'dyndep'"));
    }
    index += 1;
    if fields.get(index).is_some_and(|field| field == "|@") {
        return Err(error(line_number, "expected newline, got '|@'"));
    }
    if fields
        .get(index)
        .is_some_and(|field| field != "|" && field != "||")
    {
        return Err(error(line_number, "explicit inputs not supported"));
    }
    let mut implicit_inputs = Vec::new();
    if fields.get(index).is_some_and(|field| field == "|") {
        index += 1;
        while fields.get(index).is_some_and(|field| field != "||") {
            implicit_inputs.push(fields[index].clone());
            index += 1;
        }
    }
    if fields.get(index).is_some_and(|field| field == "||") {
        return Err(error(line_number, "order-only inputs not supported"));
    }
    if !terminated {
        return Err(error(line_number, "unexpected EOF"));
    }

    Ok(Dyndeps {
        used: false,
        restat: false,
        implicit_inputs: implicit_inputs
            .iter()
            .map(|path| dynamic_node(graph, path))
            .collect(),
        implicit_outputs: implicit_outputs
            .iter()
            .map(|path| dynamic_node(graph, path))
            .collect(),
    })
}

pub fn parse_dyndep(input: &str, graph: &mut Graph) -> Result<DyndepFile, DyndepError> {
    let records = input
        .split_inclusive('\n')
        .enumerate()
        .map(|(index, record)| {
            let terminated = record.ends_with('\n');
            let line = record
                .strip_suffix('\n')
                .unwrap_or(record)
                .strip_suffix('\r')
                .unwrap_or_else(|| record.strip_suffix('\n').unwrap_or(record));
            (index + 1, line.to_string(), terminated)
        })
        .collect::<Vec<_>>();
    if records.is_empty() {
        return Err(error(1, "expected 'ninja_dyndep_version = ...'"));
    }

    let mut have_version = false;
    let mut file = DyndepFile::new();
    let mut index = 0;
    while index < records.len() {
        let (line_number, line, terminated) = &records[index];
        if line.is_empty() || line.starts_with('#') {
            index += 1;
            continue;
        }
        if !have_version {
            if line.starts_with("build") {
                return Err(error(*line_number, "expected 'ninja_dyndep_version = ...'"));
            }
            parse_version(*line_number, line, *terminated)?;
            have_version = true;
            index += 1;
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            return Err(error(*line_number, "unexpected indent"));
        }
        if !line.starts_with("build") {
            return Err(error(*line_number, "unexpected identifier"));
        }

        let output_name = tokens(line).get(1).cloned();
        let mut dyndeps = parse_build(graph, &mut file, *line_number, line, *terminated)?;
        if records
            .get(index + 1)
            .is_some_and(|(_, line, _)| line.starts_with(char::is_whitespace))
        {
            let (binding_line, binding, _) = &records[index + 1];
            let Some((name, value)) = binding.trim().split_once('=') else {
                return Err(error(*binding_line, "expected variable binding"));
            };
            if name.trim() != "restat" {
                return Err(error(*binding_line, "binding is not 'restat'"));
            }
            dyndeps.restat = !value.trim().is_empty();
            index += 1;
        }
        let output = output_name.expect("validated build output");
        let node = nodeget(graph, output.as_bytes()).expect("validated output node");
        let edge = node
            .borrow()
            .gen
            .as_ref()
            .and_then(|edge| edge.upgrade())
            .expect("validated output edge");
        file.insert(edge_identity(&edge), dyndeps);
        index += 1;
    }
    if !have_version {
        return Err(error(1, "expected 'ninja_dyndep_version = ...'"));
    }
    Ok(file)
}

pub fn load_dyndep(graph: &mut Graph, dyndep: &NodeRef) -> Result<(), String> {
    let path = {
        let dyndep = dyndep.borrow();
        String::from_utf8_lossy(&dyndep.path.s[..dyndep.path.n]).into_owned()
    };
    let input = fs::read_to_string(&path).map_err(|error| format!("loading '{path}': {error}"))?;
    let file = parse_dyndep(&input, graph).map_err(|error| error.to_string())?;
    let expected_edges = graph
        .edges
        .iter()
        .filter(|edge| {
            edge.borrow()
                .dyndep
                .as_ref()
                .is_some_and(|candidate| Rc::ptr_eq(candidate, dyndep))
        })
        .cloned()
        .collect::<Vec<_>>();

    for edge in &expected_edges {
        if !file.contains_key(&edge_identity(edge)) {
            let output = edge
                .borrow()
                .out
                .first()
                .map(|output| {
                    let output = output.borrow();
                    String::from_utf8_lossy(&output.path.s[..output.path.n]).into_owned()
                })
                .unwrap_or_default();
            return Err(format!(
                "'{output}' not mentioned in its dyndep file '{path}'"
            ));
        }
    }

    for identity in file.keys() {
        let edge = graph
            .edges
            .iter()
            .find(|edge| edge_identity(edge) == *identity)
            .expect("dyndep parser only records graph edges");
        let belongs_to_file = edge
            .borrow()
            .dyndep
            .as_ref()
            .is_some_and(|candidate| Rc::ptr_eq(candidate, dyndep));
        if !belongs_to_file {
            let output = edge
                .borrow()
                .out
                .first()
                .map(|output| {
                    let output = output.borrow();
                    String::from_utf8_lossy(&output.path.s[..output.path.n]).into_owned()
                })
                .unwrap_or_default();
            return Err(format!(
                "dyndep file '{path}' mentions output '{output}' whose build statement does not have a dyndep binding for the file"
            ));
        }
    }

    for (identity, dyndeps) in file {
        let edge = graph
            .edges
            .iter()
            .find(|edge| edge_identity(edge) == identity)
            .cloned()
            .expect("dyndep parser only records graph edges");
        for output in &dyndeps.implicit_outputs {
            if let Some(generator) = output.borrow().gen.as_ref().and_then(|edge| edge.upgrade()) {
                if !Rc::ptr_eq(&generator, &edge) {
                    let output = output.borrow();
                    return Err(format!(
                        "multiple rules generate {}",
                        String::from_utf8_lossy(&output.path.s[..output.path.n])
                    ));
                }
            }
        }
        for output in dyndeps.implicit_outputs {
            output.borrow_mut().gen = Some(Rc::downgrade(&edge));
            edge.borrow_mut().out.push(output);
        }
        edgeadddeps(&edge, &dyndeps.implicit_inputs);
        if dyndeps.restat {
            edge.borrow_mut()
                .bindings
                .insert("restat".into(), xasprintf(format_args!("1")));
        }
    }
    dyndep.borrow_mut().dyndep_pending = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::graph::{graphinit, mkedge, mknode};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_LOAD_TEST: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> Graph {
        let mut graph = graphinit();
        let edge = mkedge(&mut graph, mkenv(None));
        for path in ["out", "otherout"] {
            let output = mknode(&mut graph, xasprintf(format_args!("{path}")));
            output.borrow_mut().gen = Some(Rc::downgrade(&edge));
            edge.borrow_mut().out.push(output);
        }
        edge.borrow_mut().outimpidx = 2;
        graph
    }

    fn add_output(graph: &mut Graph, path: &str) {
        let edge = mkedge(graph, mkenv(None));
        let output = mknode(graph, xasprintf(format_args!("{path}")));
        output.borrow_mut().gen = Some(Rc::downgrade(&edge));
        edge.borrow_mut().out.push(output);
        edge.borrow_mut().outimpidx = 1;
    }

    fn parse(input: &str) -> Result<(Graph, DyndepFile), DyndepError> {
        let mut graph = fixture();
        let file = parse_dyndep(input, &mut graph)?;
        Ok((graph, file))
    }

    fn paths(nodes: &[NodeRef]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| {
                let node = node.borrow();
                String::from_utf8_lossy(&node.path.s[..node.path.n]).into_owned()
            })
            .collect()
    }

    fn entry_for<'a>(graph: &Graph, file: &'a DyndepFile, output: &str) -> &'a Dyndeps {
        let node = nodeget(graph, output.as_bytes()).unwrap();
        let edge = node
            .borrow()
            .gen
            .as_ref()
            .and_then(|edge| edge.upgrade())
            .unwrap();
        file.get(&edge_identity(&edge)).unwrap()
    }

    macro_rules! valid_version_case {
        ($name:ident, $input:expr) => {
            #[test]
            fn $name() {
                assert!(parse($input).is_ok());
            }
        };
    }

    valid_version_case!(ninja_dyndep_parser_version_1, "ninja_dyndep_version = 1\n");
    valid_version_case!(
        ninja_dyndep_parser_version_1_extra,
        "ninja_dyndep_version = 1-extra\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_version_1_0,
        "ninja_dyndep_version = 1.0\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_version_1_0_extra,
        "ninja_dyndep_version = 1.0-extra\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_comment_before_version,
        "# comment\nninja_dyndep_version = 1\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_blank_before_version,
        "\nninja_dyndep_version = 1\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_version_crlf,
        "ninja_dyndep_version = 1\r\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_comment_version_crlf,
        "# comment\r\nninja_dyndep_version = 1\r\n"
    );
    valid_version_case!(
        ninja_dyndep_parser_blank_version_crlf,
        "\r\nninja_dyndep_version = 1\r\n"
    );

    macro_rules! invalid_case {
        ($name:ident, $input:expr, $message:expr) => {
            #[test]
            fn $name() {
                let error = match parse($input) {
                    Err(error) => error,
                    Ok(_) => panic!("invalid dyndep input unexpectedly parsed"),
                };
                assert!(
                    error.message.contains($message),
                    "expected {:?} in {:?}",
                    $message,
                    error.message
                );
            }
        };
    }

    invalid_case!(
        ninja_dyndep_parser_empty,
        "",
        "expected 'ninja_dyndep_version"
    );
    invalid_case!(
        ninja_dyndep_parser_version_unexpected_eof,
        "ninja_dyndep_version = 1.0",
        "unexpected EOF"
    );
    invalid_case!(
        ninja_dyndep_parser_unsupported_version_0,
        "ninja_dyndep_version = 0\n",
        "unsupported"
    );
    invalid_case!(
        ninja_dyndep_parser_unsupported_version_1_1,
        "ninja_dyndep_version = 1.1\n",
        "unsupported"
    );
    invalid_case!(
        ninja_dyndep_parser_duplicate_version,
        "ninja_dyndep_version = 1\nninja_dyndep_version = 1\n",
        "unexpected identifier"
    );
    invalid_case!(
        ninja_dyndep_parser_missing_version_other_variable,
        "not_ninja_dyndep_version = 1\n",
        "expected 'ninja_dyndep_version"
    );
    invalid_case!(
        ninja_dyndep_parser_missing_version_build,
        "build out: dyndep\n",
        "expected 'ninja_dyndep_version"
    );
    invalid_case!(
        ninja_dyndep_parser_unexpected_equal,
        "= 1\n",
        "unexpected '='"
    );
    invalid_case!(
        ninja_dyndep_parser_unexpected_indent,
        " = 1\n",
        "unexpected indent"
    );
    invalid_case!(
        ninja_dyndep_parser_duplicate_output,
        "ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out: dyndep\n",
        "multiple statements for 'out'"
    );
    invalid_case!(
        ninja_dyndep_parser_duplicate_output_through_other,
        "ninja_dyndep_version = 1\nbuild out: dyndep\nbuild otherout: dyndep\n",
        "multiple statements for 'otherout'"
    );
    invalid_case!(
        ninja_dyndep_parser_no_output_eof,
        "ninja_dyndep_version = 1\nbuild",
        "unexpected EOF"
    );
    invalid_case!(
        ninja_dyndep_parser_no_output_before_colon,
        "ninja_dyndep_version = 1\nbuild :\n",
        "expected path"
    );
    invalid_case!(
        ninja_dyndep_parser_output_has_no_statement,
        "ninja_dyndep_version = 1\nbuild missing: dyndep\n",
        "no build statement exists"
    );
    invalid_case!(
        ninja_dyndep_parser_output_eof,
        "ninja_dyndep_version = 1\nbuild out",
        "unexpected EOF"
    );
    invalid_case!(
        ninja_dyndep_parser_output_no_rule,
        "ninja_dyndep_version = 1\nbuild out:",
        "expected build command name"
    );
    invalid_case!(
        ninja_dyndep_parser_output_bad_rule,
        "ninja_dyndep_version = 1\nbuild out: touch",
        "expected build command name"
    );
    invalid_case!(
        ninja_dyndep_parser_build_eof,
        "ninja_dyndep_version = 1\nbuild out: dyndep",
        "unexpected EOF"
    );
    invalid_case!(
        ninja_dyndep_parser_explicit_output,
        "ninja_dyndep_version = 1\nbuild out exp: dyndep\n",
        "explicit outputs not supported"
    );
    invalid_case!(
        ninja_dyndep_parser_explicit_input,
        "ninja_dyndep_version = 1\nbuild out: dyndep exp\n",
        "explicit inputs not supported"
    );
    invalid_case!(
        ninja_dyndep_parser_order_only_input,
        "ninja_dyndep_version = 1\nbuild out: dyndep ||\n",
        "order-only inputs not supported"
    );
    invalid_case!(
        ninja_dyndep_parser_bad_binding,
        "ninja_dyndep_version = 1\nbuild out: dyndep\n  not_restat = 1\n",
        "binding is not 'restat'"
    );
    invalid_case!(
        ninja_dyndep_parser_restat_twice,
        "ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = 1\n  restat = 1\n",
        "unexpected indent"
    );

    #[test]
    fn ninja_dyndep_parser_no_implicit_dependencies() {
        let (graph, file) = parse("ninja_dyndep_version = 1\nbuild out: dyndep\n").unwrap();
        let entry = entry_for(&graph, &file, "out");
        assert!(!entry.restat);
        assert!(entry.implicit_inputs.is_empty());
        assert!(entry.implicit_outputs.is_empty());
    }

    #[test]
    fn ninja_dyndep_parser_empty_implicit_sections() {
        let (graph, file) = parse("ninja_dyndep_version = 1\nbuild out | : dyndep |\n").unwrap();
        let entry = entry_for(&graph, &file, "out");
        assert!(entry.implicit_inputs.is_empty());
        assert!(entry.implicit_outputs.is_empty());
    }

    #[test]
    fn ninja_dyndep_parser_one_implicit_input() {
        let (graph, file) = parse("ninja_dyndep_version = 1\nbuild out: dyndep | impin\n").unwrap();
        assert_eq!(
            paths(&entry_for(&graph, &file, "out").implicit_inputs),
            ["impin"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_multiple_implicit_inputs() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out: dyndep | impin1 impin2\n").unwrap();
        assert_eq!(
            paths(&entry_for(&graph, &file, "out").implicit_inputs),
            ["impin1", "impin2"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_one_implicit_output() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out | impout: dyndep\n").unwrap();
        assert_eq!(
            paths(&entry_for(&graph, &file, "out").implicit_outputs),
            ["impout"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_multiple_implicit_outputs() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out | impout1 impout2 : dyndep\n").unwrap();
        assert_eq!(
            paths(&entry_for(&graph, &file, "out").implicit_outputs),
            ["impout1", "impout2"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_implicit_inputs_and_outputs() {
        let (graph, file) = parse(
            "ninja_dyndep_version = 1\nbuild out | impout1 impout2: dyndep | impin1 impin2\n",
        )
        .unwrap();
        let entry = entry_for(&graph, &file, "out");
        assert_eq!(paths(&entry.implicit_outputs), ["impout1", "impout2"]);
        assert_eq!(paths(&entry.implicit_inputs), ["impin1", "impin2"]);
    }

    #[test]
    fn ninja_dyndep_parser_restat_binding() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = 1\n").unwrap();
        assert!(entry_for(&graph, &file, "out").restat);
    }

    #[test]
    fn ninja_dyndep_parser_other_output_of_same_edge() {
        let (graph, file) = parse("ninja_dyndep_version = 1\nbuild otherout: dyndep\n").unwrap();
        assert!(file
            .get(&edge_identity(
                &nodeget(&graph, b"out")
                    .unwrap()
                    .borrow()
                    .gen
                    .as_ref()
                    .and_then(|edge| edge.upgrade())
                    .unwrap()
            ))
            .is_some());
    }

    #[test]
    fn ninja_dyndep_parser_multiple_edges() {
        let mut graph = fixture();
        add_output(&mut graph, "out2");
        let file = parse_dyndep(
            "ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out2: dyndep\n  restat = 1\n",
            &mut graph,
        )
        .unwrap();
        assert_eq!(file.len(), 2);
        assert!(!entry_for(&graph, &file, "out").restat);
        assert!(entry_for(&graph, &file, "out2").restat);
    }

    fn load_fixture(manifest: &str, dyndep: Option<&str>) -> (Graph, NodeRef, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "samurai-dyndep-load-{}-{}",
            std::process::id(),
            NEXT_LOAD_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let dyndep_path = directory.join("dd");
        let manifest_path = directory.join("build.ninja");
        fs::write(
            &manifest_path,
            format!(
                "rule r\n  command = unused\n{}",
                manifest.replace("$dd", &dyndep_path.to_string_lossy())
            ),
        )
        .unwrap();
        if let Some(dyndep) = dyndep {
            fs::write(&dyndep_path, dyndep).unwrap();
        }
        let mut graph = graphinit();
        let mut parser = crate::parse::parseinit();
        let mut state = crate::env::envinit();
        crate::parse::parse(
            manifest_path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root.clone(),
            &mut state,
        )
        .unwrap();
        let node = nodeget(&graph, dyndep_path.to_string_lossy().as_bytes()).unwrap();
        (graph, node, directory)
    }

    fn node_paths(nodes: &[NodeRef]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| {
                let node = node.borrow();
                String::from_utf8_lossy(&node.path.s[..node.path.n]).into_owned()
            })
            .collect()
    }

    #[test]
    fn ninja_graph_dyndep_load_trivial() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out: r in || $dd\n  dyndep = $dd\n",
            Some("ninja_dyndep_version = 1\nbuild out: dyndep\n"),
        );
        assert!(dyndep.borrow().dyndep_pending);
        load_dyndep(&mut graph, &dyndep).unwrap();
        assert!(!dyndep.borrow().dyndep_pending);
        let edge = nodeget(&graph, b"out")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        assert_eq!(node_paths(&edge.borrow().out), ["out"]);
        assert_eq!(edge.borrow().input.len(), 2);
        assert_eq!(edge.borrow().inimpidx, 1);
        assert_eq!(edge.borrow().inorderidx, 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_implicit() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1: r in || $dd\n  dyndep = $dd\nbuild out2: r in\n",
            Some("ninja_dyndep_version = 1\nbuild out1: dyndep | out2\n"),
        );
        load_dyndep(&mut graph, &dyndep).unwrap();
        let edge = nodeget(&graph, b"out1")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        let inputs = node_paths(&edge.borrow().input);
        assert_eq!(inputs[0..2], ["in", "out2"]);
        assert_eq!(edge.borrow().inimpidx, 1);
        assert_eq!(edge.borrow().inorderidx, 2);
        assert_eq!(nodeget(&graph, b"out2").unwrap().borrow().uses.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_missing_file() {
        let (mut graph, dyndep, directory) =
            load_fixture("build out: r in || $dd\n  dyndep = $dd\n", None);
        assert!(load_dyndep(&mut graph, &dyndep)
            .unwrap_err()
            .contains("loading"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_missing_entry() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out: r in || $dd\n  dyndep = $dd\n",
            Some("ninja_dyndep_version = 1\n"),
        );
        assert!(load_dyndep(&mut graph, &dyndep)
            .unwrap_err()
            .contains("'out' not mentioned"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_extra_entry() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out: r in || $dd\n  dyndep = $dd\nbuild out2: r in || $dd\n",
            Some("ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out2: dyndep\n"),
        );
        assert!(load_dyndep(&mut graph, &dyndep)
            .unwrap_err()
            .contains("does not have a dyndep binding"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_rejects_duplicate_dynamic_output() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1 | shared: r in1\nbuild out2: r in2 || $dd\n  dyndep = $dd\n",
            Some("ninja_dyndep_version = 1\nbuild out2 | shared: dyndep\n"),
        );
        assert_eq!(
            load_dyndep(&mut graph, &dyndep),
            Err("multiple rules generate shared".into())
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_multiple_and_restat() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1: r in1 || $dd\n  dyndep = $dd\nbuild out2: r in2 || $dd\n  dyndep = $dd\n",
            Some(
                "ninja_dyndep_version = 1\nbuild out1 | out1imp: dyndep | in1imp\nbuild out2: dyndep | in2imp\n  restat = 1\n",
            ),
        );
        load_dyndep(&mut graph, &dyndep).unwrap();
        let edge1 = nodeget(&graph, b"out1")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        let edge2 = nodeget(&graph, b"out2")
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        assert_eq!(node_paths(&edge1.borrow().out), ["out1", "out1imp"]);
        assert_eq!(node_paths(&edge1.borrow().input)[0..2], ["in1", "in1imp"]);
        assert_eq!(node_paths(&edge2.borrow().input)[0..2], ["in2", "in2imp"]);
        let restat = crate::env::edgevar(&edge2, "restat", false).unwrap();
        assert_eq!(&restat.s[..restat.n], b"1");
        assert!(Rc::ptr_eq(
            &nodeget(&graph, b"out1imp")
                .unwrap()
                .borrow()
                .gen
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap(),
            &edge1
        ));
        fs::remove_dir_all(directory).unwrap();
    }
}
