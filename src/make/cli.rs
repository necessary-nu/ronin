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

/// What GNU Make exits with when it abandons a build instead of finishing one.
///
/// Every way of not building is this one status: an option it does not know, no
/// makefile to read, a makefile it cannot evaluate, a target with no rule, a
/// recipe that failed. One is not a failure in Make's vocabulary — it is the
/// answer `-q` gives to a question — so a build that gives up must not report
/// it. Scripts branch on the difference.
const ABANDONED: i32 = 2;

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
    /// One bit per [`Switch`] the command line took back. Not the complement of
    /// `switches`: `--no-print-directory` withdraws the announcement `-C`
    /// implies, so refusing differs from never having asked.
    negated: u16,
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
            negated: 0,
        }
    }

    /// Each spelling clears the other, so the one written last wins.
    const fn add(&mut self, switch: Switch) {
        self.switches |= switch.bit();
        self.negated &= !switch.bit();
    }

    const fn withdraw(&mut self, switch: Switch) {
        self.switches &= !switch.bit();
        self.negated |= switch.bit();
    }

    const fn given(&self, switch: Switch) -> bool {
        self.switches & switch.bit() != 0
    }

    const fn refused(&self, switch: Switch) -> bool {
        self.negated & switch.bit() != 0
    }

    /// Take on the switches a parent make put in `MAKEFLAGS`.
    ///
    /// GNU Make reads that variable as though it were typed on the command
    /// line, which is what makes `-s` and `-k` reach the whole tree from the
    /// top of it. Ronin has to read it for the same reason, and more sharply:
    /// this is the only way a switch reaches a sub-make, since `$(MAKE)` is a
    /// path and carries nothing.
    ///
    /// Only the switches. Everything else in there belongs to somebody else —
    /// the assignments are `session_for`'s and the jobserver's auth token is
    /// the transport's, and a long option is skipped whole rather than read
    /// letter by letter, since `--jobserver-auth` is full of letters that name
    /// switches. The two long withdrawals are recognised by name before that
    /// skip, having no letter to travel as.
    // [spec:ronin:req:make.recursive-invocation]
    fn adopt_inherited(&mut self, inherited: Option<&str>) {
        for word in inherited
            .unwrap_or_default()
            .split_ascii_whitespace()
            .take_while(|word| *word != "--")
        {
            if let Some(switch) = Switch::long_negation(word.as_bytes()) {
                self.withdraw(switch);
                continue;
            }
            if word.starts_with("--") || word.contains('=') {
                continue;
            }
            for letter in word.trim_start_matches('-').bytes() {
                if let Some(switch) = Switch::short(letter) {
                    self.add(switch);
                } else if let Some(switch) = Switch::short_negation(letter) {
                    self.withdraw(switch);
                }
            }
        }
    }

    /// The switches a sub-make has to be told about again.
    ///
    /// These travel in `MAKEFLAGS`, where GNU Make puts them and where the job
    /// budget already travels — the two are spliced rather than one replacing
    /// the other. They were once appended to `$(MAKE)` instead, on the belief
    /// that `MAKEFLAGS` was the jobserver's alone; that made `$(MAKE)` several
    /// words, and a consumer that treats the answer as a path cannot exec it.
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
        // The one negating letter, and it travels with the rest: `make -k -S`
        // hands a child `MAKEFLAGS=S`.
        if self.refused(Switch::KeepGoing) {
            propagated.push(OsString::from("-S"));
        }
        propagated
    }

    /// The withdrawals with no letter, which GNU Make writes after the group:
    /// `make -k --no-print-directory` gives `MAKEFLAGS=k --no-print-directory`.
    fn withdrawn(&self) -> Vec<&'static str> {
        let mut withdrawn = Vec::new();
        for (switch, spelling) in [
            (Switch::PrintDirectory, "--no-print-directory"),
            (Switch::Silent, "--no-silent"),
        ] {
            if self.refused(switch) {
                withdrawn.push(spelling);
            }
        }
        withdrawn
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
            && !self.refused(Switch::PrintDirectory)
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

    /// GNU Make has exactly one of these.
    const fn short_negation(letter: u8) -> Option<Self> {
        match letter {
            b'S' => Some(Self::KeepGoing),
            _ => None,
        }
    }

    fn long_negation(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"--no-keep-going" | b"--stop" => Self::KeepGoing,
            b"--no-print-directory" => Self::PrintDirectory,
            b"--no-silent" => Self::Silent,
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
            "  -S       stop at the first failure, taking back -k\n",
            "  -v       print the version, as --version does\n",
            "  --no-print-directory, --no-silent\n",
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
/// Not GNU Make's banner: the tool answering is Ronin, and the first line says
/// so. It opens with `GNU Make compatible:` rather than the product name
/// because GNU Make's own test suite refuses any binary whose `-v` does not
/// begin `GNU Make `, and being measurable by that suite is worth more than
/// the word order. It stops short of a version there — a bare `GNU Make 4.4.1`
/// prefix is what a naive parser reads as the real thing, and this is a
/// compatibility claim rather than an identity. The version a Makefile can
/// branch on is `MAKE_VERSION`, and that one does name a GNU Make release.
// [spec:ronin:req:product.make-identity]
fn version() -> String {
    format!(
        "GNU Make compatible: {PRODUCT_NAME} {}\nMake front end for GNU Make {} makefiles\n",
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
        exit_code: ABANDONED,
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
        if let Some(switch) = Switch::long_negation(argument) {
            invocation.withdraw(switch);
            index += 1;
            continue;
        }
        match argument {
            b"--" => options_enabled = false,
            // `-v` is GNU Make's short spelling, and its own test suite asks
            // for the version that way before it will run anything at all.
            b"--version" | b"-v" => return Ok(Action::Immediate(reported(version()))),
            b"--help" => return Ok(Action::Immediate(reported(usage()))),
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
                    if let Some(switch) = Switch::short_negation(option) {
                        invocation.withdraw(switch);
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

/// The evaluation session one Make invocation describes.
// [spec:ronin:req:make.recursive-invocation]
fn session_for(
    invocation: &Invocation,
    makefile: &Path,
    jobs: usize,
    invoked_as: &Path,
    inherited: Option<&str>,
    level: usize,
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
        // One word, and that word is a path. GNU Make answers `$(MAKE)` this
        // way and a great deal of software execs the answer rather than running
        // it through a shell — upstream's own suite adopts it as the program for
        // every later invocation. Nothing has to ride along, because the path
        // already names Make mode: that is the whole point of selecting the
        // front end by name. Switches and assignments travel in MAKEFLAGS.
        subkati_args: vec![invoked_as.as_os_str().to_owned()],
        // What a diagnostic with no file and line leads with. GNU Make leads
        // with its own name and, below the top, with the level too.
        program_name: if level == 0 {
            PRODUCT_NAME.to_owned()
        } else {
            format!("{PRODUCT_NAME}[{level}]")
        },
        // The evaluator declares what a Makefile may assume of the language;
        // these two are about who runs the recipes, which is Ronin. Both are
        // real: Make mode serves the jobserver as well as consuming it, in the
        // named-pipe form GNU Make 4.4 introduced.
        extra_features: vec!["jobserver".to_owned(), "jobserver-fifo".to_owned()],
        ..Flags::default()
    };
    session.flags.targets = invocation
        .goals
        .iter()
        .map(|goal| session.intern(goal.to_vec()))
        .collect();
    session
}

/// Tell the makefile what the invocation it is being read by looks like.
///
/// The evaluator imports the process environment itself, so a value that
/// arrived there is already the makefile's, with the origin GNU Make gives it.
/// Only the top of a tree, where nothing set one, needs an answer supplied:
/// `MAKELEVEL` is zero there, and `MAKEFLAGS` is this invocation's own switches,
/// which no parent can have described.
// [spec:ronin:req:make.recursive-invocation]
fn record_invocation(
    session: &mut Session,
    name: &'static str,
    value: String,
) -> Result<(), Error> {
    if std::env::var_os(name).is_some() {
        return Ok(());
    }
    let name = session.intern(name);
    let value = Variable::with_simple_string(
        Bytes::from(value.into_bytes()),
        VarOrigin::Environment,
        None,
        None,
    );
    session
        .set_global_var(name, value, false, None)
        .map_err(|error| {
            CliError::InvocationFailed {
                exit_code: ABANDONED,
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
    options.environment.extend(flag_environment(invocation));
    Ok(options)
}

/// How many recipes at once, for the pool the evaluator declares for itself.
const fn job_count(options: &BuildOptions) -> usize {
    match options.jobs {
        JobLimit::Fixed(jobs) => jobs.get(),
        JobLimit::Auto | JobLimit::Unlimited => usize::MAX,
    }
}

/// How a sub-make is told what this invocation was asked for.
///
/// GNU Make leads `MAKEFLAGS` with the group of single-letter switches, no
/// dash, and the job count and jobserver token follow — an empty group shows as
/// the leading space children already tolerate, so this writes only the letters
/// and lets the publication append the rest. Command-line assignments go here
/// too, which is where `session_for` already reads a parent's.
///
/// `MFLAGS` is the same switches spelled as a command line rather than as a
/// bare group, and carries no assignments: GNU Make keeps those in
/// `MAKEOVERRIDES`, which `MAKEFLAGS` expands and `MFLAGS` does not.
///
/// `CARGO_MAKEFLAGS` is deliberately absent. It exists so the Rust ecosystem's
/// jobserver clients find this build's token budget, and the jobserver
/// publication is what puts the budget there. Make's switches are not a budget,
/// and writing them here would shadow an outer Cargo's auth with a value that
/// has none.
// [spec:ronin:req:make.recursive-invocation]
fn flag_environment(invocation: &Invocation) -> Vec<(&'static str, OsString)> {
    let letters: String = invocation
        .propagated()
        .iter()
        .filter_map(|switch| switch.to_str().and_then(|switch| switch.strip_prefix('-')))
        .collect();
    let mut assignments = invocation
        .variables
        .iter()
        .filter_map(|assignment| assignment.to_str().ok())
        .collect::<Vec<_>>();
    // GNU Make holds these in MAKEOVERRIDES, which is a variable table and so
    // hands them back in name order rather than in the order they were typed.
    assignments.sort_unstable();
    let mut makeflags = letters.clone();
    for withdrawn in invocation.withdrawn() {
        makeflags.push(' ');
        makeflags.push_str(withdrawn);
    }
    if !assignments.is_empty() {
        // GNU Make ends the switches at a `--` before the assignments, so that
        // one beginning with a dash cannot be read as another switch.
        makeflags.push_str(" -- ");
        makeflags.push_str(&assignments.join(" "));
    }
    let mut environment = vec![("MAKEFLAGS", OsString::from(makeflags))];
    if !letters.is_empty() {
        environment.push(("MFLAGS", OsString::from(format!("-{letters}"))));
    }
    environment
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
            exit_code: ABANDONED,
        },
        announcing,
        directory,
    )
}

/// The path this invocation arrived through, which is what `$(MAKE)` reports.
///
/// Make mode is only ever entered by name, so there is always such a path and
/// this always has an answer. It is reported as invoked rather than as
/// resolved: `current_exe` follows the symlink to a `ronin`-named binary, and
/// that name selects the other front end.
///
/// Absolute, because a sub-make runs somewhere else and a relative `./make`
/// would not survive the trip. Searched on `PATH` when the name carries no
/// directory, since that is how a symlinked `make` is normally reached, and the
/// entry chosen is the one that canonicalizes to this binary — so a different
/// `make` earlier on the path cannot capture recursion.
// [spec:ronin:req:make.recursive-invocation]
fn make_named_invocation(arguments: &[BString], executable: &Path) -> PathBuf {
    let program = arguments
        .first()
        .and_then(|program| program.to_os_str().ok())
        .map_or_else(|| PathBuf::from("make"), PathBuf::from);
    if program.components().count() > 1 {
        return std::path::absolute(&program).unwrap_or(program);
    }
    let ours = std::fs::canonicalize(executable).ok();
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|directory| directory.join(&program))
        .find(|entry| ours.is_some() && std::fs::canonicalize(entry).ok() == ours)
        // Nothing on PATH is this binary under this name, which happens when
        // the name was supplied rather than followed — `exec -a make`. The
        // invoked name is still the honest answer: it is how the parent got
        // here, and the resolved one would not re-enter Make mode at all.
        .unwrap_or(program)
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
    let mut invocation = match parse(arguments)? {
        Action::Immediate(result) => return Ok(result),
        Action::Execute(invocation) => *invocation,
    };
    // Before anything reads a switch, and `announcing` reads one immediately.
    invocation.adopt_inherited(runner.makeflags.as_deref());
    let invoked_as = make_named_invocation(arguments, &runner.executable);
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
        &invoked_as,
        runner.makeflags.as_deref(),
        level,
    );
    record_invocation_variables(&mut session, &invocation, level)?;

    let mut graph = match evaluated(session, &reported) {
        Ok(graph) => graph,
        Err(result) => return Ok(result),
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
    let (up_to_date, outcome) = match outcome {
        Ok(outcome) => outcome,
        Err(failure) => {
            return Ok(departed(
                abandoned(reported, failure),
                announcing,
                &directory,
            ))
        }
    };
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

/// What the makefile is told about the invocation reading it.
///
/// The same switches the children are told, so a Makefile that branches on
/// `$(findstring s,$(MAKEFLAGS))` is asking about this invocation and not about
/// the one that spawned it, and the depth it sits at.
// [spec:ronin:req:make.recursive-invocation]
fn record_invocation_variables(
    session: &mut Session,
    invocation: &Invocation,
    level: usize,
) -> Result<(), Error> {
    record_invocation(session, MAKELEVEL, level.to_string())?;
    for (name, value) in flag_environment(invocation) {
        record_invocation(session, name, value.to_string_lossy().into_owned())?;
    }
    Ok(())
}

/// The graph a makefile describes, or the result of not getting one.
///
/// A makefile that will not evaluate is a build abandoned, so it leaves with the
/// same status as any other. The diagnostic is passed through as the evaluator
/// wrote it, because the evaluator already put the program's name in front of
/// it; naming it again here would say it twice.
// [spec:ronin:req:make.recursive-invocation]
fn evaluated(session: Session, reported: &str) -> Result<crate::frontend::BuildGraph, RunResult> {
    crate::make::load_makefile(session).map_err(|failure| RunResult {
        stdout: terminated(reported),
        stderr: terminated(failure.to_string()),
        exit_code: ABANDONED,
    })
}

/// A build Make could not carry out — a goal with no rule is the usual one.
///
/// This exists so the failure does not leave as an error. The process boundary
/// answers an error with Ninja's status, which is one for every failure alike;
/// Make abandons with two whatever the reason was, and in Make mode the status
/// is Make's.
// [spec:ronin:req:make.recursive-invocation]
fn abandoned(reported: String, failure: Error) -> RunResult {
    RunResult {
        stdout: terminated(reported),
        stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
        exit_code: ABANDONED,
    }
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
        // Ninja reports the failing command's own status here, which is the
        // right answer for Ninja and the wrong one for Make: GNU Make has two
        // statuses, and every way of not finishing is the second. A recipe that
        // exits 3 makes Make exit 2, not 3.
        exit_code: if outcome.exit_code() == 0 {
            0
        } else {
            ABANDONED
        },
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

    /// `$(MAKE)` is one word and that word is the invoked path. GNU Make answers
    /// the same way and enough consumers exec the answer that a second word is a
    /// defect — upstream's own suite dies with ENOENT on it.
    // [spec:ronin:req:make.recursive-invocation/test]
    #[test]
    fn make_is_the_invoked_path_and_nothing_else() {
        let arguments = |program: &str| vec![BString::from(program)];
        let executable = std::path::Path::new("/opt/ronin");

        // A path is reported as invoked, absolute so it survives the sub-make's
        // change of directory, and never resolved to the ronin-named binary.
        let invoked = super::make_named_invocation(&arguments("/usr/local/bin/make"), executable);
        assert_eq!(invoked, std::path::Path::new("/usr/local/bin/make"));

        // A name nothing on PATH resolves to this binary stays the name: it is
        // how the parent arrived, and the resolved path would select Ninja.
        let invoked = super::make_named_invocation(&arguments("make"), executable);
        assert_eq!(invoked, std::path::Path::new("make"));
    }

    /// MAKEFLAGS is how a switch reaches a sub-make, now that `$(MAKE)` is a
    /// path and carries nothing. The long option is in here because its spelling
    /// contains `s`, `e`, `r` and `i`, and reading it letterwise would silence a
    /// build that asked for no such thing.
    // [spec:ronin:req:make.recursive-invocation/test]
    #[test]
    fn a_sub_make_takes_on_the_switches_its_parent_recorded() {
        let mut invocation = parsed(&["make", "all"]);
        invocation.adopt_inherited(Some("ks -- FOO=bar"));
        assert!(invocation.given(super::Switch::KeepGoing));
        assert!(invocation.given(super::Switch::Silent));

        let mut invocation = parsed(&["make", "all"]);
        invocation.adopt_inherited(Some("w -j4 --jobserver-auth=fifo:/tmp/x"));
        assert!(invocation.given(super::Switch::PrintDirectory));
        assert!(!invocation.given(super::Switch::Silent));
        assert!(!invocation.given(super::Switch::NoBuiltinRules));
    }

    /// Switches and assignments go to MAKEFLAGS, not into `$(MAKE)`. MFLAGS
    /// spells the switches as a command line and carries no assignments, which
    /// is where GNU Make puts the two apart.
    // [spec:ronin:req:make.recursive-invocation/test]
    #[test]
    fn a_sub_make_is_told_the_switches_and_the_assignments_through_makeflags() {
        let invocation = parsed(&["make", "-k", "-s", "FOO=bar", "all"]);
        let environment = super::flag_environment(&invocation);
        let value = |name: &str| {
            environment
                .iter()
                .find(|(existing, _)| *existing == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(value("MAKEFLAGS").as_deref(), Some("ks -- FOO=bar"));
        assert_eq!(value("MFLAGS").as_deref(), Some("-ks"));
        // The Rust jobserver clients read this one first, and Make's switches
        // are not a token budget. Writing it here would shadow a real one.
        assert_eq!(value("CARGO_MAKEFLAGS"), None);
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
                // An option Make does not know is a build it will not attempt,
                // and GNU Make abandons with two whatever the reason.
                assert_eq!(result.exit_code, super::ABANDONED);
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
    fn a_negating_spelling_takes_back_the_switch_it_names() {
        for (spelling, switch) in [
            ("-S", Switch::KeepGoing),
            ("--no-keep-going", Switch::KeepGoing),
            ("--stop", Switch::KeepGoing),
            ("--no-print-directory", Switch::PrintDirectory),
            ("--no-silent", Switch::Silent),
        ] {
            let invocation = parsed(&["make", spelling]);
            assert!(invocation.refused(switch), "{spelling} refused nothing");
            assert!(!invocation.given(switch), "{spelling} also asked for it");
        }

        assert!(!parsed(&["make", "-k", "-S"]).given(Switch::KeepGoing));
        assert!(parsed(&["make", "-S", "-k"]).given(Switch::KeepGoing));
        assert!(!parsed(&["make", "-w", "--no-print-directory"]).announcing());
        assert!(parsed(&["make", "--no-print-directory", "-w"]).announcing());
    }

    /// GNU Make's help: "Turn off -w, even if it was turned on implicitly".
    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn refusing_to_print_the_directory_withdraws_what_dash_c_implied() {
        assert!(parsed(&["make", "-C", "."]).announcing());
        assert!(!parsed(&["make", "-C", ".", "--no-print-directory"]).announcing());
        // Silent withdraws the implication and not the request, which is a
        // different rule and still holds.
        assert!(!parsed(&["make", "-C", ".", "-s"]).announcing());
        assert!(parsed(&["make", "-C", ".", "-s", "-w"]).announcing());
    }

    // [spec:ronin:req:make.recursive-invocation/test]
    #[test]
    fn a_sub_make_is_told_what_was_taken_back_as_well_as_what_was_asked_for() {
        let makeflags = |arguments: &[&str]| {
            let mut all = vec!["make"];
            all.extend_from_slice(arguments);
            super::flag_environment(&parsed(&all))
                .into_iter()
                .find(|(name, _)| *name == "MAKEFLAGS")
                .map(|(_, value)| value.to_string_lossy().into_owned())
                .unwrap()
        };
        // Read off GNU Make 4.4.1, once per spelling.
        assert_eq!(makeflags(&["-S"]), "S");
        assert_eq!(makeflags(&["-k", "-S"]), "S");
        assert_eq!(
            makeflags(&["--no-print-directory"]),
            " --no-print-directory"
        );
        assert_eq!(
            makeflags(&["-k", "--no-print-directory"]),
            "k --no-print-directory"
        );
        assert_eq!(makeflags(&["-k", "--no-silent", "-S"]), "S --no-silent");

        let mut adopted = parsed(&["make", "all"]);
        adopted.adopt_inherited(Some("S --no-print-directory --no-silent"));
        assert!(adopted.refused(Switch::KeepGoing));
        assert!(adopted.refused(Switch::PrintDirectory));
        assert!(adopted.refused(Switch::Silent));
        assert!(!adopted.announcing());
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
        let environment = super::flag_environment(&invocation);
        assert_eq!(
            environment
                .iter()
                .find(|(name, _)| *name == "MAKEFLAGS")
                .map(|(_, value)| value.to_string_lossy().into_owned())
                .as_deref(),
            Some("Beiqrw")
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
