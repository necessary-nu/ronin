//! Manifest parser translated from `parse.c`.

use crate::env::{
    edgevar, envaddrule, enveval, envrule, mkpool, mkrule, poolget, ruleaddvar, EnvState,
    EnvironmentId,
};
use crate::error::ManifestError;
use crate::graph::{mkedge, mknode, nodeuse, Graph, NodeId};
use crate::scan::{
    scanchar, scanindent, scankeyword, scanname, scannewline, scanpaths, scanpipe, scanstring,
    Scanner, Token,
};
use crate::util::{canonpath, BStr, BString, ByteSlice, EvalString};

type ManifestResult<T> = Result<T, ManifestError>;

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
}

// [spec:samurai:def:parse.parselet-fn]
// [spec:samurai:sem:parse.parselet-fn]
fn parselet(scanner: &mut Scanner) -> ManifestResult<(String, EvalString)> {
    let name = scanname(scanner)?;
    let value = parse_assignment(scanner)?;
    Ok((name, value))
}

fn parse_assignment(scanner: &mut Scanner) -> ManifestResult<EvalString> {
    scanchar(scanner, '=')?;
    let value = scanstring(scanner, false)?.unwrap_or_default();
    scannewline(scanner)?;
    Ok(value)
}

// [spec:samurai:def:parse.parserule-fn]
// [spec:samurai:sem:parse.parserule-fn]
fn parserule(
    scanner: &mut Scanner,
    graph: &mut Graph,
    environment: EnvironmentId,
) -> ManifestResult<()> {
    let name = scanname(scanner)?;
    let rule = mkrule(graph, name.clone());
    scannewline(scanner)?;
    let mut command = false;
    let mut rspfile = false;
    let mut rspfile_content = false;
    while scanindent(scanner)? {
        let (name, value) = parselet(scanner)?;
        if !matches!(
            name.as_str(),
            "command"
                | "depfile"
                | "dyndep"
                | "description"
                | "deps"
                | "generator"
                | "pool"
                | "restat"
                | "rspfile"
                | "rspfile_content"
                | "msvc_deps_prefix"
        ) {
            return Err(format!("unexpected rule variable '{name}'").into());
        }
        command |= name == "command";
        rspfile |= name == "rspfile";
        rspfile_content |= name == "rspfile_content";
        ruleaddvar(graph, rule, name, value);
    }
    if !command {
        return Err(format!("rule '{name}' has no command").into());
    }
    if rspfile != rspfile_content {
        return Err(
            format!("rule '{name}' has rspfile and no rspfile_content or vice versa").into(),
        );
    }
    Ok(envaddrule(graph, environment, rule)?)
}

fn evaluated_path(
    graph: &Graph,
    path: &EvalString,
    environment: EnvironmentId,
) -> ManifestResult<BString> {
    let value = enveval(graph, environment, path);
    if value.is_empty() {
        return Err("empty path".into());
    }
    Ok(value)
}

fn take_paths(scanner: &mut Scanner) -> Vec<EvalString> {
    std::mem::take(&mut scanner.paths)
}

fn node_for(
    graph: &mut Graph,
    path: &EvalString,
    environment: EnvironmentId,
) -> ManifestResult<NodeId> {
    let mut path = evaluated_path(graph, path, environment)?;
    canonpath(&mut path);
    Ok(mknode(graph, path))
}

