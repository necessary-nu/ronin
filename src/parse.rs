//! Manifest parser translated from `parse.c`.

use crate::env::{
    edgevar, envaddrule, enveval, envrule, mkpool, mkrule, poolget, ruleaddvar, EnvState,
    EnvironmentId,
};
use crate::error::{ManifestError, ManifestProblem};
use crate::graph::{mkedge, mknode, nodeuse, Graph, NodeId, PathStyle};
use crate::names::Names;
use crate::scan::{
    scanchar, scanindent, scankeyword, scanname, scannewline, scanpaths, scanpipe, scanstring,
    AllowedSeparators, ScannedEvalString, Scanner, Separator, Source, TokenKind,
};
use crate::util::{canonpath, is_canonical, BStr, BString, ByteSlice, IdVec};

type ManifestResult<T> = Result<T, ManifestError>;

fn manifest_error(scanner: &Scanner<'_>, problem: ManifestProblem) -> ManifestError {
    ManifestError::at(scanner.source_span(scanner.position()), problem)
}

// [spec:samurai:def:parse.parseoptions]
#[derive(Clone, Copy, Default)]
pub(crate) struct ParseOptions {
    pub(crate) dupbuildwarn: bool,
}

// [spec:samurai:def:parse.parseinit-fn]
// [spec:samurai:sem:parse.parseinit-fn]
#[derive(Default)]
pub(crate) struct Parser {
    pub(crate) options: ParseOptions,
    pub(crate) defaults: Vec<NodeId>,
    working_directory: crate::os::WorkingDirectory,
}

impl Parser {
    pub(crate) fn with_options_in(
        options: ParseOptions,
        working_directory: crate::os::WorkingDirectory,
    ) -> Self {
        Self {
            options,
            working_directory,
            ..Self::default()
        }
    }
}

// [spec:samurai:def:parse.parselet-fn]
// [spec:samurai:sem:parse.parselet-fn]
fn parselet<'source>(
    scanner: &mut Scanner<'source>,
) -> ManifestResult<(&'source BStr, ScannedEvalString<'source>)> {
    let name = scanname(scanner)?;
    let value = parse_assignment(scanner)?;
    Ok((name.text, value))
}

fn parse_assignment<'source>(
    scanner: &mut Scanner<'source>,
) -> ManifestResult<ScannedEvalString<'source>> {
    scanchar(scanner, '=')?;
    let value = scanstring(scanner, false)?.unwrap_or_default();
    scannewline(scanner)?;
    Ok(value)
}

// [spec:samurai:def:parse.parserule-fn]
// [spec:samurai:sem:parse.parserule-fn]
fn parserule(
    scanner: &mut Scanner<'_>,
    graph: &mut Graph,
    environment: EnvironmentId,
) -> ManifestResult<()> {
    let name = scanname(scanner)?.text;
    let rule = mkrule(graph, name.to_owned());
    scannewline(scanner)?;
    let mut command = false;
    let mut rspfile = false;
    let mut rspfile_content = false;
    while scanindent(scanner)? {
        let (name, value) = parselet(scanner)?;
        if !matches!(
            &**name,
            b"command"
                | b"depfile"
                | b"dyndep"
                | b"description"
                | b"deps"
                | b"generator"
                | b"pool"
                | b"restat"
                | b"rspfile"
                | b"rspfile_content"
                | b"msvc_deps_prefix"
        ) {
            return Err(manifest_error(
                scanner,
                ManifestProblem::UnexpectedRuleVariable {
                    name: name.to_str_lossy().into_owned(),
                },
            ));
        }
        command |= name == "command";
        rspfile |= name == "rspfile";
        rspfile_content |= name == "rspfile_content";
        let name = graph.names_mut().intern(name);
        let value = value.into_owned(graph.names_mut());
        ruleaddvar(graph, rule, name, value);
    }
    if !command {
        return Err(manifest_error(
            scanner,
            ManifestProblem::RuleMissingCommand {
                name: name.to_str_lossy().into_owned(),
            },
        ));
    }
    if rspfile != rspfile_content {
        return Err(manifest_error(
            scanner,
            ManifestProblem::IncompleteResponseFileBinding {
                name: name.to_str_lossy().into_owned(),
            },
        ));
    }
    Ok(envaddrule(graph, environment, rule)?)
}

