//! Manifest parser translated from `parse.c`.
//!
//! The parser is a front end over [`crate::frontend`] and nothing else: it
//! reads manifest bytes, expands them against the scopes it has declared, and
//! asks the graph-construction boundary for the rules, pools, edges, and
//! default targets they describe. Every failure the boundary reports is
//! located here, against the manifest token that asked for it.

use crate::error::{ManifestError, ManifestProblem, NameKind};
use crate::frontend::{BuildGraph, FrontendError, Node, Scope, Template};
use crate::scan::{
    scanchar, scanindent, scankeyword, scanname, scannewline, scanpaths, scanpipe, scanstring,
    AllowedSeparators, ScannedEvalPart, ScannedEvalString, Scanner, Separator, Source, TokenKind,
};
use crate::util::{canonpath, BStr, BString, ByteSlice};
use std::path::Path;

type ManifestResult<T> = Result<T, ManifestError>;

/// Where a manifest diagnostic points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Anchor {
    /// The token being read, which is where Ninja reports nearly everything.
    Token,
    /// Where scanning has reached.
    ///
    /// Several of Ninja's checks run only once a statement's indented block is
    /// over — a rule with no `command`, a pool with no `depth`, and everything
    /// a build statement's own bindings can change. By then its lexer has
    /// peeked for one more indented line and put back what it found instead,
    /// so scanning stands at the *start* of the line that ended the block —
    /// its leading spaces included, which is why an indented blank line there
    /// is named rather than the statement after it. The column is zero, so
    /// these carry no source context.
    AfterBlock,
}

fn manifest_error(
    scanner: &Scanner<'_>,
    anchor: Anchor,
    problem: ManifestProblem,
) -> ManifestError {
    let span = match anchor {
        Anchor::Token => scanner.last_token(),
        Anchor::AfterBlock => scanner.position(),
    };
    ManifestError::at(scanner.source_span(span), problem)
}

// [spec:ronin:def:parse.parseoptions]
/// How a Ninja manifest is read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestOptions {
    phony_cycle_warns: bool,
}

impl Default for ManifestOptions {
    fn default() -> Self {
        Self {
            phony_cycle_warns: true,
        }
    }
}

impl ManifestOptions {
    /// Whether a phony statement naming itself is filtered with a warning, or
    /// left in place so the cycle is reported as an error.
    ///
    /// Ninja's default is to warn: `CMake` 2.8.12 and 3.0 emitted these, and
    /// tolerating them is what `-w phonycycle=warn` buys. `=err` keeps the
    /// self-edge so dependency-cycle detection sees it.
    pub const fn warn_on_phony_cycle(&mut self, warn: bool) {
        self.phony_cycle_warns = warn;
    }
}

/// A Ninja manifest read into a graph.
pub struct Manifest {
    /// The graph the manifest describes.
    pub graph: BuildGraph,
    /// Diagnostics the manifest raised that did not stop it being read.
    pub warnings: Vec<String>,
}

// [spec:ronin:def:parse.parseinit-fn]
// [spec:ronin:sem:parse.parseinit-fn]
#[derive(Default)]
struct Parser {
    options: ManifestOptions,
    /// Diagnostics that do not stop the parse, in the order they were raised.
    ///
    /// The parser cannot write them itself: a manifest may be read through a
    /// library caller's sink, or through no sink at all, so where a warning
    /// goes is the invocation's decision rather than the parser's.
    warnings: Vec<String>,
    working_directory: crate::os::WorkingDirectory,
}

/// Reads a Ninja manifest into a graph, without reading a command line first.
///
/// `directory` is what relative paths in the manifest resolve against — the
/// directory Ninja would have been invoked from, not the manifest's own, since
/// a manifest names its includes and its outputs relative to the former.
///
/// # Errors
///
/// Returns an error when `directory` cannot be resolved, when the manifest or
/// anything it includes cannot be read, or when what they describe is not a
/// graph that can exist.
// [spec:ronin:req:frontend.graph-construction]
pub fn load_manifest(
    directory: impl AsRef<Path>,
    manifest: impl AsRef<Path>,
    options: ManifestOptions,
) -> Result<Manifest, crate::Error> {
    let directory = directory.as_ref();
    let working_directory = crate::os::WorkingDirectory::new(directory).map_err(|source| {
        crate::error::CliError::ChangeDirectory {
            path: BString::from(directory.to_string_lossy().as_bytes()),
            source,
        }
    })?;
    Ok(load_manifest_in(manifest, working_directory, options)?)
}

