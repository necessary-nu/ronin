//! The Make front end's command line.
//!
//! GNU Make's options, mapped onto the settings Ronin's scheduler already has:
//! `-j` is the job limit, `-k` the failure limit, `-n` the dry run, `-C` the
//! directory the build runs in, `-f` the makefile, bare words are goals, and a
//! word with an `=` in it is a command-line variable, which outranks both the
//! makefile's own assignments and the environment.
//!
//! Everything an invocation says about the build it says through
//! [`BuildOptions`], the same value the Ninja front end fills in, so a Makefile
//! and a manifest reach one scheduler rather than two configurations of it.
//!
//! `-C` is the one place Ronin moves the process working directory. Make
//! evaluation reads that directory directly — `$(shell)`, `$(wildcard)`,
//! `include`, and `CURDIR` are answered by the vendored evaluator against the
//! process, not against a directory Ronin could hand it — so a `-C` that did
//! not chdir would evaluate the wrong tree and then build the right one. That
//! is why Make mode is entered from the executable rather than from
//! [`Runner`]'s library path, which never moves it.

use crate::build::{BuildOptions, JobLimit};
use crate::cli::{RunResult, Runner, PRODUCT_NAME};
use crate::error::CliError;
use crate::frontend::{Build, Outcome, Persistence};
use crate::util::{terminated, BString, ByteSlice};
use crate::Error;
use kati::bytes::Bytes;
use kati::flags::Flags;
use kati::session::Session;
use kati::var::{VarOrigin, Variable};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The makefiles GNU Make reads when no `-f` names one, in its own order.
const DEFAULT_MAKEFILES: [&str; 3] = ["GNUmakefile", "makefile", "Makefile"];

/// How deep in a recursive Make tree this invocation is.
///
/// GNU Make counts recursion in the environment: the value that arrives there
/// is this invocation's depth, and the value handed to its own recipes is one
/// deeper.
const MAKELEVEL: &str = "MAKELEVEL";

/// What one Make invocation asks for.
struct Invocation {
    /// The directories `-C` named, in order. GNU Make enters each in turn, so
    /// `-C a -C b` is `a/b` rather than `b`, which is where it differs from
    /// Ninja's single overwritten directory.
    directories: Vec<PathBuf>,
    makefile: Option<PathBuf>,
    goals: Vec<BString>,
    /// `VAR=value` in command-line order, which is the order Make applies them.
    variables: Vec<Bytes>,
    jobs: Option<JobLimit>,
    /// The load average above which no further recipe starts. `-l` with no
    /// number lifts the limit rather than imposing one, which is what a limit
    /// of zero already means to the scheduler.
    load: Option<f64>,
    /// One bit per [`Switch`] the command line gave. Every switch here is
    /// answered the same way — it was given or it was not — and a field each is
    /// what would let a spelling and a meaning drift apart.
    switches: u16,
}

impl Invocation {
    const fn new() -> Self {
        Self {
            directories: Vec::new(),
            makefile: None,
            goals: Vec::new(),
            variables: Vec::new(),
            jobs: None,
            load: None,
            switches: 0,
        }
    }

    const fn add(&mut self, switch: Switch) {
        self.switches |= switch.bit();
    }

    const fn given(&self, switch: Switch) -> bool {
        self.switches & switch.bit() != 0
    }

    /// The switches a sub-make has to be told about again.
    ///
    /// GNU Make sends these down through `MAKEFLAGS`; Ronin sends them in
    /// `$(MAKE)`, because the `MAKEFLAGS` it publishes describes the job budget
    /// its children draw on and rewriting that would cost them the budget.
    fn propagated(&self) -> Vec<OsString> {
        let mut propagated = Vec::new();
        for (switch, spelling) in [
            (Switch::AlwaysMake, "-B"),
            (Switch::EnvironmentOverrides, "-e"),
            (Switch::IgnoreErrors, "-i"),
            (Switch::KeepGoing, "-k"),
            (Switch::DryRun, "-n"),
            (Switch::Question, "-q"),
            (Switch::NoBuiltinRules, "-r"),
            (Switch::Silent, "-s"),
            (Switch::PrintDirectory, "-w"),
        ] {
            if self.given(switch) {
                propagated.push(OsString::from(spelling));
            }
        }
        propagated
    }

    /// Whether this invocation brackets its build with the directory it ran in.
    ///
    /// GNU Make's rule, which every option here is a part of: `-w` asks for the
    /// pair outright, `-C` asks for it by implication because the paths a
    /// recipe prints are about to stop resolving against the caller's
    /// directory, `-s` withdraws the implication but not the request, and `-q`
    /// prints nothing at all because its whole answer is a status.
    const fn announcing(&self) -> bool {
        !self.given(Switch::Question)
            && (self.given(Switch::PrintDirectory)
                || (!self.directories.is_empty() && !self.given(Switch::Silent)))
    }
}