fn evaluated_path(
    scanner: &Scanner<'_>,
    graph: &Graph,
    path: &ScannedEvalString<'_>,
    environment: EnvironmentId,
) -> ManifestResult<BString> {
    let value = enveval(graph, environment, path);
    if value.is_empty() {
        return Err(manifest_error(scanner, ManifestProblem::EmptyPath));
    }
    Ok(value)
}

/// Evaluate one path reference and intern it, reusing `scratch`.
///
/// Most references name a path that is already interned, so evaluating into a
/// shared buffer and interning from bytes leaves the common case allocating
/// nothing at all.
fn node_for(
    scanner: &Scanner<'_>,
    graph: &mut Graph,
    path: &ScannedEvalString<'_>,
    environment: EnvironmentId,
    scratch: &mut Vec<u8>,
) -> ManifestResult<NodeId> {
    // A path that expands to nothing and is already canonical needs neither
    // evaluation nor canonicalization, so it can be hashed and probed against
    // the manifest bytes themselves — no copy, and no allocation at all unless
    // the node turns out to be new.
    if let ScannedEvalString::Plain(bytes) = path {
        if is_canonical(bytes) {
            return Ok(crate::graph::mknode(graph, bytes));
        }
    }
    scratch.clear();
    crate::env::enveval_into(graph, environment, path, scratch);
    if scratch.is_empty() {
        return Err(manifest_error(scanner, ManifestProblem::EmptyPath));
    }
    canonpath(scratch);
    Ok(crate::graph::mknode(graph, scratch))
}

