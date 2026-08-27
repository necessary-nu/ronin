//! Ronin command-line parsing and runtime orchestration.

mod runtime_options;

use crate::Error;
use crate::build::{BuildOptions, ColorChoice, JobLimit, OutputStyle, ProgressCarry};
use crate::error::{
    BuildError, BuildStop, CliError, EncodingContext, PersistenceError, PersistenceOperation,
    ToolAvailability, ToolError,
};
use crate::frontend::{Build, BuildGraph, ManifestOptions, Node, Persistence};
use crate::util::{BString, ByteSlice, ByteVec, terminated};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

type CliResult<T> = Result<T, Error>;

// [spec:ronin:req:product.ronin-identity]
/// The product name used in Ronin diagnostics and executable metadata.
pub const PRODUCT_NAME: &str = "ronin";

// [spec:ronin:req:compat.version-reporting]
/// The Ninja language compatibility version reported by `--version`.
pub const NINJA_COMPAT_VERSION: &str = "1.14.0";

/// The compatibility level as a comparable pair, for `ninja_required_version`.
///
/// This and [`NINJA_COMPAT_VERSION`] must agree; a test asserts they do. They
/// are separate because the reported token is Ninja-shaped text and the gate
/// needs numbers, and keeping the gate's numbers here rather than spelled out
/// in the parser is what stopped them drifting apart the first time: the
/// parser refused everything past 1.9 while the implementation had grown
/// through 1.14, so features Ronin had were unreachable behind a version
/// check that predated them.
pub(crate) const NINJA_COMPAT_MAJOR: i32 = 1;
pub(crate) const NINJA_COMPAT_MINOR: i32 = 14;

// [spec:ronin:req:compat.ninja-owned-names]
const DEFAULT_MANIFEST: &str = "build.ninja";
const NINJA_STATUS_ENV: &str = "NINJA_STATUS";
/// The cross-tool convention for suppressing colour; see <https://no-color.org>.
const NO_COLOR_ENV: &str = "NO_COLOR";

/// Buffered output and process status produced by one Ronin invocation.
#[derive(Debug, Eq, PartialEq)]
pub struct RunResult {
    /// Standard output not already streamed while running build commands.
    pub stdout: Vec<u8>,
    /// Standard error not already streamed while running build commands.
    pub stderr: Vec<u8>,
    /// The process exit code requested by the invocation.
    pub exit_code: i32,
}

impl RunResult {
    fn stdout(output: impl Into<Vec<u8>>) -> Self {
        Self {
            stdout: output.into(),
            stderr: Vec::new(),
            exit_code: 0,
        }
    }

    fn exit(stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>, exit_code: i32) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code,
        }
    }
}

/// A reusable Ronin invocation context with an explicit working directory.
///
/// Constructing a runner snapshots the process values that affect Ninja
/// integration, so executing it does not mutate or rediscover process-global
/// state.
// [spec:ronin:req:runtime.explicit-invocation-boundary]
pub struct Runner {
    pub(crate) working_directory: crate::os::WorkingDirectory,
    pub(crate) makeflags: Option<String>,
    status_format: Option<String>,
    pub(crate) terminal: crate::build::TerminalContext,
    pub(crate) connect_jobserver: fn() -> Result<jobserver::Client, crate::error::ProcessError>,
    /// This executable's own path, which is what `$(MAKE)` has to name for a
    /// recursive Makefile to re-enter Ronin rather than the first `make` on the
    /// path. Read once, of the process, because it cannot change under it.
    #[cfg(all(unix, feature = "make"))]
    pub(crate) executable: PathBuf,
    /// How deep in a recursive Make tree this invocation is, as its parent said.
    #[cfg(all(unix, feature = "make"))]
    pub(crate) makelevel: Option<String>,
    /// The second option stream GNU Make reads out of the environment, decoded
    /// exactly as `MAKEFLAGS` is and then emptied. Held apart from `makeflags`
    /// because whether the name was there at all is observable: a Make that was
    /// not given one does not invent one for its children.
    #[cfg(all(unix, feature = "make"))]
    pub(crate) gnumakeflags: Option<String>,
}

impl Runner {
    /// Creates an isolated runner rooted at `working_directory`.
    ///
    /// Environment-driven Ninja integration is disabled. Use
    /// [`Runner::from_process`] for the command-line executable.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory does not exist or cannot be
    /// canonicalized.
    pub fn new(working_directory: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            working_directory: crate::os::WorkingDirectory::new(working_directory)?,
            makeflags: None,
            status_format: None,
            terminal: crate::build::TerminalContext::default(),
            connect_jobserver: crate::jobserver::inherited_client,
            #[cfg(all(unix, feature = "make"))]
            executable: std::env::current_exe().unwrap_or_else(|_| PathBuf::from(PRODUCT_NAME)),
            #[cfg(all(unix, feature = "make"))]
            makelevel: None,
            #[cfg(all(unix, feature = "make"))]
            gnumakeflags: None,
        })
    }

    /// Creates a runner rooted at the current directory with Ninja's relevant
    /// environment values captured.
    ///
    /// # Errors
    ///
    /// Returns an error when the process working directory cannot be read or
    /// canonicalized.
    pub fn from_process() -> std::io::Result<Self> {
        #[cfg(all(unix, feature = "make"))]
        let _directory = crate::make::stable_process_directory_guard();
        let mut runner = Self::new(std::env::current_dir()?)?;
        runner.makeflags = std::env::var("MAKEFLAGS").ok();
        #[cfg(all(unix, feature = "make"))]
        {
            runner.makelevel = std::env::var("MAKELEVEL").ok();
            runner.gnumakeflags = std::env::var(crate::make::cli::GNUMAKEFLAGS).ok();
        }
        runner.status_format = std::env::var(NINJA_STATUS_ENV).ok();
        // Asked once, of the real descriptor, because the build writes through
        // a sink that cannot be asked what it is.
        runner.terminal = crate::build::TerminalContext {
            is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdout()),
            no_color: std::env::var_os(NO_COLOR_ENV).is_some_and(|value| !value.is_empty()),
        };
        Ok(runner)
    }

    /// Runs Ronin with UTF-8 arguments and returns buffered standard output.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when argument parsing or execution fails.
    pub fn run(&self, arguments: &[String]) -> CliResult<String> {
        let arguments = arguments
            .iter()
            .cloned()
            .map(BString::from)
            .collect::<Vec<_>>();
        string_result(run_bytes(self, &arguments, None, None)?)
    }

    /// Runs Ronin with native operating-system arguments and buffers all output.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when argument conversion or execution fails.
    pub fn run_os(&self, arguments: &[OsString]) -> CliResult<RunResult> {
        let arguments = byte_arguments(arguments)?;
        run_bytes(self, &arguments, None, None)
    }

    /// Runs Ronin while streaming build and diagnostic output to supplied sinks.
    ///
    /// Immediate command-line output remains in the returned [`RunResult`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when argument conversion, output, or execution
    /// fails.
    pub fn run_os_with_sinks(
        &self,
        arguments: &[OsString],
        output: &mut dyn std::io::Write,
        diagnostics: &mut dyn std::io::Write,
    ) -> CliResult<RunResult> {
        let arguments = byte_arguments(arguments)?;
        run_bytes(self, &arguments, Some(output), Some(diagnostics))
    }
}

/// How many commands Ninja mode runs at once when nothing says otherwise.
///
/// Ninja's own guess at the machine: one job per processor plus two, so a
/// build keeps the machine busy across the stalls a compiler spends on I/O.
/// Make mode does not use it, because Make runs one recipe at a time until it
/// is told otherwise.
pub(crate) fn default_jobs() -> usize {
    let jobs = match crate::os::osnproc() {
        i64::MIN..=1 => 2,
        2 => 3,
        count => count + 2,
    };
    usize::try_from(jobs).unwrap_or(usize::MAX)
}

// [spec:ronin:def:samu.usage-fn]
// [spec:ronin:sem:samu.usage-fn]
pub(crate) fn usage(program: &str) -> String {
    let default_jobs = default_jobs();
    format!(
        concat!(
            "usage: {} [options] [targets...]\n",
            "\n",
            "if targets are unspecified, builds the 'default' target (see manual).\n",
            "\n",
            "options:\n",
            "  --version      print Ninja compatibility version (\"{}\")\n",
            "  -v, --verbose  show all command lines while building\n",
            "  --quiet        don't show progress status, just command output\n",
            "  --status FMT   progress status format using Ninja-style $vars\n",
            "                 (e.g. --status '[$finished/$total] ')\n",
            "  --output STYLE build output style: ninja, cargo [default=ninja]\n",
            "  --shell SHELL  shell for commands, or 'none' to skip it when unneeded\n",
            "  --compat       hand every command to the shell, exactly as Ninja does\n",
            "  --color WHEN   colorize output: auto, always, never [default=auto]\n",
            "\n",
            "  -C DIR   change to DIR before doing anything else\n",
            "  -f FILE  specify input build file [default={}]\n",
            "\n",
            "  -j N     run N jobs in parallel (0 means infinity) [default={} on this system]\n",
            "  -k N     keep going until N jobs fail (0 means infinity) [default=1]\n",
            "  -l N     do not start new jobs if the load average is greater than N\n",
            "  -n       dry run (don't run commands but act like they succeeded)\n",
            "\n",
            "  -d MODE  enable debugging (use '-d list' to list modes)\n",
            "  -t TOOL  run a subtool (use '-t list' to list subtools)\n",
            "    terminates toplevel options; further flags are passed to the tool\n",
            "  -w FLAG  adjust warnings (use '-w list' to list warnings)"
        ),
        program, NINJA_COMPAT_VERSION, DEFAULT_MANIFEST, default_jobs,
    )
}

