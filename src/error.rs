use crate::graph::{EdgeId, NodeId};
use crate::source::SourceSpan;
use crate::util::BString;
use std::error;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

/// The subsystem that originated a Ronin error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Command-line parsing or invocation setup failed.
    Cli,
    /// Manifest scanning, parsing, or dyndep loading failed.
    Manifest,
    /// Graph construction or evaluation failed.
    Graph,
    /// Build planning or execution failed.
    Build,
    /// A Ninja-compatible persistent state or filesystem operation failed.
    Persistence,
    /// Process supervision or jobserver integration failed.
    Process,
    /// A Ninja tool-mode operation failed.
    Tool,
}

/// A structured Ronin failure.
///
/// `Display` remains the Ninja-compatible diagnostic boundary. [`Error::kind`]
/// is derived from the underlying semantic variant, and [`error::Error::source`]
/// exposes the typed subsystem error and its retained source chain.
// [spec:ronin:req:runtime.semantic-errors]
#[derive(Debug)]
pub struct Error(ErrorRepr);

#[derive(Debug)]
enum ErrorRepr {
    Cli(CliError),
    Manifest(ManifestError),
    Graph(GraphError),
    Build(BuildError),
    Persistence(PersistenceError),
    Process(ProcessError),
    Tool(ToolError),
}

mod build;

impl Error {
    /// Returns the subsystem that originated this failure.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match &self.0 {
            ErrorRepr::Cli(_) => ErrorKind::Cli,
            ErrorRepr::Manifest(error) => error.kind(),
            ErrorRepr::Graph(_) => ErrorKind::Graph,
            ErrorRepr::Build(error) => error.kind(),
            ErrorRepr::Persistence(_) => ErrorKind::Persistence,
            ErrorRepr::Process(_) => ErrorKind::Process,
            ErrorRepr::Tool(error) => error.kind(),
        }
    }

    /// Add context that belongs to the executable without changing the error
    /// exposed by the reusable library entry points.
    pub(crate) const fn at_process_boundary(mut self) -> Self {
        let manifest = match &mut self.0 {
            ErrorRepr::Manifest(error) | ErrorRepr::Build(BuildError::Manifest(error)) => {
                Some(error)
            }
            _ => None,
        };
        if let Some(ManifestError::Read {
            process_context, ..
        }) = manifest
        {
            *process_context = true;
        }
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            ErrorRepr::Cli(error) => error.fmt(formatter),
            ErrorRepr::Manifest(error) => error.fmt(formatter),
            ErrorRepr::Graph(error) => error.fmt(formatter),
            ErrorRepr::Build(error) => error.fmt(formatter),
            ErrorRepr::Persistence(error) => error.fmt(formatter),
            ErrorRepr::Process(error) => error.fmt(formatter),
            ErrorRepr::Tool(error) => error.fmt(formatter),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match &self.0 {
            ErrorRepr::Cli(error) => error.source(),
            ErrorRepr::Manifest(error) => error.source(),
            ErrorRepr::Graph(error) => error.source(),
            ErrorRepr::Build(error) => error.source(),
            ErrorRepr::Persistence(error) => error.source(),
            ErrorRepr::Process(error) => error.source(),
            ErrorRepr::Tool(error) => error.source(),
        }
    }
}

macro_rules! into_public_error {
    ($variant:ident, $source:ty) => {
        impl From<$source> for Error {
            fn from(error: $source) -> Self {
                Self(ErrorRepr::$variant(error))
            }
        }
    };
}

into_public_error!(Cli, CliError);
into_public_error!(Manifest, ManifestError);
into_public_error!(Graph, GraphError);
into_public_error!(Build, BuildError);
into_public_error!(Persistence, PersistenceError);
into_public_error!(Process, ProcessError);
into_public_error!(Tool, ToolError);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EncodingContext {
    Argument,
    BuildDirectory,
    ChangeDirectory,
    ManifestPath,
    StatusValue,
    JobsValue,
    KeepGoingValue,
    LoadValue,
    DebugValue,
    WarningValue,
    ToolValue,
}

