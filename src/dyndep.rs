//! Ninja version-1 dynamic dependency file parser.

use crate::error::{ManifestError, ScanError};
#[cfg(test)]
use crate::graph::PathStyle;
use crate::graph::{edgeadddeps, mknode, nodeget, EdgeId, Graph, NodeId};
use crate::runtime::RuntimeState;
use crate::scan::{
    scanchar, scanindent, scankeyword, scanname, scannewline, scanpipe, scanstring,
    AllowedSeparators, ByteSpan, ScannedEvalPart, ScannedEvalString, Scanner, Separator, Source,
    TokenKind,
};
use crate::source::SourceSpan;
use crate::util::{canonpath, xasprintf, BString, ByteSlice};
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

#[derive(Clone)]
struct StagedPath {
    value: BString,
    span: SourceSpan,
}

#[derive(Clone, Default)]
pub(crate) struct Dyndeps {
    pub(crate) restat: bool,
    implicit_inputs: Vec<StagedPath>,
    implicit_outputs: Vec<StagedPath>,
}

pub(crate) struct DyndepEntry {
    output: StagedPath,
    dyndeps: Dyndeps,
}

// [spec:samurai:req:runtime.dyndep-transaction]
#[derive(Default)]
pub(crate) struct DyndepFile {
    slots: Vec<Option<DyndepEntry>>,
    edges: Vec<EdgeId>,
}

impl DyndepFile {
    fn insert(
        &mut self,
        edge: EdgeId,
        output: StagedPath,
        dyndeps: Dyndeps,
    ) -> Result<(), Dyndeps> {
        if self.slots.len() <= edge.index() {
            self.slots.resize_with(edge.index() + 1, || None);
        }
        if self.slots[edge.index()].is_some() {
            return Err(dyndeps);
        }
        self.slots[edge.index()] = Some(DyndepEntry { output, dyndeps });
        self.edges.push(edge);
        Ok(())
    }

    pub(crate) fn get(&self, edge: EdgeId) -> Option<&Dyndeps> {
        self.entry(edge).map(|entry| &entry.dyndeps)
    }

    fn entry(&self, edge: EdgeId) -> Option<&DyndepEntry> {
        self.slots.get(edge.index()).and_then(Option::as_ref)
    }

    fn iter(&self) -> impl Iterator<Item = (EdgeId, &DyndepEntry)> {
        self.edges
            .iter()
            .map(|edge| (*edge, self.entry(*edge).expect("indexed dyndep entry")))
    }

    pub(crate) fn implicit_outputs(&self, edge: EdgeId) -> impl Iterator<Item = &BString> {
        self.get(edge)
            .into_iter()
            .flat_map(|dyndeps| &dyndeps.implicit_outputs)
            .map(|path| &path.value)
    }
}

impl IntoIterator for DyndepFile {
    type Item = (EdgeId, DyndepEntry);
    type IntoIter = DyndepIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        DyndepIntoIter {
            slots: self.slots,
            edges: self.edges.into_iter(),
        }
    }
}

pub(crate) struct DyndepIntoIter {
    slots: Vec<Option<DyndepEntry>>,
    edges: std::vec::IntoIter<EdgeId>,
}

impl Iterator for DyndepIntoIter {
    type Item = (EdgeId, DyndepEntry);