// [spec:ronin:def:samu.debugflag-fn]
// [spec:ronin:sem:samu.debugflag-fn]
pub(crate) fn debugflag(options: &mut BuildOptions, flag: &str) -> CliResult<()> {
    match flag {
        "stats" => options.stats = true,
        "explain" => options.explain = true,
        "keepdepfile" => options.keepdepfile = true,
        "keeprsp" => options.keeprsp = true,
        _ => {
            return Err(CliError::UnknownDebugFlag {
                flag: flag.to_owned(),
            }
            .into());
        }
    }
    Ok(())
}

// [spec:ronin:def:samu.loadflag-fn]
// [spec:ronin:sem:samu.loadflag-fn]
pub(crate) fn loadflag(options: &mut BuildOptions, flag: &str) -> CliResult<()> {
    let value: f64 = flag
        .parse()
        .map_err(|_| CliError::InvalidParameter { option: "-l" })?;
    options.maxload = value;
    Ok(())
}

// [spec:ronin:def:samu.warnflag-fn]
// [spec:ronin:sem:samu.warnflag-fn]
/// Apply one `-w` flag, reporting any warning the flag itself raises.
// [spec:ronin:req:compat.cli-and-tools]
pub(crate) fn warnflag(
    options: &mut ManifestOptions,
    warnings: &mut Vec<String>,
    flag: &str,
) -> CliResult<()> {
    match flag {
        // Both spellings are accepted and neither does anything: duplicate
        // outputs have been unconditionally fatal since Ninja deprecated the
        // flag, and saying so is the whole of its remaining behaviour.
        "dupbuild=err" | "dupbuild=warn" => {
            warnings.push("deprecated warning 'dupbuild'".to_owned());
        }
        "phonycycle=err" => options.warn_on_phony_cycle(false),
        "phonycycle=warn" => options.warn_on_phony_cycle(true),
        _ => {
            return Err(CliError::UnknownWarningFlag {
                flag: flag.to_owned(),
            }
            .into());
        }
    }
    Ok(())
}

// [spec:ronin:def:samu.jobsflag-fn]
// [spec:ronin:sem:samu.jobsflag-fn]
pub(crate) fn jobsflag(options: &mut BuildOptions, flag: &str) -> CliResult<()> {
    let value: i64 = flag
        .parse()
        .map_err(|_| CliError::InvalidParameter { option: "-j" })?;
    if value < 0 {
        return Err(CliError::InvalidParameter { option: "-j" }.into());
    }
    options.jobs = if value == 0 {
        JobLimit::Unlimited
    } else {
        let value =
            usize::try_from(value).map_err(|_| CliError::InvalidParameter { option: "-j" })?;
        JobLimit::fixed(value).ok_or(CliError::InvalidParameter { option: "-j" })?
    };
    Ok(())
}

// [spec:ronin:def:samu.progname-fn]
// [spec:ronin:sem:samu.progname-fn]
pub(crate) fn progname(argument: Option<&str>, default: &str) -> String {
    argument
        .and_then(|argument| argument.rsplit('/').next())
        .unwrap_or(default)
        .to_owned()
}

fn append_output(output: &mut String, addition: &str) {
    if addition.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(addition);
}

#[allow(
    clippy::cast_precision_loss,
    reason = "parse timing output is an approximate human-readable metric"
)]
fn append_stats(output: &mut String, parse_count: usize, parse_elapsed: std::time::Duration) {
    let micros = parse_elapsed.as_secs_f64() * 1_000_000.0;
    let average = if parse_count == 0 {
        0.0
    } else {
        micros / parse_count as f64
    };
    append_output(
        output,
        &format!(
            "metric          \tcount \tavg (us) \ttotal (ms)\n\
             .ninja parse    \t{parse_count:<6}\t{average:<9.1}\t{:.1}",
            parse_elapsed.as_secs_f64() * 1_000.0
        ),
    );
}

struct RunInvocation {
    build_options: BuildOptions,
    parse_options: ManifestOptions,
    /// Warnings raised by the command line itself, before any manifest is read.
    warnings: Vec<String>,
    working_directory: crate::os::WorkingDirectory,
    /// The directory named by `-C`, applied once after the whole command line
    /// is read. Ninja keeps a single `working_dir` that each `-C` overwrites,
    /// so repeated flags do not compose: the last one is resolved against the
    /// directory Ninja was invoked from, not against the one before it.
    change_directory: Option<BString>,
    manifest: BString,
    targets: Vec<BString>,
    selected_tool: Option<crate::tool::Tool>,
    tool_arguments: Vec<BString>,
}

enum RunAction {
    Immediate(RunResult),
    /// Boxed because an invocation is far larger than an immediate result and
    /// this is returned once per process.
    Execute(Box<RunInvocation>),
}

const fn debugging_modes() -> &'static str {
    concat!(
        "debugging modes:\n",
        "  stats        print operation counts/timing info\n",
        "  explain      explain what caused a command to execute\n",
        "  keepdepfile  don't delete depfiles after they're read by ronin\n",
        "  keeprsp      don't delete @response files on success\n",
        "multiple modes can be enabled via -d FOO -d BAR\n"
    )
}

const fn warning_flags() -> &'static str {
    "warning flags:\n  phonycycle={err,warn}  phony build statement references itself\n"
}

fn status_placeholder(name: &str) -> CliResult<&'static str> {
    match name {
        "started" => Ok("%s"),
        "total" => Ok("%t"),
        "running" => Ok("%r"),
        "remaining" => Ok("%u"),
        "finished" => Ok("%f"),
        "rate" => Ok("%o"),
        "current_rate" => Ok("%c"),
        "progress" => Ok("%p"),
        "predicted_progress" => Ok("%P"),
        "elapsed" => Ok("%w"),
        "elapsed_seconds" => Ok("%e"),
        "eta" => Ok("%W"),
        "eta_seconds" => Ok("%E"),
        "description" => Ok("\u{1f}"),
        _ => Err(CliError::UnknownStatusVariable {
            name: name.to_owned(),
        }
        .into()),
    }
}

/// Send any warnings a manifest or the command line raised to stderr, as Ninja
/// does.
///
/// Streamed to the diagnostic sink when the invocation has one, so a warning
/// appears while the build runs rather than after it; buffered into the result
/// otherwise, for callers that collect output instead of streaming it.
// [spec:ronin:req:compat.manifest-semantics]
fn emit_warnings(
    raised: &mut Vec<String>,
    diagnostics: Option<&mut (dyn std::io::Write + '_)>,
    buffered: &mut Vec<u8>,
) -> CliResult<()> {
    if raised.is_empty() {
        return Ok(());
    }
    let mut rendered = Vec::new();
    for warning in raised.drain(..) {
        rendered.extend_from_slice(format!("{PRODUCT_NAME}: warning: {warning}\n").as_bytes());
    }
    match diagnostics {
        Some(sink) => {
            sink.write_all(&rendered)
                .and_then(|()| sink.flush())
                .map_err(CliError::write_output)?;
        }
        None => buffered.extend_from_slice(&rendered),
    }
    Ok(())
}

/// Read a `--shell` value: a shell to use, or `none` to never use one.
// [spec:ronin:req:product.command-execution]
fn shell_mode(value: &[u8]) -> CliResult<crate::subprocess::ShellMode> {
    if value == b"none" {
        return Ok(crate::subprocess::ShellMode::Auto);
    }
    if value.is_empty() {
        return Err(CliError::UnknownOptionValue {
            option: "--shell",
            value: String::new(),
        }
        .into());
    }
    Ok(crate::subprocess::ShellMode::Program(
        std::path::PathBuf::from(value.to_os_str().map_err(|_| CliError::InvalidEncoding {
            context: EncodingContext::Argument,
        })?),
    ))
}

/// Name the option in the error when an enumerated value is not one of them.
// [spec:ronin:req:product.output-style]
fn require_value<T>(option: &'static str, value: &[u8], parsed: Option<T>) -> CliResult<T> {
    parsed.ok_or_else(|| {
        CliError::UnknownOptionValue {
            option,
            value: value.to_str_lossy().into_owned(),
        }
        .into()
    })
}

// [spec:ronin:req:runtime.output-byte-boundaries]
fn expand_status_format(format: &str) -> CliResult<String> {
    let bytes = format.as_bytes();
    let mut output = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let start = index;
            while bytes.get(index).is_some_and(|byte| *byte != b'$') {
                index += 1;
            }
            output.push_str(&format[start..index]);
            continue;
        }
        index += 1;
        let Some(next) = bytes.get(index).copied() else {
            return Err(CliError::InvalidStatusEscape.into());
        };
        if next == b'$' {
            output.push('$');
            index += 1;
            continue;
        }
        let (name, end) = if next == b'{' {
            let start = index + 1;
            let close = bytes[start..]
                .iter()
                .position(|byte| *byte == b'}')
                .map(|offset| start + offset)
                .ok_or(CliError::UnterminatedStatusVariable)?;
            (&format[start..close], close + 1)
        } else if next.is_ascii_alphanumeric() || next == b'_' {
            let start = index;
            let mut end = start + 1;
            while bytes
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                end += 1;
            }
            (&format[start..end], end)
        } else {
            let escaped = format[index..]
                .chars()
                .next()
                .expect("index points at the next UTF-8 scalar");
            output.push(escaped);
            index += escaped.len_utf8();
            continue;
        };
        output.push_str(status_placeholder(name)?);
        index = end;
    }
    Ok(output)
}

