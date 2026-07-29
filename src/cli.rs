//! Ronin command-line parsing and runtime orchestration.

use crate::build::BuildOptions;
use crate::parse::ParseOptions;
use crate::util::{BString, ByteSlice, ByteVec};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// [spec:samurai:req:product.ronin-identity]
pub const PRODUCT_NAME: &str = "ronin";

// [spec:samurai:req:compat.version-reporting]
pub const NINJA_COMPAT_VERSION: &str = "1.9.0";

// [spec:samurai:req:compat.ninja-owned-names]
const DEFAULT_MANIFEST: &str = "build.ninja";
const NINJA_STATUS_ENV: &str = "NINJA_STATUS";

pub struct RunResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
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

// [spec:samurai:def:samu.usage-fn]
// [spec:samurai:sem:samu.usage-fn]
pub fn usage(program: &str) -> String {
    let default_jobs = match crate::os::osnproc() {
        i64::MIN..=1 => 2,
        2 => 3,
        count => count + 2,
    };
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

// [spec:samurai:def:samu.debugflag-fn]
// [spec:samurai:sem:samu.debugflag-fn]
pub(crate) fn debugflag(options: &mut BuildOptions, flag: &str) -> Result<(), String> {
    match flag {
        "stats" => options.stats = true,
        "explain" => options.explain = true,
        "keepdepfile" => options.keepdepfile = true,
        "keeprsp" => options.keeprsp = true,
        _ => return Err(format!("unknown debug flag '{flag}'")),
    }
    Ok(())
}

// [spec:samurai:def:samu.loadflag-fn]
// [spec:samurai:sem:samu.loadflag-fn]
pub(crate) fn loadflag(options: &mut BuildOptions, flag: &str) -> Result<(), String> {
    let value: f64 = flag
        .parse()
        .map_err(|_| "invalid -l parameter".to_owned())?;
    options.maxload = value;
    Ok(())
}

// [spec:samurai:def:samu.warnflag-fn]
// [spec:samurai:sem:samu.warnflag-fn]
pub(crate) fn warnflag(options: &mut ParseOptions, flag: &str) -> Result<(), String> {
    match flag {
        "dupbuild=err" => options.dupbuildwarn = false,
        "dupbuild=warn" => options.dupbuildwarn = true,
        _ => return Err(format!("unknown warning flag '{flag}'")),
    }
    Ok(())
}

// [spec:samurai:def:samu.jobsflag-fn]
// [spec:samurai:sem:samu.jobsflag-fn]
pub(crate) fn jobsflag(options: &mut BuildOptions, flag: &str) -> Result<(), String> {
    let value: i64 = flag
        .parse()
        .map_err(|_| "invalid -j parameter".to_owned())?;
    if value < 0 {
        return Err("invalid -j parameter".into());
    }
    options.maxjobs = if value == 0 {
        usize::MAX
    } else {
        value as usize
    };
    Ok(())
}

// [spec:samurai:def:samu.progname-fn]
// [spec:samurai:sem:samu.progname-fn]
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
    parse_options: ParseOptions,
    manifest: BString,
    targets: Vec<BString>,
    selected_tool: Option<crate::tool::Tool>,
    tool_arguments: Vec<BString>,
}

enum RunAction {
    Immediate(RunResult),
    Execute(RunInvocation),
}

fn debugging_modes() -> &'static str {
    concat!(
        "debugging modes:\n",
        "  stats        print operation counts/timing info\n",
        "  explain      explain what caused a command to execute\n",
        "  keepdepfile  don't delete depfiles after they're read by ronin\n",
        "  keeprsp      don't delete @response files on success\n",
        "multiple modes can be enabled via -d FOO -d BAR\n"
    )
}

fn warning_flags() -> &'static str {
    "warning flags:\n  phonycycle={err,warn}  phony build statement references itself\n"
}

fn status_placeholder(name: &str) -> Result<&'static str, String> {
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
        _ => Err(format!("unknown variable '{name}' in --status format")),
    }
}