    fn next(&mut self) -> Option<Self::Item> {
        let edge = self.edges.next()?;
        let entry = self.slots[edge.index()]
            .take()
            .expect("indexed dyndep entry");
        Some((edge, entry))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DyndepErrorKind {
    Scan(ScanError),
    ExpectedVersion,
    UnsupportedVersion(BString),
    EmptyPath,
    ExpectedPath,
    UnexpectedEof,
    NoBuildStatement(BString),
    MultipleStatements(BString),
    ExplicitOutputsUnsupported,
    ExpectedDyndepCommand,
    ExplicitInputsUnsupported,
    OrderOnlyInputsUnsupported,
    ValidationExpectedNewline,
    BindingNotRestat,
    UnexpectedEquals,
    UnexpectedIdentifier,
}

impl fmt::Display for DyndepErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scan(error) => error.diagnostic().fmt(formatter),
            Self::ExpectedVersion => formatter.write_str("expected 'ninja_dyndep_version = ...'"),
            Self::UnsupportedVersion(value) => {
                write!(formatter, "unsupported 'ninja_dyndep_version = {value}'")
            }
            Self::EmptyPath => formatter.write_str("empty path"),
            Self::ExpectedPath => formatter.write_str("expected path"),
            Self::UnexpectedEof => formatter.write_str("unexpected EOF"),
            Self::NoBuildStatement(output) => {
                write!(formatter, "no build statement exists for '{output}'")
            }
            Self::MultipleStatements(output) => {
                write!(formatter, "multiple statements for '{output}'")
            }
            Self::ExplicitOutputsUnsupported => {
                formatter.write_str("explicit outputs not supported")
            }
            Self::ExpectedDyndepCommand => {
                formatter.write_str("expected build command name 'dyndep'")
            }
            Self::ExplicitInputsUnsupported => formatter.write_str("explicit inputs not supported"),
            Self::OrderOnlyInputsUnsupported => {
                formatter.write_str("order-only inputs not supported")
            }
            Self::ValidationExpectedNewline => formatter.write_str("expected newline, got '|@'"),
            Self::BindingNotRestat => formatter.write_str("binding is not 'restat'"),
            Self::UnexpectedEquals => formatter.write_str("unexpected '='"),
            Self::UnexpectedIdentifier => formatter.write_str("unexpected identifier"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DyndepError {
    pub(crate) span: SourceSpan,
    kind: DyndepErrorKind,
}

impl fmt::Display for DyndepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}: {}",
            self.span.path().display(),
            self.span.line,
            self.kind
        )
    }
}

impl std::error::Error for DyndepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            DyndepErrorKind::Scan(error) => Some(error),
            _ => None,
        }
    }
}

fn error(scanner: &Scanner<'_>, line: usize, kind: DyndepErrorKind) -> DyndepError {
    let mut span = scanner.position();
    span.line = line;
    span.column = 0;
    DyndepError {
        span: scanner.source_span(span),
        kind,
    }
}

fn scanner_error(error: ScanError) -> DyndepError {
    DyndepError {
        span: error.span.clone(),
        kind: DyndepErrorKind::Scan(error),
    }
}

macro_rules! scan {
    ($scanner:expr, $operation:expr) => {
        $operation.map_err(scanner_error)?
    };
}

fn evaluate_empty(value: ScannedEvalString<'_>) -> BString {
    let capacity = value
        .parts
        .iter()
        .map(|part| match part {
            ScannedEvalPart::Literal(value) => value.len(),
            ScannedEvalPart::EscapedByte(_) => 1,
            ScannedEvalPart::Variable(_) => 0,
        })
        .sum();
    let mut result = Vec::with_capacity(capacity);
    for part in value.parts {
        match part {
            ScannedEvalPart::Literal(value) => result.extend_from_slice(value),
            ScannedEvalPart::EscapedByte(byte) => result.push(byte),
            ScannedEvalPart::Variable(_) => {}
        }
    }
    result.into()
}