/// A GNU Make option that takes no argument: the whole of it is that it was
/// given.
///
/// Both spellings resolve to one of these before anything is set, so a letter
/// and its long name cannot come to mean different things.
#[derive(Clone, Copy)]
enum Switch {
    AlwaysMake,
    DryRun,
    EnvironmentOverrides,
    IgnoreErrors,
    KeepGoing,
    NoBuiltinRules,
    PrintDirectory,
    Question,
    Silent,
}

impl Switch {
    /// The switch one letter names, which is how a cluster like `-ki` reads.
    const fn short(letter: u8) -> Option<Self> {
        Some(match letter {
            b'B' => Self::AlwaysMake,
            b'e' => Self::EnvironmentOverrides,
            b'i' => Self::IgnoreErrors,
            b'k' => Self::KeepGoing,
            b'n' => Self::DryRun,
            b'q' => Self::Question,
            b'r' => Self::NoBuiltinRules,
            b's' => Self::Silent,
            b'w' => Self::PrintDirectory,
            _ => return None,
        })
    }

    /// The switch a long name names. GNU Make spells `-n` three ways and `-s`
    /// two, and every spelling is the one switch.
    fn long(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"--always-make" => Self::AlwaysMake,
            b"--dry-run" | b"--just-print" | b"--recon" => Self::DryRun,
            b"--environment-overrides" => Self::EnvironmentOverrides,
            b"--ignore-errors" => Self::IgnoreErrors,
            b"--keep-going" => Self::KeepGoing,
            b"--no-builtin-rules" => Self::NoBuiltinRules,
            b"--print-directory" => Self::PrintDirectory,
            b"--question" => Self::Question,
            b"--silent" | b"--quiet" => Self::Silent,
            _ => return None,
        })
    }

    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// What reading the command line concluded.
enum Action {
    Immediate(RunResult),
    /// Boxed because an invocation is far larger than an immediate result.
    Execute(Box<Invocation>),
}

fn usage() -> String {
    format!(
        concat!(
            "usage: {name} [options] [target...] [VAR=value...]\n",
            "\n",
            "Ronin's Make front end. Makefile syntax and variable semantics are\n",
            "GNU Make's; the diagnostics, the scheduler and the persistent state\n",
            "are Ronin's.\n",
            "\n",
            "options:\n",
            "  -f FILE  read FILE as the makefile [default={default}]\n",
            "  -C DIR   change to DIR before reading anything, once per option\n",
            "  -j [N]   run N recipes at once, or as many as are ready\n",
            "  -l [N]   start no recipe while the load average is above N\n",
            "  -B       rebuild every target, whatever its timestamps say\n",
            "  -e       let the environment outrank the makefile's assignments\n",
            "  -i       ignore the status of every recipe line\n",
            "  -k       keep going after a recipe fails\n",
            "  -n       print the recipes without running them\n",
            "  -q       report whether the goals are up to date, and build nothing\n",
            "  -r       do not read the built-in rules\n",
            "  -s       do not report what is being built\n",
            "  -w       announce the directory the build runs in\n",
            "  --ninja  read a Ninja manifest instead, whatever this program is called\n",
            "  --version, --help\n",
            "\n",
            "Each switch is also spelled as GNU Make's long option, and the ones\n",
            "taking no argument cluster: -ki is -k and -i.\n"
        ),
        name = PRODUCT_NAME,
        default = DEFAULT_MAKEFILES.join(", "),
    )
}

/// The identity Make mode reports for itself.
///
/// Not GNU Make's banner: the tool answering is Ronin. The version a Makefile
/// can branch on is `MAKE_VERSION`, and that one does name a GNU Make release.
// [spec:ronin:req:product.make-identity]
fn version() -> String {
    format!(
        "{PRODUCT_NAME} {}\nMake front end for GNU Make {} makefiles\n",
        env!("CARGO_PKG_VERSION"),
        crate::make::MAKE_VERSION,
    )
}

/// What an invocation that answered a question rather than building says.
const fn reported(text: String) -> RunResult {
    RunResult {
        stdout: text.into_bytes(),
        stderr: Vec::new(),
        exit_code: 0,
    }
}

fn refuse(message: impl std::fmt::Display) -> Action {
    Action::Immediate(RunResult {
        stdout: Vec::new(),
        stderr: format!(
            "{}\n{}",
            crate::util::diagnostic(PRODUCT_NAME, message),
            usage()
        )
        .into_bytes(),
        exit_code: 1,
    })
}

/// The value an option takes, whether it was attached or stands alone.
fn value(
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
    option: &str,
) -> Result<BString, Error> {
    if !attached.is_empty() {
        return Ok(BString::from(attached));
    }
    *index += 1;
    arguments.get(*index).cloned().ok_or_else(|| {
        CliError::MissingOptionValue {
            option: option.to_owned(),
        }
        .into()
    })
}