// [spec:samurai:def:parse.parseedge-fn]
// [spec:samurai:sem:parse.parseedge-fn]
#[allow(
    clippy::too_many_lines,
    reason = "a complete Ninja build production shares scanner state and duplicate-output handling"
)]
fn parseedge(
    scanner: &mut Scanner<'_>,
    graph: &mut Graph,
    environment: EnvironmentId,
    state: &EnvState,
    options: ParseOptions,
    scratch: &mut Vec<u8>,
) -> ManifestResult<()> {
    let mut output_paths = scanpaths(scanner)?;
    let explicit_output_count = output_paths.len();
    if scanpipe(scanner, AllowedSeparators::IMPLICIT)? == Some(Separator::Implicit) {
        output_paths.extend(scanpaths(scanner)?);
    }
    if output_paths.is_empty() {
        return Err(manifest_error(
            scanner,
            ManifestProblem::BuildWithoutOutputs,
        ));
    }
    scanchar(scanner, ':')?;
    let rule_name = scanname(scanner)?.text;
    let rule = envrule(graph, environment, rule_name).ok_or_else(|| {
        manifest_error(
            scanner,
            ManifestProblem::UndefinedRule {
                name: rule_name.to_str_lossy().into_owned(),
            },
        )
    })?;

    let mut input_paths = scanpaths(scanner)?;
    let explicit_input_count = input_paths.len();
    let mut separator = scanpipe(scanner, AllowedSeparators::INPUTS)?;
    if separator == Some(Separator::Implicit) {
        input_paths.extend(scanpaths(scanner)?);
        separator = scanpipe(scanner, AllowedSeparators::AFTER_IMPLICIT)?;
    }
    let non_order_only_input_count = input_paths.len();
    if separator == Some(Separator::OrderOnly) {
        input_paths.extend(scanpaths(scanner)?);
        separator = scanpipe(scanner, AllowedSeparators::VALIDATION)?;
    }
    let validation_paths = if separator == Some(Separator::Validation) {
        scanpaths(scanner)?
    } else {
        Vec::new()
    };
    scannewline(scanner)?;

    let mut bindings = Vec::new();
    while scanindent(scanner)? {
        bindings.push(parselet(scanner)?);
    }

    let mut out = IdVec::new();
    let mut retained_explicit_output_count = 0;
    for (index, output) in output_paths.iter().enumerate() {
        let node = node_for(scanner, graph, output, environment, scratch)?;
        if graph.node(node).gen.is_some() || out.contains(&node) {
            if !options.dupbuildwarn {
                return Err(manifest_error(
                    scanner,
                    ManifestProblem::DuplicateOutput {
                        path: graph.node_path(node).to_owned(),
                    },
                ));
            }
            continue;
        }
        retained_explicit_output_count += usize::from(index < explicit_output_count);
        out.push(node);
    }
    if out.is_empty() {
        if options.dupbuildwarn {
            return Ok(());
        }
        return Err(manifest_error(
            scanner,
            ManifestProblem::BuildWithoutOutputs,
        ));
    }

    let input = input_paths
        .iter()
        .map(|path| node_for(scanner, graph, path, environment, scratch))
        .collect::<Result<IdVec<_>, _>>()?;
    let validation = validation_paths
        .iter()
        .map(|path| node_for(scanner, graph, path, environment, scratch))
        .collect::<Result<IdVec<_>, _>>()?;
    let edge = mkedge(graph, environment);
    let edge_env = graph.edge(edge).env;
    for output in &out {
        graph.node_mut(*output).gen = Some(edge);
    }
    for input_node in &input {
        nodeuse(graph, *input_node, edge);
    }
    for validation_node in &validation {
        graph.add_validation_use(*validation_node, edge);
    }
    {
        let edge_mut = graph.edge_mut(edge);
        edge_mut.rule = Some(rule);
        edge_mut.out = out;
        edge_mut.input = input;
        edge_mut.validation = validation;
        edge_mut.set_explicit_output_count(retained_explicit_output_count);
        edge_mut.set_input_partitions(explicit_input_count, non_order_only_input_count);
    }

    for (name, value) in bindings {
        let value = enveval(graph, edge_env, &value);
        let name = graph.names_mut().intern(name);
        graph.edge_mut(edge).bindings.insert(name, value);
    }

    if let Some(pool_name) =
        edgevar(graph, edge, Names::POOL, PathStyle::ShellEscaped).filter(|pool| !pool.is_empty())
    {
        graph.edge_mut(edge).pool = Some(poolget(state, BStr::new(&pool_name))?);
    }

    if let Some(mut dyndep_path) =
        edgevar(graph, edge, Names::DYNDEP, PathStyle::Raw).filter(|path| !path.is_empty())
    {
        canonpath(&mut dyndep_path);
        let dyndep = mknode(graph, dyndep_path.clone());
        if !graph.edge(edge).input.contains(&dyndep) {
            return Err(manifest_error(
                scanner,
                ManifestProblem::DyndepNotInput { path: dyndep_path },
            ));
        }
        graph.edge_mut(edge).dyndep = Some(dyndep);
    }

    let ignore_phony_self_reference = {
        let edge = graph.edge(edge);
        graph.is_phony_rule(edge.rule)
            && edge.out.len() == 1
            && edge.explicit_output_count() == 1
            && edge.explicit_input_count() == edge.input.len()
    };
    if ignore_phony_self_reference {
        let output = graph.edge(edge).out[0];
        let removed = graph
            .edge(edge)
            .input
            .iter()
            .enumerate()
            .filter_map(|(index, input)| (*input == output).then_some(index))
            .collect::<Vec<_>>();
        if !removed.is_empty() {
            for index in removed.into_iter().rev() {
                graph.edge_mut(edge).remove_input(index);
            }
            graph
                .node_mut(output)
                .uses
                .retain(|candidate| *candidate != edge);
        }
    }
    Ok(())
}

// [spec:samurai:def:parse.parseinclude-fn]
// [spec:samurai:sem:parse.parseinclude-fn]
fn parseinclude(
    scanner: &mut Scanner<'_>,
    graph: &mut Graph,
    parser: &mut Parser,
    environment: EnvironmentId,
    state: &mut EnvState,
    newscope: bool,
) -> ManifestResult<()> {
    let path = scanstring(scanner, true)?
        .ok_or_else(|| manifest_error(scanner, ManifestProblem::ExpectedIncludePath))?;
    scannewline(scanner)?;
    let path = evaluated_path(scanner, graph, &path, environment)?;
    let environment = if newscope {
        crate::env::mkenv(graph, Some(environment))
    } else {
        environment
    };
    parse(
        path.to_path().expect("byte paths are valid on Unix"),
        graph,
        parser,
        environment,
        state,
    )
}

