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
    keep_going: bool,
    dry_run: bool,
    silent: bool,
}

impl Invocation {
    const fn new() -> Self {
        Self {
            directories: Vec::new(),
            makefile: None,
            goals: Vec::new(),
            variables: Vec::new(),
            jobs: None,
            keep_going: false,
            dry_run: false,
            silent: false,
        }
    }

    /// The switches a sub-make has to be told about again.
    ///
    /// GNU Make sends these down through `MAKEFLAGS`; Ronin sends them in
    /// `$(MAKE)`, because the `MAKEFLAGS` it publishes describes the job budget
    /// its children draw on and rewriting that would cost them the budget.
    fn propagated(&self) -> Vec<OsString> {
        let mut propagated = Vec::new();
        for (set, flag) in [
            (self.keep_going, "-k"),
            (self.dry_run, "-n"),
            (self.silent, "-s"),
        ] {
            if set {
                propagated.push(OsString::from(flag));
            }
        }
        propagated
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
            "  -k       keep going after a recipe fails\n",
            "  -n       print the recipes without running them\n",
            "  -s       do not report what is being built\n",
            "  --ninja  read a Ninja manifest instead, whatever this program is called\n",
            "  --version, --help\n"
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
        match argument {
            b"--" => options_enabled = false,
            b"--version" => return Ok(Action::Immediate(reported(version()))),
            b"--help" => return Ok(Action::Immediate(reported(usage()))),
            selector if crate::multicall::is_selector(selector) => {}
            b"--keep-going" => invocation.keep_going = true,
            b"--dry-run" | b"--just-print" | b"--recon" => invocation.dry_run = true,
            b"--silent" | b"--quiet" => invocation.silent = true,
            b"--jobs" => invocation.jobs = Some(jobs_value(arguments, &mut index, b"")?),
            b"--file" | b"--makefile" => {
                let named = value(arguments, &mut index, b"", "--file")?;
                invocation.makefile = Some(path_of(named.as_bytes())?);
            }
            b"--directory" => {
                let named = value(arguments, &mut index, b"", "--directory")?;
                invocation.directories.push(path_of(named.as_bytes())?);
            }
            option if option.starts_with(b"--") => {
                if let Some(named) = option
                    .strip_prefix(b"--file=")
                    .or_else(|| option.strip_prefix(b"--makefile="))
                {
                    invocation.makefile = Some(path_of(named)?);
                } else if let Some(named) = option.strip_prefix(b"--directory=") {
                    invocation.directories.push(path_of(named)?);
                } else if let Some(count) = option.strip_prefix(b"--jobs=") {
                    invocation.jobs = Some(jobs_value(arguments, &mut index, count)?);
                } else {
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
                    match option {
                        b'k' => invocation.keep_going = true,
                        b'n' => invocation.dry_run = true,
                        b's' => invocation.silent = true,
                        b'h' => return Ok(Action::Immediate(reported(usage()))),
                        b'j' => {
                            invocation.jobs =
                                Some(jobs_value(arguments, &mut index, &argument[short..])?);
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
        is_silent_mode: invocation.silent,
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
        maxfail: if invocation.keep_going { usize::MAX } else { 1 },
        dryrun: invocation.dry_run,
        // Make's `-n` exists to show the recipes rather than to run them, so it
        // asks for the commands themselves and not for the descriptions a
        // build would otherwise report.
        verbose: invocation.dry_run,
        quiet: invocation.silent,
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
    if !invocation.directories.is_empty() {
        // GNU Make announces a `-C` build the same way, because every error
        // parser that inherited its convention reads this line to resolve the
        // relative paths a compiler then prints.
        let announcement = format!(
            "{PRODUCT_NAME}: Entering directory `{}'",
            directory.display()
        );
        match output.as_deref_mut() {
            Some(sink) => {
                writeln!(sink, "{announcement}").map_err(CliError::write_output)?;
                sink.flush().map_err(CliError::write_output)?;
            }
            None => reported.push_str(&announcement),
        }
    }
    let working_directory = crate::os::WorkingDirectory::new(&directory)
        .map_err(|source| CliError::CurrentDirectory { source })?;

    let Some(makefile) = invocation
        .makefile
        .clone()
        .or_else(|| default_makefile(&directory))
    else {
        return Ok(RunResult {
            stdout: Vec::new(),
            stderr: format!(
                "{PRODUCT_NAME}: *** No targets specified and no makefile found.  Stop.\n"
            )
            .into_bytes(),
            exit_code: 1,
        });
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
                exit_code: 1,
            })
        }
    };
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
    let outcome = planned.and_then(|planned| {
        let up_to_date = planned.already_up_to_date();
        planned.run().map(|outcome| (up_to_date, outcome))
    });
    let flushed = persistence.finish();
    let (up_to_date, outcome) = outcome?;
    flushed?;
    Ok(finished(reported, up_to_date, &outcome, invocation.silent))
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
    use super::{parse, Action, Invocation};
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
        assert!(invocation.keep_going);
        assert!(invocation.dry_run);
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
        assert!(invocation.keep_going && invocation.dry_run);
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

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn an_option_no_front_end_has_is_refused_by_name() {
        let arguments = [BString::from("make"), BString::from("--always-make")];
        let Action::Immediate(result) = parse(&arguments).unwrap() else {
            panic!("an unknown option cannot describe a build");
        };
        assert_eq!(result.exit_code, 1);
        let diagnostic = String::from_utf8_lossy(&result.stderr);
        assert!(
            diagnostic.starts_with("ronin: unrecognized option '--always-make'"),
            "{diagnostic}"
        );
    }
}