// [spec:ronin:def:os.oschdir-fn]
// [spec:ronin:sem:os.oschdir-fn]
// [spec:ronin:def:os-posix.oschdir-fn]
// [spec:ronin:sem:os-posix.oschdir-fn]
// [spec:ronin:req:compat.cli-and-tools]
fn enter_directory(
    working_directory: &mut crate::os::WorkingDirectory,
    directory: &BString,
) -> CliResult<()> {
    let path = directory.to_path().map_err(|_| CliError::InvalidEncoding {
        context: EncodingContext::ChangeDirectory,
    })?;
    working_directory.change_to(path).map_err(|source| {
        CliError::ChangeDirectory {
            path: directory.clone(),
            source,
        }
        .into()
    })
}

fn option_value(
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
    option: &str,
) -> CliResult<BString> {
    if !attached.is_empty() {
        return Ok(BString::from(attached));
    }
    *index += 1;
    Ok(arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| CliError::MissingOptionValue {
            option: option.to_owned(),
        })?)
}

fn invalid_option(arguments: &[BString], message: impl std::fmt::Display) -> RunAction {
    let program = progname(
        arguments
            .first()
            .map(|argument| argument.to_str_lossy())
            .as_deref(),
        PRODUCT_NAME,
    );
    RunAction::Immediate(RunResult::exit(
        [],
        format!(
            "{}\n{}\n",
            crate::util::diagnostic(&program, message),
            usage(PRODUCT_NAME)
        ),
        1,
    ))
}

// [spec:ronin:def:samu.parseenvargs-fn+1]
// [spec:ronin:sem:samu.parseenvargs-fn+1]
// [spec:ronin:def:samu.main-fn+1]
// [spec:ronin:sem:samu.main-fn+1]
// [spec:ronin:req:product.no-samuflags]
// [spec:ronin:req:compat.cli-and-tools]
#[allow(
    clippy::too_many_lines,
    reason = "Ninja-compatible short-option parsing is one state machine with shared cursor semantics"
)]
fn parse_run_arguments(
    arguments: &[BString],
    working_directory: &crate::os::WorkingDirectory,
) -> CliResult<RunAction> {
    let mut invocation = RunInvocation {
        build_options: BuildOptions::default(),
        parse_options: ManifestOptions::default(),
        warnings: Vec::new(),
        working_directory: working_directory.clone(),
        change_directory: None,
        manifest: DEFAULT_MANIFEST.into(),
        targets: Vec::new(),
        selected_tool: None,
        tool_arguments: Vec::new(),
    };
    let mut index = 1;
    let mut options_enabled = true;
    while index < arguments.len() {
        let argument = arguments[index].as_bytes();
        if !options_enabled || !argument.starts_with(b"-") || argument == b"-" {
            invocation.targets.push(arguments[index].clone());
            index += 1;
            continue;
        }
        if argument == b"--" {
            options_enabled = false;
            index += 1;
            continue;
        }
        match argument {
            b"--version" => {
                return Ok(RunAction::Immediate(RunResult::stdout(format!(
                    "{NINJA_COMPAT_VERSION}\n"
                ))));
            }
            b"--help" => {
                let program = progname(
                    arguments
                        .first()
                        .map(|argument| argument.to_str_lossy())
                        .as_deref(),
                    PRODUCT_NAME,
                );
                return Ok(RunAction::Immediate(RunResult::exit(
                    [],
                    format!("{}\n", usage(&program)),
                    1,
                )));
            }
            b"--verbose" => invocation.build_options.verbose = true,
            b"--quiet" => invocation.build_options.quiet = true,
            b"--status" => {
                index += 1;
                let format = arguments
                    .get(index)
                    .ok_or_else(|| CliError::MissingOptionValue {
                        option: "--status".to_owned(),
                    })?
                    .to_str()
                    .map_err(|_| CliError::InvalidEncoding {
                        context: EncodingContext::StatusValue,
                    })?;
                invocation.build_options.statusfmt = expand_status_format(format)?;
                invocation.build_options.status_from_cli = true;
            }
            option if option.starts_with(b"--status=") => {
                let format = std::str::from_utf8(&option[b"--status=".len()..]).map_err(|_| {
                    CliError::InvalidEncoding {
                        context: EncodingContext::StatusValue,
                    }
                })?;
                invocation.build_options.statusfmt = expand_status_format(format)?;
                invocation.build_options.status_from_cli = true;
            }
            // [spec:ronin:req:product.command-execution]
            b"--compat" => {
                invocation.build_options.shell = crate::subprocess::ShellMode::Compat;
            }
            b"--shell" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| CliError::MissingOptionValue {
                        option: "--shell".to_owned(),
                    })?;
                invocation.build_options.shell = shell_mode(value)?;
            }
            option if option.starts_with(b"--shell=") => {
                invocation.build_options.shell = shell_mode(&option[b"--shell=".len()..])?;
            }
            b"--output" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| CliError::MissingOptionValue {
                        option: "--output".to_owned(),
                    })?;
                invocation.build_options.style =
                    require_value("--output", value, OutputStyle::parse(value))?;
            }
            option if option.starts_with(b"--output=") => {
                let value = &option[b"--output=".len()..];
                invocation.build_options.style =
                    require_value("--output", value, OutputStyle::parse(value))?;
            }
            b"--color" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| CliError::MissingOptionValue {
                        option: "--color".to_owned(),
                    })?;
                invocation.build_options.color =
                    require_value("--color", value, ColorChoice::parse(value))?;
            }
            option if option.starts_with(b"--color=") => {
                let value = &option[b"--color=".len()..];
                invocation.build_options.color =
                    require_value("--color", value, ColorChoice::parse(value))?;
            }
            option if option.starts_with(b"--") => {
                return Ok(invalid_option(
                    arguments,
                    format_args!("unrecognized option '{}'", option.to_str_lossy()),
                ));
            }
            _ => {
                let mut short = 1;
                while short < argument.len() {
                    let option = argument[short];
                    short += 1;
                    match option {
                        b'v' => invocation.build_options.verbose = true,
                        b'n' => invocation.build_options.dryrun = true,
                        b'h' => {
                            let program = progname(
                                arguments
                                    .first()
                                    .map(|argument| argument.to_str_lossy())
                                    .as_deref(),
                                PRODUCT_NAME,
                            );
                            return Ok(RunAction::Immediate(RunResult::exit(
                                [],
                                format!("{}\n", usage(&program)),
                                1,
                            )));
                        }
                        b'C' | b'f' | b'j' | b'k' | b'l' | b'd' | b'w' | b't' => {
                            let value = option_value(
                                arguments,
                                &mut index,
                                &argument[short..],
                                &format!("-{}", char::from(option)),
                            )?;
                            short = argument.len();
                            match option {
                                b'C' => invocation.change_directory = Some(value),
                                b'f' => invocation.manifest = value,
                                b'j' => jobsflag(
                                    &mut invocation.build_options,
                                    value.to_str().map_err(|_| CliError::InvalidEncoding {
                                        context: EncodingContext::JobsValue,
                                    })?,
                                )?,
                                b'k' => {
                                    let value = value
                                        .to_str()
                                        .map_err(|_| CliError::InvalidEncoding {
                                            context: EncodingContext::KeepGoingValue,
                                        })?
                                        .parse::<i64>()
                                        .map_err(|_| CliError::KeepGoingNotNumeric)?;
                                    invocation.build_options.maxfail = if value <= 0 {
                                        usize::MAX
                                    } else {
                                        usize::try_from(value).map_err(|_| {
                                            CliError::InvalidParameter { option: "-k" }
                                        })?
                                    };
                                }
                                b'l' => loadflag(
                                    &mut invocation.build_options,
                                    value.to_str().map_err(|_| CliError::InvalidEncoding {
                                        context: EncodingContext::LoadValue,
                                    })?,
                                )?,
                                b'd' => {
                                    let value =
                                        value.to_str().map_err(|_| CliError::InvalidEncoding {
                                            context: EncodingContext::DebugValue,
                                        })?;
                                    if value == "list" {
                                        return Ok(RunAction::Immediate(RunResult::exit(
                                            debugging_modes(),
                                            [],
                                            1,
                                        )));
                                    }
                                    debugflag(&mut invocation.build_options, value)?;
                                }
                                b'w' => {
                                    let value =
                                        value.to_str().map_err(|_| CliError::InvalidEncoding {
                                            context: EncodingContext::WarningValue,
                                        })?;
                                    if value == "list" {
                                        return Ok(RunAction::Immediate(RunResult::exit(
                                            warning_flags(),
                                            [],
                                            1,
                                        )));
                                    }
                                    warnflag(
                                        &mut invocation.parse_options,
                                        &mut invocation.warnings,
                                        value,
                                    )?;
                                }
                                b't' => {
                                    let value =
                                        value.to_str().map_err(|_| CliError::InvalidEncoding {
                                            context: EncodingContext::ToolValue,
                                        })?;
                                    invocation.selected_tool = Some(crate::tool::toolget(value)?);
                                    invocation
                                        .tool_arguments
                                        .extend_from_slice(&arguments[index + 1..]);
                                    return Ok(RunAction::Execute(Box::new(invocation)));
                                }
                                _ => unreachable!(),
                            }
                        }
                        _ => {
                            return Ok(invalid_option(
                                arguments,
                                format_args!("invalid option -- '{}'", char::from(option)),
                            ));
                        }
                    }
                }
            }
        }
        index += 1;
    }
    Ok(RunAction::Execute(Box::new(invocation)))
}