fn parse_version(scanner: &mut Scanner<'_>, name: &str) -> Result<(), DyndepError> {
    let line = scanner.line();
    if name != "ninja_dyndep_version" {
        return Err(error(scanner, line, DyndepErrorKind::ExpectedVersion));
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
        .map_or(Some(0), |part| part.parse::<u32>().ok());
    if major != Some(1) || minor != Some(0) {
        return Err(error(
            scanner,
            line,
            DyndepErrorKind::UnsupportedVersion(value),
        ));
    }
    Ok(())
}

fn staged_path(
    scanner: &Scanner<'_>,
    path: ScannedEvalString<'_>,
    mut span: ByteSpan,
) -> Result<StagedPath, DyndepError> {
    let mut path = evaluate_empty(path);
    if path.is_empty() {
        return Err(error(scanner, span.line, DyndepErrorKind::EmptyPath));
    }
    canonpath(&mut path);
    span.byte_end = scanner.position().byte_start;
    Ok(StagedPath {
        value: path,
        span: scanner.source_span(span),
    })
}

fn parse_implicit_inputs(
    scanner: &mut Scanner<'_>,
    line: usize,
) -> Result<Vec<StagedPath>, DyndepError> {
    let mut inputs = Vec::new();
    match scan!(scanner, scanpipe(scanner, AllowedSeparators::INPUTS)) {
        Some(Separator::Implicit) => {
            loop {
                let start = scanner.position();
                let Some(path) = scan!(scanner, scanstring(scanner, true)) else {
                    break;
                };
                inputs.push(staged_path(scanner, path, start)?);
            }
            match scan!(
                scanner,
                scanpipe(scanner, AllowedSeparators::AFTER_IMPLICIT)
            ) {
                Some(Separator::OrderOnly) => {
                    return Err(error(
                        scanner,
                        line,
                        DyndepErrorKind::OrderOnlyInputsUnsupported,
                    ));
                }
                Some(Separator::Validation) => {
                    return Err(error(
                        scanner,
                        line,
                        DyndepErrorKind::ValidationExpectedNewline,
                    ));
                }
                Some(Separator::Implicit) | None => {}
            }
        }
        Some(Separator::OrderOnly) => {
            return Err(error(
                scanner,
                line,
                DyndepErrorKind::OrderOnlyInputsUnsupported,
            ));
        }
        Some(Separator::Validation) => {
            return Err(error(
                scanner,
                line,
                DyndepErrorKind::ValidationExpectedNewline,
            ));
        }
        None => {}
    }
    Ok(inputs)
}

fn parse_build(
    graph: &Graph,
    file: &mut DyndepFile,
    scanner: &mut Scanner<'_>,
) -> Result<(), DyndepError> {
    let line = scanner.line();
    let output_start = scanner.position();
    let Some(output) = scan!(scanner, scanstring(scanner, true)) else {
        return Err(error(
            scanner,
            line,
            if scanner.current().is_none() {
                DyndepErrorKind::UnexpectedEof
            } else {
                DyndepErrorKind::ExpectedPath
            },
        ));
    };
    let output = staged_path(scanner, output, output_start)?;
    let Some(node) = nodeget(graph, output.value.as_bytes()) else {
        return Err(error(
            scanner,
            line,
            DyndepErrorKind::NoBuildStatement(output.value),
        ));
    };
    let Some(edge) = graph.node(node).gen else {
        return Err(error(
            scanner,
            line,
            DyndepErrorKind::NoBuildStatement(output.value),
        ));
    };
    if file.get(edge).is_some() {
        return Err(error(
            scanner,
            line,
            DyndepErrorKind::MultipleStatements(output.value),
        ));
    }

    if scan!(scanner, scanstring(scanner, true)).is_some() {
        return Err(error(
            scanner,
            line,
            DyndepErrorKind::ExplicitOutputsUnsupported,
        ));
    }
    let mut implicit_outputs = Vec::new();
    if scan!(scanner, scanpipe(scanner, AllowedSeparators::IMPLICIT)).is_some() {
        loop {
            let start = scanner.position();
            let Some(path) = scan!(scanner, scanstring(scanner, true)) else {
                break;
            };
            implicit_outputs.push(staged_path(scanner, path, start)?);
        }
    }
    if scanner.current().is_none() {
        return Err(error(scanner, line, DyndepErrorKind::UnexpectedEof));
    }
    scan!(scanner, scanchar(scanner, ':'));
    let rule = scanname(scanner)
        .map_err(|_| error(scanner, line, DyndepErrorKind::ExpectedDyndepCommand))?
        .text;
    if rule != "dyndep" {
        return Err(error(scanner, line, DyndepErrorKind::ExpectedDyndepCommand));
    }

    if scan!(scanner, scanstring(scanner, true)).is_some() {
        return Err(error(
            scanner,
            line,
            DyndepErrorKind::ExplicitInputsUnsupported,
        ));
    }
    let implicit_inputs = parse_implicit_inputs(scanner, line)?;
    scan!(scanner, scannewline(scanner));

    let mut dyndeps = Dyndeps {
        restat: false,
        implicit_inputs,
        implicit_outputs,
    };
    if scan!(scanner, scanindent(scanner)) {
        let binding_line = scanner.line();
        let name = scan!(scanner, scanname(scanner)).text;
        scan!(scanner, scanchar(scanner, '='));
        let value = scan!(scanner, scanstring(scanner, false)).unwrap_or_default();
        scan!(scanner, scannewline(scanner));
        if name != "restat" {
            return Err(error(
                scanner,
                binding_line,
                DyndepErrorKind::BindingNotRestat,
            ));
        }
        dyndeps.restat = !evaluate_empty(value).is_empty();
    }
    let duplicate_output = output.value.clone();
    file.insert(edge, output, dyndeps).map_err(|_| {
        error(
            scanner,
            line,
            DyndepErrorKind::MultipleStatements(duplicate_output),
        )
    })
}

pub(crate) fn parse_dyndep_source(
    source: &Arc<Source>,
    graph: &Graph,
) -> Result<DyndepFile, DyndepError> {
    let mut scanner = Scanner::new(source);
    let mut have_version = false;
    let mut file = DyndepFile::default();
    loop {
        if scanner.current() == Some(b'=') {
            return Err(error(
                &scanner,
                scanner.line(),
                DyndepErrorKind::UnexpectedEquals,
            ));
        }
        match scan!(&scanner, scankeyword(&mut scanner)) {
            Some(token) if token.kind == TokenKind::Build && have_version => {
                parse_build(graph, &mut file, &mut scanner)?;
            }
            Some(token) if token.kind == TokenKind::Build => {
                return Err(error(
                    &scanner,
                    scanner.line(),
                    DyndepErrorKind::ExpectedVersion,
                ));
            }
            Some(token) if token.kind == TokenKind::Variable && !have_version => {
                parse_version(&mut scanner, token.lexeme.text)?;
                have_version = true;
            }
            Some(_) => {
                let kind = if have_version {
                    DyndepErrorKind::UnexpectedIdentifier
                } else {
                    DyndepErrorKind::ExpectedVersion
                };
                return Err(error(&scanner, scanner.line(), kind));
            }
            None if have_version => return Ok(file),
            None => {
                return Err(error(
                    &scanner,
                    scanner.line(),
                    DyndepErrorKind::ExpectedVersion,
                ));
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn parse_dyndep(input: Vec<u8>, graph: &Graph) -> Result<DyndepFile, DyndepError> {
    parse_dyndep_source(&Source::from_bytes("input", input), graph)
}

fn edge_output(graph: &Graph, edge: EdgeId) -> BString {
    graph
        .edge(edge)
        .out
        .first()
        .map(|output| graph.node(*output).path.clone())
        .unwrap_or_default()
}

fn validate_dyndep(
    graph: &Graph,
    dyndep: NodeId,
    path: &BString,
    file: &DyndepFile,
) -> Result<(), ManifestError> {
    for edge in graph
        .node(dyndep)
        .uses
        .iter()
        .copied()
        .filter(|edge| graph.edge(*edge).dyndep == Some(dyndep))
    {
        if file.get(edge).is_none() {
            return Err(ManifestError::DyndepMissingOutput {
                path: path.clone(),
                output: edge_output(graph, edge),
            });
        }
    }

    for (edge, entry) in file.iter() {
        let belongs_to_file = graph.edge(edge).dyndep == Some(dyndep);
        if !belongs_to_file {
            return Err(ManifestError::DyndepWrongOwner {
                path: path.clone(),
                output: entry.output.value.clone(),
                span: entry.output.span.clone(),
            });
        }
    }

    let mut staged_outputs = HashSet::new();
    for (_, entry) in file.iter() {
        for output in &entry.dyndeps.implicit_outputs {
            let existing_generator =
                nodeget(graph, output.value.as_bytes()).and_then(|node| graph.node(node).gen);
            if existing_generator.is_some() || !staged_outputs.insert(output.value.as_bytes()) {
                return Err(ManifestError::DyndepDuplicateOutput {
                    path: path.clone(),
                    output: output.value.clone(),
                    span: output.span.clone(),
                });
            }
        }
    }
    Ok(())
}

fn commit_dyndep(graph: &mut Graph, runtime: &mut RuntimeState, dyndep: NodeId, file: DyndepFile) {
    for (edge, entry) in file {
        let Dyndeps {
            restat,
            implicit_inputs,
            implicit_outputs,
        } = entry.dyndeps;
        let inputs = implicit_inputs
            .into_iter()
            .map(|input| mknode(graph, input.value))
            .collect::<Vec<_>>();
        let outputs = implicit_outputs
            .into_iter()
            .map(|output| mknode(graph, output.value))
            .collect::<Vec<_>>();
        for output in outputs {
            graph.node_mut(output).gen = Some(edge);
            graph.edge_mut(edge).out.push(output);
        }
        edgeadddeps(graph, edge, &inputs);
        if restat {
            graph
                .edge_mut(edge)
                .bindings
                .insert("restat".into(), xasprintf(format_args!("1")));
        }
    }
    runtime.node_mut(dyndep).set_dyndep_pending(false);
}

pub(crate) fn load_dyndep(
    graph: &mut Graph,
    runtime: &mut RuntimeState,
    dyndep: NodeId,
) -> Result<(), ManifestError> {
    let path = graph.node(dyndep).path.clone();
    let source = Source::from_path(path.to_path().expect("byte paths are valid on Unix")).map_err(
        |source| ManifestError::DyndepRead {
            path: path.clone(),
            source,
        },
    )?;
    let file = parse_dyndep_source(&source, graph)?;
    validate_dyndep(graph, dyndep, &path, &file)?;
    commit_dyndep(graph, runtime, dyndep, file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mkenv;
    use crate::graph::{mkedge, mknode};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_LOAD_TEST: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> Graph {
        let mut graph = Graph::default();
        let root = mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        for path in ["out", "otherout"] {
            let output = mknode(&mut graph, xasprintf(format_args!("{path}")));
            graph.node_mut(output).gen = Some(edge);
            graph.edge_mut(edge).out.push(output);
        }
        graph.edge_mut(edge).set_explicit_output_count(2);
        graph
    }

    fn add_output(graph: &mut Graph, path: &str) {
        let root = mkenv(graph, None);
        let edge = mkedge(graph, root);
        let output = mknode(graph, xasprintf(format_args!("{path}")));
        graph.node_mut(output).gen = Some(edge);
        graph.edge_mut(edge).out.push(output);
        graph.edge_mut(edge).set_explicit_output_count(1);
    }

    fn parse(input: &str) -> Result<(Graph, DyndepFile), DyndepError> {
        let graph = fixture();
        let file = parse_dyndep(input.as_bytes().to_vec(), &graph)?;
        Ok((graph, file))
    }

    fn paths(paths: &[StagedPath]) -> Vec<String> {
        paths
            .iter()
            .map(|path| String::from_utf8_lossy(path.value.as_bytes()).into_owned())
            .collect()
    }

    fn entry_for<'a>(graph: &Graph, file: &'a DyndepFile, output: &str) -> &'a Dyndeps {
        let node = nodeget(graph, output.as_bytes()).unwrap();
        let edge = graph.node(node).gen.unwrap();
        file.get(edge).unwrap()
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
                let Err(error) = parse($input) else {
                    panic!("invalid dyndep input unexpectedly parsed");
                };
                let diagnostic = error.kind.to_string();
                assert!(
                    diagnostic.contains($message),
                    "expected {:?} in {:?}",
                    $message,
                    diagnostic
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
    fn ninja_dyndep_parser_preserves_non_utf8_paths() {
        let graph = fixture();
        let file = parse_dyndep(
            b"ninja_dyndep_version = 1\nbuild out | imp-\xff: dyndep | dep-\xfe\n".to_vec(),
            &graph,
        )
        .unwrap();
        let entry = entry_for(&graph, &file, "out");
        assert_eq!(entry.implicit_outputs[0].value.as_bytes(), b"imp-\xff");
        assert_eq!(entry.implicit_inputs[0].value.as_bytes(), b"dep-\xfe");
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
        assert!(file.get(edge).is_some());
    }

    #[test]
    fn ninja_dyndep_parser_multiple_edges() {
        let mut graph = fixture();
        add_output(&mut graph, "out2");
        let file = parse_dyndep(
            b"ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out2: dyndep\n  restat = 1\n"
                .to_vec(),
            &graph,
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
        let mut graph = Graph::default();
        let mut parser = crate::parse::Parser::default();
        let mut state = crate::env::EnvState::new(&mut graph);
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
        let mut runtime = RuntimeState::new(&graph);
        assert!(runtime.node(dyndep).dyndep_pending());
        load_dyndep(&mut graph, &mut runtime, dyndep).unwrap();
        assert!(!runtime.node(dyndep).dyndep_pending());
        let edge = graph.node(nodeget(&graph, b"out").unwrap()).gen.unwrap();
        assert_eq!(node_paths(&graph, &graph.edge(edge).out), ["out"]);
        assert_eq!(graph.edge(edge).input.len(), 2);
        assert_eq!(graph.edge(edge).explicit_input_count(), 1);
        assert_eq!(graph.edge(edge).non_order_only_input_count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_implicit() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1: r in || $dd\n  dyndep = $dd\nbuild out2: r in\n",
            Some("ninja_dyndep_version = 1\nbuild out1: dyndep | out2\n"),
        );
        let mut runtime = RuntimeState::new(&graph);
        load_dyndep(&mut graph, &mut runtime, dyndep).unwrap();
        let edge = graph.node(nodeget(&graph, b"out1").unwrap()).gen.unwrap();
        let inputs = node_paths(&graph, &graph.edge(edge).input);
        assert_eq!(inputs[0..2], ["in", "out2"]);
        assert_eq!(graph.edge(edge).explicit_input_count(), 1);
        assert_eq!(graph.edge(edge).non_order_only_input_count(), 2);
        assert_eq!(graph.node(nodeget(&graph, b"out2").unwrap()).uses.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_missing_file() {
        let (mut graph, dyndep, directory) =
            load_fixture("build out: r in || $dd\n  dyndep = $dd\n", None);
        let mut runtime = RuntimeState::new(&graph);
        assert!(load_dyndep(&mut graph, &mut runtime, dyndep)
            .unwrap_err()
            .to_string()
            .contains("loading"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_missing_entry() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out: r in || $dd\n  dyndep = $dd\n",
            Some("ninja_dyndep_version = 1\n"),
        );
        let mut runtime = RuntimeState::new(&graph);
        assert!(load_dyndep(&mut graph, &mut runtime, dyndep)
            .unwrap_err()
            .to_string()
            .contains("'out' not mentioned"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_load_extra_entry() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out: r in || $dd\n  dyndep = $dd\nbuild out2: r in || $dd\n",
            Some("ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out2: dyndep\n"),
        );
        let mut runtime = RuntimeState::new(&graph);
        assert!(load_dyndep(&mut graph, &mut runtime, dyndep)
            .unwrap_err()
            .to_string()
            .contains("does not have a dyndep binding"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_graph_dyndep_rejects_duplicate_dynamic_output() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1 | shared: r in1\nbuild out2: r in2 || $dd\n  dyndep = $dd\n",
            Some("ninja_dyndep_version = 1\nbuild out2 | shared: dyndep\n"),
        );
        let mut runtime = RuntimeState::new(&graph);
        assert_eq!(
            load_dyndep(&mut graph, &mut runtime, dyndep)
                .unwrap_err()
                .to_string(),
            "multiple rules generate shared"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    // [spec:samurai:req:runtime.dyndep-transaction/test]
    #[test]
    fn ronin_graph_dyndep_late_validation_failure_rolls_back_every_change() {
        let (mut graph, dyndep, directory) = load_fixture(
            "build out1: r in1 || $dd\n  dyndep = $dd\n\
             build out2: r in2 || $dd\n  dyndep = $dd\n",
            Some(
                "ninja_dyndep_version = 1\n\
                 build out1 | shared: dyndep | discovered\n  restat = 1\n\
                 build out2 | shared: dyndep\n",
            ),
        );
        let out1 = nodeget(&graph, b"out1").unwrap();
        let edge1 = graph.node(out1).gen.unwrap();
        let original_outputs = graph.edge(edge1).out.clone();
        let original_inputs = graph.edge(edge1).input.clone();
        let original_uses = graph.node(dyndep).uses.clone();
        let original_node_count = graph.node_ids().len();
        let mut runtime = RuntimeState::new(&graph);

        assert_eq!(
            load_dyndep(&mut graph, &mut runtime, dyndep)
                .unwrap_err()
                .to_string(),
            "multiple rules generate shared"
        );
        assert_eq!(graph.node_ids().len(), original_node_count);
        assert!(nodeget(&graph, b"shared").is_none());
        assert!(nodeget(&graph, b"discovered").is_none());
        assert_eq!(graph.edge(edge1).out, original_outputs);
        assert_eq!(graph.edge(edge1).input, original_inputs);
        assert!(!graph.edge(edge1).bindings.contains_key("restat"));
        assert_eq!(graph.node(dyndep).uses, original_uses);
        assert!(runtime.node(dyndep).dyndep_pending());
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
        let mut runtime = RuntimeState::new(&graph);
        load_dyndep(&mut graph, &mut runtime, dyndep).unwrap();
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
        let restat = crate::env::edgevar(&graph, edge2, "restat", PathStyle::Raw).unwrap();
        assert_eq!(restat.as_bytes(), b"1");
        assert_eq!(
            graph.node(nodeget(&graph, b"out1imp").unwrap()).gen,
            Some(edge1)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