// [spec:samurai:def:parse.parseedge-fn]
// [spec:samurai:sem:parse.parseedge-fn]
#[allow(
    clippy::too_many_lines,
    reason = "a complete Ninja build production shares scanner state and duplicate-output handling"
)]
fn parseedge(
    scanner: &mut Scanner,
    graph: &mut Graph,
    environment: EnvironmentId,
    state: &EnvState,
    options: ParseOptions,
) -> ManifestResult<()> {
    scanpaths(scanner)?;
    let mut output_paths = take_paths(scanner);
    let explicit_output_count = output_paths.len();
    if scanpipe(scanner, 1)? == 1 {
        scanpaths(scanner)?;
        output_paths.extend(take_paths(scanner));
    }
    if output_paths.is_empty() {
        return Err("build has no outputs".into());
    }
    scanchar(scanner, ':')?;
    let rule_name = scanname(scanner)?;
    let rule = envrule(graph, environment, &rule_name)
        .ok_or_else(|| format!("undefined rule '{rule_name}'"))?;

    scanpaths(scanner)?;
    let mut input_paths = take_paths(scanner);
    let inimpidx = input_paths.len();
    let mut separator = scanpipe(scanner, 1 | 2 | 4)?;
    if separator == 1 {
        scanpaths(scanner)?;
        input_paths.extend(take_paths(scanner));
        separator = scanpipe(scanner, 2 | 4)?;
    }
    let inorderidx = input_paths.len();
    if separator == 2 {
        scanpaths(scanner)?;
        input_paths.extend(take_paths(scanner));
        separator = scanpipe(scanner, 4)?;
    }
    let validation_paths = if separator == 4 {
        scanpaths(scanner)?;
        take_paths(scanner)
    } else {
        Vec::new()
    };
    scannewline(scanner)?;

    let mut bindings = Vec::new();
    while scanindent(scanner)? {
        bindings.push(parselet(scanner)?);
    }

    let mut out = Vec::new();
    let mut outimpidx = 0;
    for (index, output) in output_paths.iter().enumerate() {
        let node = node_for(graph, output, environment)?;
        if graph.node(node).gen.is_some() || out.contains(&node) {
            if !options.dupbuildwarn {
                return Err(format!(
                    "multiple rules generate '{}'",
                    String::from_utf8_lossy(graph.node(node).path.as_bytes())
                )
                .into());
            }
            continue;
        }
        outimpidx += usize::from(index < explicit_output_count);
        out.push(node);
    }
    if out.is_empty() {
        if options.dupbuildwarn {
            return Ok(());
        }
        return Err("build has no outputs".into());
    }

    let input = input_paths
        .iter()
        .map(|path| node_for(graph, path, environment))
        .collect::<Result<Vec<_>, _>>()?;
    let validation = validation_paths
        .iter()
        .map(|path| node_for(graph, path, environment))
        .collect::<Result<Vec<_>, _>>()?;
    let edge = mkedge(graph, environment);
    let edge_env = graph.edge(edge).env;
    for output in &out {
        graph.node_mut(*output).gen = Some(edge);
    }
    for input_node in &input {
        nodeuse(graph, *input_node, edge);
    }
    for validation_node in &validation {
        graph.node_mut(*validation_node).validation_uses.push(edge);
    }
    {
        let edge_mut = graph.edge_mut(edge);
        edge_mut.rule = Some(rule);
        edge_mut.outimpidx = outimpidx;
        edge_mut.out = out;
        edge_mut.inimpidx = inimpidx;
        edge_mut.inorderidx = inorderidx;
        edge_mut.input = input;
        edge_mut.validation = validation;
    }

    for (name, value) in bindings {
        let value = enveval(graph, edge_env, &value);
        graph.edge_mut(edge).bindings.insert(name, value);
    }

    if let Some(pool_name) = edgevar(graph, edge, "pool", true).filter(|pool| !pool.is_empty()) {
        let pool_name = String::from_utf8_lossy(pool_name.as_bytes());
        graph.edge_mut(edge).pool = Some(poolget(state, &pool_name)?);
    }

    if let Some(mut dyndep_path) =
        edgevar(graph, edge, "dyndep", false).filter(|path| !path.is_empty())
    {
        canonpath(&mut dyndep_path);
        let dyndep = mknode(graph, dyndep_path.clone());
        if !graph.edge(edge).input.contains(&dyndep) {
            return Err(format!(
                "dyndep '{}' is not an input",
                String::from_utf8_lossy(dyndep_path.as_bytes())
            )
            .into());
        }
        graph.node_mut(dyndep).dyndep_pending = true;
        graph.edge_mut(edge).dyndep = Some(dyndep);
    }

    let ignore_phony_self_reference = {
        let edge = graph.edge(edge);
        edge.rule
            .is_some_and(|rule| graph.rule(rule).name == "phony")
            && edge.out.len() == 1
            && edge.outimpidx == 1
            && edge.inimpidx == edge.input.len()
    };
    if ignore_phony_self_reference {
        let output = graph.edge(edge).out[0];
        let mut removed = 0;
        graph.edge_mut(edge).input.retain(|input| {
            let keep = *input != output;
            removed += usize::from(!keep);
            keep
        });
        if removed != 0 {
            graph
                .node_mut(output)
                .uses
                .retain(|candidate| *candidate != edge);
            let edge = graph.edge_mut(edge);
            edge.inimpidx -= removed;
            edge.inorderidx -= removed;
        }
    }
    Ok(())
}