/// Settle what the environment decides rather than the command line: the job
/// budget, the inherited jobserver this build may join, and the terminal.
///
/// `unshared` is the limit for a build that named none and found no jobserver
/// to join, which is the one answer the two front ends give differently.
///
// [spec:ronin:req:compat.process-integration+2]
pub(crate) fn normalize_runtime_options(
    options: &mut BuildOptions,
    makeflags: Option<&str>,
    status_format: Option<&str>,
    terminal: crate::build::TerminalContext,
    connect_jobserver: impl FnOnce() -> Result<jobserver::Client, crate::error::ProcessError>,
    unshared: JobLimit,
) -> CliResult<()> {
    let config = crate::jobserver::parse_makeflags_value(makeflags)?;
    let inheritable = config.has_mode() && config.is_native();
    if options.jobs == JobLimit::Auto {
        if inheritable && !options.dryrun {
            match connect_jobserver() {
                Ok(client) => {
                    options.jobs = JobLimit::Unlimited;
                    options.jobserver =
                        Some(crate::jobserver::Transport::inherit(client, makeflags));
                }
                _ => {
                    // An inherited transport is only an optional outer budget.
                    // A stale FIFO or descriptor must not prevent either front end
                    // from loading its source and using the ordinary local Ninja
                    // scheduler, which is also pinned Ninja's fallback.
                    options.jobs = unshared;
                }
            }
        } else {
            options.jobs = unshared;
        }
    }
    // A build serves a budget unless it is spending one, which is what keeps
    // the two mutually exclusive without asking who is at the top of the tree.
    options.serve_jobserver = options.jobserver.is_none();
    if let Some(status) = status_format {
        status.clone_into(&mut options.statusfmt);
    }
    options.terminal = terminal;
    Ok(())
}

fn default_target_names(graph: &BuildGraph) -> Vec<BString> {
    graph
        .default_targets()
        .into_iter()
        .map(|node| BString::from(graph.path(node)))
        .collect()
}

fn run_clean_tool(
    graph: &BuildGraph,
    arguments: &[BString],
    dryrun: bool,
    verbose: bool,
    quiet: bool,
    disk: crate::os::RealDiskInterface,
) -> CliResult<String> {
    let mut include_generators = false;
    let mut rule_mode = false;
    let mut names = Vec::new();
    for argument in arguments {
        match argument.as_bytes() {
            b"-g" => include_generators = true,
            b"-r" => rule_mode = true,
            option if option.starts_with(b"-") => {
                return Err(ToolError::UnknownOption {
                    tool: "clean",
                    option: argument.clone(),
                }
                .into());
            }
            _ => names.push(argument.clone()),
        }
    }
    if rule_mode && names.is_empty() {
        return Err(ToolError::MissingArgument {
            diagnostic: "expected a rule to clean",
        }
        .into());
    }
    let rule_names = if rule_mode {
        names
            .iter()
            .map(|name| {
                name.to_str()
                    .map(str::to_owned)
                    .map_err(|_| ToolError::InvalidRuleEncoding { tool: "clean" })
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    if rule_mode {
        for rule in &rule_names {
            graph
                .rule(graph.root(), rule.as_bytes())
                .ok_or_else(|| ToolError::UnknownRule { name: rule.clone() })?;
        }
    }
    let (targets, rules) = if rule_mode {
        (&[][..], rule_names.as_slice())
    } else {
        (names.as_slice(), &[][..])
    };
    Ok(format_clean_report(
        &crate::tool::clean_with_report_in(
            graph.arenas(),
            targets,
            rules,
            include_generators,
            dryrun,
            disk,
        )?,
        verbose,
        quiet,
    ))
}

fn format_clean_report(removed: &[BString], verbose: bool, quiet: bool) -> String {
    if quiet {
        return String::new();
    }
    let mut output = String::from("Cleaning...");
    if verbose {
        output.push('\n');
        for path in removed {
            output.push_str("Remove ");
            output.push_str(&path.to_str_lossy());
            output.push('\n');
        }
    } else {
        output.push(' ');
    }
    let _ = write!(output, "{} files.", removed.len());
    output
}

fn run_compdb_tool(
    graph: &crate::graph::Graph,
    arguments: &[BString],
    working_directory: &Path,
) -> CliResult<BString> {
    let mut expand_rsp = false;
    let mut rules = Vec::new();
    for argument in arguments {
        match argument.as_bytes() {
            b"-x" => expand_rsp = true,
            option if option.starts_with(b"-") => {
                return Err(ToolError::UnknownOption {
                    tool: "compdb",
                    option: argument.clone(),
                }
                .into());
            }
            _ => rules.push(
                argument
                    .to_str()
                    .map(str::to_owned)
                    .map_err(|_| ToolError::InvalidRuleEncoding { tool: "compdb" })?,
            ),
        }
    }
    Ok(crate::tool::compdb(
        graph,
        &rules,
        expand_rsp,
        working_directory,
    ))
}

fn run_compdb_targets_tool(
    graph: &crate::graph::Graph,
    arguments: &[BString],
    working_directory: &Path,
) -> CliResult<BString> {
    let mut expand_rsp = false;
    let mut targets = Vec::new();
    for argument in arguments {
        match argument.as_bytes() {
            b"-x" => expand_rsp = true,
            b"-h" | b"--help" => {
                return Err(ToolError::Usage {
                    text: "usage: ronin -t compdb-targets [-hx] target [targets]",
                }
                .into());
            }
            option if option.starts_with(b"-") => {
                return Err(ToolError::UnknownOption {
                    tool: "compdb-targets",
                    option: argument.clone(),
                }
                .into());
            }
            _ => targets.push(argument.clone()),
        }
    }
    Ok(crate::tool::compdb_for_targets(
        graph,
        &targets,
        expand_rsp,
        working_directory,
    )?)
}

fn tool_result(output: impl AsRef<[u8]>) -> RunResult {
    tool_result_with(output, Vec::new())
}

fn tool_result_with(output: impl AsRef<[u8]>, stderr: Vec<u8>) -> RunResult {
    RunResult::exit(terminated(output), stderr, 0)
}

const fn tool_help(tool: crate::tool::Tool) -> Option<&'static str> {
    match tool {
        crate::tool::Tool::Clean => Some(concat!(
            "usage: ronin -t clean [options] [targets]\n\n",
            "options:\n",
            "  -g     also clean files marked as ninja generator output\n",
            "  -r     interpret targets as a list of rules to clean instead\n"
        )),
        crate::tool::Tool::Commands => Some(concat!(
            "usage: ronin -t commands [options] [targets]\n\n",
            "options:\n",
            "  -s     only print the final command to build [target], not the whole chain\n"
        )),
        crate::tool::Tool::Inputs => Some(concat!(
            "Usage '-t inputs [options] [targets]\n\n",
            "List all inputs used for a set of targets, sorted in dependency order.\n",
            "Note that by default, results are shell escaped, and sorted alphabetically,\n",
            "and never include validation target paths.\n\n",
            "Options:\n",
            "  -h, --help          Print this message.\n",
            "  -0, --print0            Use \\0, instead of \\n as a line terminator.\n",
            "  -E, --no-shell-escape   Do not shell escape the result.\n",
            "  -d, --dependency-order  Sort results by dependency order.\n"
        )),
        crate::tool::Tool::MultiInputs => Some(concat!(
            "Usage '-t multi-inputs [options] [targets]\n\n",
            "Print one or more sets of inputs required to build targets, sorted in dependency order.\n",
            "The tool works like inputs tool but with addition of the target for each line.\n",
            "The output will be a series of lines with the following elements:\n",
            "<target> <delimiter> <input> <terminator>\n",
            "Note that a given input may appear for several targets if it is used by more than one targets.\n",
            "Options:\n",
            "  -h, --help                   Print this message.\n",
            "  -d  --delimiter=DELIM        Use DELIM instead of TAB for field delimiter.\n",
            "  -0, --print0                 Use \\0, instead of \\n as a line terminator.\n"
        )),
        crate::tool::Tool::Compdb => Some(concat!(
            "usage: ronin -t compdb [options] [rules]\n\n",
            "options:\n",
            "  -x     expand @rspfile style response file invocations\n"
        )),
        crate::tool::Tool::CompdbTargets => Some(concat!(
            "usage: ronin -t compdb [-hx] target [targets]\n\n",
            "options:\n",
            "  -h     display this help message\n",
            "  -x     expand @rspfile style response file invocations\n"
        )),
        crate::tool::Tool::Rules => Some(concat!(
            "usage: ronin -t rules [options]\n\n",
            "options:\n",
            "  -d     also print the description of the rule\n",
            "  -h     print this message\n"
        )),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct ToolRunContext<'a> {
    dry_run: bool,
    verbose: bool,
    quiet: bool,
    working_directory: &'a crate::os::WorkingDirectory,
}

impl<'a> ToolRunContext<'a> {
    const fn new(
        options: &BuildOptions,
        working_directory: &'a crate::os::WorkingDirectory,
    ) -> Self {
        Self {
            dry_run: options.dryrun,
            verbose: options.verbose,
            quiet: options.quiet,
            working_directory,
        }
    }
}

fn run_flag_tool(
    tool: crate::tool::Tool,
    runner: &Runner,
    arguments: &[BString],
    manifest: &BString,
    dryrun: bool,
    working_directory: &crate::os::WorkingDirectory,
) -> CliResult<RunResult> {
    match tool {
        crate::tool::Tool::List => Ok(RunResult::stdout(crate::tool::tool_list())),
        // The one tool in this set that reads a file, and it reads its own:
        // the input may be a Makefile rather than a manifest, and a manifest
        // that will not parse is the finding rather than a reason to stop
        // before reporting one.
        crate::tool::Tool::Lint => crate::lint::run(runner, manifest, arguments, working_directory),
        crate::tool::Tool::Restat => {
            let mut builddir = None;
            let mut filters = Vec::new();
            let mut index = 0;
            while index < arguments.len() {
                match arguments[index].as_bytes() {
                    b"--builddir" => {
                        index += 1;
                        builddir = Some(
                            arguments
                                .get(index)
                                .ok_or_else(|| CliError::MissingOptionValue {
                                    option: "--builddir".to_owned(),
                                })?
                                .clone(),
                        );
                    }
                    option if option.starts_with(b"--builddir=") => {
                        builddir = Some(BString::from(&option[b"--builddir=".len()..]));
                    }
                    b"-h" | b"--help" => {
                        return Ok(RunResult::exit(
                            "usage: ronin -t restat [--builddir=DIR] [outputs]\n",
                            [],
                            1,
                        ));
                    }
                    option if option.starts_with(b"-") => {
                        return Err(ToolError::UnknownOption {
                            tool: "restat",
                            option: arguments[index].clone(),
                        }
                        .into());
                    }
                    _ => filters.push(arguments[index].clone()),
                }
                index += 1;
            }
            let directory = builddir
                .as_ref()
                .map(|directory| {
                    directory.to_path().map(Path::to_path_buf).map_err(|_| {
                        CliError::InvalidEncoding {
                            context: EncodingContext::BuildDirectory,
                        }
                    })
                })
                .transpose()?;
            let directory = directory.map_or_else(
                || working_directory.as_path().to_owned(),
                |directory| working_directory.resolve(&directory),
            );
            let path = directory.join(".ninja_log");
            if !path.exists() {
                return Ok(RunResult::stdout([]));
            }
            let mut log = crate::log::BuildLog::open(Some(&directory)).map_err(|source| {
                PersistenceError::io(PersistenceOperation::LoadBuildLog, path.clone(), source)
            })?;
            if !dryrun {
                let disk = crate::os::RealDiskInterface::new(working_directory.clone());
                crate::log::logrestat(&mut log, &filters, |path| disk.stat(path)).map_err(
                    |source| {
                        PersistenceError::io(
                            PersistenceOperation::RecompactBuildLog,
                            path.clone(),
                            source,
                        )
                    },
                )?;
            }
            log.finish().map_err(|source| {
                PersistenceError::io(PersistenceOperation::FlushBuildLog, path, source)
            })?;
            Ok(RunResult::stdout([]))
        }
        crate::tool::Tool::Urtle => Ok(RunResult::stdout(crate::tool::urtle())),
        _ => Err(ToolError::Availability(ToolAvailability::BeforeManifest).into()),
    }
}

fn run_manifest_tool(
    tool: crate::tool::Tool,
    graph: &BuildGraph,
    arguments: &[BString],
    options: ToolRunContext<'_>,
) -> CliResult<RunResult> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_bytes(), b"-h" | b"--help"))
        && let Some(help) = tool_help(tool)
    {
        return Ok(RunResult::exit(help, [], 1));
    }
    if tool == crate::tool::Tool::CompdbTargets && arguments.is_empty() {
        return Ok(RunResult::exit(
            tool_help(tool).unwrap_or_default(),
            "ronin: error: compdb-targets expects the name of at least one target\n",
            1,
        ));
    }
    let default_arguments;
    let arguments = if arguments.is_empty()
        && matches!(
            tool,
            crate::tool::Tool::Commands
                | crate::tool::Tool::Graph
                | crate::tool::Tool::Inputs
                | crate::tool::Tool::MultiInputs
        ) {
        default_arguments = default_target_names(graph);
        &default_arguments
    } else {
        arguments
    };
    match tool {
        crate::tool::Tool::Browse => {
            Err(ToolError::Availability(ToolAvailability::BrowseUnsupported).into())
        }
        crate::tool::Tool::Clean => run_clean_tool(
            graph,
            arguments,
            options.dry_run,
            options.verbose,
            options.quiet,
            crate::os::RealDiskInterface::new(options.working_directory.clone()),
        )
        .map(tool_result),
        crate::tool::Tool::Compdb => run_compdb_tool(
            graph.arenas(),
            arguments,
            options.working_directory.as_path(),
        )
        .map(tool_result),
        crate::tool::Tool::CompdbTargets => run_compdb_targets_tool(
            graph.arenas(),
            arguments,
            options.working_directory.as_path(),
        )
        .map(tool_result),
        crate::tool::Tool::Commands
        | crate::tool::Tool::Graph
        | crate::tool::Tool::Inputs
        | crate::tool::Tool::MultiInputs
        | crate::tool::Tool::Targets
        | crate::tool::Tool::Rules => Ok(tool_result(crate::tool::run(
            tool,
            graph.arenas(),
            arguments,
            options.working_directory.as_path(),
        )?)),
        _ => Err(ToolError::Availability(ToolAvailability::RequiresPersistentRuntimeState).into()),
    }
}