/// `-j`'s argument, which GNU Make lets stand alone only when it is a number.
///
/// `make -j all` is unlimited jobs and one goal; `make -j 8 all` is eight jobs
/// and one goal. Reading the next word unconditionally would swallow the goal.
fn jobs_value(
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
) -> Result<JobLimit, Error> {
    let invalid = || Error::from(CliError::InvalidParameter { option: "-j" });
    let digits = if attached.is_empty() {
        let Some(next) = arguments
            .get(*index + 1)
            .filter(|argument| argument.iter().all(u8::is_ascii_digit) && !argument.is_empty())
        else {
            return Ok(JobLimit::Unlimited);
        };
        *index += 1;
        next.clone()
    } else {
        BString::from(attached)
    };
    let count = digits
        .to_str()
        .ok()
        .and_then(|digits| digits.parse::<usize>().ok())
        .ok_or_else(invalid)?;
    // GNU Make rejects `-j0`; every other count is a limit.
    JobLimit::fixed(count).ok_or_else(invalid)
}

/// `-l`'s argument, which stands alone only when it is a number.
///
/// The same shape as `-j`'s, for the same reason: `make -l all` is one goal and
/// no load limit at all, so reading the next word unconditionally would swallow
/// it. A bare `-l` lifts the limit, which is the zero the scheduler reads as
/// "do not consult the load average".
fn load_value(arguments: &[BString], index: &mut usize, attached: &[u8]) -> Result<f64, Error> {
    let numeric = |argument: &&BString| {
        !argument.is_empty()
            && argument
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b'.')
    };
    let digits = if attached.is_empty() {
        let Some(next) = arguments.get(*index + 1).filter(numeric) else {
            return Ok(0.0);
        };
        *index += 1;
        next.clone()
    } else {
        BString::from(attached)
    };
    digits
        .to_str()
        .ok()
        .and_then(|digits| digits.parse::<f64>().ok())
        .ok_or_else(|| CliError::InvalidParameter { option: "-l" }.into())
}