#[allow(
    dead_code,
    reason = "semantic metadata is retained for matching without changing Ninja-compatible Display text"
)]
#[derive(Debug)]
pub(crate) enum CliError {
    UnknownDebugFlag {
        flag: String,
    },
    UnknownWarningFlag {
        flag: String,
    },
    InvalidParameter {
        option: &'static str,
    },
    KeepGoingNotNumeric,
    UnknownStatusVariable {
        name: String,
    },
    UnknownOptionValue {
        option: &'static str,
        value: String,
    },
    InvalidStatusEscape,
    UnterminatedStatusVariable,
    MissingOptionValue {
        option: String,
    },
    InvalidEncoding {
        context: EncodingContext,
    },
    CurrentDirectory {
        source: io::Error,
    },
    ReadInput {
        source: io::Error,
    },
    ChangeDirectory {
        path: BString,
        source: io::Error,
    },
    /// Writing to a caller-supplied output or diagnostic sink failed.
    WriteOutput {
        source: io::Error,
    },
    InvocationFailed {
        exit_code: i32,
        diagnostic: String,
    },
    ManifestRetryLimit {
        path: BString,
        attempts: usize,
    },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDebugFlag { flag } => write!(formatter, "unknown debug flag '{flag}'"),
            Self::UnknownWarningFlag { flag } => {
                write!(formatter, "unknown warning flag '{flag}'")
            }
            Self::InvalidParameter { option } => write!(formatter, "invalid {option} parameter"),
            Self::KeepGoingNotNumeric => {
                formatter.write_str("-k parameter not numeric; did you mean -k 0?")
            }
            Self::UnknownStatusVariable { name } => {
                write!(formatter, "unknown variable '{name}' in --status format")
            }
            Self::UnknownOptionValue { option, value } => {
                write!(formatter, "invalid {option} value '{value}'")
            }
            Self::InvalidStatusEscape => formatter
                .write_str("invalid --status: bad $-escape (literal $ must be written as $$)"),
            Self::UnterminatedStatusVariable => {
                formatter.write_str("invalid --status: unterminated variable")
            }
            Self::MissingOptionValue { option } => write!(formatter, "missing {option} value"),
            Self::InvalidEncoding { context } => formatter.write_str(match context {
                EncodingContext::Argument => {
                    "argument is not representable as bytes on this platform"
                }
                EncodingContext::BuildDirectory => {
                    "build directory is not representable on this platform"
                }
                EncodingContext::ChangeDirectory => "-C path is not representable on this platform",
                EncodingContext::ManifestPath => {
                    "manifest path is not representable on this platform"
                }
                EncodingContext::StatusValue => "invalid --status value",
                EncodingContext::JobsValue => "invalid -j parameter",
                EncodingContext::KeepGoingValue => "invalid -k parameter",
                EncodingContext::LoadValue => "invalid -l parameter",
                EncodingContext::DebugValue => "invalid -d parameter",
                EncodingContext::WarningValue => "invalid -w parameter",
                EncodingContext::ToolValue => "invalid -t parameter",
            }),
            Self::CurrentDirectory { source }
            | Self::ReadInput { source }
            | Self::WriteOutput { source } => formatter.write_str(&system_message(source)),
            // Ninja reports this one through `Fatal`, quoting the directory it
            // was asked for and the system's own message for why it could not
            // be entered.
            Self::ChangeDirectory { path, source } => write!(
                formatter,
                "fatal: chdir to '{path}' - {}",
                system_message(source)
            ),
            Self::InvocationFailed { diagnostic, .. } => formatter.write_str(diagnostic),
            Self::ManifestRetryLimit { path, attempts } => {
                write!(formatter, "manifest '{path}' dirty after {attempts} tries")
            }
        }
    }
}

impl error::Error for CliError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::CurrentDirectory { source }
            | Self::ReadInput { source }
            | Self::ChangeDirectory { source, .. }
            | Self::WriteOutput { source } => Some(source),
            _ => None,
        }
    }
}

impl CliError {
    pub(crate) const fn write_output(source: io::Error) -> Self {
        Self::WriteOutput { source }
    }
}

/// Renders a system error the way `strerror` does.
///
/// Ninja quotes `strerror(errno)` directly and so does GNU Make. Rust's
/// `io::Error` prints the same text with ` (os error N)` appended, which
/// neither carries, so the suffix it added is removed again rather than the
/// message being rebuilt from the raw code. Taken off the rendered text rather
/// than off the error, because a source that owns one renders it inside a
/// sentence of its own.
pub(crate) fn system_message(error: &(impl fmt::Display + ?Sized)) -> String {
    const OS_ERROR: &str = " (os error ";
    let rendered = error.to_string();
    let mut kept = String::with_capacity(rendered.len());
    let mut rest = rendered.as_str();
    while let Some(open) = rest.find(OS_ERROR) {
        let number = &rest[open + OS_ERROR.len()..];
        let Some(close) = number.find(')') else { break };
        let code = &number[..close];
        if code.is_empty() || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        kept.push_str(&rest[..open]);
        rest = &number[close + 1..];
    }
    kept.push_str(rest);
    kept
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeparatorKind {
    Implicit,
    OrderOnly,
    Validation,
}

impl fmt::Display for SeparatorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Implicit => "|",
            Self::OrderOnly => "||",
            Self::Validation => "|@",
        })
    }
}

/// What a name was being read for, so a diagnostic can say so.
///
/// Ninja names the grammar position rather than reporting a bare `expected
/// name`, which on its own leaves the reader to work out which of the several
/// names on the line was wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NameKind {
    Pool,
    Rule,
    Variable,
    BuildCommand,
}

/// A token as Ninja names it when reporting one it did not expect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FoundToken {
    Newline,
    Identifier,
    Indent,
    Eof,
    Character(char),
    /// A dependency separator, which is one token however many bytes it is.
    Separator(SeparatorKind),
    /// A byte no token starts with, which Ninja reports as having read its
    /// error token rather than as having read anything in particular.
    LexingError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScanErrorKind {
    TabsNotAllowed,
    LexingError,
    ExpectedName(NameKind),
    UnexpectedToken(FoundToken),
    InvalidVariableName,
    CaretEscapeRequiresVersion,
    InvalidDollarEscape,
    ExpectedAsciiToken,
    ExpectedCharacter { expected: char, found: FoundToken },
    UnexpectedEof,
    ExpectedNewline(FoundToken),
}

impl fmt::Display for FoundToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Newline => formatter.write_str("newline"),
            Self::Identifier => formatter.write_str("identifier"),
            Self::Indent => formatter.write_str("indent"),
            Self::Eof => formatter.write_str("eof"),
            Self::Character(found) => write!(formatter, "'{found}'"),
            Self::Separator(found) => write!(formatter, "'{found}'"),
            Self::LexingError => formatter.write_str("lexing error"),
        }
    }
}