// [spec:samurai:def:parse.parseinclude-fn]
// [spec:samurai:sem:parse.parseinclude-fn]
fn parseinclude(
    scanner: &mut Scanner,
    graph: &mut Graph,
    parser: &mut Parser,
    environment: EnvironmentId,
    state: &mut EnvState,
    newscope: bool,
) -> ManifestResult<()> {
    let path = scanstring(scanner, true)?.ok_or_else(|| "expected include path".to_owned())?;
    scannewline(scanner)?;
    let path = evaluated_path(graph, &path, environment)?;
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
    scanner: &mut Scanner,
    graph: &Graph,
    parser: &mut Parser,
    environment: EnvironmentId,
) -> ManifestResult<()> {
    scanpaths(scanner)?;
    let targets = take_paths(scanner);
    scannewline(scanner)?;
    if targets.is_empty() {
        return Err("expected target name".into());
    }
    for target in targets {
        let mut target = evaluated_path(graph, &target, environment)?;
        canonpath(&mut target);
        parser.defaults.push(
            crate::graph::nodeget(graph, target.as_bytes()).ok_or_else(|| {
                format!(
                    "unknown target '{}'",
                    String::from_utf8_lossy(target.as_bytes())
                )
            })?,
        );
    }
    Ok(())
}

// [spec:samurai:def:parse.parsepool-fn]
// [spec:samurai:sem:parse.parsepool-fn]
fn parsepool(
    scanner: &mut Scanner,
    graph: &mut Graph,
    state: &mut EnvState,
    environment: EnvironmentId,
) -> ManifestResult<()> {
    let name = scanname(scanner)?;
    let pool = mkpool(graph, state, name)?;
    scannewline(scanner)?;
    while scanindent(scanner)? {
        let (name, value) = parselet(scanner)?;
        if name != "depth" {
            return Err(format!("unexpected pool variable '{name}'").into());
        }
        let value = enveval(graph, environment, &value);
        let text = String::from_utf8_lossy(value.as_bytes());
        graph.pool_mut(pool).maxjobs = text
            .parse()
            .map_err(|_| format!("invalid pool depth '{text}'"))?;
    }
    if graph.pool(pool).maxjobs <= 0 {
        return Err("pool has no depth".into());
    }
    Ok(())
}