fn expand_status_format(format: &str) -> Result<String, String> {
    let bytes = format.as_bytes();
    let mut output = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            output.push(char::from(bytes[index]));
            index += 1;
            continue;
        }
        index += 1;
        let Some(next) = bytes.get(index).copied() else {
            return Err("invalid --status: bad $-escape (literal $ must be written as $$)".into());
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
                .ok_or_else(|| "invalid --status: unterminated variable".to_owned())?;
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
            output.push(char::from(next));
            index += 1;
            continue;
        };
        output.push_str(status_placeholder(name)?);
        index = end;
    }
    Ok(output)
}

// [spec:samurai:def:os.oschdir-fn]
// [spec:samurai:sem:os.oschdir-fn]
// [spec:samurai:def:os-posix.oschdir-fn]
// [spec:samurai:sem:os-posix.oschdir-fn]
fn set_current_directory(directory: &BString) -> Result<(), String> {
    std::env::set_current_dir(
        directory
            .to_path()
            .map_err(|_| "-C path is not representable on this platform")?,
    )
    .map_err(|error| error.to_string())
}

fn option_value(
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
    option: &str,
) -> Result<BString, String> {
    if !attached.is_empty() {
        return Ok(BString::from(attached));
    }
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("missing {option} value"))
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

// [spec:samurai:def:samu.parseenvargs-fn+1]
// [spec:samurai:sem:samu.parseenvargs-fn+1]
// [spec:samurai:def:samu.main-fn+1]
// [spec:samurai:sem:samu.main-fn+1]
// [spec:samurai:req:product.no-samuflags]
// [spec:samurai:req:compat.cli-and-tools]
fn parse_run_arguments(arguments: &[BString]) -> Result<RunAction, String> {
    let mut invocation = RunInvocation {
        build_options: BuildOptions::default(),
        parse_options: ParseOptions::default(),
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
                ))))
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
                    .ok_or_else(|| "missing --status value".to_owned())?
                    .to_str()
                    .map_err(|_| "invalid --status value")?;
                invocation.build_options.statusfmt = expand_status_format(format)?;
                invocation.build_options.status_from_cli = true;
            }
            option if option.starts_with(b"--status=") => {
                let format = std::str::from_utf8(&option[b"--status=".len()..])
                    .map_err(|_| "invalid --status value")?;
                invocation.build_options.statusfmt = expand_status_format(format)?;
                invocation.build_options.status_from_cli = true;
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
                                b'C' => set_current_directory(&value)?,
                                b'f' => invocation.manifest = value,
                                b'j' => jobsflag(
                                    &mut invocation.build_options,
                                    value.to_str().map_err(|_| "invalid -j parameter")?,
                                )?,
                                b'k' => {
                                    let value = value
                                        .to_str()
                                        .map_err(|_| "invalid -k parameter")?
                                        .parse::<i64>()
                                        .map_err(|_| {
                                            "-k parameter not numeric; did you mean -k 0?"
                                                .to_owned()
                                        })?;
                                    invocation.build_options.maxfail = if value <= 0 {
                                        usize::MAX
                                    } else {
                                        value as usize
                                    };
                                }
                                b'l' => loadflag(
                                    &mut invocation.build_options,
                                    value.to_str().map_err(|_| "invalid -l parameter")?,
                                )?,
                                b'd' => {
                                    let value =
                                        value.to_str().map_err(|_| "invalid -d parameter")?;
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
                                        value.to_str().map_err(|_| "invalid -w parameter")?;
                                    if value == "list" {
                                        return Ok(RunAction::Immediate(RunResult::exit(
                                            warning_flags(),
                                            [],
                                            1,
                                        )));
                                    }
                                    if matches!(value, "phonycycle=err" | "phonycycle=warn") {
                                        continue;
                                    }
                                    warnflag(&mut invocation.parse_options, value)?;
                                }
                                b't' => {
                                    let value =
                                        value.to_str().map_err(|_| "invalid -t parameter")?;
                                    invocation.selected_tool = Some(crate::tool::toolget(value)?);
                                    invocation
                                        .tool_arguments
                                        .extend_from_slice(&arguments[index + 1..]);
                                    return Ok(RunAction::Execute(invocation));
                                }
                                _ => unreachable!(),
                            }
                        }
                        _ => {
                            return Ok(invalid_option(
                                arguments,
                                format_args!("invalid option -- '{}'", char::from(option)),
                            ))
                        }
                    }
                }
            }
        }
        index += 1;
    }
    Ok(RunAction::Execute(invocation))
}