impl fmt::Display for ScanErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // What Ninja says about a byte no token can start with. It names
            // the tab because a tab where a statement belongs is nearly always
            // a Makefile habit, and says nothing else about anything else.
            Self::TabsNotAllowed => formatter.write_str("tabs are not allowed, use spaces"),
            Self::LexingError => formatter.write_str("lexing error"),
            Self::ExpectedName(kind) => formatter.write_str(match kind {
                NameKind::Pool => "expected pool name",
                NameKind::Rule => "expected rule name",
                NameKind::Variable => "expected variable name",
                NameKind::BuildCommand => "expected build command name",
            }),
            // A token that reads fine and belongs somewhere else. Ninja names
            // it and says no more, an indent included.
            Self::UnexpectedToken(found) => write!(formatter, "unexpected {found}"),
            // Ninja reports every malformed `$` the same way, terminated or not.
            Self::InvalidVariableName | Self::InvalidDollarEscape => {
                formatter.write_str("bad $-escape (literal $ must be written as $$)")
            }
            Self::CaretEscapeRequiresVersion => formatter.write_str(
                "using $^ escape requires specifying 'ninja_required_version' with version greater or equal 1.14",
            ),
            Self::ExpectedAsciiToken => formatter.write_str("expected ASCII token"),
            // Ninja names what it found as well as what it wanted, and adds a
            // hint for the colon because a path containing one is the usual
            // reason to land here.
            Self::ExpectedCharacter { expected, found } => {
                write!(formatter, "expected '{expected}', got {found}")?;
                if *expected == ':' {
                    formatter.write_str(" ($ also escapes ':')")?;
                }
                Ok(())
            }
            // Ninja does not distinguish an end of file reached after a line
            // continuation from any other, and neither does this.
            Self::UnexpectedEof => formatter.write_str("unexpected EOF"),
            // Names what it found, as every other token Ninja expected does.
            Self::ExpectedNewline(found) => write!(formatter, "expected newline, got {found}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScanError {
    pub(crate) span: SourceSpan,
    pub(crate) kind: ScanErrorKind,
}

impl ScanError {
    pub(crate) const fn diagnostic(&self) -> &ScanErrorKind {
        &self.kind
    }
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_located(formatter, &self.span, &self.kind)
    }
}

impl error::Error for ScanError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ManifestProblem {
    UnexpectedRuleVariable { name: String },
    RuleMissingCommand { name: String },
    IncompleteResponseFileBinding { name: String },
    EmptyPath,
    BuildWithoutOutputs,
    UndefinedRule { name: String },
    DuplicateOutput { path: BString },
    RepeatedOutput { path: BString },
    DyndepNotInput { path: BString },
    ExpectedTargetName,
    UnknownTarget { path: BString },
    UnexpectedPoolVariable { name: String },
    InvalidPoolDepth { value: BString },
    PoolWithoutDepth,
    RequiredVersionTooNew { version: BString },
    DuplicateRule { name: BString },
    DuplicatePool { name: BString },
    UnknownPoolName { name: BString },
    LoadFailed { path: BString, reason: String },
}

impl fmt::Display for ManifestProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Ninja does not say which kind of block the binding was in; the
            // located line above already points at it.
            Self::UnexpectedRuleVariable { name } | Self::UnexpectedPoolVariable { name } => {
                write!(formatter, "unexpected variable '{name}'")
            }
            Self::RuleMissingCommand { .. } => formatter.write_str("expected 'command =' line"),
            Self::IncompleteResponseFileBinding { .. } => {
                formatter.write_str("rspfile and rspfile_content need to be both specified")
            }
            Self::EmptyPath => formatter.write_str("empty path"),
            Self::BuildWithoutOutputs => formatter.write_str("expected path"),
            Self::UndefinedRule { name } => write!(formatter, "unknown build rule '{name}'"),
            // Unquoted, as Ninja writes it. Two build statements generating one
            // path and one statement naming it twice are separate complaints:
            // the second is a typo in the line being read, not a conflict with
            // something elsewhere, and Ninja says so.
            Self::DuplicateOutput { path } => write!(formatter, "multiple rules generate {path}"),
            Self::RepeatedOutput { path } => {
                write!(formatter, "{path} is defined as an output multiple times")
            }
            Self::DyndepNotInput { path } => write!(formatter, "dyndep '{path}' is not an input"),
            Self::ExpectedTargetName => formatter.write_str("expected target name"),
            Self::UnknownTarget { path } => write!(formatter, "unknown target '{path}'"),
            Self::InvalidPoolDepth { .. } => formatter.write_str("invalid pool depth"),
            Self::PoolWithoutDepth => formatter.write_str("expected 'depth =' line"),
            Self::DuplicateRule { name } => write!(formatter, "duplicate rule '{name}'"),
            Self::DuplicatePool { name } => write!(formatter, "duplicate pool '{name}'"),
            Self::UnknownPoolName { name } => write!(formatter, "unknown pool name '{name}'"),
            Self::LoadFailed { path, reason } => {
                write!(formatter, "loading '{path}': {reason}")
            }
            // Ninja's own wording, because a manifest that asks for too new a
            // Ninja is a thing generators and CI logs read about, and the
            // samurai phrasing this used to carry is not what they expect.
            Self::RequiredVersionTooNew { version } => {
                write!(
                    formatter,
                    "fatal: ninja version ({}) incompatible with build file \
                     ninja_required_version version ({version}).",
                    crate::cli::NINJA_COMPAT_VERSION
                )
            }
        }
    }
}

#[allow(
    dead_code,
    reason = "semantic metadata is retained for matching without changing Ninja-compatible Display text"
)]
#[derive(Debug)]
pub(crate) enum ManifestError {
    Read {
        path: PathBuf,
        source: io::Error,
        process_context: bool,
    },
    Scan(ScanError),
    Problem {
        span: Option<SourceSpan>,
        problem: ManifestProblem,
    },
    Graph(GraphError),
    Dyndep(crate::dyndep::DyndepError),
    DyndepRead {
        path: BString,
        source: io::Error,
    },
    DyndepMissingOutput {
        path: BString,
        output: BString,
    },
    DyndepWrongOwner {
        path: BString,
        output: BString,
        span: SourceSpan,
    },
    DyndepDuplicateOutput {
        path: BString,
        output: BString,
        span: SourceSpan,
    },
}

impl ManifestError {
    pub(crate) fn read(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Read {
            path: path.into(),
            source,
            process_context: false,
        }
    }

    pub(crate) const fn at(span: SourceSpan, problem: ManifestProblem) -> Self {
        Self::Problem {
            span: Some(span),
            problem,
        }
    }

    const fn kind(&self) -> ErrorKind {
        match self {
            Self::Graph(_) => ErrorKind::Graph,
            _ => ErrorKind::Manifest,
        }
    }
}