fn run_log_tool(
    tool: crate::tool::Tool,
    graph: &BuildGraph,
    build_log: &mut crate::log::BuildLog,
    deps_log: &mut crate::deps::DepsLog,
    arguments: &[BString],
    options: ToolRunContext<'_>,
) -> CliResult<RunResult> {
    match tool {
        crate::tool::Tool::Deps => Ok(tool_result(crate::tool::deps_in(
            graph.arenas(),
            deps_log,
            arguments,
            &crate::os::RealDiskInterface::new(options.working_directory.clone()),
        )?)),
        crate::tool::Tool::MissingDeps => {
            let target_names;
            let arguments = if arguments.is_empty() {
                target_names = default_target_names(graph);
                &target_names
            } else {
                arguments
            };
            let targets = arguments
                .iter()
                .map(|target| {
                    crate::graph::nodeget(graph.arenas(), target.as_bytes()).ok_or_else(|| {
                        ToolError::UnknownTarget {
                            path: target.clone(),
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (output, exit_code) = crate::tool::missing_deps(graph.arenas(), deps_log, &targets);
            Ok(RunResult::exit(output, [], exit_code))
        }
        crate::tool::Tool::Query => Ok(tool_result(crate::tool::query(graph.arenas(), arguments)?)),
        crate::tool::Tool::CleanDead => {
            let logged = build_log.entries.keys().cloned().collect::<Vec<_>>();
            Ok(tool_result(format_clean_report(
                &crate::tool::clean_dead_with_report_in(
                    graph.arenas(),
                    &logged,
                    options.dry_run,
                    crate::os::RealDiskInterface::new(options.working_directory.clone()),
                )?,
                options.verbose,
                options.quiet,
            )))
        }
        crate::tool::Tool::Recompact => {
            if !options.dry_run {
                let build_log_path = build_log.path().to_owned();
                let arenas = graph.arenas();
                crate::log::logrecompact(build_log, |path| {
                    crate::graph::nodeget(arenas, path.as_bytes())
                        .is_none_or(|node| arenas.node(node).generator.is_none())
                })
                .map_err(|source| {
                    PersistenceError::io(
                        PersistenceOperation::RecompactBuildLog,
                        build_log_path,
                        source,
                    )
                })?;
                let deps_log_path = deps_log.path().to_owned();
                crate::deps::depsrecompact(deps_log, graph.arenas()).map_err(|source| {
                    PersistenceError::io(
                        PersistenceOperation::RecompactDepsLog,
                        deps_log_path,
                        source,
                    )
                })?;
            }
            Ok(RunResult::stdout([]))
        }
        _ => {
            Err(ToolError::Availability(ToolAvailability::DoesNotUsePersistentRuntimeState).into())
        }
    }
}

/// Runs Ronin with UTF-8 arguments and returns its buffered standard output.
///
/// This convenience entry point is intended for simple embedding and tests.
/// Use [`run_os`] when arguments or output must remain byte exact.
///
/// # Errors
///
/// Returns an [`Error`] when argument parsing, manifest loading, graph
/// evaluation, a tool operation, or build execution fails.
pub fn run(arguments: &[String]) -> CliResult<String> {
    process_runner()?.run(arguments)
}

fn string_result(result: RunResult) -> CliResult<String> {
    let RunResult {
        stdout,
        stderr,
        exit_code,
    } = result;
    let mut stdout = String::from_utf8_lossy(&stdout).into_owned();
    if stdout.ends_with('\n') {
        stdout.pop();
    }
    if exit_code == 0 {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&stderr);
        let diagnostic = if stderr.is_empty() {
            stdout
        } else {
            stderr.into_owned()
        };
        Err(CliError::InvocationFailed {
            exit_code,
            diagnostic,
        }
        .into())
    }
}

pub(crate) fn byte_arguments(arguments: &[OsString]) -> CliResult<Vec<BString>> {
    arguments
        .iter()
        .cloned()
        .map(|argument| {
            Vec::from_os_string(argument)
                .map(BString::from)
                .map_err(|_| CliError::InvalidEncoding {
                    context: EncodingContext::Argument,
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(crate) fn process_runner() -> CliResult<Runner> {
    Runner::from_process()
        .map_err(|source| CliError::CurrentDirectory { source })
        .map_err(Into::into)
}

/// Runs Ronin with native operating-system arguments.
///
/// Output is buffered in the returned [`RunResult`]. Use
/// [`Runner::run_os_with_sinks`] when build output should be streamed.
///
/// # Errors
///
/// Returns an [`Error`] when argument conversion, invocation setup, manifest
/// loading, graph evaluation, a tool operation, or build execution fails.
pub fn run_os(arguments: &[OsString]) -> CliResult<RunResult> {
    process_runner()?.run_os(arguments)
}

/// The sinks one build writes through.
struct BuildIo<'a, 'sink> {
    output: Option<&'a mut (dyn std::io::Write + 'sink)>,
    diagnostics: Option<&'a mut (dyn std::io::Write + 'sink)>,
}

impl<'a, 'sink> BuildIo<'a, 'sink> {
    /// The build these sinks and `options` describe, over `graph`.
    fn build(
        self,
        graph: &'a mut BuildGraph,
        persistence: &'a mut Persistence,
        options: &BuildOptions,
    ) -> Build<'a, 'sink> {
        let mut build = Build::with_options(graph, persistence, options.clone());
        if let Some(sink) = self.output {
            build = build.output(sink);
        }
        if let Some(sink) = self.diagnostics {
            build = build.diagnostics(sink);
        }
        build
    }
}

/// Rebuild the manifest itself, for a manifest some build statement generates.
///
/// Reports whether it actually changed, which is what sends the invocation
/// round again to read the one it has now.
fn rebuild_manifest<'a>(
    graph: &'a mut BuildGraph,
    persistence: &'a mut Persistence,
    options: &BuildOptions,
    io: BuildIo<'a, '_>,
    manifest: &BString,
    output: &mut String,
    progress_carry: &mut ProgressCarry,
) -> CliResult<bool> {
    let Some(target) = graph
        .lookup(manifest.as_bytes())
        .filter(|node| graph.generator(*node).is_some())
    else {
        return Ok(false);
    };
    let planned = io
        .build(graph, persistence, options)
        .carrying(*progress_carry)
        .plan(&[target])?;
    if planned.already_up_to_date() {
        return Ok(false);
    }
    let outcome = planned.run()?;
    // Keep this regeneration's edges in the count so the build the invocation
    // asked for opens its progress where Ninja's shared Status leaves it.
    *progress_carry = outcome.progress_carry();
    append_output(output, &String::from_utf8_lossy(outcome.output()));
    // Failing to regenerate the manifest is not a build outcome: Ninja names
    // the manifest, reports it as an error, and leaves with a plain failure
    // rather than the command's own status.
    //
    // Except when the failure already reported itself. Ninja's caller asks
    // `!err.empty()` before saying anything, so a regeneration that stopped
    // with an empty reason is indistinguishable from one that had nothing to
    // do: nothing further is printed, and the invocation goes on to build the
    // targets it was asked for with the manifest it already has. Odd, and the
    // contract — the diagnostic has been printed by then, so the run is not
    // silent about it, only unstopped by it.
    match outcome.stopped {
        Some((BuildStop::Reported, _)) => Ok(false),
        Some((reason, _)) => Err(BuildError::ManifestRebuild {
            path: manifest.clone(),
            reason,
        }
        .into()),
        None => Ok(!outcome.regenerated().is_empty()),
    }
}

/// How the build the invocation actually asked for ended.
struct BuildOutcome {
    already_up_to_date: bool,
    /// Set when the build ended without finishing, carrying Ninja's reason and
    /// the status to leave with.
    stopped: Option<(String, i32)>,
}

fn build_targets<'a>(
    graph: &'a mut BuildGraph,
    persistence: &'a mut Persistence,
    options: &BuildOptions,
    io: BuildIo<'a, '_>,
    names: &[BString],
    output: &mut String,
    progress_carry: ProgressCarry,
) -> CliResult<BuildOutcome> {
    // A name nothing in the graph interned is the invocation's mistake rather
    // than the build's, and Ninja reports it the same way it reports the rest of
    // what its user asked for.
    let targets = if names.is_empty() {
        graph.default_targets()
    } else {
        names
            .iter()
            .map(|name| {
                graph.lookup(name.as_bytes()).ok_or_else(|| {
                    BuildError::target_context(BuildError::UnknownTarget { path: name.clone() })
                        .into()
                })
            })
            .collect::<CliResult<Vec<Node>>>()?
    };
    let mut build = io
        .build(graph, persistence, options)
        .carrying(progress_carry);
    // The targets came from a command line, so a failure to start reads as an
    // error against the invocation rather than as the engine's own report.
    build.invocation_errors = true;
    let planned = build.plan(&targets)?;
    let already_up_to_date = planned.already_up_to_date();
    let outcome = planned.run()?;
    append_output(output, &String::from_utf8_lossy(outcome.output()));
    // A build that stops is a result, not a diagnostic: Ninja says so on stdout
    // after the build's own output and exits with the failing command's status.
    // Anything else is a genuine error.
    let stopped = outcome.stopped().map(|reason| {
        (
            format!("{PRODUCT_NAME}: build stopped: {reason}."),
            outcome.exit_code(),
        )
    });
    Ok(BuildOutcome {
        already_up_to_date,
        stopped,
    })
}

// [spec:ronin:def:samu.getbuilddir-fn]
// [spec:ronin:sem:samu.getbuilddir-fn]
#[allow(
    clippy::too_many_lines,
    reason = "manifest rebuild and reload is one bounded orchestration loop with ordered cleanup"
)]
pub(crate) fn run_bytes<'sink>(
    runner: &Runner,
    arguments: &[BString],
    mut build_output: Option<&'sink mut dyn std::io::Write>,
    mut build_diagnostics: Option<&'sink mut dyn std::io::Write>,
) -> CliResult<RunResult> {
    let mut invocation = match parse_run_arguments(arguments, &runner.working_directory)? {
        RunAction::Immediate(result) => return Ok(result),
        RunAction::Execute(invocation) => *invocation,
    };
    // Deferred until the whole command line has been read, as Ninja defers it:
    // the announcement has to know whether a tool was selected, and a flag that
    // fails to parse has to fail before the directory moves under it.
    let mut entering = None;
    if let Some(directory) = invocation.change_directory.clone() {
        if invocation.selected_tool.is_none() && !invocation.build_options.quiet {
            // Ninja's announcement, funny quotes included: Emacs and every
            // error parser that inherited GNU Make's convention read this line
            // to resolve the relative paths a compiler prints, so the quoting
            // is part of the interface rather than decoration.
            let announcement = format!("{PRODUCT_NAME}: Entering directory `{directory}'");
            // Written before the move so a directory that cannot be entered is
            // still named on the way out.
            if let Some(sink) = build_output.as_deref_mut() {
                writeln!(sink, "{announcement}").map_err(CliError::write_output)?;
                sink.flush().map_err(CliError::write_output)?;
            } else {
                entering = Some(announcement);
            }
        }
        enter_directory(&mut invocation.working_directory, &directory)?;
    }
    invocation.build_options.working_directory = invocation.working_directory.clone();
    // Carries output written before loading the manifest when there was no
    // sink to stream it to, so it still leads what an embedding caller gets.
    let mut output = entering.take().unwrap_or_default();
    // Warnings and other non-fatal diagnostics are retained here only when the
    // invocation has no diagnostic sink to stream them to.
    let mut warnings = Vec::new();
    // An explicit `-j` declines an inherited budget. Manifest mode may publish
    // its own fixed limit to recipe children, but it does so silently.
    let jobserver_notice = runtime_options::normalize_ninja_runtime_options(
        &mut invocation.build_options,
        runner.makeflags.as_deref(),
        runner.status_format.as_deref(),
        runner.terminal,
        runner.connect_jobserver,
        JobLimit::fixed(default_jobs()).expect("the default job count is nonzero"),
    )?;
    if let Some(notice) = jobserver_notice {
        notice.emit(
            invocation.build_options.quiet,
            build_output.as_deref_mut(),
            build_diagnostics.as_deref_mut(),
            &mut output,
            &mut warnings,
        )?;
    }
    if let Some(tool) = invocation
        .selected_tool
        .filter(|tool| tool.stage() == crate::tool::ToolStage::Flags)
    {
        return run_flag_tool(
            tool,
            runner,
            &invocation.tool_arguments,
            &invocation.manifest,
            invocation.build_options.dryrun,
            &invocation.working_directory,
        );
    }

    // The command line's own warnings go out before the manifest is read, as
    // Ninja's do, so a flag's complaint survives a manifest that then fails.
    // Command-line warnings come first, as they do in Ninja: the flag was read
    // before the manifest was.
    emit_warnings(
        &mut invocation.warnings,
        build_diagnostics.as_deref_mut(),
        &mut warnings,
    )?;
    let mut parse_count = 0;
    let mut parse_elapsed = std::time::Duration::ZERO;
    // Ninja builds a regenerated manifest and then the requested targets on one
    // shared Status, so the regeneration edges stay in the following build's
    // `[N/M]` denominator. Ronin re-parses with a fresh Builder each cycle, so
    // it carries those counters across the loop by hand instead.
    let mut progress_carry = ProgressCarry::default();
    for _ in 0..100 {
        let parse_started = std::time::Instant::now();
        let mut manifest_warnings = Vec::new();
        let parsed = crate::parse::load_manifest_reporting(
            invocation
                .manifest
                .to_path()
                .map_err(|_| CliError::InvalidEncoding {
                    context: EncodingContext::ManifestPath,
                })?,
            invocation.working_directory.clone(),
            invocation.parse_options,
            &mut manifest_warnings,
        );
        parse_count += 1;
        parse_elapsed += parse_started.elapsed();
        // Before the failure is propagated, not after: Ninja writes a warning
        // where it raises it, so one raised by an earlier statement survives a
        // later statement that stops the parse.
        emit_warnings(
            &mut manifest_warnings,
            build_diagnostics.as_deref_mut(),
            &mut warnings,
        )?;
        let mut graph = parsed?;

        if let Some(tool) = invocation
            .selected_tool
            .filter(|tool| tool.stage() == crate::tool::ToolStage::Manifest)
        {
            return run_manifest_tool(
                tool,
                &graph,
                &invocation.tool_arguments,
                ToolRunContext::new(&invocation.build_options, &invocation.working_directory),
            );
        }

        let logical_builddir = graph
            .variable(graph.root(), b"builddir")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value.to_os_str().expect("byte strings are valid on Unix")));
        let builddir = logical_builddir.as_deref().map_or_else(
            || invocation.working_directory.as_path().to_owned(),
            |directory| invocation.working_directory.resolve(directory),
        );
        let (mut persistence, warning) = Persistence::open(&mut graph, &builddir)?;
        if let Some(warning) = warning {
            append_output(&mut output, &warning);
        }
        if let Some(tool) = invocation.selected_tool {
            // A tool over the logs is reached only here, where the manifest
            // front end has just opened both of them.
            let (build_log, deps_log) = persistence
                .logs_mut()
                .expect("the manifest front end opens both logs");
            let result = run_log_tool(
                tool,
                &graph,
                build_log,
                deps_log,
                &invocation.tool_arguments,
                ToolRunContext::new(&invocation.build_options, &invocation.working_directory),
            );
            let persistence_result = persistence.finish();
            let result = result?;
            persistence_result?;
            return Ok(result);
        }

        let manifest_result = rebuild_manifest(
            &mut graph,
            &mut persistence,
            &invocation.build_options,
            BuildIo {
                output: build_output.as_deref_mut(),
                diagnostics: build_diagnostics.as_deref_mut(),
            },
            &invocation.manifest,
            &mut output,
            &mut progress_carry,
        );
        let manifest_rebuilt = match manifest_result {
            Ok(rebuilt) => rebuilt,
            Err(error) => {
                let _ = persistence.finish();
                return Err(error);
            }
        };
        if manifest_rebuilt {
            persistence.finish()?;
            if invocation.build_options.dryrun {
                return Ok(tool_result_with(output, std::mem::take(&mut warnings)));
            }
            continue;
        }

        let outcome = build_targets(
            &mut graph,
            &mut persistence,
            &invocation.build_options,
            BuildIo {
                output: build_output.as_deref_mut(),
                diagnostics: build_diagnostics.as_deref_mut(),
            },
            &invocation.targets,
            &mut output,
            progress_carry,
        );
        let persistence_result = persistence.finish();
        let outcome = outcome?;
        persistence_result?;
        if let Some((message, status)) = outcome.stopped {
            append_output(&mut output, &message);
            return Ok(RunResult::exit(
                terminated(output),
                std::mem::take(&mut warnings),
                status,
            ));
        }
        if outcome.already_up_to_date && output.is_empty() && !invocation.build_options.quiet {
            output = "ronin: no work to do.".into();
        }
        if invocation.build_options.stats {
            append_stats(&mut output, parse_count, parse_elapsed);
        }
        return Ok(tool_result_with(output, std::mem::take(&mut warnings)));
    }
    Err(CliError::ManifestRetryLimit {
        path: invocation.manifest,
        attempts: 100,
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch_directory::Scratch;
    use std::fs;

    fn parse_options(arguments: &[&str]) -> CliResult<BuildOptions> {
        let arguments = arguments
            .iter()
            .map(|argument| BString::from(*argument))
            .collect::<Vec<_>>();
        let working_directory = crate::os::WorkingDirectory::new(".").unwrap();
        match parse_run_arguments(&arguments, &working_directory)? {
            RunAction::Execute(invocation) => Ok(invocation.build_options),
            RunAction::Immediate(_) => panic!("these arguments describe a build"),
        }
    }

    // [spec:ronin:req:compat.version-reporting/test]
    #[test]
    fn the_reported_token_and_the_manifest_gate_agree() {
        assert_eq!(
            NINJA_COMPAT_VERSION
                .split('.')
                .take(2)
                .collect::<Vec<_>>()
                .join("."),
            format!("{NINJA_COMPAT_MAJOR}.{NINJA_COMPAT_MINOR}"),
            "the advertised version and the ninja_required_version gate must not drift"
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_rendering_and_a_colour_choice_are_named_on_the_command_line() {
        let separate = parse_options(&["ronin", "--output", "cargo", "--color", "never"]).unwrap();
        assert_eq!(separate.style, OutputStyle::Cargo);
        assert_eq!(separate.color, ColorChoice::Never);
        let joined = parse_options(&["ronin", "--output=cargo", "--color=always"]).unwrap();
        assert_eq!(joined.style, OutputStyle::Cargo);
        assert_eq!(joined.color, ColorChoice::Always);
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn ninja_rendering_is_what_an_unadorned_invocation_gets() {
        let options = parse_options(&["ronin"]).unwrap();
        assert_eq!(options.style, OutputStyle::Ninja);
        assert_eq!(options.color, ColorChoice::Auto);
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn an_unknown_rendering_or_colour_choice_is_rejected_by_name() {
        let style = parse_options(&["ronin", "--output=fancy"])
            .err()
            .expect("an unknown style is rejected");
        assert!(
            style.to_string().contains("invalid --output value 'fancy'"),
            "{style}"
        );
        let color = parse_options(&["ronin", "--color", "sometimes"])
            .err()
            .expect("an unknown colour choice is rejected");
        assert!(
            color
                .to_string()
                .contains("invalid --color value 'sometimes'"),
            "{color}"
        );
        let missing = parse_options(&["ronin", "--output"])
            .err()
            .expect("a style option needs a value");
        assert!(
            missing.to_string().contains("missing --output value"),
            "{missing}"
        );
    }

    // [spec:ronin:req:runtime.output-byte-boundaries/test]
    #[test]
    fn status_expansion_preserves_unicode_slices_and_escapes() {
        assert_eq!(
            expand_status_format("λ [$finished/$total] résumé 😀").unwrap(),
            "λ [%f/%t] résumé 😀"
        );
        assert_eq!(
            expand_status_format("cost $$5; escaped $λ").unwrap(),
            "cost $5; escaped λ"
        );
    }

    // [spec:ronin:req:runtime.output-byte-boundaries/test]
    #[cfg(unix)]
    #[test]
    fn tool_targets_and_graph_output_preserve_native_bytes() {
        use std::os::unix::ffi::OsStrExt;

        let directory = Scratch::named("ronin-byte-tool-");
        let manifest = directory.join("build.ninja");
        fs::write(&manifest, b"build target-\xff: phony\n").unwrap();
        let arguments = [
            BString::from("ronin"),
            BString::from("-f"),
            BString::from(manifest.as_os_str().as_bytes()),
            BString::from("-t"),
            BString::from("graph"),
            BString::from(b"target-\xff"),
        ];
        let runner = Runner::new(&directory).unwrap();
        let result = run_bytes(&runner, &arguments, None, None).unwrap();
        assert!(
            result
                .stdout
                .windows(b"target-\xff".len())
                .any(|window| window == b"target-\xff")
        );
    }

    // [spec:ronin:req:runtime.explicit-invocation-boundary/test]
    // [spec:ronin:req:compat.cli-and-tools/test]
    #[test]
    fn runner_resolves_the_last_change_without_mutating_process_cwd() {
        // Where the process is, read while nothing is allowed to be standing
        // somewhere else.
        //
        // What this test claims is that Ninja's `-C` is resolved by the runner
        // and never by the process, and the evidence for it is a process-global
        // the product is entitled to move: a Make compilation evaluates each
        // unit from its own directory, and a deferred recipe expands in the
        // directory of the unit that wrote it. Both restore what they found,
        // and both hold `COMPILATION_DIRECTORY` exclusively across the move —
        // which is what that lock is for. A libtest binary runs all of it at
        // once, so reading the global without the shared side is reading a
        // value another thread is allowed to be halfway through changing, and
        // the assertion then fails naming a scratch directory this test never
        // heard of. `Runner::from_process` reads the same global under the same
        // guard; a test asserting on it is owed no less.
        //
        // Taken for the read alone and not across the run, which is what keeps
        // this a claim about the runner rather than a lock that makes it true:
        // `run` launches commands, and `subprocess::spawn` takes the shared
        // side for itself at the moment it forks.
        let process_directory = || {
            #[cfg(all(unix, feature = "make"))]
            let _stable = crate::make::stable_process_directory_guard();
            std::env::current_dir().unwrap()
        };
        let original_directory = process_directory();
        let base = Scratch::named("ronin-runner-directory-");
        let working_directory = base.join("second");
        fs::create_dir_all(&working_directory).unwrap();
        // Ninja keeps one directory that each -C overwrites, so this decoy is
        // never entered: `second` resolves against the invocation directory,
        // not against `first`.
        fs::create_dir_all(base.join("first")).unwrap();
        fs::write(
            working_directory.join("rules.ninja"),
            "rule copy\n  command = cp $in $out\n",
        )
        .unwrap();
        fs::write(
            working_directory.join("build.ninja"),
            "include rules.ninja\nbuild output: copy input\ndefault output\n",
        )
        .unwrap();
        fs::write(working_directory.join("input"), "explicit directory").unwrap();

        let runner = Runner::new(&base).unwrap();
        let result = runner
            .run(&[
                "ronin".into(),
                "-C".into(),
                "first".into(),
                "-C".into(),
                "second".into(),
            ])
            .unwrap();
        assert!(result.contains("cp input output"));
        assert!(result.contains("Entering directory `second'"));
        assert_eq!(
            fs::read_to_string(working_directory.join("output")).unwrap(),
            "explicit directory"
        );
        assert!(working_directory.join(".ninja_log").exists());
        assert!(working_directory.join(".ninja_deps").exists());
        assert_eq!(process_directory(), original_directory);

        let error = runner
            .run(&[
                "ronin".into(),
                "-C".into(),
                "first".into(),
                "-j".into(),
                "not-a-number".into(),
            ])
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid -j parameter");
        assert_eq!(process_directory(), original_directory);
    }

    #[test]
    fn runner_streams_only_to_explicit_sinks() {
        let directory = Scratch::named("ronin-runner-sinks-");
        fs::write(
            directory.join("build.ninja"),
            "rule emit\n  command = printf child-output\nbuild output: emit\n",
        )
        .unwrap();
        let runner = Runner::new(&directory).unwrap();
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let result = runner
            .run_os_with_sinks(&[OsString::from("ronin")], &mut output, &mut diagnostics)
            .unwrap();
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
        assert!(
            output
                .windows(b"child-output".len())
                .any(|value| value == b"child-output")
        );
        assert!(diagnostics.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rust_cli_serves_a_jobserver_unless_it_is_spending_one() {
        let normalize = |jobs, makeflags| {
            let mut options = BuildOptions {
                jobs,
                ..BuildOptions::default()
            };
            normalize_runtime_options(
                &mut options,
                makeflags,
                None,
                crate::build::TerminalContext::default(),
                || unreachable!("an explicit -j declines an inherited jobserver"),
                JobLimit::fixed(default_jobs()).expect("the default job count is nonzero"),
            )
            .unwrap();
            options.serve_jobserver
        };
        let four = JobLimit::fixed(4).unwrap();

        // Nothing in the environment names a jobserver, so this build is the
        // top of its tree and owns the budget its children share.
        assert!(normalize(four, None));
        assert!(normalize(four, Some("-k")));
        // A manifest build with an explicit `-j` declines the inherited budget
        // and may publish its own fixed limit to jobserver-aware recipe tools.
        assert!(normalize(
            four,
            Some(" -j8 --jobserver-auth=fifo:/tmp/parent")
        ));
        assert!(normalize(four, Some(" -j8 --jobserver-auth=3,4")));
        // An unbounded `-j` is not a finite budget, so serving is harmlessly
        // requested here and skipped by the builder.
        assert!(normalize(
            JobLimit::Unlimited,
            Some(" -j8 --jobserver-auth=fifo:/tmp/parent")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rust_cli_uses_supported_make_jobserver() {
        let mut options = BuildOptions::default();
        normalize_runtime_options(
            &mut options,
            Some("-j --jobserver-auth=fifo:/tmp/ronin-jobserver"),
            None,
            crate::build::TerminalContext::default(),
            || {
                jobserver::Client::new(0).map_err(|source| crate::error::ProcessError::Jobserver {
                    operation: crate::error::JobserverOperation::StartHelper,
                    source,
                })
            },
            JobLimit::fixed(default_jobs()).expect("the default job count is nonzero"),
        )
        .unwrap();
        assert_eq!(options.jobs, JobLimit::Unlimited);
        assert!(options.jobserver.is_some());
        assert!(!options.serve_jobserver);

        let mut explicit = BuildOptions {
            jobs: JobLimit::fixed(2).unwrap(),
            ..BuildOptions::default()
        };
        normalize_runtime_options(
            &mut explicit,
            Some("-j --jobserver-auth=fifo:/tmp/ignored"),
            None,
            crate::build::TerminalContext::default(),
            || panic!("explicit -j must not connect to an inherited jobserver"),
            JobLimit::fixed(default_jobs()).expect("the default job count is nonzero"),
        )
        .unwrap();
        assert_eq!(explicit.jobs, JobLimit::fixed(2).unwrap());
        assert!(explicit.jobserver.is_none());

        let fallback = JobLimit::fixed(3).unwrap();
        let mut unavailable = BuildOptions::default();
        normalize_runtime_options(
            &mut unavailable,
            Some("-j --jobserver-auth=fifo:/tmp/missing"),
            None,
            crate::build::TerminalContext::default(),
            || {
                Err(crate::error::ProcessError::Jobserver {
                    operation: crate::error::JobserverOperation::StartHelper,
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                })
            },
            fallback,
        )
        .unwrap();
        assert_eq!(unavailable.jobs, fallback);
        assert!(unavailable.jobserver.is_none());
        assert!(unavailable.serve_jobserver);

        let mut dryrun = BuildOptions {
            dryrun: true,
            ..BuildOptions::default()
        };
        normalize_runtime_options(
            &mut dryrun,
            Some("-j --jobserver-auth=fifo:/tmp/unused"),
            None,
            crate::build::TerminalContext::default(),
            || panic!("a dry run must not connect to an inherited jobserver"),
            fallback,
        )
        .unwrap();
        assert_eq!(dryrun.jobs, fallback);
        assert!(dryrun.jobserver.is_none());
    }

    #[test]
    fn rust_cli_builds_requested_target_with_logs() {
        let directory = Scratch::named("ronin-rust-cli-");
        let input = directory.join("in");
        let output = directory.join("out");
        let manifest = directory.join("build.ninja");
        fs::write(&input, "cli").unwrap();
        fs::write(
            &manifest,
            format!(
                "builddir = {}\nrule copy\n  command = cp $in $out\nbuild {}: copy {}\ndefault {}\n",
                directory.display(),
                output.display(),
                input.display(),
                output.display()
            ),
        )
        .unwrap();
        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments).unwrap();
        assert!(status.contains("cp "));
        assert_eq!(fs::read_to_string(&output).unwrap(), "cli");
        assert!(directory.join(".ninja_log").exists());
        assert!(directory.join(".ninja_deps").exists());
    }

    #[test]
    fn rust_cli_rebuilds_and_reloads_manifest_before_targets() {
        let directory = Scratch::named("ronin-rust-cli-manifest-");
        let manifest = directory.join("build.ninja");
        let template = directory.join("next.ninja");
        let output = directory.join("out");
        let render_manifest = |value: &str| {
            format!(
                "builddir = {}\nrule regen\n  command = cp $in $out\nrule emit\n  command = printf {value} > $out\nbuild {}: regen {}\nbuild {}: emit\ndefault {}\n",
                directory.display(),
                manifest.display(),
                template.display(),
                output.display(),
                output.display()
            )
        };
        fs::write(&manifest, render_manifest("old")).unwrap();
        fs::write(&template, render_manifest("new")).unwrap();
        // Make the template unambiguously newer than the manifest by
        // construction rather than by racing the clock. The old shape slept
        // 20 ms between the two writes so the template would land a later
        // mtime; a loaded scheduler can eat that gap, and a filesystem whose
        // mtime resolution is coarser than it (1 s without nanosecond stamps,
        // 2 s on FAT) drops both writes into one tick regardless — so the
        // stale manifest this case sets up was a race, not a fact. Backdate
        // the manifest ten seconds instead: the ordering is asserted, not
        // hoped for. The manifest goes into the PAST, never the future, so the
        // copy regeneration writes over it (stamped at the current time) is not
        // itself seen as stale, which would loop the rebuild to its limit.
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&manifest)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(older))
            .unwrap();

        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments).unwrap();
        assert!(status.contains("cp "));
        assert!(status.contains("printf new"));
        assert_eq!(fs::read_to_string(&output).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            render_manifest("new")
        );
    }

    #[test]
    fn rust_cli_continues_when_manifest_restat_prunes_rebuild() {
        let directory = Scratch::named("ronin-rust-cli-manifest-restat-");
        let manifest = directory.join("build.ninja");
        let trigger = directory.join("trigger");
        let output = directory.join("out");
        fs::write(&trigger, "").unwrap();
        fs::write(
            &manifest,
            format!(
                "builddir = {}\nrule steady\n  command = true\n  restat = 1\nrule emit\n  command = printf built > $out\nbuild {}: steady {}\nbuild {}: emit\ndefault {}\n",
                directory.display(),
                manifest.display(),
                trigger.display(),
                output.display(),
                output.display()
            ),
        )
        .unwrap();

        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments).unwrap();
        assert!(status.lines().any(|line| line.ends_with("true")));
        assert!(status.contains("printf built"));
        assert_eq!(fs::read_to_string(&output).unwrap(), "built");
    }

    #[test]
    fn rust_cli_clean_rule_and_generator_options() {
        let directory = Scratch::named("ronin-rust-cli-clean-");
        let manifest = directory.join("build.ninja");
        let ordinary = directory.join("ordinary");
        let generated = directory.join("generated");
        fs::write(
            &manifest,
            format!(
                "rule emit\n  command = touch $out\nrule regen\n  command = touch $out\n  generator = 1\nbuild {}: emit\nbuild {}: regen\n",
                ordinary.display(),
                generated.display()
            ),
        )
        .unwrap();
        fs::write(&ordinary, "").unwrap();
        fs::write(&generated, "").unwrap();
        let base = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            "-t".into(),
            "clean".into(),
        ];
        let mut rule_arguments = base.clone();
        rule_arguments.extend(["-r".into(), "emit".into()]);
        assert_eq!(run(&rule_arguments).unwrap(), "Cleaning... 1 files.");
        assert!(!ordinary.exists() && generated.exists());

        fs::write(&ordinary, "").unwrap();
        assert_eq!(run(&base).unwrap(), "Cleaning... 1 files.");
        assert!(!ordinary.exists() && generated.exists());

        let mut generator_arguments = base;
        generator_arguments.push("-g".into());
        assert_eq!(run(&generator_arguments).unwrap(), "Cleaning... 1 files.");
        assert!(!generated.exists());
    }

    #[test]
    fn rust_cli_compdb_expands_response_files_without_rule_filter() {
        let directory = Scratch::named("ronin-rust-cli-compdb-");
        let manifest = directory.join("build.ninja");
        let input = directory.join("in");
        let output = directory.join("out");
        fs::write(
            &manifest,
            format!(
                "rule cc\n  command = cc @$rspfile -o $out\n  rspfile = $out.rsp\n  rspfile_content = -DCLI $in\nbuild {}: cc {}\n",
                output.display(),
                input.display()
            ),
        )
        .unwrap();
        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            "-t".into(),
            "compdb".into(),
            "-x".into(),
        ];
        let database = run(&arguments).unwrap();
        assert!(database.contains("-DCLI"));
        assert!(!database.contains(&format!("@{}.rsp", output.display())));
    }
}