// [spec:samurai:req:compat.process-integration]
fn normalize_runtime_options(
    options: &mut BuildOptions,
    makeflags: Option<&str>,
) -> Result<(), String> {
    if options.maxjobs == 0 {
        let jobserver = crate::jobserver::parse_makeflags_value(makeflags)?;
        if cfg!(unix)
            && matches!(
                jobserver.mode,
                crate::jobserver::JobserverMode::PosixFifo | crate::jobserver::JobserverMode::Pipe
            )
        {
            options.maxjobs = usize::MAX;
            options.jobserver = jobserver;
        } else {
            options.maxjobs = match crate::os::osnproc() {
                i64::MIN..=1 => 2,
                2 => 3,
                count => (count + 2) as usize,
            };
        }
    }
    if let Ok(status) = std::env::var(NINJA_STATUS_ENV) {
        options.statusfmt = status;
    }
    Ok(())
}

fn default_target_names(parser: &crate::parse::Parser, graph: &crate::graph::Graph) -> Vec<String> {
    crate::parse::defaultnodes(parser, graph)
        .into_iter()
        .map(|node| {
            let node = graph.node(node);
            String::from_utf8_lossy(node.path.as_bytes()).into_owned()
        })
        .collect()
}

fn default_target_paths(
    parser: &crate::parse::Parser,
    graph: &crate::graph::Graph,
) -> Vec<BString> {
    crate::parse::defaultnodes(parser, graph)
        .into_iter()
        .map(|node| graph.node(node).path.clone())
        .collect()
}

fn run_clean_tool(
    graph: &crate::graph::Graph,
    state: &crate::env::EnvState,
    arguments: &[String],
    dryrun: bool,
    verbose: bool,
    quiet: bool,
) -> Result<String, String> {
    let mut include_generators = false;
    let mut rule_mode = false;
    let mut names = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-g" => include_generators = true,
            "-r" => rule_mode = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown clean option '{option}'"));
            }
            name => names.push(name.to_owned()),
        }
    }
    if rule_mode && names.is_empty() {
        return Err("expected a rule to clean".into());
    }
    if rule_mode {
        for rule in &names {
            crate::env::envrule(graph, state.root, rule)
                .ok_or_else(|| format!("unknown rule '{rule}'"))?;
        }
    }
    let (targets, rules) = if rule_mode {
        (&[][..], names.as_slice())
    } else {
        (names.as_slice(), &[][..])
    };
    crate::tool::clean_with_report(graph, targets, rules, include_generators, dryrun)
        .map(|removed| format_clean_report(&removed, verbose, quiet))
        .map_err(|error| error.to_string())
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

fn run_compdb_tool(graph: &crate::graph::Graph, arguments: &[String]) -> Result<String, String> {
    let mut expand_rsp = false;
    let mut rules = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-x" => expand_rsp = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown compdb option '{option}'"));
            }
            rule => rules.push(rule.to_owned()),
        }
    }
    Ok(crate::tool::compdb(graph, &rules, expand_rsp))
}

fn run_compdb_targets_tool(
    graph: &crate::graph::Graph,
    arguments: &[String],
) -> Result<String, String> {
    let mut expand_rsp = false;
    let mut targets = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-x" => expand_rsp = true,
            "-h" | "--help" => {
                return Err("usage: ronin -t compdb-targets [-hx] target [targets]".into())
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown compdb-targets option '{option}'"));
            }
            target => targets.push(target.to_owned()),
        }
    }
    crate::tool::compdb_for_targets(graph, &targets, expand_rsp)
}

fn tool_result(output: String) -> RunResult {
    let mut output = output.into_bytes();
    if !output.is_empty() && !matches!(output.last(), Some(b'\n' | b'\0')) {
        output.push(b'\n');
    }
    RunResult::stdout(output)
}