/// Render a manifest diagnostic in Ninja's shape.
///
/// Ninja prints a located header, then the offending source line, then a caret
/// under the column — the caret is most of what makes a parse error legible,
/// and its rules are Ninja's own: the header carries the line but not the
/// column, the context is dropped entirely when the column is at the start of
/// the line or past the truncation point, and a line longer than that point is
/// cut with an ellipsis.
fn write_located(
    formatter: &mut fmt::Formatter<'_>,
    span: &SourceSpan,
    message: &dyn fmt::Display,
) -> fmt::Result {
    write!(
        formatter,
        "error: {}:{}: {message}",
        span.path().display(),
        span.line
    )?;
    write_source_context(formatter, span)
}

/// The source line and caret Ninja prints under a located diagnostic, after the
/// `path:line: message` header the caller has already written.
///
/// Split out from [`write_located`] because a dyndep diagnostic writes its own
/// header — its `error:` comes from the target context that wraps it, not from
/// this printer — but reaches the same lexer and so owes the same context. The
/// rules are Ninja's own: the context is dropped when the column is at the start
/// of the line or past the truncation point, and a line longer than that point
/// is cut with an ellipsis.
pub(crate) fn write_source_context(
    formatter: &mut fmt::Formatter<'_>,
    span: &SourceSpan,
) -> fmt::Result {
    const TRUNCATE_COLUMN: usize = 72;
    // Ninja counts this column from zero, and from the start of the line.
    let column = span.column.saturating_sub(1);
    if column == 0 || column >= TRUNCATE_COLUMN {
        // Ninja terminates the message itself and the printer terminates the
        // line, so an error with no source context to show ends with a blank
        // line. Nothing depends on it, but it is in the bytes.
        return formatter.write_str("\n");
    }
    let bytes = span.source_bytes();
    let Some(line_start) = span.byte_start.checked_sub(column) else {
        return Ok(());
    };
    let line = &bytes[line_start.min(bytes.len())..];
    let mut length = 0;
    let mut truncated = true;
    while length < TRUNCATE_COLUMN {
        match line.get(length) {
            None | Some(b'\n') => {
                truncated = false;
                break;
            }
            Some(_) => length += 1,
        }
    }
    write!(formatter, "\n{}", String::from_utf8_lossy(&line[..length]))?;
    if truncated {
        formatter.write_str("...")?;
    }
    write!(formatter, "\n{:column$}^ near here", "")
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read {
                path,
                source,
                process_context: true,
            } => write!(
                formatter,
                "error: loading '{}': {}",
                path.display(),
                system_message(source)
            ),
            Self::Read { source, .. } => formatter.write_str(&system_message(source)),
            Self::Scan(error) => error.fmt(formatter),
            // Ninja reports a too-new required version through `Fatal`, not
            // through the lexer, so it carries no file or line and no caret.
            Self::Problem {
                problem: problem @ ManifestProblem::RequiredVersionTooNew { .. },
                ..
            } => problem.fmt(formatter),
            Self::Problem {
                span: Some(span),
                problem,
            } => write_located(formatter, span, problem),
            Self::Problem {
                span: None,
                problem,
            } => write!(formatter, "error: {problem}"),
            Self::Graph(error) => error.fmt(formatter),
            Self::Dyndep(error) => error.fmt(formatter),
            Self::DyndepRead { path, source } => {
                write!(formatter, "loading '{path}': {}", system_message(source))
            }
            Self::DyndepMissingOutput { path, output } => {
                write!(
                    formatter,
                    "'{output}' not mentioned in its dyndep file '{path}'"
                )
            }
            Self::DyndepWrongOwner { path, output, .. } => write!(
                formatter,
                "dyndep file '{path}' mentions output '{output}' whose build statement does not have a dyndep binding for the file"
            ),
            Self::DyndepDuplicateOutput { output, .. } => {
                write!(formatter, "multiple rules generate {output}")
            }
        }
    }
}

impl error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::DyndepRead { source, .. } => Some(source),
            Self::Scan(error) => Some(error),
            Self::Graph(error) => Some(error),
            Self::Dyndep(error) => Some(error),
            Self::Problem { .. }
            | Self::DyndepMissingOutput { .. }
            | Self::DyndepWrongOwner { .. }
            | Self::DyndepDuplicateOutput { .. } => None,
        }
    }
}

impl From<ScanError> for ManifestError {
    fn from(error: ScanError) -> Self {
        Self::Scan(error)
    }
}

impl From<GraphError> for ManifestError {
    fn from(error: GraphError) -> Self {
        Self::Graph(error)
    }
}

impl From<crate::dyndep::DyndepError> for ManifestError {
    fn from(error: crate::dyndep::DyndepError) -> Self {
        Self::Dyndep(error)
    }
}

#[allow(
    dead_code,
    reason = "semantic metadata is retained for matching without changing Ninja-compatible Display text"
)]
#[derive(Debug)]
pub(crate) enum GraphError {
    DuplicateRule {
        name: BString,
    },
    DuplicatePool {
        name: BString,
    },
    UnknownPool {
        name: BString,
    },
    /// A cycle in the graph, named by the path around it.
    ///
    /// The path is carried rendered because the graph it names is long gone by
    /// the time anything formats this. An empty path means the cycle was
    /// detected by a check that never walked one, and Ninja has no path to
    /// print either.
    DependencyCycle {
        node: Option<NodeId>,
        path: Vec<BString>,
        /// The self-referencing phony that `-w phonycycle` is about, in the one
        /// shape `CMake` used to emit.
        phony_self_cycle: bool,
    },
    NoRootNodes,
    Stat {
        node: NodeId,
        path: BString,
        source: io::Error,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRule { name } => write!(formatter, "rule '{name}' redefined"),
            Self::DuplicatePool { name } => write!(formatter, "pool '{name}' redefined"),
            Self::UnknownPool { name } => write!(formatter, "unknown pool '{name}'"),
            Self::DependencyCycle { path, .. } if path.is_empty() => {
                formatter.write_str("dependency cycle")
            }
            Self::DependencyCycle {
                path,
                phony_self_cycle,
                ..
            } => {
                formatter.write_str("dependency cycle: ")?;
                for step in path {
                    write!(formatter, "{step} -> ")?;
                }
                // The path closes on the node it started from, which is what
                // makes it read as a cycle rather than a list.
                write!(formatter, "{}", path[0])?;
                if *phony_self_cycle {
                    formatter.write_str(" [-w phonycycle=err]")?;
                }
                Ok(())
            }
            Self::NoRootNodes => {
                formatter.write_str("could not determine root nodes of build graph")
            }
            Self::Stat { source, .. } => formatter.write_str(&system_message(source)),
        }
    }
}