/// Read a manifest against a working directory the caller already resolved.
pub(crate) fn load_manifest_in(
    manifest: impl AsRef<Path>,
    working_directory: crate::os::WorkingDirectory,
    options: ManifestOptions,
) -> ManifestResult<Manifest> {
    let mut warnings = Vec::new();
    let graph = load_manifest_reporting(manifest, working_directory, options, &mut warnings)?;
    Ok(Manifest { graph, warnings })
}

/// The same, keeping the warnings raised before a failure that stopped it.
///
/// Ninja writes a warning the moment it raises one, so a manifest that warns
/// about its third statement and then fails on its fourth prints both. Ronin's
/// parser collects rather than writes, because where a diagnostic goes is the
/// invocation's decision — which means the collection has to outlive the
/// failure for the invocation to have anything left to decide about.
pub(crate) fn load_manifest_reporting(
    manifest: impl AsRef<Path>,
    working_directory: crate::os::WorkingDirectory,
    options: ManifestOptions,
    warnings: &mut Vec<String>,
) -> ManifestResult<BuildGraph> {
    let mut graph = BuildGraph::new();
    let mut parser = Parser {
        options,
        warnings: Vec::new(),
        working_directory,
    };
    let root = graph.root();
    let result = parse(manifest, &mut graph, &mut parser, root);
    warnings.append(&mut parser.warnings);
    result?;
    Ok(graph)
}

/// Restate a construction failure as the manifest problem it is, so the caller
/// can locate it where Ninja locates that particular one.
fn construction_problem(error: FrontendError) -> ManifestProblem {
    match error {
        FrontendError::EmptyPath => ManifestProblem::EmptyPath,
        FrontendError::EdgeWithoutOutputs => ManifestProblem::BuildWithoutOutputs,
        FrontendError::DuplicateOutput { path } => ManifestProblem::DuplicateOutput {
            path: BString::from(path),
        },
        FrontendError::RepeatedOutput { path } => ManifestProblem::RepeatedOutput {
            path: BString::from(path),
        },
        FrontendError::DuplicateRule { name } => ManifestProblem::DuplicateRule {
            name: BString::from(name),
        },
        FrontendError::DuplicatePool { name } => ManifestProblem::DuplicatePool {
            name: BString::from(name),
        },
        FrontendError::UnknownPool { name } => ManifestProblem::UnknownPoolName {
            name: BString::from(name),
        },
        FrontendError::DyndepNotInput { path } => ManifestProblem::DyndepNotInput {
            path: BString::from(path),
        },
        FrontendError::UncomposableSubninja { .. } => {
            unreachable!("only the Make compiler creates subninja construction errors")
        }
    }
}

// [spec:ronin:def:parse.parselet-fn]
// [spec:ronin:sem:parse.parselet-fn]
fn parselet<'source>(
    scanner: &mut Scanner<'source>,
) -> ManifestResult<(&'source BStr, ScannedEvalString<'source>)> {
    let name = scanname(scanner, NameKind::Variable)?;
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

/// What a scanned value expands against.
///
/// Almost everything expands against a scope alone. A build statement's paths
/// do not: Ninja evaluates them against a scope holding the statement's own
/// bindings, so `build $stem.o: cc $stem.c` sees a `stem` the statement itself
/// declares — which is why they cannot be expanded until its block is read.
#[derive(Clone, Copy)]
struct Env {
    scope: Scope,
    /// The statement whose bindings are searched first, for the paths of a
    /// build statement that has declared some.
    edge: Option<crate::frontend::Edge>,
}

impl Env {
    const fn scoped(scope: Scope) -> Self {
        Self { scope, edge: None }
    }

    fn variable<'graph>(self, graph: &'graph BuildGraph, name: &BStr) -> Option<&'graph [u8]> {
        self.edge
            .and_then(|edge| graph.edge_binding(edge, name))
            .or_else(|| graph.variable(self.scope, name))
    }
}

