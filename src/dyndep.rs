//! Ninja version-1 dynamic dependency file parser.

use crate::graph::{edgeadddeps, mknode, nodeget, EdgeId, Graph, NodeId};
use crate::scan::{
    scanchar, scanfrombytes, scanindent, scankeyword, scanname, scannewline, scanpipe, scanstring,
    Scanner, Token,
};
use crate::util::{canonpath, xasprintf, BString, ByteSlice, ByteVec, EvalPart, EvalString};
use std::fmt;
use std::fs;

#[derive(Clone, Default)]
pub struct Dyndeps {
    pub restat: bool,
    pub implicit_inputs: Vec<NodeId>,
    pub implicit_outputs: Vec<NodeId>,
}

#[derive(Default)]
pub struct DyndepFile {
    slots: Vec<Option<Dyndeps>>,
    edges: Vec<EdgeId>,
}

impl DyndepFile {
    fn insert(&mut self, edge: EdgeId, dyndeps: Dyndeps) -> Result<(), Dyndeps> {
        if self.slots.len() <= edge.index() {
            self.slots.resize_with(edge.index() + 1, || None);
        }
        if self.slots[edge.index()].is_some() {
            return Err(dyndeps);
        }
        self.slots[edge.index()] = Some(dyndeps);
        self.edges.push(edge);
        Ok(())
    }

    pub fn get(&self, edge: &EdgeId) -> Option<&Dyndeps> {
        self.slots.get(edge.index()).and_then(Option::as_ref)
    }

    fn iter(&self) -> impl Iterator<Item = (EdgeId, &Dyndeps)> {
        self.edges
            .iter()
            .map(|edge| (*edge, self.get(edge).expect("indexed dyndep entry")))
    }
}

impl IntoIterator for DyndepFile {
    type Item = (EdgeId, Dyndeps);
    type IntoIter = DyndepIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        DyndepIntoIter {
            slots: self.slots,
            edges: self.edges.into_iter(),
        }
    }
}

pub struct DyndepIntoIter {
    slots: Vec<Option<Dyndeps>>,
    edges: std::vec::IntoIter<EdgeId>,
}

impl Iterator for DyndepIntoIter {
    type Item = (EdgeId, Dyndeps);

    fn next(&mut self) -> Option<Self::Item> {
        let edge = self.edges.next()?;
        let dyndeps = self.slots[edge.index()]
            .take()
            .expect("indexed dyndep entry");
        Some((edge, dyndeps))
    }
}

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

fn scanner_error(scanner: &Scanner, message: String) -> DyndepError {
    let prefix = format!(
        "{}:{}:{}: ",
        scanner.path.display(),
        scanner.line,
        scanner.col
    );
    error(
        scanner.line,
        message.strip_prefix(&prefix).unwrap_or(&message).to_owned(),
    )
}

macro_rules! scan {
    ($scanner:expr, $operation:expr) => {
        $operation.map_err(|message| scanner_error($scanner, message))?
    };
}

fn evaluate_empty(value: EvalString) -> BString {
    let capacity = value
        .parts
        .iter()
        .map(|part| match part {
            EvalPart::Literal(value) => value.len(),
            EvalPart::Variable(_) => 0,
        })
        .sum();
    let mut result = BString::from(Vec::with_capacity(capacity));
    for part in value.parts {
        if let EvalPart::Literal(value) = part {
            result.push_str(value.as_bytes());
        }
    }
    result
}

fn parse_version(scanner: &mut Scanner) -> Result<(), DyndepError> {
    let line = scanner.line;
    let name = scanner
        .take_variable()
        .ok_or_else(|| error(line, "expected 'ninja_dyndep_version = ...'"))?;
    if name != "ninja_dyndep_version" {
        return Err(error(line, "expected 'ninja_dyndep_version = ...'"));
    }
    scan!(scanner, scanchar(scanner, '='));
    let value = scan!(scanner, scanstring(scanner, false)).unwrap_or_default();
    scan!(scanner, scannewline(scanner));
    let value = evaluate_empty(value);
    let text = value.to_str_lossy();
    let numeric = text.split('-').next().unwrap_or_default();
    let mut components = numeric.split('.');
    let major = components.next().and_then(|part| part.parse::<u32>().ok());
    let minor = components
        .next()
        .map(|part| part.parse::<u32>().ok())
        .unwrap_or(Some(0));
    if major != Some(1) || minor != Some(0) {
        return Err(error(
            line,
            format!("unsupported 'ninja_dyndep_version = {text}'"),
        ));
    }
    Ok(())
}

fn dynamic_node(graph: &mut Graph, path: EvalString, line: usize) -> Result<NodeId, DyndepError> {
    let mut path = evaluate_empty(path);
    if path.is_empty() {
        return Err(error(line, "empty path"));
    }
    canonpath(&mut path);
    Ok(mknode(graph, path))
}