impl error::Error for GraphError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Stat { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Which step of Ninja's `WriteFile` a failure came out of.
///
/// `WriteFile` is `fopen`, `fwrite`, `fclose`, and it names whichever of the
/// three refused. The split is not cosmetic: stdio buffers the payload, so a
/// response file shorter than the stream's buffer never reaches the disk until
/// the close, and a full disk is reported against the close rather than the
/// write. Anything at or past the buffer goes out of `fwrite` and is reported
/// there. Both sentences are Ninja's, and which one a failure earns is part of
/// what it says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileWriteStep {
    Create,
    Write,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildOperation {
    CreateOutputDirectory,
    WriteResponseFile(FileWriteStep),
    WriteOutput,
    WriteDiagnostic,
    StatOutput,
    /// Giving an output a fresh date instead of making it, under `-t`. Its
    /// failures are the archive ones — a member, unlike a file, cannot be
    /// brought into existence by touching it.
    TouchOutput,
}

#[allow(
    dead_code,
    reason = "semantic metadata is retained for matching without changing Ninja-compatible Display text"
)]
#[derive(Debug)]
pub(crate) enum BuildError {
    MissingInput {
        node: NodeId,
        path: BString,
        needed_by: Option<(NodeId, BString)>,
        /// Whether the name was one the `-q` pass merely ASKED about.
        ///
        /// GNU Make leaves such a makefile `updated` with `us_question` where
        /// one whose recipe ran and lost is left `us_failed`, and refuses a goal
        /// over either. The difference is a whole exit code — `MAKE_TROUBLE`
        /// against `MAKE_FAILURE` — so the refusal carries which of the two
        /// verdicts it is, stamped where the graph still knew.
        questioned: bool,
    },
    EdgeNotRunning {
        edge: EdgeId,
    },
    UnknownTarget {
        path: BString,
    },
    MissingRule {
        node: NodeId,
        path: BString,
    },
    InvalidDepsEncoding {
        edge: EdgeId,
    },
    /// A front end that binds a command as its edge is launched could not
    /// produce one. The text is that front end's own diagnostic, already
    /// rendered, because only it knows what it was reading.
    LateCommand {
        diagnostic: String,
    },
    DependencyFileMissing {
        edge: EdgeId,
        path: Option<BString>,
    },
    UnsupportedDepsType {
        edge: EdgeId,
        deps_type: String,
    },
    Interrupted {
        status: Option<ExitStatus>,
    },
    SubcommandFailed {
        edge: EdgeId,
        command: BString,
        status: ExitStatus,
    },
    /// The build ended without completing. Ninja reports why on stdout and
    /// exits with the status of the last command that failed, so the reason and
    /// the status travel together.
    Stopped {
        reason: BuildStop,
        status: i32,
    },
    /// A build that was regenerating the manifest stopped. Ninja reports this
    /// one as an error against the manifest rather than as a build outcome, and
    /// does not carry the command's status out.
    ManifestRebuild {
        path: BString,
        reason: BuildStop,
    },
    Io {
        operation: BuildOperation,
        path: Option<BString>,
        edge: Option<EdgeId>,
        source: io::Error,
    },
    Clock {
        source: std::time::SystemTimeError,
    },
    TargetContext {
        source: Box<Self>,
    },
    Manifest(ManifestError),
    Graph(GraphError),
    Persistence(PersistenceError),
    Process(ProcessError),
    Tool(ToolError),
}

/// Why a build stopped, in Ninja's words.
///
/// Ninja distinguishes running out of allowed failures from running out of work
/// that does not depend on one, and says so differently. The distinction is
/// worth keeping because the two mean different things to whoever reads the
/// log: the first says the build was cut off, the second says it went as far as
/// it could.
#[derive(Debug)]
pub(crate) enum BuildStop {
    /// The allowed number of failures was used up. Plural when `-k` allowed
    /// more than one, as Ninja's is.
    SubcommandFailed { plural: bool },
    /// Commands failed, and everything still wanted depended on them.
    CannotMakeProgress,
    /// No failures, no progress. Ninja's own words for a state it does not
    /// expect to reach.
    Stuck,
    /// The user cut the build short.
    Interrupted,
    /// Something other than a command's exit status ended the build. Ninja
    /// reports that failure itself rather than a summary of the run.
    Failed(Box<BuildError>),
    /// The failure has already been reported where it happened, so there is
    /// nothing left to say here.
    ///
    /// Ninja's build loop carries one error string, and the failures that
    /// print themselves through `Error()` never write to it. The summary line
    /// is printed regardless and so carries an empty reason — `build stopped:
    /// .` — rather than repeating the diagnostic. Reproduced rather than
    /// improved on, because a caller reading Ninja's stdout is reading that
    /// empty reason, and because Ninja's own manifest-regeneration path keys
    /// off the string being empty.
    Reported,
}