/// Expand one scanned value against `env`, appending to `out`.
///
/// A value the scanner found no `$` in is its own expansion, which is the
/// shape almost every path in a real manifest has.
fn expand_into(graph: &BuildGraph, env: Env, value: &ScannedEvalString<'_>, out: &mut Vec<u8>) {
    let parts = match value {
        ScannedEvalString::Plain(bytes) => {
            out.extend_from_slice(bytes);
            return;
        }
        ScannedEvalString::Parts(parts) => parts,
    };
    let capacity = parts
        .iter()
        .map(|part| match part {
            ScannedEvalPart::Literal(literal) => literal.len(),
            ScannedEvalPart::EscapedByte(_) => 1,
            ScannedEvalPart::Variable(name) => env.variable(graph, name).map_or(0, <[u8]>::len),
        })
        .sum();
    out.reserve(capacity);
    for part in parts {
        match part {
            ScannedEvalPart::Literal(literal) => out.extend_from_slice(literal),
            ScannedEvalPart::EscapedByte(byte) => out.push(*byte),
            ScannedEvalPart::Variable(name) => {
                if let Some(value) = env.variable(graph, name) {
                    out.extend_from_slice(value);
                }
            }
        }
    }
}

// [spec:ronin:def:env.enveval-fn]
// [spec:ronin:sem:env.enveval-fn]
fn expand(graph: &BuildGraph, scope: Scope, value: &ScannedEvalString<'_>) -> BString {
    let mut out = Vec::new();
    expand_into(graph, Env::scoped(scope), value, &mut out);
    BString::from(out)
}

/// Intern a scanned value's names so it can be expanded per edge instead.
fn template_for(graph: &mut BuildGraph, value: &ScannedEvalString<'_>) -> Template {
    let parts = match value {
        ScannedEvalString::Plain(bytes) => return Template::literal(bytes),
        ScannedEvalString::Parts(parts) => parts,
    };
    let mut template = Template::default();
    for part in parts {
        match part {
            ScannedEvalPart::Literal(literal) => template.push_literal(literal),
            ScannedEvalPart::EscapedByte(byte) => template.push_literal(&[*byte]),
            ScannedEvalPart::Variable(name) => {
                let name = graph.binding(name);
                template.push_variable(name);
            }
        }
    }
    template
}

// [spec:ronin:def:parse.parserule-fn]
// [spec:ronin:sem:parse.parserule-fn]
fn parserule(
    scanner: &mut Scanner<'_>,
    graph: &mut BuildGraph,
    scope: Scope,
) -> ManifestResult<()> {
    let name = scanname(scanner, NameKind::Rule)?.text;
    // Ninja tests for a duplicate as soon as the name's line ends, so the
    // diagnostic points at that line ending rather than at the end of the
    // whole block.
    scannewline(scanner)?;
    let named_at = scanner.source_span(scanner.last_token());
    let mut bindings = Vec::new();
    let mut command = false;
    let mut rspfile = false;
    let mut rspfile_content = false;
    while scanindent(scanner)? {
        let (binding, value) = parselet(scanner)?;
        if !matches!(
            &**binding,
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
                Anchor::Token,
                ManifestProblem::UnexpectedRuleVariable {
                    name: binding.to_str_lossy().into_owned(),
                },
            ));
        }
        command |= binding == "command";
        rspfile |= binding == "rspfile";
        rspfile_content |= binding == "rspfile_content";
        let value = template_for(graph, &value);
        bindings.push((graph.binding(binding), value));
    }
    if !command {
        return Err(manifest_error(
            scanner,
            Anchor::AfterBlock,
            ManifestProblem::RuleMissingCommand {
                name: name.to_str_lossy().into_owned(),
            },
        ));
    }
    if rspfile != rspfile_content {
        return Err(manifest_error(
            scanner,
            Anchor::AfterBlock,
            ManifestProblem::IncompleteResponseFileBinding {
                name: name.to_str_lossy().into_owned(),
            },
        ));
    }
    graph
        .define_rule(scope, name, bindings)
        .map_err(|error| ManifestError::at(named_at, construction_problem(error)))?;
    Ok(())
}

fn evaluated_path(
    scanner: &Scanner<'_>,
    graph: &BuildGraph,
    path: &ScannedEvalString<'_>,
    scope: Scope,
) -> ManifestResult<BString> {
    let value = expand(graph, scope, path);
    if value.is_empty() {
        return Err(manifest_error(
            scanner,
            Anchor::Token,
            ManifestProblem::EmptyPath,
        ));
    }
    Ok(value)
}