// [spec:samurai:def:parse.parsedefault-fn]
// [spec:samurai:sem:parse.parsedefault-fn]
fn parsedefault(
    scanner: &mut Scanner<'_>,
    graph: &Graph,
    parser: &mut Parser,
    environment: EnvironmentId,
) -> ManifestResult<()> {
    let targets = scanpaths(scanner)?;
    scannewline(scanner)?;
    if targets.is_empty() {
        return Err(manifest_error(scanner, ManifestProblem::ExpectedTargetName));
    }
    for target in targets {
        let mut target = evaluated_path(scanner, graph, &target, environment)?;
        canonpath(&mut target);
        parser.defaults.push(
            crate::graph::nodeget(graph, target.as_bytes()).ok_or_else(|| {
                manifest_error(
                    scanner,
                    ManifestProblem::UnknownTarget {
                        path: target.clone(),
                    },
                )
            })?,
        );
    }
    Ok(())
}

// [spec:samurai:def:parse.parsepool-fn]
// [spec:samurai:sem:parse.parsepool-fn]
fn parsepool(
    scanner: &mut Scanner<'_>,
    graph: &mut Graph,
    state: &mut EnvState,
    environment: EnvironmentId,
) -> ManifestResult<()> {
    let name = scanname(scanner)?.text;
    let pool = mkpool(graph, state, name.to_owned())?;
    scannewline(scanner)?;
    while scanindent(scanner)? {
        let (name, value) = parselet(scanner)?;
        if name != "depth" {
            return Err(manifest_error(
                scanner,
                ManifestProblem::UnexpectedPoolVariable {
                    name: name.to_str_lossy().into_owned(),
                },
            ));
        }
        let value = enveval(graph, environment, &value);
        let depth = String::from_utf8_lossy(value.as_bytes())
            .parse()
            .ok()
            .and_then(std::num::NonZeroUsize::new)
            .ok_or_else(|| {
                manifest_error(
                    scanner,
                    ManifestProblem::InvalidPoolDepth {
                        value: value.clone(),
                    },
                )
            })?;
        graph.pool_mut(pool).set_depth(depth);
    }
    if graph.pool(pool).depth().is_none() {
        return Err(manifest_error(scanner, ManifestProblem::PoolWithoutDepth));
    }
    Ok(())
}

// [spec:samurai:def:parse.checkversion-fn]
// [spec:samurai:sem:parse.checkversion-fn]
fn checkversion(scanner: &Scanner<'_>, version: &BStr) -> ManifestResult<(i32, i32)> {
    let bytes = version.as_bytes();
    let major_end = bytes
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(bytes.len());
    if major_end == 0 {
        return Err(manifest_error(
            scanner,
            ManifestProblem::InvalidRequiredVersion,
        ));
    }
    let major = std::str::from_utf8(&bytes[..major_end])
        .unwrap()
        .parse::<i32>()
        .map_err(|_| manifest_error(scanner, ManifestProblem::InvalidRequiredVersion))?;
    let mut minor = 0;
    if bytes.get(major_end) == Some(&b'.') {
        let minor_bytes = &bytes[major_end + 1..];
        let minor_end = minor_bytes
            .iter()
            .position(|byte| !byte.is_ascii_digit())
            .unwrap_or(minor_bytes.len());
        if minor_end != 0 {
            minor = std::str::from_utf8(&minor_bytes[..minor_end])
                .unwrap()
                .parse::<i32>()
                .map_err(|_| manifest_error(scanner, ManifestProblem::InvalidRequiredVersion))?;
        }
    }
    if major > 1 || major == 1 && minor > 9 {
        Err(manifest_error(
            scanner,
            ManifestProblem::RequiredVersionTooNew {
                version: BString::from(bytes),
            },
        ))
    } else {
        Ok((major, minor))
    }
}