fn tool_help(tool: crate::tool::Tool) -> Option<&'static str> {
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
struct ToolRunOptions {
    dry_run: bool,
    verbose: bool,
    quiet: bool,
}

impl From<&BuildOptions> for ToolRunOptions {
    fn from(options: &BuildOptions) -> Self {
        Self {
            dry_run: options.dryrun,
            verbose: options.verbose,
            quiet: options.quiet,
        }
    }
}

fn run_flag_tool(
    tool: crate::tool::Tool,
    arguments: &[String],
    dryrun: bool,
) -> Result<RunResult, String> {
    match tool {
        crate::tool::Tool::List => Ok(RunResult::stdout(crate::tool::tool_list())),
        crate::tool::Tool::Restat => {
            let mut builddir = None;
            let mut filters = Vec::new();
            let mut index = 0;
            while index < arguments.len() {
                match arguments[index].as_str() {
                    "--builddir" => {
                        index += 1;
                        builddir = Some(
                            arguments
                                .get(index)
                                .ok_or_else(|| "missing --builddir value".to_owned())?
                                .clone(),
                        );
                    }
                    option if option.starts_with("--builddir=") => {
                        builddir = Some(option["--builddir=".len()..].to_owned());
                    }
                    "-h" | "--help" => {
                        return Ok(RunResult::exit(
                            "usage: ronin -t restat [--builddir=DIR] [outputs]\n",
                            [],
                            1,
                        ))
                    }
                    option if option.starts_with('-') => {
                        return Err(format!("unknown restat option '{option}'"))
                    }
                    output => filters.push(output.to_owned()),
                }
                index += 1;
            }
            let directory = builddir.as_deref().map(Path::new);
            let path = directory.map_or_else(
                || PathBuf::from(".ninja_log"),
                |directory| directory.join(".ninja_log"),
            );
            if !path.exists() {
                return Ok(RunResult::stdout([]));
            }
            let mut graph = crate::graph::graphinit();
            let mut log = crate::log::loginit(directory, &mut graph)
                .map_err(|error| format!("loading build log {}: {error}", path.display()))?;
            if !dryrun {
                let filter_refs = filters.iter().map(String::as_str).collect::<Vec<_>>();
                let disk = crate::os::RealDiskInterface;
                crate::log::logrestat(&mut log, &filter_refs, |path| disk.stat(path))
                    .map_err(|error| format!("failed recompaction: {error}"))?;
            }
            crate::log::logclose(log).map_err(|error| error.to_string())?;
            Ok(RunResult::stdout([]))
        }
        crate::tool::Tool::Urtle => Ok(RunResult::stdout(crate::tool::urtle())),
        _ => Err("tool is not available before loading the manifest".into()),
    }
}

fn run_manifest_tool(
    tool: crate::tool::Tool,
    graph: &crate::graph::Graph,
    parser: &crate::parse::Parser,
    state: &crate::env::EnvState,
    arguments: &[String],
    options: ToolRunOptions,
) -> Result<RunResult, String> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
    {
        if let Some(help) = tool_help(tool) {
            return Ok(RunResult::exit(help, [], 1));
        }
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
        default_arguments = default_target_names(parser, graph);
        &default_arguments
    } else {
        arguments
    };
    match tool {
        crate::tool::Tool::Browse => Err("browse tool not supported on this platform".into()),
        crate::tool::Tool::Clean => run_clean_tool(
            graph,
            state,
            arguments,
            options.dry_run,
            options.verbose,
            options.quiet,
        )
        .map(tool_result),
        crate::tool::Tool::Compdb => run_compdb_tool(graph, arguments).map(tool_result),
        crate::tool::Tool::CompdbTargets => {
            run_compdb_targets_tool(graph, arguments).map(tool_result)
        }
        crate::tool::Tool::Commands
        | crate::tool::Tool::Graph
        | crate::tool::Tool::Inputs
        | crate::tool::Tool::MultiInputs
        | crate::tool::Tool::Targets
        | crate::tool::Tool::Rules => crate::tool::run(tool, graph, arguments).map(tool_result),
        _ => Err("tool requires persistent runtime state".into()),
    }
}