// [spec:samurai:def:parse.checkversion-fn]
// [spec:samurai:sem:parse.checkversion-fn]
fn checkversion(version: &BStr) -> ManifestResult<(i32, i32)> {
    let bytes = version.as_bytes();
    let major_end = bytes
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(bytes.len());
    if major_end == 0 {
        return Err("invalid ninja_required_version".into());
    }
    let major = std::str::from_utf8(&bytes[..major_end])
        .unwrap()
        .parse::<i32>()
        .map_err(|_| "invalid ninja_required_version".to_owned())?;
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
                .map_err(|_| "invalid ninja_required_version".to_owned())?;
        }
    }
    if major > 1 || major == 1 && minor > 9 {
        Err(format!(
            "ninja_required_version {} is newer than 1.9",
            String::from_utf8_lossy(bytes)
        )
        .into())
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
    let mut scanner = crate::scan::Scanner::from_path(name)?;
    while let Some(token) = scankeyword(&mut scanner)? {
        match token {
            Token::Rule => parserule(&mut scanner, graph, environment)?,
            Token::Build => parseedge(&mut scanner, graph, environment, state, parser.options)?,
            Token::Include => parseinclude(&mut scanner, graph, parser, environment, state, false)?,
            Token::Subninja => parseinclude(&mut scanner, graph, parser, environment, state, true)?,
            Token::Default => parsedefault(&mut scanner, graph, parser, environment)?,
            Token::Pool => parsepool(&mut scanner, graph, state, environment)?,
            Token::Variable => {
                let name = scanner
                    .take_variable()
                    .expect("variable token carries its name");
                let value = parse_assignment(&mut scanner)?;
                let value = enveval(graph, environment, &value);
                if name == "ninja_required_version" {
                    let (major, minor) = checkversion(BStr::new(value.as_bytes()))?;
                    scanner.manifest_version_major = major;
                    scanner.manifest_version_minor = minor;
                }
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
        assert_eq!(graph.node(dyndep).path.as_bytes(), expected);
        assert!(graph.node(dyndep).dyndep_pending);
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
            graph.node(graph.edge(edge).validation[0]).path.as_bytes(),
            b"baz"
        );
        assert_eq!(
            graph
                .node(crate::graph::nodeget(&graph, b"baz").unwrap())
                .validation_uses
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
        assert_eq!(graph.edge(edge).outimpidx, 1);
        assert_eq!(edge, output_edge(&graph, b"foo"));
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_empty() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo | : cat bar\n").unwrap();
        let edge = output_edge(&graph, b"foo");
        assert_eq!(graph.edge(edge).out.len(), 1);
        assert_eq!(graph.edge(edge).outimpidx, 1);
    }

    #[test]
    fn ninja_manifest_parser_no_explicit_output() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild | imp: cat bar\n").unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(graph.edge(edge).out.len(), 1);
        assert_eq!(graph.edge(edge).outimpidx, 0);
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
        assert_eq!(graph.pool(pool).maxjobs, 15);
    }

    #[test]
    fn ninja_manifest_parser_rejects_bad_pools() {
        assert!(parse_source("pool foo\n  depth = -1\n").is_err());
        assert!(parse_source("pool foo\n  depth = word\n").is_err());
        assert!(parse_source("pool foo\n  bar = 1\n").is_err());
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
        let command = crate::env::edgevar(&graph, edge, "command", false).unwrap();
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
        let inner_command = edgevar(&graph, inner, "command", false).unwrap();
        let outer_command = edgevar(&graph, outer, "command", false).unwrap();
        let second_outer_command = edgevar(&graph, outer2, "command", false).unwrap();
        assert_eq!(inner_command.as_bytes(), b"varref inner");
        assert_eq!(outer_command.as_bytes(), b"varref outer");
        assert_eq!(second_outer_command.as_bytes(), b"varref outer");
        fs::remove_dir_all(directory).unwrap();
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
        let value = crate::env::envvar(&graph, state.root, "var").unwrap();
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
        assert_eq!(graph.node(defaults[0]).path.as_bytes(), b"foo bar");
    }

    #[test]
    fn ninja_state_complex_target_is_preserved() {
        let (graph, _, _) = parse_source(
            "rule copy\n  command = cp $in $out\nname = foo %2F bar?baz&x=1\nbuild $name: copy foo\n",
        )
        .unwrap();
        let node = crate::graph::nodeget(&graph, b"foo %2F bar?baz&x=1").unwrap();
        assert_eq!(graph.node(node).path.as_bytes(), b"foo %2F bar?baz&x=1");
    }
}