fn parse_build(
    graph: &mut Graph,
    file: &mut DyndepFile,
    scanner: &mut Scanner,
) -> Result<(), DyndepError> {
    let line = scanner.line;
    let Some(output) = scan!(scanner, scanstring(scanner, true)) else {
        return Err(error(
            line,
            if scanner.current().is_none() {
                "unexpected EOF"
            } else {
                "expected path"
            },
        ));
    };
    let mut output = evaluate_empty(output);
    if output.is_empty() {
        return Err(error(line, "empty path"));
    }
    canonpath(&mut output);
    let Some(node) = nodeget(graph, output.as_bytes()) else {
        return Err(error(
            line,
            format!("no build statement exists for '{}'", output.to_str_lossy()),
        ));
    };
    let Some(edge) = graph.node(node).gen else {
        return Err(error(
            line,
            format!("no build statement exists for '{}'", output.to_str_lossy()),
        ));
    };
    if file.get(&edge).is_some() {
        return Err(error(
            line,
            format!("multiple statements for '{}'", output.to_str_lossy()),
        ));
    }

    if scan!(scanner, scanstring(scanner, true)).is_some() {
        return Err(error(line, "explicit outputs not supported"));
    }
    let mut implicit_outputs = Vec::new();
    if scan!(scanner, scanpipe(scanner, 1)) != 0 {
        while let Some(path) = scan!(scanner, scanstring(scanner, true)) {
            implicit_outputs.push(dynamic_node(graph, path, line)?);
        }
    }
    if scanner.current().is_none() {
        return Err(error(line, "unexpected EOF"));
    }
    scan!(scanner, scanchar(scanner, ':'));
    let rule =
        scanname(scanner).map_err(|_| error(line, "expected build command name 'dyndep'"))?;
    if rule != "dyndep" {
        return Err(error(line, "expected build command name 'dyndep'"));
    }

    if scan!(scanner, scanstring(scanner, true)).is_some() {
        return Err(error(line, "explicit inputs not supported"));
    }
    let mut implicit_inputs = Vec::new();
    match scan!(scanner, scanpipe(scanner, 1 | 2 | 4)) {
        1 => {
            while let Some(path) = scan!(scanner, scanstring(scanner, true)) {
                implicit_inputs.push(dynamic_node(graph, path, line)?);
            }
            match scan!(scanner, scanpipe(scanner, 2 | 4)) {
                2 => return Err(error(line, "order-only inputs not supported")),
                4 => return Err(error(line, "expected newline, got '|@'")),
                _ => {}
            }
        }
        2 => return Err(error(line, "order-only inputs not supported")),
        4 => return Err(error(line, "expected newline, got '|@'")),
        _ => {}
    }
    scan!(scanner, scannewline(scanner));

    let mut dyndeps = Dyndeps {
        restat: false,
        implicit_inputs,
        implicit_outputs,
    };
    if scan!(scanner, scanindent(scanner)) {
        let binding_line = scanner.line;
        let name = scan!(scanner, scanname(scanner));
        scan!(scanner, scanchar(scanner, '='));
        let value = scan!(scanner, scanstring(scanner, false)).unwrap_or_default();
        scan!(scanner, scannewline(scanner));
        if name != "restat" {
            return Err(error(binding_line, "binding is not 'restat'"));
        }
        dyndeps.restat = !evaluate_empty(value).is_empty();
    }
    file.insert(edge, dyndeps).map_err(|_| {
        error(
            line,
            format!("multiple statements for '{}'", output.to_str_lossy()),
        )
    })
}

pub fn parse_dyndep(input: Vec<u8>, graph: &mut Graph) -> Result<DyndepFile, DyndepError> {
    let mut scanner = scanfrombytes("input", input);
    let mut have_version = false;
    let mut file = DyndepFile::default();
    loop {
        if scanner.current() == Some(b'=') {
            return Err(error(scanner.line, "unexpected '='"));
        }
        match scan!(&scanner, scankeyword(&mut scanner)) {
            Some(Token::Build) if have_version => parse_build(graph, &mut file, &mut scanner)?,
            Some(Token::Build) => {
                return Err(error(scanner.line, "expected 'ninja_dyndep_version = ...'"));
            }
            Some(Token::Variable) if !have_version => {
                parse_version(&mut scanner)?;
                have_version = true;
            }
            Some(_) if have_version => return Err(error(scanner.line, "unexpected identifier")),
            Some(_) => {
                return Err(error(scanner.line, "expected 'ninja_dyndep_version = ...'"));
            }
            None if have_version => return Ok(file),
            None => {
                return Err(error(scanner.line, "expected 'ninja_dyndep_version = ...'"));
            }
        }
    }
}