/// A failure the graph reported about a build statement, located where Ninja
/// stands once that statement's bindings have been read.
fn edge_problem(scanner: &Scanner<'_>, error: FrontendError) -> ManifestError {
    manifest_error(scanner, Anchor::AfterBlock, construction_problem(error))
}

/// Expand one of a build statement's path references and intern it, reusing
/// `scratch`.
///
/// Most references name a path that is already interned, so expanding into a
/// shared buffer and interning from bytes leaves the common case allocating
/// nothing at all — and a reference that needs no expansion is handed to the
/// boundary as the manifest bytes themselves.
fn node_for(
    scanner: &Scanner<'_>,
    graph: &mut BuildGraph,
    path: &ScannedEvalString<'_>,
    env: Env,
    scratch: &mut Vec<u8>,
) -> ManifestResult<Node> {
    let interned = match path {
        ScannedEvalString::Plain(bytes) => graph.node(bytes),
        ScannedEvalString::Parts(_) => {
            scratch.clear();
            expand_into(graph, env, path, scratch);
            graph.node(scratch)
        }
    };
    interned.map_err(|error| edge_problem(scanner, error))
}

// [spec:ronin:def:parse.parseedge-fn]
// [spec:ronin:sem:parse.parseedge-fn]
#[allow(
    clippy::too_many_lines,
    reason = "a complete Ninja build production shares one scanner cursor across its clauses"
)]
fn parseedge(
    scanner: &mut Scanner<'_>,
    graph: &mut BuildGraph,
    scope: Scope,
    parser: &mut Parser,
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
            Anchor::Token,
            ManifestProblem::BuildWithoutOutputs,
        ));
    }
    scanchar(scanner, ':')?;
    let rule_name = scanname(scanner, NameKind::BuildCommand)?.text;
    let rule = graph.rule(scope, rule_name).ok_or_else(|| {
        manifest_error(
            scanner,
            Anchor::Token,
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
        let (name, value) = parselet(scanner)?;
        // Ninja expands each binding against the scope around the statement
        // rather than against the statement, so the bindings cannot see each
        // other however they are ordered.
        let value = expand(graph, scope, &value);
        bindings.push((graph.binding(name), Vec::from(value)));
    }

    // Everything still unchecked about the statement needs a value its own
    // bindings can change, so this is where Ninja checks all of it: the pool
    // it names, the paths it expands to, and the dyndep among them. Its lexer
    // has read past the block by now, which is why each of these diagnostics
    // names the line that *ended* the block and shows no source context — the
    // column is zero there — and why they are anchored past the block rather
    // than at the token that raised them.
    //
    // The order is Ninja's and it is observable, because a statement wrong in
    // two ways is reported by whichever it reaches first.
    let edge = graph.begin_edge(scope, rule, bindings);
    let env = Env {
        scope,
        edge: Some(edge),
    };
    graph
        .resolve_pool(edge)
        .map_err(|error| edge_problem(scanner, error))?;
    for path in &output_paths {
        let output = node_for(scanner, graph, path, env, scratch)?;
        graph
            .attach_output(edge, output)
            .map_err(|error| edge_problem(scanner, error))?;
    }
    graph.set_explicit_outputs(edge, explicit_output_count);
    for path in &input_paths {
        let input = node_for(scanner, graph, path, env, scratch)?;
        graph.attach_input(edge, input);
    }
    graph.set_input_partitions(edge, explicit_input_count, non_order_only_input_count);
    for path in &validation_paths {
        let validation = node_for(scanner, graph, path, env, scratch)?;
        graph.attach_validation(edge, validation);
    }

    // Only warn mode filters the self-reference; under `=err` it is left in
    // place so cycle detection reports it, which is the whole point of the
    // flag. Ninja filters before it resolves the dyndep, so a phony statement
    // whose dyndep is its own output is rejected rather than accepted.
    if parser.options.phony_cycle_warns {
        if let Some(output) = graph.drop_phony_self_reference(edge) {
            parser.warnings.push(format!(
                "phony target '{}' names itself as an input; ignoring [-w phonycycle=warn]",
                graph.path(output).as_bstr()
            ));
        }
    }
    graph
        .resolve_dyndep(edge)
        .map_err(|error| edge_problem(scanner, error))?;
    Ok(())
}

// [spec:ronin:def:parse.parseinclude-fn]
// [spec:ronin:sem:parse.parseinclude-fn]
fn parseinclude(
    scanner: &mut Scanner<'_>,
    graph: &mut BuildGraph,
    parser: &mut Parser,
    scope: Scope,
    newscope: bool,
) -> ManifestResult<()> {
    // An `include` that names nothing is not a syntax error. Ninja evaluates
    // whatever it read and tries to open it, so a bare `include` fails as a
    // file that is not there — under the empty name it asked for.
    let path = scanstring(scanner, true)?.unwrap_or_default();
    // Taken before the newline is read, so a file that cannot be loaded is
    // reported against the line that asked for it.
    let asked_at = scanner.source_span(scanner.last_token());
    scannewline(scanner)?;
    let path = expand(graph, scope, &path);
    let scope = if newscope {
        graph.child_scope(scope)
    } else {
        scope
    };
    parse_at(
        path.to_path().expect("byte paths are valid on Unix"),
        graph,
        parser,
        scope,
        Some(asked_at),
    )
}

// [spec:ronin:def:parse.parsedefault-fn]
// [spec:ronin:sem:parse.parsedefault-fn]
fn parsedefault(
    scanner: &mut Scanner<'_>,
    graph: &mut BuildGraph,
    scope: Scope,
) -> ManifestResult<()> {
    let targets = scanpaths(scanner)?;
    scannewline(scanner)?;
    if targets.is_empty() {
        return Err(manifest_error(
            scanner,
            Anchor::Token,
            ManifestProblem::ExpectedTargetName,
        ));
    }
    for target in targets {
        let mut target = evaluated_path(scanner, graph, &target, scope)?;
        canonpath(&mut target);
        let node = graph.lookup(target.as_bytes()).ok_or_else(|| {
            manifest_error(
                scanner,
                Anchor::Token,
                ManifestProblem::UnknownTarget {
                    path: target.clone(),
                },
            )
        })?;
        graph.add_default(node);
    }
    Ok(())
}

// [spec:ronin:def:parse.parsepool-fn]
// [spec:ronin:sem:parse.parsepool-fn]
fn parsepool(
    scanner: &mut Scanner<'_>,
    graph: &mut BuildGraph,
    scope: Scope,
) -> ManifestResult<()> {
    let name = scanname(scanner, NameKind::Pool)?.text;
    let named_at = scanner.source_span(scanner.position());
    let pool = graph
        .define_pool(name)
        .map_err(|error| ManifestError::at(named_at, construction_problem(error)))?;
    scannewline(scanner)?;
    while scanindent(scanner)? {
        let (name, value) = parselet(scanner)?;
        if name != "depth" {
            return Err(manifest_error(
                scanner,
                Anchor::Token,
                ManifestProblem::UnexpectedPoolVariable {
                    name: name.to_str_lossy().into_owned(),
                },
            ));
        }
        let value = expand(graph, scope, &value);
        let depth = String::from_utf8_lossy(value.as_bytes())
            .parse()
            .ok()
            .and_then(std::num::NonZeroUsize::new)
            .ok_or_else(|| {
                manifest_error(
                    scanner,
                    Anchor::Token,
                    ManifestProblem::InvalidPoolDepth { value },
                )
            })?;
        graph.set_pool_depth(pool, depth);
    }
    if graph.pool_depth(pool).is_none() {
        return Err(manifest_error(
            scanner,
            Anchor::AfterBlock,
            ManifestProblem::PoolWithoutDepth,
        ));
    }
    Ok(())
}

// [spec:ronin:def:parse.checkversion-fn]
// [spec:ronin:sem:parse.checkversion-fn]
/// The leading digits of `bytes` as a number, or zero when there are none.
///
/// This is C's `atoi`, which is what Ninja reads a version component with, and
/// the reason a `ninja_required_version` cannot be malformed: anything that is
/// not a number reads as zero, and a manifest requiring version zero is simply
/// an old one. A component too large to hold saturates instead of wrapping, so
/// an absurd requirement stays a requirement this cannot meet.
fn version_component(bytes: &[u8]) -> i32 {
    let digits = bytes
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(bytes.len());
    if digits == 0 {
        return 0;
    }
    std::str::from_utf8(&bytes[..digits])
        .unwrap_or_default()
        .parse()
        .unwrap_or(i32::MAX)
}

fn checkversion(scanner: &Scanner<'_>, version: &BStr) -> ManifestResult<(i32, i32)> {
    let bytes = version.as_bytes();
    let (major, rest) = bytes
        .iter()
        .position(|byte| *byte == b'.')
        .map_or((bytes, &[][..]), |dot| (&bytes[..dot], &bytes[dot + 1..]));
    let major = version_component(major);
    let minor_end = rest
        .iter()
        .position(|byte| *byte == b'.')
        .unwrap_or(rest.len());
    let minor = version_component(&rest[..minor_end]);
    if major > crate::cli::NINJA_COMPAT_MAJOR
        || major == crate::cli::NINJA_COMPAT_MAJOR && minor > crate::cli::NINJA_COMPAT_MINOR
    {
        Err(manifest_error(
            scanner,
            Anchor::Token,
            ManifestProblem::RequiredVersionTooNew {
                version: BString::from(bytes),
            },
        ))
    } else {
        Ok((major, minor))
    }
}

// [spec:ronin:def:parse.parse-fn]
// [spec:ronin:sem:parse.parse-fn]
// [spec:ronin:req:compat.manifest-semantics]
fn parse(
    name: impl AsRef<Path>,
    graph: &mut BuildGraph,
    parser: &mut Parser,
    scope: Scope,
) -> ManifestResult<()> {
    // The manifest named on the command line: nothing in a manifest points at
    // it, so a failure to read it has nowhere to point back to.
    parse_at(name, graph, parser, scope, None)
}

fn parse_at(
    name: impl AsRef<Path>,
    graph: &mut BuildGraph,
    parser: &mut Parser,
    scope: Scope,
    located: Option<crate::source::SourceSpan>,
) -> ManifestResult<()> {
    let path = name.as_ref().to_owned();
    let input = std::fs::read(parser.working_directory.resolve(&path)).map_err(|error| {
        // Ninja reports this against the manifest that asked for the file, not
        // as a bare I/O failure with nothing to locate it by.
        match located {
            Some(span) => ManifestError::at(
                span,
                ManifestProblem::LoadFailed {
                    path: BString::from(path.to_string_lossy().as_bytes()),
                    reason: crate::error::system_message(&error),
                },
            ),
            None => ManifestError::read(&path, error),
        }
    })?;
    let source = Source::from_bytes(&path, input);
    let mut scanner = Scanner::new(&source);
    // One expansion buffer per manifest, reused by every path in it, so a
    // manifest of statements that need expanding allocates once rather than
    // once per path.
    let mut scratch = Vec::new();
    while let Some(token) = scankeyword(&mut scanner)? {
        match token.kind {
            TokenKind::Rule => parserule(&mut scanner, graph, scope)?,
            TokenKind::Build => {
                parseedge(&mut scanner, graph, scope, parser, &mut scratch)?;
            }
            TokenKind::Include => parseinclude(&mut scanner, graph, parser, scope, false)?,
            TokenKind::Subninja => parseinclude(&mut scanner, graph, parser, scope, true)?,
            TokenKind::Default => parsedefault(&mut scanner, graph, scope)?,
            TokenKind::Pool => parsepool(&mut scanner, graph, scope)?,
            TokenKind::Variable => {
                let name = token.lexeme.text;
                let value = parse_assignment(&mut scanner)?;
                let value = expand(graph, scope, &value);
                if name == "ninja_required_version" {
                    let (major, minor) = checkversion(&scanner, BStr::new(value.as_bytes()))?;
                    // Ninja accepts a manifest written for an older major but
                    // says so, because the language it was written against is
                    // not the one about to interpret it.
                    if crate::cli::NINJA_COMPAT_MAJOR > major {
                        parser.warnings.push(format!(
                            "ninja executable version ({}) greater than build file \
                             ninja_required_version ({}); versions may be incompatible.",
                            crate::cli::NINJA_COMPAT_VERSION,
                            value.as_bstr()
                        ));
                    }
                    scanner.set_manifest_version(major, minor);
                }
                graph.bind(scope, name, Vec::from(value));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod ninja_manifest_tests {
    use super::*;
    use crate::env::edgevar;
    use crate::graph::{Graph, PathStyle};
    use crate::names::Names;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_MANIFEST: AtomicUsize = AtomicUsize::new(0);

    fn parse_source(source: &str) -> ManifestResult<Manifest> {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-parser-{}-{}.ninja",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, source).unwrap();
        let result = parse_path(&path);
        fs::remove_file(path).unwrap();
        result
    }

    fn parse_path(path: &Path) -> ManifestResult<Manifest> {
        load_manifest_in(
            path,
            crate::os::WorkingDirectory::default(),
            ManifestOptions::default(),
        )
    }

    fn parse_graph(source: &str) -> ManifestResult<Graph> {
        parse_source(source).map(|manifest| manifest.graph.into_arenas())
    }

    fn parse_graph_at(path: &Path) -> ManifestResult<Graph> {
        parse_path(path).map(|manifest| manifest.graph.into_arenas())
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
        let graph = parse_graph(source).unwrap();
        let edge = output_edge(&graph, b"result");
        let dyndep = graph.edge(edge).dyndep.unwrap();
        assert_eq!(graph.node_path(dyndep).as_bytes(), expected);
        let runtime = crate::runtime::RuntimeState::new(&graph);
        assert!(runtime.node(dyndep).dyndep_pending());
    }

    #[test]
    fn ninja_manifest_parser_validations() {
        let graph =
            parse_graph("rule cat\n  command = cat $in > $out\nbuild foo: cat bar |@ baz\n")
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
        let graph = parse_graph("rule cat\n  command = cat $in > $out\nbuild foo | imp: cat bar\n")
            .unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(graph.edge(edge).out.len(), 2);
        assert_eq!(graph.edge(edge).explicit_output_count(), 1);
        assert_eq!(edge, output_edge(&graph, b"foo"));
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_empty() {
        let graph =
            parse_graph("rule cat\n  command = cat $in > $out\nbuild foo | : cat bar\n").unwrap();
        let edge = output_edge(&graph, b"foo");
        assert_eq!(graph.edge(edge).out.len(), 1);
        assert_eq!(graph.edge(edge).explicit_output_count(), 1);
    }

    #[test]
    fn ninja_manifest_parser_no_explicit_output() {
        let graph =
            parse_graph("rule cat\n  command = cat $in > $out\nbuild | imp: cat bar\n").unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(graph.edge(edge).out.len(), 1);
        assert_eq!(graph.edge(edge).explicit_output_count(), 0);
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_duplicate_error() {
        // One statement naming an output twice is its own complaint: nothing
        // elsewhere generates it, so "multiple rules generate" would be a lie.
        let error = parse_error(
            "rule cat\n  command = cat $in > $out\nbuild foo baz | foo baq foo: cat bar\n",
        );
        assert!(
            error.ends_with(":4: foo is defined as an output multiple times\n"),
            "{error}"
        );
    }

    #[test]
    fn ninja_manifest_parser_phony_self_reference_ignored() {
        let graph = parse_graph("build a: phony a\n").unwrap();
        let edge = output_edge(&graph, b"a");
        assert!(graph.edge(edge).input.is_empty());
        assert!(graph
            .node(crate::graph::nodeget(&graph, b"a").unwrap())
            .uses
            .is_empty());
    }

    #[test]
    fn ninja_manifest_parser_reserved_words() {
        let manifest = parse_source(
            "rule build\n  command = rule run $out\nbuild subninja: build include default foo.cc\ndefault subninja\n",
        )
        .unwrap();
        assert!(manifest.graph.lookup(b"subninja").is_some());
        assert_eq!(manifest.graph.default_targets().len(), 1);
    }

    #[test]
    fn ninja_manifest_parser_dyndep_not_specified() {
        let graph =
            parse_graph("rule cat\n  command = cat $in > $out\nbuild result: cat in\n").unwrap();
        assert!(graph.edge(output_edge(&graph, b"result")).dyndep.is_none());
    }

    #[test]
    fn ninja_manifest_parser_dyndep_not_input() {
        let error = parse_error(
            "rule touch\n  command = touch $out\nbuild result: touch\n  dyndep = notin\n",
        );
        assert!(
            error.contains(": dyndep 'notin' is not an input"),
            "{error}"
        );
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
        let graph = parse_graph(
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
        let duplicate = parse_error("pool console\n  depth = 2\n");
        assert!(
            duplicate
                .ends_with(":1: duplicate pool 'console'\npool console\n            ^ near here"),
            "{duplicate}"
        );
        assert!(parse_source(
            "rule run\n  command = echo\n  pool = unnamed_pool\nbuild out: run in\n"
        )
        .is_err());
    }

    #[test]
    fn ninja_manifest_parser_rejects_unknown_rule_binding() {
        let error = parse_error("rule cc\n  command = foo\n  othervar = bar\n");
        assert!(
            error.contains(": unexpected variable 'othervar'"),
            "{error}"
        );
    }

    #[test]
    fn ninja_manifest_parser_default_cycle_has_no_root() {
        let manifest =
            parse_source("rule cat\n  command = cat $in > $out\nbuild a: cat a\n").unwrap();
        assert!(manifest.graph.default_targets().is_empty());
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
    // [spec:ronin:req:compat.manifest-semantics/test]
    fn ninja_manifest_parser_preserves_non_utf8_paths() {
        let directory = temporary_directory("non-utf8");
        let path = directory.join("build.ninja");
        fs::write(
            &path,
            b"rule cat\n  command = cat $in > $out\nbuild out-\xff: cat in-\xfe\n",
        )
        .unwrap();
        let graph = parse_graph_at(&path).unwrap();
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
        let graph = parse_graph_at(&root).unwrap();
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
        let graph = parse_graph_at(&root).unwrap();
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
        assert!(
            error.ends_with(":1: duplicate rule 'phony'\nrule phony\n          ^ near here"),
            "{error}"
        );
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
        let graph = parse_graph_at(&root).unwrap();
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
        let graph = parse_path(&root).unwrap().graph;
        let value = graph.variable(graph.root(), b"var").unwrap();
        assert_eq!(value, b"inner");
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
        assert!(error.to_string().contains("multiple rules generate out1"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_output_is_always_fatal() {
        // `-w dupbuild=warn` no longer suppresses this: Ninja deprecated the
        // flag and made duplicate outputs unconditionally fatal, so accepting
        // the manifest here would be the divergence.
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-duplicate-{}-{}.ninja",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            "rule cat\n  command = cat\nbuild out: cat in1\nbuild out: cat in2\n",
        )
        .unwrap();
        let Err(error) = parse_path(&path) else {
            panic!("a duplicate output unexpectedly parsed");
        };
        assert!(
            error.to_string().contains("multiple rules generate"),
            "{error}"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_rejects_unterminated_lines() {
        // Located like every other lexer diagnostic, and worded the same
        // whether or not a line continuation is what ran off the end: Ninja
        // reaches the same `unexpected EOF` from both.
        let unterminated = parse_error("x = 3");
        assert!(
            unterminated.ends_with(":1: unexpected EOF\nx = 3\n     ^ near here"),
            "{unterminated}"
        );
        // The continuation puts the end of the file on the line after it, where
        // the column is zero and the context is dropped.
        let continued = parse_error("x = $\n");
        assert!(continued.ends_with(":2: unexpected EOF\n"), "{continued}");
        let continued = parse_error("x = a$\n b$\n $\n");
        assert!(continued.ends_with(":4: unexpected EOF\n"), "{continued}");
    }

    #[test]
    fn ninja_manifest_parser_indented_blank_terminates_rule() {
        assert!(parse_source("rule r\n  command = r\n  \n  generator = 1\n").is_err());
    }

    #[test]
    fn ninja_manifest_parser_default_escaped_space() {
        let manifest = parse_source(
            "rule cat\n  command = cat\nbuild foo$ bar: cat input\ndefault foo$ bar\n",
        )
        .unwrap();
        let defaults = manifest.graph.default_targets();
        assert_eq!(defaults.len(), 1);
        assert_eq!(manifest.graph.path(defaults[0]), b"foo bar");
    }

    #[test]
    fn ninja_state_complex_target_is_preserved() {
        let graph = parse_graph(
            "rule copy\n  command = cp $in $out\nname = foo %2F bar?baz&x=1\nbuild $name: copy foo\n",
        )
        .unwrap();
        let node = crate::graph::nodeget(&graph, b"foo %2F bar?baz&x=1").unwrap();
        assert_eq!(graph.node_path(node).as_bytes(), b"foo %2F bar?baz&x=1");
    }
}