// [spec:samurai:def:parse.parse-fn]
// [spec:samurai:sem:parse.parse-fn]
// [spec:samurai:req:compat.manifest-semantics]
pub(crate) fn parse(
    name: impl AsRef<std::path::Path>,
    graph: &mut Graph,
    parser: &mut Parser,
    environment: EnvironmentId,
    state: &mut EnvState,
) -> ManifestResult<()> {
    let path = name.as_ref().to_owned();
    let input = std::fs::read(parser.working_directory.resolve(&path))
        .map_err(|error| ManifestError::read(&path, error))?;
    let source = Source::from_bytes(&path, input);
    let mut scanner = Scanner::new(&source);
    // One buffer per manifest, reused by every path reference in it.
    let mut path_scratch = Vec::new();
    while let Some(token) = scankeyword(&mut scanner)? {
        match token.kind {
            TokenKind::Rule => parserule(&mut scanner, graph, environment)?,
            TokenKind::Build => {
                parseedge(
                    &mut scanner,
                    graph,
                    environment,
                    state,
                    parser.options,
                    &mut path_scratch,
                )?;
            }
            TokenKind::Include => {
                parseinclude(&mut scanner, graph, parser, environment, state, false)?;
            }
            TokenKind::Subninja => {
                parseinclude(&mut scanner, graph, parser, environment, state, true)?;
            }
            TokenKind::Default => parsedefault(&mut scanner, graph, parser, environment)?,
            TokenKind::Pool => parsepool(&mut scanner, graph, state, environment)?,
            TokenKind::Variable => {
                let name = token.lexeme.text;
                let value = parse_assignment(&mut scanner)?;
                let value = enveval(graph, environment, &value);
                if name == "ninja_required_version" {
                    let (major, minor) = checkversion(&scanner, BStr::new(value.as_bytes()))?;
                    scanner.set_manifest_version(major, minor);
                }
                let name = graph.names_mut().intern(name);
                crate::env::envaddvar(graph, environment, name, value);
            }
        }
    }
    Ok(())
}

// [spec:samurai:def:parse.defaultnodes-fn]
// [spec:samurai:sem:parse.defaultnodes-fn]
pub(crate) fn defaultnodes(parser: &Parser, graph: &Graph) -> Vec<NodeId> {
    if parser.defaults.is_empty() {
        graph
            .node_ids()
            .filter(|node| {
                let node = graph.node(*node);
                node.gen.is_some() && node.uses.is_empty()
            })
            .collect()
    } else {
        parser.defaults.clone()
    }
}