impl fmt::Display for BuildStop {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubcommandFailed { plural: false } => formatter.write_str("subcommand failed"),
            Self::SubcommandFailed { plural: true } => formatter.write_str("subcommands failed"),
            Self::CannotMakeProgress => {
                formatter.write_str("cannot make progress due to previous errors")
            }
            Self::Stuck => formatter.write_str("stuck [this is a bug]"),
            Self::Interrupted => formatter.write_str("interrupted by user"),
            Self::Failed(error) => error.fmt(formatter),
            Self::Reported => Ok(()),
        }
    }
}

impl BuildStop {
    /// Reads Ninja's account of why the build loop ended.
    ///
    /// Ninja does not decide this from the last error but from how many
    /// failures it was still allowed: exhausting the allowance means the build
    /// was cut off, while ending with allowance to spare means everything left
    /// depended on something that had already failed. An error that is not a
    /// command's own exit status is reported as itself, since a summary would
    /// lose the only description of what went wrong.
    pub(crate) fn from_failure(
        error: BuildError,
        failures: usize,
        limit: usize,
        allowed: usize,
    ) -> Self {
        match error {
            BuildError::Interrupted { .. } => Self::Interrupted,
            BuildError::SubcommandFailed { .. } if failures >= limit => Self::SubcommandFailed {
                plural: allowed > 1,
            },
            BuildError::SubcommandFailed { .. } => Self::CannotMakeProgress,
            // `WriteFile` reports through `Error()` on stderr and returns
            // false without touching the build loop's error string, so the
            // failure has been named once, where it happened, and the summary
            // that follows has nothing to add.
            BuildError::Io {
                operation: BuildOperation::WriteResponseFile(_),
                ..
            } => Self::Reported,
            other => Self::Failed(Box::new(other)),
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingInput {
                path,
                needed_by: None,
                ..
            } => write!(formatter, "'{path}' missing and no known rule to make it"),
            Self::MissingInput {
                path,
                needed_by: Some((_, needed_by)),
                ..
            } => write!(
                formatter,
                "'{path}', needed by '{needed_by}', missing and no known rule to make it"
            ),
            Self::EdgeNotRunning { .. } => formatter.write_str("edge was not running"),
            Self::UnknownTarget { path } => write!(formatter, "unknown target: '{path}'"),
            Self::MissingRule { path, .. } => {
                write!(formatter, "'{path}' missing and no known rule to make it")
            }
            Self::InvalidDepsEncoding { .. } => {
                formatter.write_str("deps binding is not valid UTF-8")
            }
            Self::LateCommand { diagnostic } => formatter.write_str(diagnostic),
            Self::DependencyFileMissing { .. } => {
                formatter.write_str("subcommand succeeded but dependency file is missing")
            }
            Self::UnsupportedDepsType { deps_type, .. } => {
                write!(formatter, "unsupported deps type '{deps_type}'")
            }
            Self::Interrupted { .. } => formatter.write_str("interrupted by user"),
            Self::SubcommandFailed { command, .. } => {
                write!(formatter, "subcommand failed: {command}")
            }
            Self::Stopped { reason, .. } => write!(formatter, "build stopped: {reason}."),
            Self::ManifestRebuild { path, reason } => {
                write!(formatter, "error: rebuilding '{path}': {reason}")
            }
            // Ninja names the call, the file it was given and the step that
            // refused, because a bare errno does not say which of an edge's
            // files could not be written.
            Self::Io {
                operation: BuildOperation::WriteResponseFile(step),
                path: Some(path),
                source,
                ..
            } => write!(
                formatter,
                "WriteFile({path}): {}. {}",
                match step {
                    FileWriteStep::Create => "Unable to create file",
                    FileWriteStep::Write => "Unable to write to the file",
                    FileWriteStep::Close => "Unable to close the file",
                },
                system_message(source)
            ),
            Self::Io { source, .. } => formatter.write_str(&system_message(source)),
            Self::Clock { source } => source.fmt(formatter),
            Self::TargetContext { source } => write!(formatter, "error: {source}"),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Graph(error) => error.fmt(formatter),
            Self::Persistence(error) => error.fmt(formatter),
            Self::Process(error) => error.fmt(formatter),
            Self::Tool(error) => error.fmt(formatter),
        }
    }
}

impl error::Error for BuildError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Clock { source } => Some(source),
            Self::TargetContext { source } => Some(source),
            Self::Manifest(error) => Some(error),
            Self::Graph(error) => Some(error),
            Self::Persistence(error) => Some(error),
            Self::Process(error) => Some(error),
            Self::Tool(error) => Some(error),
            _ => None,
        }
    }
}