pub fn load_dyndep(graph: &mut Graph, dyndep: NodeId) -> Result<(), String> {
    let path = graph.node(dyndep).path.clone();
    let input = fs::read(path.to_path().expect("byte paths are valid on Unix"))
        .map_err(|error| format!("loading '{path}': {error}"))?;
    let file = parse_dyndep(input, graph).map_err(|error| error.to_string())?;

    for edge in graph
        .node(dyndep)
        .uses
        .iter()
        .copied()
        .filter(|edge| graph.edge(*edge).dyndep == Some(dyndep))
    {
        if file.get(&edge).is_none() {
            let output = graph
                .edge(edge)
                .out
                .first()
                .map(|output| {
                    let output = graph.node(*output);
                    String::from_utf8_lossy(output.path.as_bytes()).into_owned()
                })
                .unwrap_or_default();
            return Err(format!(
                "'{output}' not mentioned in its dyndep file '{path}'"
            ));
        }
    }

    for (edge, _) in file.iter() {
        let belongs_to_file = graph.edge(edge).dyndep == Some(dyndep);
        if !belongs_to_file {
            let output = graph
                .edge(edge)
                .out
                .first()
                .map(|output| {
                    let output = graph.node(*output);
                    String::from_utf8_lossy(output.path.as_bytes()).into_owned()
                })
                .unwrap_or_default();
            return Err(format!(
                "dyndep file '{path}' mentions output '{output}' whose build statement does not have a dyndep binding for the file"
            ));
        }
    }

    for (edge, dyndeps) in file {
        for output in &dyndeps.implicit_outputs {
            if let Some(generator) = graph.node(*output).gen {
                if generator != edge {
                    let output = graph.node(*output);
                    return Err(format!(
                        "multiple rules generate {}",
                        String::from_utf8_lossy(output.path.as_bytes())
                    ));
                }
            }
        }
        for output in dyndeps.implicit_outputs {
            graph.node_mut(output).gen = Some(edge);
            graph.edge_mut(edge).out.push(output);
        }
        edgeadddeps(graph, edge, &dyndeps.implicit_inputs);
        if dyndeps.restat {
            graph
                .edge_mut(edge)
                .bindings
                .insert("restat".into(), xasprintf(format_args!("1")));
        }
    }
    graph.node_mut(dyndep).dyndep_pending = false;
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
        let root = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        for path in ["out", "otherout"] {
            let output = mknode(&mut graph, xasprintf(format_args!("{path}")));
            graph.node_mut(output).gen = Some(edge);
            graph.edge_mut(edge).out.push(output);
        }
        graph.edge_mut(edge).outimpidx = 2;
        graph
    }

    fn add_output(graph: &mut Graph, path: &str) {
        let root = mkenv(graph, None);
        let edge = mkedge(graph, root);
        let output = mknode(graph, xasprintf(format_args!("{path}")));
        graph.node_mut(output).gen = Some(edge);
        graph.edge_mut(edge).out.push(output);
        graph.edge_mut(edge).outimpidx = 1;
    }

    fn parse(input: &str) -> Result<(Graph, DyndepFile), DyndepError> {
        let mut graph = fixture();
        let file = parse_dyndep(input.as_bytes().to_vec(), &mut graph)?;
        Ok((graph, file))
    }

    fn paths(graph: &Graph, nodes: &[NodeId]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| {
                let node = graph.node(*node);
                String::from_utf8_lossy(node.path.as_bytes()).into_owned()
            })
            .collect()
    }

    fn entry_for<'a>(graph: &Graph, file: &'a DyndepFile, output: &str) -> &'a Dyndeps {
        let node = nodeget(graph, output.as_bytes()).unwrap();
        let edge = graph.node(node).gen.unwrap();
        file.get(&edge).unwrap()
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
            paths(&graph, &entry_for(&graph, &file, "out").implicit_inputs),
            ["impin"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_multiple_implicit_inputs() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out: dyndep | impin1 impin2\n").unwrap();
        assert_eq!(
            paths(&graph, &entry_for(&graph, &file, "out").implicit_inputs),
            ["impin1", "impin2"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_one_implicit_output() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out | impout: dyndep\n").unwrap();
        assert_eq!(
            paths(&graph, &entry_for(&graph, &file, "out").implicit_outputs),
            ["impout"]
        );
    }

    #[test]
    fn ninja_dyndep_parser_multiple_implicit_outputs() {
        let (graph, file) =
            parse("ninja_dyndep_version = 1\nbuild out | impout1 impout2 : dyndep\n").unwrap();
        assert_eq!(
            paths(&graph, &entry_for(&graph, &file, "out").implicit_outputs),
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
        assert_eq!(
            paths(&graph, &entry.implicit_outputs),
            ["impout1", "impout2"]
        );
        assert_eq!(paths(&graph, &entry.implicit_inputs), ["impin1", "impin2"]);
    }

    #[test]
    fn ninja_dyndep_parser_preserves_non_utf8_paths() {
        let mut graph = fixture();
        let file = parse_dyndep(
            b"ninja_dyndep_version = 1\nbuild out | imp-\xff: dyndep | dep-\xfe\n".to_vec(),
            &mut graph,
        )
        .unwrap();
        let entry = entry_for(&graph, &file, "out");
        assert_eq!(
            graph.node(entry.implicit_outputs[0]).path.as_bytes(),
            b"imp-\xff"
        );
        assert_eq!(
            graph.node(entry.implicit_inputs[0]).path.as_bytes(),
            b"dep-\xfe"
        );
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
        let edge = graph.node(nodeget(&graph, b"out").unwrap()).gen.unwrap();
        assert!(file.get(&edge).is_some());
    }

    #[test]
    fn ninja_dyndep_parser_multiple_edges() {
        let mut graph = fixture();
        add_output(&mut graph, "out2");
        let file = parse_dyndep(
            b"ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out2: dyndep\n  restat = 1\n"
                .to_vec(),
            &mut graph,
        )
        .unwrap();
        assert_eq!(file.edges.len(), 2);
        assert!(!entry_for(&graph, &file, "out").restat);
        assert!(entry_for(&graph, &file, "out2").restat);
    }

    fn load_fixture(manifest: &str, dyndep: Option<&str>) -> (Graph, NodeId, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "ronin-dyndep-load-{}-{}",
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
        let mut state = crate::env::envinit(&mut graph);
        crate::parse::parse(
            manifest_path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        )
        .unwrap();
        let node = nodeget(&graph, dyndep_path.to_string_lossy().as_bytes()).unwrap();
        (graph, node, directory)
    }

    fn node_paths(graph: &Graph, nodes: &[NodeId]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| {
                let node = graph.node(*node);
                String::from_utf8_lossy(node.path.as_bytes()).into_owned()
            })
            .collect()
    }

    #[test]
    fn ninja_graph_dyndep_load_trivial() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out: r in || $dd\n  dyndep = $dd\n",
            Some("ninja_dyndep_version = 1\nbuild out: dyndep\n"),
        );
        assert!(graph.node(dyndep).dyndep_pending);
        load_dyndep(&mut graph, dyndep).unwrap();
        assert!(!graph.node(dyndep).dyndep_pending);
        let edge = graph.node(nodeget(&graph, b"out").unwrap()).gen.unwrap();
        assert_eq!(node_paths(&graph, &graph.edge(edge).out), ["out"]);
        assert_eq!(graph.edge(edge).input.len(), 2);
        assert_eq!(graph.edge(edge).inimpidx, 1);
        assert_eq!(graph.edge(edge).inorderidx, 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_implicit() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1: r in || $dd\n  dyndep = $dd\nbuild out2: r in\n",
            Some("ninja_dyndep_version = 1\nbuild out1: dyndep | out2\n"),
        );
        load_dyndep(&mut graph, dyndep).unwrap();
        let edge = graph.node(nodeget(&graph, b"out1").unwrap()).gen.unwrap();
        let inputs = node_paths(&graph, &graph.edge(edge).input);
        assert_eq!(inputs[0..2], ["in", "out2"]);
        assert_eq!(graph.edge(edge).inimpidx, 1);
        assert_eq!(graph.edge(edge).inorderidx, 2);
        assert_eq!(graph.node(nodeget(&graph, b"out2").unwrap()).uses.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_missing_file() {
        let (mut graph, dyndep, directory) =
            load_fixture("build out: r in || $dd\n  dyndep = $dd\n", None);
        assert!(load_dyndep(&mut graph, dyndep)
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
        assert!(load_dyndep(&mut graph, dyndep)
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
        assert!(load_dyndep(&mut graph, dyndep)
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
            load_dyndep(&mut graph, dyndep),
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
        load_dyndep(&mut graph, dyndep).unwrap();
        let edge1 = graph.node(nodeget(&graph, b"out1").unwrap()).gen.unwrap();
        let edge2 = graph.node(nodeget(&graph, b"out2").unwrap()).gen.unwrap();
        assert_eq!(
            node_paths(&graph, &graph.edge(edge1).out),
            ["out1", "out1imp"]
        );
        assert_eq!(
            node_paths(&graph, &graph.edge(edge1).input)[0..2],
            ["in1", "in1imp"]
        );
        assert_eq!(
            node_paths(&graph, &graph.edge(edge2).input)[0..2],
            ["in2", "in2imp"]
        );
        let restat = crate::env::edgevar(&graph, edge2, "restat", false).unwrap();
        assert_eq!(restat.as_bytes(), b"1");
        assert_eq!(
            graph.node(nodeget(&graph, b"out1imp").unwrap()).gen,
            Some(edge1)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