#[cfg(test)]
mod ninja_manifest_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_MANIFEST: AtomicUsize = AtomicUsize::new(0);

    fn parse_source(source: &str) -> ManifestResult<(Graph, Parser, EnvState)> {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-parser-{}-{}.ninja",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, source).unwrap();
        let mut graph = crate::graph::Graph::default();
        let mut parser = Parser::default();
        let mut state = crate::env::EnvState::new(&mut graph);
        let result = parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        );
        fs::remove_file(path).unwrap();
        result.map(|()| (graph, parser, state))
    }

    fn parse_path(path: &std::path::Path) -> ManifestResult<(Graph, Parser, EnvState)> {
        let mut graph = crate::graph::Graph::default();
        let mut parser = Parser::default();
        let mut state = crate::env::EnvState::new(&mut graph);
        parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        )?;
        Ok((graph, parser, state))
    }

    fn temporary_directory(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-{label}-{}-{}",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn output_edge(graph: &Graph, output: &[u8]) -> crate::graph::EdgeId {
        graph
            .node(crate::graph::nodeget(graph, output).unwrap())
            .gen
            .unwrap()
    }

    fn parse_error(source: &str) -> String {
        match parse_source(source) {
            Ok(_) => panic!("manifest unexpectedly parsed"),
            Err(error) => error.to_string(),
        }
    }

    fn assert_dyndep(source: &str, expected: &[u8]) {
        let (graph, _, _) = parse_source(source).unwrap();
        let edge = output_edge(&graph, b"result");
        let dyndep = graph.edge(edge).dyndep.unwrap();
        assert_eq!(graph.node_path(dyndep).as_bytes(), expected);
        let runtime = crate::runtime::RuntimeState::new(&graph);
        assert!(runtime.node(dyndep).dyndep_pending());
    }

    #[test]
    fn ninja_manifest_parser_validations() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo: cat bar |@ baz\n")
                .unwrap();
        let edge = output_edge(&graph, b"foo");
        assert_eq!(graph.edge(edge).input.len(), 1);
        assert_eq!(graph.edge(edge).validation.len(), 1);
        assert_eq!(
            graph.node_path(graph.edge(edge).validation[0]).as_bytes(),
            b"baz"
        );
        assert_eq!(
            graph
                .node_validation_uses(crate::graph::nodeget(&graph, b"baz").unwrap())
                .len(),
            1
        );
    }

    #[test]
    fn ninja_manifest_parser_implicit_output() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo | imp: cat bar\n")
                .unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(graph.edge(edge).out.len(), 2);
        assert_eq!(graph.edge(edge).explicit_output_count(), 1);
        assert_eq!(edge, output_edge(&graph, b"foo"));
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_empty() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo | : cat bar\n").unwrap();
        let edge = output_edge(&graph, b"foo");
        assert_eq!(graph.edge(edge).out.len(), 1);
        assert_eq!(graph.edge(edge).explicit_output_count(), 1);
    }

    #[test]
    fn ninja_manifest_parser_no_explicit_output() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild | imp: cat bar\n").unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(graph.edge(edge).out.len(), 1);
        assert_eq!(graph.edge(edge).explicit_output_count(), 0);
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_duplicate_error() {
        let error = parse_error(
            "rule cat\n  command = cat $in > $out\nbuild foo baz | foo baq foo: cat bar\n",
        );
        assert!(error.contains("multiple rules generate 'foo'"));
    }

    #[test]
    fn ninja_manifest_parser_phony_self_reference_ignored() {
        let (graph, _, _) = parse_source("build a: phony a\n").unwrap();
        let edge = output_edge(&graph, b"a");
        assert!(graph.edge(edge).input.is_empty());
        assert!(graph
            .node(crate::graph::nodeget(&graph, b"a").unwrap())
            .uses
            .is_empty());
    }

    #[test]
    fn ninja_manifest_parser_reserved_words() {
        let (graph, parser, _) = parse_source(
            "rule build\n  command = rule run $out\nbuild subninja: build include default foo.cc\ndefault subninja\n",
        )
        .unwrap();
        assert!(crate::graph::nodeget(&graph, b"subninja").is_some());
        assert_eq!(defaultnodes(&parser, &graph).len(), 1);
    }

    #[test]
    fn ninja_manifest_parser_dyndep_not_specified() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild result: cat in\n").unwrap();
        assert!(graph.edge(output_edge(&graph, b"result")).dyndep.is_none());
    }

    #[test]
    fn ninja_manifest_parser_dyndep_not_input() {
        let error = parse_error(
            "rule touch\n  command = touch $out\nbuild result: touch\n  dyndep = notin\n",
        );
        assert_eq!(error, "dyndep 'notin' is not an input");
    }

    #[test]
    fn ninja_manifest_parser_dyndep_explicit_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\nbuild result: cat in\n  dyndep = in\n",
            b"in",
        );
    }

    #[test]
    fn ninja_manifest_parser_dyndep_implicit_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\nbuild result: cat in | dd\n  dyndep = dd\n",
            b"dd",
        );
    }

    #[test]
    fn ninja_manifest_parser_dyndep_order_only_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\nbuild result: cat in || dd\n  dyndep = dd\n",
            b"dd",
        );
    }

    #[test]
    fn ninja_manifest_parser_dyndep_rule_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\n  dyndep = $in\nbuild result: cat in\n",
            b"in",
        );
    }

    #[test]
    fn ninja_manifest_parser_selects_pool() {
        let (graph, _, _) = parse_source(
            "pool link_pool\n  depth = 15\nrule link\n  command = link\n  pool = link_pool\nbuild result: link input\n",
        )
        .unwrap();
        let edge = output_edge(&graph, b"result");
        let pool = graph.edge(edge).pool.unwrap();
        assert_eq!(graph.pool(pool).name, "link_pool");
        assert_eq!(graph.pool(pool).depth().unwrap().get(), 15);
    }

    #[test]
    fn ninja_manifest_parser_rejects_bad_pools() {
        assert!(parse_source("pool foo\n  depth = -1\n").is_err());
        assert!(parse_source("pool foo\n  depth = word\n").is_err());
        assert!(parse_source("pool foo\n  bar = 1\n").is_err());
        assert_eq!(
            parse_error("pool console\n  depth = 2\n"),
            "pool 'console' redefined"
        );
        assert!(parse_source(
            "rule run\n  command = echo\n  pool = unnamed_pool\nbuild out: run in\n"
        )
        .is_err());
    }

    #[test]
    fn ninja_manifest_parser_rejects_unknown_rule_binding() {
        let error = parse_error("rule cc\n  command = foo\n  othervar = bar\n");
        assert_eq!(error, "unexpected rule variable 'othervar'");
    }

    #[test]
    fn ninja_manifest_parser_default_cycle_has_no_root() {
        let (graph, parser, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild a: cat a\n").unwrap();
        assert!(defaultnodes(&parser, &graph).is_empty());
    }

    #[test]
    fn ninja_manifest_parser_utf8_and_crlf() {
        parse_source(
            "# comment with crlf\r\npool link_pool\r\n  depth = 15\r\n\r\nrule utf8\r\n  command = true\r\n  description = compilación\r\n",
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    // [spec:samurai:req:compat.manifest-semantics/test]
    fn ninja_manifest_parser_preserves_non_utf8_paths() {
        let directory = temporary_directory("non-utf8");
        let path = directory.join("build.ninja");
        fs::write(
            &path,
            b"rule cat\n  command = cat $in > $out\nbuild out-\xff: cat in-\xfe\n",
        )
        .unwrap();
        let (graph, _, _) = parse_path(&path).unwrap();
        let output = crate::graph::nodeget(&graph, b"out-\xff").unwrap();
        assert!(crate::graph::nodeget(&graph, b"in-\xfe").is_some());
        let edge = graph.node(output).gen.unwrap();
        let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(command.as_bytes(), b"cat in-\xfe > out-\xff");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_subninja_scope() {
        let directory = temporary_directory("subninja-scope");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(&child, "var = inner\nbuild $builddir/inner: varref input\n").unwrap();
        fs::write(
            &root,
            format!(
                "builddir = some_dir\nrule varref\n  command = varref $var\nvar = outer\nbuild $builddir/outer: varref input\nsubninja {}\nbuild $builddir/outer2: varref input\n",
                child.display()
            ),
        )
        .unwrap();
        let (graph, _, _) = parse_path(&root).unwrap();
        let inner = output_edge(&graph, b"some_dir/inner");
        let outer = output_edge(&graph, b"some_dir/outer");
        let outer2 = output_edge(&graph, b"some_dir/outer2");
        let inner_command = edgevar(&graph, inner, Names::COMMAND, PathStyle::Raw).unwrap();
        let outer_command = edgevar(&graph, outer, Names::COMMAND, PathStyle::Raw).unwrap();
        let second_outer_command = edgevar(&graph, outer2, Names::COMMAND, PathStyle::Raw).unwrap();
        assert_eq!(inner_command.as_bytes(), b"varref inner");
        assert_eq!(outer_command.as_bytes(), b"varref outer");
        assert_eq!(second_outer_command.as_bytes(), b"varref outer");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_shadowed_phony_rule_is_not_builtin_phony() {
        let directory = temporary_directory("shadowed-phony");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(
            &child,
            "rule phony\n  command = fake-phony $in > $out\nbuild shadowed: phony in\n",
        )
        .unwrap();
        fs::write(
            &root,
            format!(
                "rule cat\n  command = cat $in > $out\nbuild real: phony in\nsubninja {}\n",
                child.display()
            ),
        )
        .unwrap();
        let (graph, _, _) = parse_path(&root).unwrap();
        let real = output_edge(&graph, b"real");
        let shadowed = output_edge(&graph, b"shadowed");
        assert!(graph.is_phony_rule(graph.edge(real).rule));
        assert!(!graph.is_phony_rule(graph.edge(shadowed).rule));

        // The shadowed rule is an ordinary command edge: collectors must keep
        // it, exactly as Ninja's rule-identity comparison does.
        let mut collector = crate::graph::CommandCollector::default();
        collector.collect_from(&graph, crate::graph::nodeget(&graph, b"shadowed").unwrap());
        assert_eq!(collector.edges, [shadowed]);
        collector.collect_from(&graph, crate::graph::nodeget(&graph, b"real").unwrap());
        assert_eq!(collector.edges, [shadowed]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_rejects_root_phony_redefinition() {
        let error = parse_error("rule phony\n  command = fake\n");
        assert_eq!(error, "rule 'phony' redefined");
    }

    #[test]
    fn ninja_manifest_parser_duplicate_rule_in_different_subninjas() {
        let directory = temporary_directory("subninja-rules");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(&child, "rule cat\n  command = child\n").unwrap();
        fs::write(
            &root,
            format!(
                "rule cat\n  command = parent\nsubninja {}\nbuild out: cat input\n",
                child.display()
            ),
        )
        .unwrap();
        parse_path(&root).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_rule_across_include_scopes() {
        let directory = temporary_directory("subninja-includes");
        let rules = directory.join("rules.ninja");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(&rules, "rule cat\n  command = cat\n").unwrap();
        fs::write(
            &child,
            format!("include {}\nbuild x: cat input\n", rules.display()),
        )
        .unwrap();
        fs::write(
            &root,
            format!(
                "include {}\nsubninja {}\nbuild y: cat input\n",
                rules.display(),
                child.display()
            ),
        )
        .unwrap();
        let (graph, _, _) = parse_path(&root).unwrap();
        assert!(crate::graph::nodeget(&graph, b"x").is_some());
        assert!(crate::graph::nodeget(&graph, b"y").is_some());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_include_updates_current_scope() {
        let directory = temporary_directory("include-scope");
        let include = directory.join("include.ninja");
        let root = directory.join("build.ninja");
        fs::write(&include, "var = inner\n").unwrap();
        fs::write(
            &root,
            format!("var = outer\ninclude {}\n", include.display()),
        )
        .unwrap();
        let (graph, _, state) = parse_path(&root).unwrap();
        let value = crate::env::envvar_named(&graph, state.root, BStr::new("var")).unwrap();
        assert_eq!(value.as_bytes(), b"inner");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_broken_and_missing_includes() {
        let directory = temporary_directory("broken-include");
        let broken = directory.join("broken.ninja");
        let root = directory.join("build.ninja");
        fs::write(&broken, "build\n").unwrap();
        fs::write(&root, format!("include {}\n", broken.display())).unwrap();
        assert!(parse_path(&root).is_err());
        fs::write(
            &root,
            format!("subninja {}\n", directory.join("missing.ninja").display()),
        )
        .unwrap();
        assert!(parse_path(&root).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_edge_in_included_file() {
        let directory = temporary_directory("duplicate-include");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(
            &child,
            "rule cat\n  command = cat\nbuild out1 out2: cat in1\nbuild out1: cat in2\n",
        )
        .unwrap();
        fs::write(&root, format!("subninja {}\n", child.display())).unwrap();
        let Err(error) = parse_path(&root) else {
            panic!("duplicate output unexpectedly parsed");
        };
        assert!(error.to_string().contains("multiple rules generate 'out1'"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_output_warning_mode() {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-duplicate-warning-{}-{}.ninja",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            "rule cat\n  command = cat\nbuild out: cat in1\nbuild out: cat in2\n",
        )
        .unwrap();
        let mut graph = crate::graph::Graph::default();
        let mut parser = Parser::default();
        parser.options.dupbuildwarn = true;
        let mut state = crate::env::EnvState::new(&mut graph);
        parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        )
        .unwrap();
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.edge(output_edge(&graph, b"out")).input.len(), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_rejects_unterminated_lines() {
        assert_eq!(parse_error("x = 3"), "unexpected EOF");
        assert_eq!(parse_error("x = $\n"), "unexpected EOF after continuation");
        assert_eq!(
            parse_error("x = a$\n b$\n $\n"),
            "unexpected EOF after continuation"
        );
    }

    #[test]
    fn ninja_manifest_parser_indented_blank_terminates_rule() {
        assert!(parse_source("rule r\n  command = r\n  \n  generator = 1\n").is_err());
    }

    #[test]
    fn ninja_manifest_parser_default_escaped_space() {
        let (graph, parser, _) = parse_source(
            "rule cat\n  command = cat\nbuild foo$ bar: cat input\ndefault foo$ bar\n",
        )
        .unwrap();
        let defaults = defaultnodes(&parser, &graph);
        assert_eq!(defaults.len(), 1);
        assert_eq!(graph.node_path(defaults[0]).as_bytes(), b"foo bar");
    }

    #[test]
    fn ninja_state_complex_target_is_preserved() {
        let (graph, _, _) = parse_source(
            "rule copy\n  command = cp $in $out\nname = foo %2F bar?baz&x=1\nbuild $name: copy foo\n",
        )
        .unwrap();
        let node = crate::graph::nodeget(&graph, b"foo %2F bar?baz&x=1").unwrap();
        assert_eq!(graph.node_path(node).as_bytes(), b"foo %2F bar?baz&x=1");
    }
}