macro_rules! propagate_build_error {
    ($source:ty, $variant:ident) => {
        impl From<$source> for BuildError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

propagate_build_error!(ManifestError, Manifest);
propagate_build_error!(GraphError, Graph);
propagate_build_error!(PersistenceError, Persistence);
propagate_build_error!(ProcessError, Process);
propagate_build_error!(ToolError, Tool);

impl From<crate::dyndep::DyndepError> for BuildError {
    fn from(error: crate::dyndep::DyndepError) -> Self {
        Self::Manifest(ManifestError::Dyndep(error))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PersistenceOperation {
    CreateBuildDirectory,
    OpenBuildLog,
    LoadBuildLog,
    FlushBuildLog,
    RecordBuildLog,
    RecompactBuildLog,
    OpenDepsLog,
    FlushDepsLog,
    RecordDepsLog,
    RecompactDepsLog,
    ReadDepfile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DepfileProblem {
    VariableReference,
    MissingColon,
    NestedInputs,
    NoOutputs,
    UndeclaredOutput(BString),
}

impl fmt::Display for DepfileProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::VariableReference => "depfile contains a variable reference",
            Self::MissingColon => "expected ':' in depfile",
            Self::NestedInputs => "inputs may not also have inputs",
            Self::NoOutputs => "no outputs declared",
            Self::UndeclaredOutput(output) => {
                return write!(
                    formatter,
                    "depfile mentions '{output}' as an output, but no such output was declared"
                );
            }
        })
    }
}

#[derive(Debug)]
pub(crate) enum PersistenceError {
    Depfile {
        path: Option<BString>,
        problem: DepfileProblem,
    },
    Io {
        operation: PersistenceOperation,
        path: PathBuf,
        source: io::Error,
    },
}

impl PersistenceError {
    pub(crate) const fn depfile(problem: DepfileProblem) -> Self {
        Self::Depfile {
            path: None,
            problem,
        }
    }

    pub(crate) const fn depfile_at(path: BString, problem: DepfileProblem) -> Self {
        Self::Depfile {
            path: Some(path),
            problem,
        }
    }

    pub(crate) fn io(
        operation: PersistenceOperation,
        path: impl Into<PathBuf>,
        source: io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Depfile {
                path: Some(path),
                problem,
            } => write!(formatter, "{path}: {problem}"),
            Self::Depfile {
                path: None,
                problem,
            } => problem.fmt(formatter),
            Self::Io {
                operation: PersistenceOperation::LoadBuildLog,
                path,
                source,
            } => write!(
                formatter,
                "loading build log {}: {}",
                path.display(),
                system_message(source)
            ),
            Self::Io {
                operation: PersistenceOperation::RecompactBuildLog,
                source,
                ..
            } => write!(formatter, "failed recompaction: {}", system_message(source)),
            Self::Io { source, .. } => formatter.write_str(&system_message(source)),
        }
    }
}

impl error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Depfile { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JobserverOperation {
    CreateJobserver,
    StartHelper,
    AcquireToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShellOperation {
    CreateOutputPipe,
    ConfigureOutputPipe,
    DuplicateOutputPipe,
    RegisterOutput,
    Spawn,
    ReadOutput,
    Wait,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SupervisorOperation {
    CreatePoller,
    RegisterSignalWake,
    ReadSignalWake,
    WaitForEvent,
}

#[allow(
    dead_code,
    reason = "semantic metadata is retained for matching without changing Ninja-compatible Display text"
)]
#[derive(Debug)]
pub(crate) enum ProcessError {
    InvalidJobserverDescriptors {
        value: String,
    },
    JobserverEnvironment {
        source: jobserver::FromEnvError,
    },
    Jobserver {
        operation: JobserverOperation,
        source: io::Error,
    },
    Shell {
        edge: EdgeId,
        command: BString,
        operation: ShellOperation,
        source: io::Error,
    },
    Supervisor {
        operation: SupervisorOperation,
        source: io::Error,
    },
    SignalDelivery {
        pid: u32,
        process_group: bool,
        signal: crate::signal::Signal,
        source: io::Error,
    },
    ThreadPanicked {
        edge: EdgeId,
    },
    CompletionChannelDisconnected,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobserverDescriptors { value } => {
                write!(formatter, "Invalid file descriptor pair [{value}]")
            }
            Self::JobserverEnvironment { source } => {
                write!(
                    formatter,
                    "Error opening inherited GNU Make jobserver: {}",
                    system_message(source)
                )
            }
            Self::Jobserver { operation, source } => {
                let context = match operation {
                    JobserverOperation::CreateJobserver => "Error creating GNU Make jobserver",
                    JobserverOperation::StartHelper => "Error starting GNU Make jobserver helper",
                    JobserverOperation::AcquireToken => "Error acquiring GNU Make jobserver token",
                };
                write!(formatter, "{context}: {}", system_message(source))
            }
            Self::Shell { source, .. } | Self::Supervisor { source, .. } => {
                formatter.write_str(&system_message(source))
            }
            Self::SignalDelivery {
                pid,
                process_group,
                signal,
                source,
            } => {
                let target = if *process_group {
                    "process group"
                } else {
                    "process"
                };
                write!(
                    formatter,
                    "failed to send {signal} to {target} {pid}: {}",
                    system_message(source)
                )
            }
            Self::ThreadPanicked { .. } => formatter.write_str("subcommand thread panicked"),
            Self::CompletionChannelDisconnected => {
                formatter.write_str("subcommand completion channel disconnected")
            }
        }
    }
}

impl error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::JobserverEnvironment { source } => Some(source),
            Self::Jobserver { source, .. }
            | Self::Shell { source, .. }
            | Self::Supervisor { source, .. }
            | Self::SignalDelivery { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolAvailability {
    BeforeManifest,
    RequiresRuntimeState,
    RequiresPersistentRuntimeState,
    DoesNotUsePersistentRuntimeState,
    BrowseUnsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolOperation {
    Clean,
    Stat,
}

#[allow(
    dead_code,
    reason = "semantic metadata is retained for matching without changing Ninja-compatible Display text"
)]
#[derive(Debug)]
pub(crate) enum ToolError {
    UnknownTarget {
        path: BString,
    },
    NotTarget {
        path: BString,
    },
    UnknownOption {
        tool: &'static str,
        option: BString,
    },
    UnknownMode {
        tool: &'static str,
        mode: String,
    },
    MissingArgument {
        diagnostic: &'static str,
    },
    Usage {
        text: &'static str,
    },
    UnknownRule {
        name: String,
    },
    InvalidRuleEncoding {
        tool: &'static str,
    },
    InvalidArgumentsEncoding {
        context: &'static str,
    },
    UnknownTool {
        name: String,
        suggestion: Option<&'static str>,
    },
    Availability(ToolAvailability),
    PathTooLong,
    Io {
        operation: ToolOperation,
        path: Option<BString>,
        source: io::Error,
    },
    Graph(GraphError),
    Manifest(ManifestError),
}