fn run_log_tool(
    tool: crate::tool::Tool,
    graph: &mut crate::graph::Graph,
    parser: &crate::parse::Parser,
    build_log: &mut crate::log::BuildLog,
    deps_log: &mut crate::deps::DepsLog,
    arguments: &[String],
    options: ToolRunOptions,
) -> Result<RunResult, String> {
    match tool {
        crate::tool::Tool::Deps => crate::tool::deps(graph, deps_log, arguments).map(tool_result),
        crate::tool::Tool::MissingDeps => {
            let target_names;
            let arguments = if arguments.is_empty() {
                target_names = default_target_names(parser, graph);
                &target_names
            } else {
                arguments
            };
            let targets = arguments
                .iter()
                .map(|target| {
                    crate::graph::nodeget(graph, target.as_bytes())
                        .ok_or_else(|| format!("unknown target '{target}'"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (output, exit_code) = crate::tool::missing_deps(graph, deps_log, &targets);
            Ok(RunResult::exit(output, [], exit_code))
        }
        crate::tool::Tool::Query => crate::tool::query(graph, arguments).map(tool_result),
        crate::tool::Tool::CleanDead => {
            let logged = build_log.entries.keys().cloned().collect::<Vec<_>>();
            crate::tool::clean_dead_with_report(graph, &logged, options.dry_run)
                .map(|removed| {
                    tool_result(format_clean_report(
                        &removed,
                        options.verbose,
                        options.quiet,
                    ))
                })
                .map_err(|error| error.to_string())
        }
        crate::tool::Tool::Recompact => {
            if !options.dry_run {
                crate::log::logrecompact(build_log, |path| {
                    crate::graph::nodeget(graph, path.as_bytes())
                        .is_none_or(|node| graph.node(node).gen.is_none())
                })
                .map_err(|error| error.to_string())?;
                crate::deps::depsrecompact(deps_log, graph).map_err(|error| error.to_string())?;
            }
            Ok(RunResult::stdout([]))
        }
        _ => Err("tool does not use persistent runtime state".into()),
    }
}

pub fn run(arguments: &[String]) -> Result<String, String> {
    let arguments = arguments
        .iter()
        .cloned()
        .map(BString::from)
        .collect::<Vec<_>>();
    let result = run_bytes(&arguments, None, None)?;
    let mut stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    if stdout.ends_with('\n') {
        stdout.pop();
    }
    if result.exit_code == 0 {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        Err(if stderr.is_empty() {
            stdout
        } else {
            stderr.into_owned()
        })
    }
}

pub fn run_os(arguments: &[OsString]) -> Result<RunResult, String> {
    let arguments = arguments
        .iter()
        .cloned()
        .map(|argument| {
            Vec::from_os_string(argument)
                .map(BString::from)
                .map_err(|_| "argument is not representable as bytes on this platform".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let stderr = std::io::stderr();
    let mut diagnostics = stderr.lock();
    run_bytes(&arguments, Some(&mut output), Some(&mut diagnostics))
}

// [spec:samurai:def:samu.getbuilddir-fn]
// [spec:samurai:sem:samu.getbuilddir-fn]
fn run_bytes(
    arguments: &[BString],
    mut build_output: Option<&mut dyn std::io::Write>,
    mut build_diagnostics: Option<&mut dyn std::io::Write>,
) -> Result<RunResult, String> {
    let mut invocation = match parse_run_arguments(arguments)? {
        RunAction::Immediate(result) => return Ok(result),
        RunAction::Execute(invocation) => invocation,
    };
    let makeflags = std::env::var("MAKEFLAGS").ok();
    normalize_runtime_options(&mut invocation.build_options, makeflags.as_deref())?;
    if let Some(tool) = invocation
        .selected_tool
        .filter(|tool| tool.stage() == crate::tool::ToolStage::Flags)
    {
        let arguments = invocation
            .tool_arguments
            .iter()
            .map(|argument| {
                argument
                    .to_str()
                    .map(str::to_owned)
                    .map_err(|_| "tool arguments must be valid UTF-8".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        return run_flag_tool(tool, &arguments, invocation.build_options.dryrun);
    }

    let mut output = String::new();
    let mut parse_count = 0;
    let mut parse_elapsed = std::time::Duration::ZERO;
    for _ in 0..100 {
        let mut graph = crate::graph::graphinit();
        let mut parser = crate::parse::parseinit();
        parser.options = invocation.parse_options;
        let mut state = crate::env::envinit(&mut graph);
        let parse_started = std::time::Instant::now();
        crate::parse::parse(
            invocation
                .manifest
                .to_path()
                .map_err(|_| "manifest path is not representable on this platform")?,
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        )?;
        parse_count += 1;
        parse_elapsed += parse_started.elapsed();

        if let Some(tool) = invocation
            .selected_tool
            .filter(|tool| tool.stage() == crate::tool::ToolStage::Manifest)
        {
            let tool_arguments = invocation
                .tool_arguments
                .iter()
                .map(|argument| {
                    argument
                        .to_str()
                        .map(str::to_owned)
                        .map_err(|_| "tool arguments must be valid UTF-8".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            return run_manifest_tool(
                tool,
                &graph,
                &parser,
                &state,
                &tool_arguments,
                ToolRunOptions::from(&invocation.build_options),
            );
        }

        let builddir = crate::env::envvar(&graph, state.root, "builddir")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value.to_os_str().expect("byte strings are valid on Unix")));
        if let Some(directory) = &builddir {
            std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        }
        let mut build_log = crate::log::loginit(builddir.as_deref(), &mut graph)
            .map_err(|error| error.to_string())?;
        let deps_path = builddir.as_ref().map_or_else(
            || PathBuf::from(".ninja_deps"),
            |path| path.join(".ninja_deps"),
        );
        let (mut deps_log, warning) =
            crate::deps::depsloadlog(&deps_path, &mut graph).map_err(|error| error.to_string())?;
        if let Some(warning) = warning {
            append_output(&mut output, &warning);
        }
        if let Some(tool) = invocation.selected_tool {
            let tool_arguments = invocation
                .tool_arguments
                .iter()
                .map(|argument| {
                    argument
                        .to_str()
                        .map(str::to_owned)
                        .map_err(|_| "tool arguments must be valid UTF-8".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = run_log_tool(
                tool,
                &mut graph,
                &parser,
                &mut build_log,
                &mut deps_log,
                &tool_arguments,
                ToolRunOptions::from(&invocation.build_options),
            );
            let build_log_result =
                crate::log::logclose(build_log).map_err(|error| error.to_string());
            let deps_log_result =
                crate::deps::depsclose(deps_log).map_err(|error| error.to_string());
            let result = result?;
            build_log_result?;
            deps_log_result?;
            return Ok(result);
        }

        let manifest_edge = crate::graph::nodeget(&graph, invocation.manifest.as_bytes())
            .and_then(|node| graph.node(node).gen);
        let manifest_result = if let Some(edge) = manifest_edge {
            let streaming = build_output.is_some();
            let mut builder = if let Some(output) = build_output.as_deref_mut() {
                if let Some(diagnostics) = build_diagnostics.as_deref_mut() {
                    crate::build::Builder::with_logs_and_sinks(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                        diagnostics,
                    )
                } else {
                    crate::build::Builder::with_logs_and_output(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                    )
                }
            } else {
                crate::build::Builder::with_logs(
                    &mut graph,
                    invocation.build_options.clone(),
                    &mut build_log,
                    &mut deps_log,
                )
            };
            let result: Result<bool, String> = (|| {
                builder.add_target(invocation.manifest.as_bytes())?;
                if builder.already_up_to_date() {
                    return Ok(false);
                }
                let result = builder.build();
                let rebuilt = builder.ran_edge_without_restat_pruning(edge);
                if !streaming {
                    append_output(&mut output, &String::from_utf8_lossy(&builder.build_output));
                }
                result?;
                Ok(rebuilt)
            })();
            drop(builder);
            result
        } else {
            Ok(false)
        };
        let manifest_rebuilt = match manifest_result {
            Ok(rebuilt) => rebuilt,
            Err(error) => {
                let _ = crate::log::logclose(build_log);
                let _ = crate::deps::depsclose(deps_log);
                return Err(error);
            }
        };
        if manifest_rebuilt {
            crate::log::logclose(build_log).map_err(|error| error.to_string())?;
            crate::deps::depsclose(deps_log).map_err(|error| error.to_string())?;
            if invocation.build_options.dryrun {
                return Ok(tool_result(output));
            }
            continue;
        }

        let selected_targets = if invocation.targets.is_empty() {
            default_target_paths(&parser, &graph)
        } else {
            invocation.targets.clone()
        };
        let (result, already_up_to_date) = {
            let streaming = build_output.is_some();
            let mut builder = if let Some(output) = build_output.as_deref_mut() {
                if let Some(diagnostics) = build_diagnostics.as_deref_mut() {
                    crate::build::Builder::with_logs_and_sinks(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                        diagnostics,
                    )
                } else {
                    crate::build::Builder::with_logs_and_output(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                    )
                }
            } else {
                crate::build::Builder::with_logs(
                    &mut graph,
                    invocation.build_options.clone(),
                    &mut build_log,
                    &mut deps_log,
                )
            };
            let mut already_up_to_date = false;
            let result: Result<String, String> = (|| {
                for target in &selected_targets {
                    builder
                        .add_target(target.as_bytes())
                        .map_err(|error| format!("error: {error}"))?;
                }
                already_up_to_date = builder.already_up_to_date();
                let result = builder.build();
                let build_output = (!streaming)
                    .then(|| String::from_utf8_lossy(&builder.build_output).into_owned());
                result?;
                Ok(build_output.unwrap_or_default())
            })();
            (result, already_up_to_date)
        };
        let build_log_result = crate::log::logclose(build_log).map_err(|error| error.to_string());
        let deps_log_result = crate::deps::depsclose(deps_log).map_err(|error| error.to_string());
        append_output(&mut output, &result?);
        build_log_result?;
        deps_log_result?;
        if already_up_to_date && output.is_empty() && !invocation.build_options.quiet {
            output = "ronin: no work to do.".into();
        }
        if invocation.build_options.stats {
            append_stats(&mut output, parse_count, parse_elapsed);
        }
        return Ok(tool_result(output));
    }
    Err(format!(
        "manifest '{}' dirty after 100 tries",
        invocation.manifest
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_RUN: AtomicUsize = AtomicUsize::new(0);

    #[cfg(unix)]
    #[test]
    fn rust_cli_uses_supported_make_jobserver() {
        let mut options = BuildOptions::default();
        normalize_runtime_options(
            &mut options,
            Some("-j --jobserver-auth=fifo:/tmp/ronin-jobserver"),
        )
        .unwrap();
        assert_eq!(options.maxjobs, usize::MAX);
        assert_eq!(
            options.jobserver,
            crate::jobserver::JobserverConfig {
                mode: crate::jobserver::JobserverMode::PosixFifo,
                path: "/tmp/ronin-jobserver".into(),
            }
        );

        let mut explicit = BuildOptions {
            maxjobs: 2,
            ..BuildOptions::default()
        };
        normalize_runtime_options(&mut explicit, Some("-j --jobserver-auth=fifo:/tmp/ignored"))
            .unwrap();
        assert_eq!(explicit.maxjobs, 2);
        assert_eq!(
            explicit.jobserver,
            crate::jobserver::JobserverConfig::default()
        );
    }

    #[test]
    fn rust_cli_builds_requested_target_with_logs() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
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
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_rebuilds_and_reloads_manifest_before_targets() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-manifest-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
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
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&template, render_manifest("new")).unwrap();

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
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_continues_when_manifest_restat_prunes_rebuild() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-manifest-restat-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
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
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_clean_rule_and_generator_options() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-clean-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
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
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_compdb_expands_response_files_without_rule_filter() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-compdb-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
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
        fs::remove_dir_all(directory).unwrap();
    }
}