/// A long option carrying its value after an `=`, in the spellings that take
/// one at all. Says whether the option was one of them.
fn attached_long(
    invocation: &mut Invocation,
    option: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> Result<bool, Error> {
    if let Some(named) = option
        .strip_prefix(b"--file=")
        .or_else(|| option.strip_prefix(b"--makefile="))
    {
        invocation.makefile = Some(path_of(named)?);
    } else if let Some(named) = option.strip_prefix(b"--directory=") {
        invocation.directories.push(path_of(named)?);
    } else if let Some(count) = option.strip_prefix(b"--jobs=") {
        invocation.jobs = Some(jobs_value(arguments, index, count)?);
    } else if let Some(load) = option.strip_prefix(b"--load-average=") {
        invocation.load = Some(load_value(arguments, index, load)?);
    } else {
        return Ok(false);
    }
    Ok(true)
}

/// Read one Make command line.
// [spec:ronin:req:product.make-identity]
fn parse(arguments: &[BString]) -> Result<Action, Error> {
    let mut invocation = Invocation::new();
    let mut index = 1;
    let mut options_enabled = true;
    while index < arguments.len() {
        let argument = arguments[index].as_bytes();
        if !options_enabled || !argument.starts_with(b"-") || argument == b"-" {
            classify_word(&mut invocation, &arguments[index]);
            index += 1;
            continue;
        }
        if let Some(switch) = Switch::long(argument) {
            invocation.add(switch);
            index += 1;
            continue;
        }
        match argument {
            b"--" => options_enabled = false,
            b"--version" => return Ok(Action::Immediate(reported(version()))),
            b"--help" => return Ok(Action::Immediate(reported(usage()))),
            selector if crate::multicall::is_selector(selector) => {}
            b"--jobs" => invocation.jobs = Some(jobs_value(arguments, &mut index, b"")?),
            b"--load-average" => invocation.load = Some(load_value(arguments, &mut index, b"")?),
            b"--file" | b"--makefile" => {
                let named = value(arguments, &mut index, b"", "--file")?;
                invocation.makefile = Some(path_of(named.as_bytes())?);
            }
            b"--directory" => {
                let named = value(arguments, &mut index, b"", "--directory")?;
                invocation.directories.push(path_of(named.as_bytes())?);
            }
            option if option.starts_with(b"--") => {
                if !attached_long(&mut invocation, option, arguments, &mut index)? {
                    return Ok(refuse(format_args!(
                        "unrecognized option '{}'",
                        option.to_str_lossy()
                    )));
                }
            }
            _ => {
                let mut short = 1;
                while short < argument.len() {
                    let option = argument[short];
                    short += 1;
                    if let Some(switch) = Switch::short(option) {
                        invocation.add(switch);
                        continue;
                    }
                    match option {
                        b'h' => return Ok(Action::Immediate(reported(usage()))),
                        b'j' => {
                            invocation.jobs =
                                Some(jobs_value(arguments, &mut index, &argument[short..])?);
                            short = argument.len();
                        }
                        b'l' => {
                            invocation.load =
                                Some(load_value(arguments, &mut index, &argument[short..])?);
                            short = argument.len();
                        }
                        b'f' | b'C' => {
                            let named = value(
                                arguments,
                                &mut index,
                                &argument[short..],
                                match option {
                                    b'f' => "-f",
                                    _ => "-C",
                                },
                            )?;
                            short = argument.len();
                            let named = path_of(named.as_bytes())?;
                            if option == b'f' {
                                invocation.makefile = Some(named);
                            } else {
                                invocation.directories.push(named);
                            }
                        }
                        _ => {
                            return Ok(refuse(format_args!(
                                "invalid option -- '{}'",
                                char::from(option)
                            )))
                        }
                    }
                }
            }
        }
        index += 1;
    }
    Ok(Action::Execute(Box::new(invocation)))
}

fn path_of(value: &[u8]) -> Result<PathBuf, Error> {
    value
        .to_os_str()
        .map(|value| PathBuf::from(value.to_owned()))
        .map_err(|_| {
            CliError::InvalidEncoding {
                context: crate::error::EncodingContext::Argument,
            }
            .into()
        })
}

/// A word that is not an option: a variable assignment, or a goal.
///
/// Make's own test, and the vendored evaluator's: a word with an `=` anywhere
/// in it assigns, and nothing else does. A command-line assignment is not an
/// environment variable — it outranks the makefile's own, which is why it
/// travels as an assignment rather than as an exported value.
fn classify_word(invocation: &mut Invocation, word: &BString) {
    if word.contains(&b'=') {
        invocation.variables.push(Bytes::from(word.to_vec()));
    } else {
        invocation.goals.push(word.clone());
    }
}

/// The first of GNU Make's default makefiles that exists in `directory`.
fn default_makefile(directory: &Path) -> Option<PathBuf> {
    DEFAULT_MAKEFILES
        .iter()
        .map(PathBuf::from)
        .find(|candidate| directory.join(candidate).is_file())
}

/// What `$(MAKE)` has to be for recursion to re-enter Ronin.
///
/// `current_exe` resolves the symlink a `make`-named invocation came through,
/// so the path it reports selects Ninja mode by name. `--make` is what makes
/// the sub-make a sub-make; without it a recursive Makefile would find a Ninja
/// front end looking for `build.ninja`.
// [spec:ronin:req:make.recursive-invocation]
fn recursive_command(executable: &Path, invocation: &Invocation) -> Vec<OsString> {
    let mut command = vec![executable.as_os_str().to_owned(), OsString::from("--make")];
    command.extend(invocation.propagated());
    command.extend(
        invocation
            .variables
            .iter()
            .filter_map(|assignment| assignment.to_os_str().ok().map(std::ffi::OsStr::to_owned)),
    );
    command
}

/// The evaluation session one Make invocation describes.
// [spec:ronin:req:make.recursive-invocation]
fn session_for(
    invocation: &Invocation,
    makefile: &Path,
    jobs: usize,
    executable: &Path,
    inherited: Option<&str>,
) -> Session {
    let mut session = Session::new();
    // A parent's command-line assignments arrive in MAKEFLAGS, after the
    // switches: those are the words with an `=` that are not options.
    let mut variables = inherited
        .into_iter()
        .flat_map(str::split_ascii_whitespace)
        .filter(|word| !word.starts_with('-') && word.contains('='))
        .map(|word| Bytes::from(word.as_bytes().to_vec()))
        .collect::<Vec<_>>();
    variables.extend(invocation.variables.iter().cloned());
    session.flags = Flags {
        makefile: Some(makefile.as_os_str().to_owned()),
        num_jobs: jobs,
        num_cpus: jobs,
        is_silent_mode: invocation.given(Switch::Silent),
        // The three options whose whole effect is on evaluation rather than on
        // the build: what the makefile starts with, what outranks it, and
        // whether a recipe line's status is worth stopping for.
        no_builtin_rules: invocation.given(Switch::NoBuiltinRules),
        environment_overrides: invocation.given(Switch::EnvironmentOverrides),
        ignore_errors: invocation.given(Switch::IgnoreErrors),
        cl_vars: variables,
        subkati_args: recursive_command(executable, invocation),
        ..Flags::default()
    };
    session.flags.targets = invocation
        .goals
        .iter()
        .map(|goal| session.intern(goal.to_vec()))
        .collect();
    session
}

/// Tell the makefile how deep in a recursive tree it is.
// [spec:ronin:req:make.recursive-invocation]
fn record_makelevel(session: &mut Session, level: usize) -> Result<(), Error> {
    // The evaluator imports the process environment itself, so a level that
    // arrived there is already the makefile's, with the origin GNU Make gives
    // it. Only the top of a tree, where nothing set one, needs an answer
    // supplied, and Make's answer there is zero.
    if std::env::var_os(MAKELEVEL).is_some() {
        return Ok(());
    }
    let name = session.intern(MAKELEVEL);
    let value = Variable::with_simple_string(
        Bytes::from(level.to_string().into_bytes()),
        VarOrigin::Environment,
        None,
        None,
    );
    session
        .set_global_var(name, value, false, None)
        .map_err(|error| {
            CliError::InvocationFailed {
                exit_code: 1,
                diagnostic: error.to_string(),
            }
            .into()
        })
}

/// The scheduler settings this invocation asks for.
// [spec:ronin:req:product.make-identity]
fn build_options(
    invocation: &Invocation,
    runner: &Runner,
    working_directory: crate::os::WorkingDirectory,
    level: usize,
) -> Result<BuildOptions, Error> {
    let mut options = BuildOptions {
        jobs: invocation.jobs.unwrap_or(JobLimit::Auto),
        // GNU Make's -k has no count: it stops when nothing is left that could
        // run, which is what an unbounded failure limit means here.
        maxfail: if invocation.given(Switch::KeepGoing) {
            usize::MAX
        } else {
            1
        },
        dryrun: invocation.given(Switch::DryRun),
        // Make's `-n` exists to show the recipes rather than to run them, so it
        // asks for the commands themselves and not for the descriptions a
        // build would otherwise report.
        verbose: invocation.given(Switch::DryRun),
        quiet: invocation.given(Switch::Silent),
        // Make's `-l` and Ninja's are one ceiling: the scheduler starts nothing
        // further while the load average is above it, and zero is no ceiling.
        maxload: invocation.load.unwrap_or_default(),
        working_directory,
        ..BuildOptions::default()
    };
    crate::cli::normalize_runtime_options(
        &mut options,
        runner.makeflags.as_deref(),
        // NINJA_STATUS is the Ninja front end's name for its own rendering, and
        // a Makefile has never heard of it.
        None,
        runner.terminal,
        runner.connect_jobserver,
        // Make runs one recipe at a time unless it is told otherwise. Ninja's
        // guess at the machine would be a different tool's answer to the same
        // Makefile.
        JobLimit::Fixed(std::num::NonZeroUsize::MIN),
    )?;
    options.environment.push((
        MAKELEVEL,
        OsString::from(level.saturating_add(1).to_string()),
    ));
    Ok(options)
}

/// How many recipes at once, for the pool the evaluator declares for itself.
const fn job_count(options: &BuildOptions) -> usize {
    match options.jobs {
        JobLimit::Fixed(jobs) => jobs.get(),
        JobLimit::Auto | JobLimit::Unlimited => usize::MAX,
    }
}

/// Enter the directories `-C` named, in order, and report where that landed.
///
/// This moves the process, which is what GNU Make's `-C` is.
fn enter_directories(directories: &[PathBuf]) -> Result<PathBuf, Error> {
    for directory in directories {
        std::env::set_current_dir(directory).map_err(|source| CliError::ChangeDirectory {
            path: BString::from(directory.as_os_str().as_encoded_bytes()),
            source,
        })?;
    }
    std::env::current_dir().map_err(|source| CliError::CurrentDirectory { source }.into())
}

/// What an invocation with nothing to read reports.
///
/// The announcement is a pair or it is nothing: an Entering with no Leaving
/// leaves every parser reading them resolving paths against a directory the
/// build has already left.
fn no_makefile(reported: String, announcing: bool, directory: &Path) -> RunResult {
    departed(
        RunResult {
            stdout: terminated(reported),
            stderr: format!(
                "{PRODUCT_NAME}: *** No targets specified and no makefile found.  Stop.\n"
            )
            .into_bytes(),
            exit_code: 1,
        },
        announcing,
        directory,
    )
}

/// Run one Make invocation to its end.
// [spec:ronin:req:product.make-identity]
// [spec:ronin:req:make.recursive-invocation]
pub(crate) fn run(
    runner: &Runner,
    arguments: &[BString],
    mut output: Option<&mut dyn Write>,
    diagnostics: Option<&mut dyn Write>,
) -> Result<RunResult, Error> {
    let invocation = match parse(arguments)? {
        Action::Immediate(result) => return Ok(result),
        Action::Execute(invocation) => *invocation,
    };
    let mut reported = String::new();
    let directory = enter_directories(&invocation.directories)?;
    let announcing = invocation.announcing();
    if announcing {
        say(
            &mut output,
            &mut reported,
            &announcement("Entering", &directory),
        )?;
    }
    let working_directory = crate::os::WorkingDirectory::new(&directory)
        .map_err(|source| CliError::CurrentDirectory { source })?;

    let Some(makefile) = invocation
        .makefile
        .clone()
        .or_else(|| default_makefile(&directory))
    else {
        return Ok(no_makefile(reported, announcing, &directory));
    };

    let level = runner
        .makelevel
        .as_deref()
        .and_then(|level| level.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let options = build_options(&invocation, runner, working_directory, level)?;
    let mut session = session_for(
        &invocation,
        &makefile,
        job_count(&options),
        &runner.executable,
        runner.makeflags.as_deref(),
    );
    record_makelevel(&mut session, level)?;

    let mut graph = match crate::make::load_makefile(session) {
        Ok(graph) => graph,
        Err(failure) => {
            return Ok(RunResult {
                stdout: terminated(reported),
                stderr: terminated(failure.to_string()),
                // A makefile that will not evaluate is a question `-q` could
                // not answer, which Make reports as two rather than as one.
                exit_code: if invocation.given(Switch::Question) {
                    2
                } else {
                    1
                },
            });
        }
    };
    if invocation.given(Switch::AlwaysMake) {
        graph.rebuild_everything();
    }
    let (mut persistence, warning) = Persistence::open(&mut graph, &directory)?;
    if let Some(warning) = warning {
        reported.push_str(&warning);
    }

    let targets = graph.default_targets();
    let mut build = Build::with_options(&mut graph, &mut persistence, options);
    if let Some(sink) = output {
        build = build.output(sink);
    }
    if let Some(sink) = diagnostics {
        build = build.diagnostics(sink);
    }
    let planned = build.plan(&targets);
    if invocation.given(Switch::Question) {
        let question = planned.map(|planned| planned.already_up_to_date());
        let flushed = persistence.finish();
        let question = question.and_then(|up_to_date| flushed.map(|()| up_to_date));
        return Ok(departed(
            answered(reported, question),
            announcing,
            &directory,
        ));
    }
    let outcome = planned.and_then(|planned| {
        let up_to_date = planned.already_up_to_date();
        planned.run().map(|outcome| (up_to_date, outcome))
    });
    let flushed = persistence.finish();
    let (up_to_date, outcome) = outcome?;
    flushed?;
    Ok(departed(
        finished(
            reported,
            up_to_date,
            &outcome,
            invocation.given(Switch::Silent),
        ),
        announcing,
        &directory,
    ))
}

/// One half of GNU Make's directory announcement, in GNU Make's own words.
///
/// Every error parser that inherited the convention reads this pair to resolve
/// the relative paths a compiler then prints, so the wording and the quoting
/// are Make 4.4's rather than Ninja's; only the name in front is Ronin's.
// [spec:ronin:req:product.make-identity]
fn announcement(verb: &str, directory: &Path) -> String {
    format!("{PRODUCT_NAME}: {verb} directory '{}'", directory.display())
}

/// Put a line where the caller will see it, in the order the build saw it.
///
/// A caller that gave a sink is watching the build happen and gets the line as
/// it is said; a caller that did not is handed it back with the result.
fn say(
    output: &mut Option<&mut dyn Write>,
    reported: &mut String,
    line: &str,
) -> Result<(), Error> {
    if let Some(sink) = output.as_deref_mut() {
        writeln!(sink, "{line}").map_err(CliError::write_output)?;
        sink.flush().map_err(CliError::write_output)?;
    } else {
        reported.push_str(line);
        reported.push('\n');
    }
    Ok(())
}

/// Close the directory announcement this invocation opened.
///
/// Last of everything the invocation says, which is where GNU Make puts it and
/// where a parser reading the pair stops resolving paths against the directory.
fn departed(mut result: RunResult, announcing: bool, directory: &Path) -> RunResult {
    if announcing {
        result
            .stdout
            .extend_from_slice(&terminated(announcement("Leaving", directory)));
    }
    result
}

/// What `-q` reports, which is a status and nothing else.
///
/// GNU Make's question mode runs no recipe and says nothing about the build:
/// zero says the goals are already up to date, one says something would have to
/// run, and two says the question could not be answered at all. That convention
/// is Make's rather than Ninja's, and it governs here because no build ran to
/// have a status of its own.
// [spec:ronin:req:make.question-status]
fn answered(reported: String, question: Result<bool, Error>) -> RunResult {
    match question {
        Ok(up_to_date) => RunResult {
            stdout: terminated(reported),
            stderr: Vec::new(),
            exit_code: i32::from(!up_to_date),
        },
        Err(failure) => RunResult {
            stdout: terminated(reported),
            stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
            exit_code: 2,
        },
    }
}

/// What the invocation reports about a build that ran.
///
/// A build that stopped is a result rather than a diagnostic: it is said on
/// stdout after the build's own output, and the status left with is the
/// failing recipe's own. That is Ninja's contract rather than Make's exit 2,
/// and where the two contracts meet the Ninja one governs.
fn finished(reported: String, up_to_date: bool, outcome: &Outcome, silent: bool) -> RunResult {
    let mut stdout = terminated(reported);
    stdout.extend_from_slice(outcome.output());
    if let Some(reason) = outcome.stopped() {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: build stopped: {reason}.\n").as_bytes());
    } else if up_to_date && stdout.is_empty() && !silent {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: no work to do.\n").as_bytes());
    }
    RunResult {
        stdout,
        stderr: Vec::new(),
        exit_code: outcome.exit_code(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, Action, Invocation, Switch};
    use crate::build::JobLimit;
    use crate::util::BString;
    use std::path::PathBuf;

    fn parsed(arguments: &[&str]) -> Invocation {
        let arguments = arguments
            .iter()
            .map(|argument| BString::from(*argument))
            .collect::<Vec<_>>();
        match parse(&arguments).unwrap() {
            Action::Execute(invocation) => *invocation,
            Action::Immediate(_) => panic!("these arguments describe a build"),
        }
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn make_options_map_onto_the_settings_the_scheduler_already_has() {
        let invocation = parsed(&[
            "make", "-j8", "-k", "-n", "-f", "other.mk", "all", "FOO=bar",
        ]);
        assert_eq!(invocation.jobs, JobLimit::fixed(8));
        assert!(invocation.given(Switch::KeepGoing));
        assert!(invocation.given(Switch::DryRun));
        assert_eq!(invocation.makefile, Some(PathBuf::from("other.mk")));
        assert_eq!(invocation.goals, vec![BString::from("all")]);
        assert_eq!(
            invocation.variables,
            vec![kati::bytes::Bytes::from_static(b"FOO=bar")]
        );
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn clustered_switches_and_repeated_directories_read_as_make_reads_them() {
        let invocation = parsed(&["make", "-kn", "-C", "a", "-C", "b"]);
        assert!(invocation.given(Switch::KeepGoing) && invocation.given(Switch::DryRun));
        assert_eq!(
            invocation.directories,
            vec![PathBuf::from("a"), PathBuf::from("b")]
        );
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn a_lone_jobs_flag_takes_a_number_and_leaves_a_goal_alone() {
        let unlimited = parsed(&["make", "-j", "all"]);
        assert_eq!(unlimited.jobs, Some(JobLimit::Unlimited));
        assert_eq!(unlimited.goals, vec![BString::from("all")]);

        let counted = parsed(&["make", "-j", "4", "all"]);
        assert_eq!(counted.jobs, JobLimit::fixed(4));
        assert_eq!(counted.goals, vec![BString::from("all")]);
    }

    // [spec:ronin:req:make.recursive-invocation/test]
    #[test]
    fn a_sub_make_is_told_the_switches_and_the_assignments_again() {
        let invocation = parsed(&["make", "-k", "-s", "FOO=bar", "all"]);
        let command = super::recursive_command(std::path::Path::new("/opt/ronin"), &invocation);
        assert_eq!(
            command,
            vec![
                std::ffi::OsString::from("/opt/ronin"),
                std::ffi::OsString::from("--make"),
                std::ffi::OsString::from("-k"),
                std::ffi::OsString::from("-s"),
                std::ffi::OsString::from("FOO=bar"),
            ]
        );
    }

    /// The diagnostic an argument list is refused with, or nothing if it
    /// described a build after all.
    fn refused(arguments: &[&str]) -> Option<String> {
        let arguments = arguments
            .iter()
            .map(|argument| BString::from(*argument))
            .collect::<Vec<_>>();
        match parse(&arguments).unwrap() {
            Action::Immediate(result) => {
                assert_eq!(result.exit_code, 1);
                Some(String::from_utf8_lossy(&result.stderr).into_owned())
            }
            Action::Execute(_) => None,
        }
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn each_switch_reads_the_same_from_its_letter_and_from_its_long_name() {
        for (letter, name) in [
            ("-B", "--always-make"),
            ("-e", "--environment-overrides"),
            ("-i", "--ignore-errors"),
            ("-k", "--keep-going"),
            ("-n", "--dry-run"),
            ("-q", "--question"),
            ("-r", "--no-builtin-rules"),
            ("-s", "--silent"),
            ("-w", "--print-directory"),
        ] {
            let short = parsed(&["make", letter]).switches;
            assert_eq!(
                short.count_ones(),
                1,
                "{letter} set no switch, or more than one"
            );
            assert_eq!(
                short,
                parsed(&["make", name]).switches,
                "{letter} differs from {name}"
            );
        }
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn a_cluster_of_the_new_switches_reads_as_make_reads_it() {
        let invocation = parsed(&["make", "-kiBrq", "all"]);
        assert!(invocation.given(Switch::KeepGoing));
        assert!(invocation.given(Switch::IgnoreErrors));
        assert!(invocation.given(Switch::AlwaysMake));
        assert!(invocation.given(Switch::NoBuiltinRules));
        assert!(invocation.given(Switch::Question));
        assert_eq!(invocation.goals, vec![BString::from("all")]);
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn a_lone_load_flag_takes_a_number_and_leaves_a_goal_alone() {
        // A bare -l lifts the limit rather than imposing one, and the word
        // after it is still a goal.
        let lifted = parsed(&["make", "-l", "all"]);
        assert_eq!(lifted.load, Some(0.0));
        assert_eq!(lifted.goals, vec![BString::from("all")]);

        for spelling in [
            ["-l", "2.5"].as_slice(),
            ["-l2.5"].as_slice(),
            ["--load-average", "2.5"].as_slice(),
            ["--load-average=2.5"].as_slice(),
        ] {
            let mut arguments = vec!["make"];
            arguments.extend_from_slice(spelling);
            arguments.push("all");
            let invocation = parsed(&arguments);
            assert_eq!(invocation.load, Some(2.5), "{spelling:?}");
            assert_eq!(invocation.goals, vec![BString::from("all")], "{spelling:?}");
        }
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn a_load_ceiling_reaches_the_setting_the_scheduler_already_honours() {
        let directory = std::env::temp_dir();
        let runner = crate::cli::Runner::new(&directory).unwrap();
        let options = |arguments: &[&str]| {
            let working = crate::os::WorkingDirectory::new(&directory).unwrap();
            super::build_options(&parsed(arguments), &runner, working, 0).unwrap()
        };
        assert!((options(&["make", "-l", "2.5"]).maxload - 2.5).abs() < f64::EPSILON);
        // Zero is what the scheduler reads as no ceiling, and it is what an
        // invocation that never mentioned one leaves behind.
        assert!(options(&["make"]).maxload.abs() < f64::EPSILON);
        assert!(options(&["make", "-l"]).maxload.abs() < f64::EPSILON);
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn the_announcement_is_asked_for_by_w_and_implied_by_c() {
        assert!(!parsed(&["make"]).announcing());
        assert!(parsed(&["make", "-w"]).announcing());
        assert!(parsed(&["make", "-C", "sub"]).announcing());
        // -s withdraws what -C implied but not what -w asked for outright,
        // and -q says nothing at all because its answer is a status.
        assert!(!parsed(&["make", "-s", "-C", "sub"]).announcing());
        assert!(parsed(&["make", "-s", "-w"]).announcing());
        assert!(!parsed(&["make", "-w", "-q"]).announcing());
    }

    // [spec:ronin:req:make.recursive-invocation/test]
    #[test]
    fn a_sub_make_is_told_every_switch_it_would_otherwise_lose() {
        let invocation = parsed(&["make", "-Beiqrw", "all"]);
        let command = super::recursive_command(std::path::Path::new("/opt/ronin"), &invocation);
        assert_eq!(
            command,
            ["/opt/ronin", "--make", "-B", "-e", "-i", "-q", "-r", "-w"]
                .map(std::ffi::OsString::from)
                .to_vec()
        );
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn an_option_no_front_end_has_is_refused_by_name() {
        let diagnostic = refused(&["make", "--print-data-base"]).expect("an unknown option");
        assert!(
            diagnostic.starts_with("ronin: unrecognized option '--print-data-base'"),
            "{diagnostic}"
        );
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn the_options_no_machinery_answers_are_still_refused_by_name() {
        // Each needs work the graph does not take — a per-node timestamp
        // override, an include search path, a database dump — and being
        // refused is what keeps an invocation that asks for one from quietly
        // building something else.
        for option in ["-t", "-o", "-W", "-I", "-p"] {
            let diagnostic = refused(&["make", option, "x"])
                .unwrap_or_else(|| panic!("{option} cannot describe a build"));
            assert!(
                diagnostic.starts_with(&format!(
                    "ronin: invalid option -- '{}'",
                    option.trim_start_matches('-')
                )),
                "{diagnostic}"
            );
        }
        for option in [
            "--touch",
            "--old-file",
            "--what-if",
            "--include-dir",
            "--debug",
        ] {
            let diagnostic = refused(&["make", option, "x"])
                .unwrap_or_else(|| panic!("{option} cannot describe a build"));
            assert!(
                diagnostic.starts_with(&format!("ronin: unrecognized option '{option}'")),
                "{diagnostic}"
            );
        }
    }
}