impl ToolError {
    pub(crate) const fn io(
        operation: ToolOperation,
        path: Option<BString>,
        source: io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path,
            source,
        }
    }

    const fn kind(&self) -> ErrorKind {
        match self {
            Self::Graph(_) => ErrorKind::Graph,
            Self::Manifest(error) => error.kind(),
            _ => ErrorKind::Tool,
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTarget { path } => write!(formatter, "unknown target '{path}'"),
            Self::NotTarget { path } => write!(
                formatter,
                "'{path}' is not a target (i.e. it is not an output of any `build` statement)"
            ),
            Self::UnknownOption { tool, option } => {
                write!(formatter, "unknown {tool} option '{option}'")
            }
            Self::UnknownMode { tool, mode } => write!(formatter, "unknown {tool} mode '{mode}'"),
            Self::MissingArgument { diagnostic } => formatter.write_str(diagnostic),
            Self::Usage { text } => formatter.write_str(text),
            Self::UnknownRule { name } => write!(formatter, "unknown rule '{name}'"),
            Self::InvalidRuleEncoding { tool: "clean" } => {
                formatter.write_str("clean rule names must be valid UTF-8")
            }
            Self::InvalidRuleEncoding { tool: "compdb" } => {
                formatter.write_str("compdb rule names must be valid UTF-8")
            }
            Self::InvalidRuleEncoding { tool } => {
                write!(formatter, "{tool} rule names must be valid UTF-8")
            }
            Self::InvalidArgumentsEncoding { context } => {
                write!(formatter, "{context} arguments must be valid UTF-8")
            }
            Self::UnknownTool {
                name,
                suggestion: Some(suggestion),
            } => write!(
                formatter,
                "fatal: unknown tool '{name}', did you mean '{suggestion}'?"
            ),
            Self::UnknownTool {
                name,
                suggestion: None,
            } => write!(formatter, "fatal: unknown tool '{name}'"),
            Self::Availability(availability) => formatter.write_str(match availability {
                ToolAvailability::BeforeManifest => {
                    "tool is not available before loading the manifest"
                }
                ToolAvailability::RequiresRuntimeState => "tool requires runtime state",
                ToolAvailability::RequiresPersistentRuntimeState => {
                    "tool requires persistent runtime state"
                }
                ToolAvailability::DoesNotUsePersistentRuntimeState => {
                    "tool does not use persistent runtime state"
                }
                ToolAvailability::BrowseUnsupported => "browse tool not supported on this platform",
            }),
            Self::PathTooLong => formatter.write_str("path too long"),
            Self::Io { source, .. } => formatter.write_str(&system_message(source)),
            Self::Graph(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
        }
    }
}

impl error::Error for ToolError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Graph(error) => Some(error),
            Self::Manifest(error) => Some(error),
            _ => None,
        }
    }
}

impl From<GraphError> for ToolError {
    fn from(error: GraphError) -> Self {
        Self::Graph(error)
    }
}

impl From<ManifestError> for ToolError {
    fn from(error: ManifestError) -> Self {
        Self::Manifest(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn derives_classifier_and_preserves_source_chain() {
        let source = io::Error::new(io::ErrorKind::InvalidData, "bad header");
        let error: Error =
            PersistenceError::io(PersistenceOperation::LoadBuildLog, ".ninja_log", source).into();
        assert_eq!(error.kind(), ErrorKind::Persistence);
        assert_eq!(
            error.to_string(),
            "loading build log .ninja_log: bad header"
        );
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("bad header")
        );

        let manifest = ManifestError::read(
            "build.ninja",
            io::Error::new(io::ErrorKind::NotFound, "missing"),
        );
        let error: Error = BuildError::from(manifest).into();
        assert_eq!(error.kind(), ErrorKind::Manifest);
        assert_eq!(error.to_string(), "missing");
        assert_eq!(
            error.at_process_boundary().to_string(),
            "error: loading 'build.ninja': missing"
        );
    }

    /// Neither Ninja nor GNU Make appends Rust's raw errno to `strerror`, and
    /// a source that renders one inside a sentence of its own carries it in the
    /// middle rather than at the end.
    #[test]
    fn a_system_error_reads_as_strerror_wherever_it_sits() {
        let missing = io::Error::from_raw_os_error(2);
        let strerror = missing.to_string().replace(" (os error 2)", "");
        assert_eq!(system_message(&missing), strerror);
        assert_eq!(
            system_message(&format!("opening 'x': {missing}: giving up")),
            format!("opening 'x': {strerror}: giving up")
        );
        assert_eq!(
            system_message(&"nothing (os error) to cut"),
            "nothing (os error) to cut"
        );
    }

    // [spec:ronin:req:compat.cli-and-tools/test]
    #[test]
    fn a_directory_that_cannot_be_entered_is_reported_in_ninjas_words() {
        let error = CliError::ChangeDirectory {
            path: BString::from("nope"),
            source: io::Error::from_raw_os_error(2),
        };
        // Ninja quotes strerror; Rust appends an error number that no Ninja
        // diagnostic carries.
        assert_eq!(
            error.to_string(),
            "fatal: chdir to 'nope' - No such file or directory"
        );
        let synthetic = CliError::ChangeDirectory {
            path: BString::from("nope"),
            source: io::Error::new(io::ErrorKind::NotADirectory, "not a directory"),
        };
        assert_eq!(
            synthetic.to_string(),
            "fatal: chdir to 'nope' - not a directory"
        );
    }

    #[test]
    fn scan_error_locates_an_unexpected_eof() {
        // Ninja raises this one through the lexer like the rest, so it carries
        // a file, a line and a caret rather than standing alone.
        let error = ScanError {
            span: SourceSpan::new(
                crate::source::Source::from_bytes("build.ninja", b"source bytes".to_vec()),
                4,
                4,
                7,
                3,
            ),
            kind: ScanErrorKind::UnexpectedEof,
        };
        assert_eq!(
            error.to_string(),
            "error: build.ninja:7: unexpected EOF\nurce bytes\n  ^ near here"
        );
        assert_eq!(error.span.path(), std::path::Path::new("build.ninja"));
        assert_eq!((error.span.line, error.span.column), (7, 3));
    }
}
