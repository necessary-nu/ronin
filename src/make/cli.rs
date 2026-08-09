//! The Make front end's command line.
//!
//! GNU Make's options are accepted at this boundary. Graph-affecting inputs go
//! to kati; controls with a Ninja counterpart configure the ordinary runner;
//! the rest are interface-compatible no-ops. `-C` selects the compilation and
//! build directory, `-f` the Makefile, bare words are goals, and a word with an
//! `=` is a command-line variable.
//!
//! The executor receives only ordinary [`BuildOptions`] and the compiled graph,
//! so a Makefile and a manifest reach one scheduler rather than two modes of
//! execution.
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
use crate::frontend::{Build, BuildGraph, Persistence};
use crate::make::report::{
    abandoned, answered, discard_intermediates, finished, no_makefile, ordinary_diagnostic,
    ABANDONED,
};
use crate::make::Shuffle;
use crate::util::{terminated, BString, ByteSlice};
use crate::Error;
use kati::bytes::Bytes;
use kati::flags::Flags;
use kati::session::Session;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

mod interface;
mod subninja;
use interface::{compiler_flag_variables, makeflags_arguments, prepend_command_line_evals};
pub(super) use subninja::compile as compile_subninja;

/// The makefiles GNU Make reads when no `-f` names one, in its own order.
const DEFAULT_MAKEFILES: [&str; 3] = ["GNUmakefile", "makefile", "Makefile"];

/// How deep in a recursive Make tree this invocation is.
///
/// GNU Make counts recursion in the environment: the value that arrives there
/// is this invocation's depth, and the value handed to its own recipes is one
/// deeper.
const MAKELEVEL: &str = "MAKELEVEL";

/// The workings GNU Make's `--debug` selects, in the bits GNU Make holds them
/// in. `a` is every one of them and `n` is none.
const DB_BASIC: u16 = 0x001;
const DB_VERBOSE: u16 = 0x002;
const DB_JOBS: u16 = 0x004;
const DB_IMPLICIT: u16 = 0x008;
const DB_PRINT: u16 = 0x010;
const DB_WHY: u16 = 0x020;
const DB_MAKEFILES: u16 = 0x100;
const DB_ALL: u16 = 0xfff;

/// What accepting a GNU Make option commits the front end to doing with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OptionClass {
    /// The option changes Makefile evaluation or the graph kati produces.
    CompilerInput,
    /// The option maps onto an ordinary control of the Ninja front end.
    NinjaControl,
    /// The spelling and argument are accepted without emulating Make's runner.
    NoOp,
}

/// The argument shape GNU Make exposes for an option.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArgumentShape {
    None,
    Required,
    /// Optional only when attached: `-Oline` or `--output-sync=line`.
    OptionalAttached,
    /// Optional, with a following numeric word consumed when present.
    OptionalNumeric,
}

/// One row of the GNU Make 4.4.1 command-line surface.
struct InterfaceOption {
    spellings: &'static [&'static str],
    argument: ArgumentShape,
    class: OptionClass,
}

/// Every option GNU Make 4.4.1 exposes, plus the three transport options it
/// writes into `MAKEFLAGS` itself.
///
/// This is a classification table, not an implementation wish list. The
/// parser uses it for deliberately ignored options, while the options with a
/// compiler or Ninja meaning are handled explicitly below.
// [spec:ronin:req:make.interface-compatibility]
const MAKE_OPTION_SURFACE: &[InterfaceOption] = &[
    InterfaceOption {
        spellings: &["-b", "-m"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-B", "--always-make"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-C", "--directory"],
        argument: ArgumentShape::Required,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-d"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["--debug"],
        argument: ArgumentShape::OptionalAttached,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-e", "--environment-overrides"],
        argument: ArgumentShape::None,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-E", "--eval"],
        argument: ArgumentShape::Required,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-f", "--file", "--makefile"],
        argument: ArgumentShape::Required,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-h", "--help"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-i", "--ignore-errors"],
        argument: ArgumentShape::None,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-I", "--include-dir"],
        argument: ArgumentShape::Required,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-j", "--jobs"],
        argument: ArgumentShape::OptionalNumeric,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["--jobserver-style"],
        argument: ArgumentShape::Required,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["--jobserver-auth", "--jobserver-fds", "--sync-mutex"],
        argument: ArgumentShape::Required,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-k", "--keep-going"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-l", "--load-average", "--max-load"],
        argument: ArgumentShape::OptionalNumeric,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-L", "--check-symlink-times"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-n", "--just-print", "--dry-run", "--recon"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-o", "--old-file", "--assume-old"],
        argument: ArgumentShape::Required,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-O", "--output-sync"],
        argument: ArgumentShape::OptionalAttached,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-p", "--print-data-base"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-q", "--question"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-r", "--no-builtin-rules"],
        argument: ArgumentShape::None,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["-R", "--no-builtin-variables"],
        argument: ArgumentShape::None,
        class: OptionClass::CompilerInput,
    },
    InterfaceOption {
        spellings: &["--shuffle"],
        argument: ArgumentShape::OptionalAttached,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-s", "--silent", "--quiet", "--no-silent"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-S", "--no-keep-going", "--stop"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-t", "--touch"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["--trace"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-v", "--version"],
        argument: ArgumentShape::None,
        class: OptionClass::NinjaControl,
    },
    InterfaceOption {
        spellings: &["-w", "--print-directory", "--no-print-directory"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["-W", "--what-if", "--new-file", "--assume-new"],
        argument: ArgumentShape::Required,
        class: OptionClass::NoOp,
    },
    InterfaceOption {
        spellings: &["--warn-undefined-variables"],
        argument: ArgumentShape::None,
        class: OptionClass::NoOp,
    },
];

/// One `--debug` argument folded into what is already selected.
///
/// GNU Make reads the first letter of each comma- or space-separated word and
/// skips the rest of it, so `--debug=basic` and `--debug=b` are one thing.
/// Three letters imply the basic narration as well as their own, and `n` takes
/// back whatever came before it. `None` for a letter GNU Make does not know.
fn debug_facets(mut level: u16, spec: &[u8]) -> Option<u16> {
    for word in spec
        .split(|byte| *byte == b',' || *byte == b' ')
        .filter(|word| !word.is_empty())
    {
        level |= match word[0].to_ascii_lowercase() {
            b'a' => DB_ALL,
            b'b' => DB_BASIC,
            b'i' => DB_BASIC | DB_IMPLICIT,
            b'j' => DB_JOBS,
            b'm' => DB_BASIC | DB_MAKEFILES,
            b'n' => {
                level = 0;
                continue;
            }
            b'p' => DB_PRINT,
            b'v' => DB_BASIC | DB_VERBOSE,
            b'w' => DB_WHY,
            _ => return None,
        };
    }
    Some(level)
}

/// What one Make invocation asks for.
struct Invocation {
    /// The directories `-C` named, in order. GNU Make enters each in turn, so
    /// `-C a -C b` is `a/b` rather than `b`, which is where it differs from
    /// Ninja's single overwritten directory.
    directories: Vec<PathBuf>,
    /// Where `-I` says to look for an `include`, in the order given.
    include_dirs: Vec<PathBuf>,
    makefile: Option<PathBuf>,
    goals: Vec<BString>,
    /// `VAR=value` in command-line order, which is the order Make applies them.
    variables: Vec<Bytes>,
    /// Makefile statements supplied through `-E`/`--eval`, in source order.
    evals: Vec<Bytes>,
    /// Each `--debug` argument as it was written. Kept as words rather than as
    /// the facets they mean, because a sub-make is handed them unchanged.
    debug: Vec<BString>,
    /// What `--shuffle` settled on, already resolved to a permutation rather
    /// than left as the word that asked for one.
    shuffle: Shuffle,
    /// A `-j` written on this invocation's own command line. Kept apart from
    /// the count inherited through `MAKEFLAGS`: only this value is an explicit
    /// override of an outer jobserver, while the inherited count is a fallback
    /// for the one Ninja scheduler when no usable jobserver arrived.
    jobs: Option<JobLimit>,
    inherited_jobs: Option<JobLimit>,
    /// The load average above which no further recipe starts. `-l` with no
    /// number lifts the limit rather than imposing one, which is what a limit
    /// of zero already means to the scheduler.
    load: Option<LoadLimit>,
    /// One bit per [`Switch`] the command line gave. Every switch here is
    /// answered the same way — it was given or it was not — and a field each is
    /// what would let a spelling and a meaning drift apart.
    switches: u16,
    /// One bit per [`Switch`] the command line took back. Not the complement of
    /// `switches`: a negation must still survive into `MAKEFLAGS`, even where
    /// the switch controls no Ronin behaviour.
    negated: u16,
    /// What `-O` asked for, which is not a switch: it has four values and the
    /// one it settles on travels to a sub-make by name.
    output_sync: Option<OutputSync>,
}

impl Invocation {
    const fn new() -> Self {
        Self {
            directories: Vec::new(),
            include_dirs: Vec::new(),
            makefile: None,
            goals: Vec::new(),
            variables: Vec::new(),
            evals: Vec::new(),
            debug: Vec::new(),
            shuffle: Shuffle::None,
            jobs: None,
            inherited_jobs: None,
            load: None,
            switches: 0,
            negated: 0,
            output_sync: None,
        }
    }

    /// Each spelling clears the other, so the one written last wins.
    const fn add(&mut self, switch: Switch) {
        self.switches |= switch.bit();
        self.negated &= !switch.bit();
        // `-R` is `-r` and more: GNU Make hands a child `MAKEFLAGS=rR`.
        if matches!(switch, Switch::NoBuiltinVariables) {
            self.switches |= Switch::NoBuiltinRules.bit();
            self.negated &= !Switch::NoBuiltinRules.bit();
        }
    }

    /// Add a directory `-I` named, or forget the ones before it.
    ///
    /// GNU Make reads `-I -` as a restart rather than as a directory called
    /// `-`, which is how a makefile that wants only its own search path says so.
    fn include_dir(&mut self, named: &[u8]) -> Result<(), Error> {
        if named == b"-" {
            self.include_dirs.clear();
        } else {
            self.include_dirs.push(path_of(named)?);
        }
        Ok(())
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

    /// The job count visible to this Make compilation unit.
    const fn effective_jobs(&self) -> Option<JobLimit> {
        match self.jobs {
            Some(jobs) => Some(jobs),
            None => self.inherited_jobs,
        }
    }

    const fn set_jobs(&mut self, source: ArgumentSource, jobs: JobLimit) {
        match source {
            ArgumentSource::Inherited => self.inherited_jobs = Some(jobs),
            ArgumentSource::CommandLine => self.jobs = Some(jobs),
        }
    }

    /// Which of Make's own workings this invocation asked to be told about.
    ///
    /// GNU Make settles this after the whole command line is read rather than
    /// as it goes: `-d` is every facet and `--trace` is the two it names,
    /// whatever their position, and the `--debug` words fold in over the top.
    fn debugging(&self) -> u16 {
        let mut level = if self.given(Switch::Debug) { DB_ALL } else { 0 };
        if self.given(Switch::Trace) {
            level |= DB_PRINT | DB_WHY;
        }
        for spec in &self.debug {
            level = debug_facets(level, spec.as_bytes()).unwrap_or(level);
        }
        level
    }

    /// The switches a sub-make has to be told about again.
    ///
    /// These travel in `MAKEFLAGS`, where GNU Make puts them and where the job
    /// budget already travels — the two are spliced rather than one replacing
    /// the other. They were once appended to `$(MAKE)` instead, on the belief
    /// that `MAKEFLAGS` was the jobserver's alone; that made `$(MAKE)` several
    /// words, and a consumer that treats the answer as a path cannot exec it.
    ///
    /// In GNU Make's own switch-table order, which is the order it writes them
    /// in: the one negating letter sits at its own place in the group rather
    /// than after it, so `make -i -S -w` hands a child `MAKEFLAGS=iSw`.
    fn propagated(&self) -> Vec<OsString> {
        let mut propagated = Vec::new();
        for (switch, spelling, asserted) in [
            (Switch::AlwaysMake, "-B", true),
            (Switch::Debug, "-d", true),
            (Switch::EnvironmentOverrides, "-e", true),
            (Switch::IgnoreErrors, "-i", true),
            (Switch::KeepGoing, "-k", true),
            (Switch::DryRun, "-n", true),
            (Switch::Question, "-q", true),
            (Switch::NoBuiltinRules, "-r", true),
            (Switch::NoBuiltinVariables, "-R", true),
            (Switch::Silent, "-s", true),
            (Switch::KeepGoing, "-S", false),
            (Switch::Touch, "-t", true),
            (Switch::PrintDirectory, "-w", true),
        ] {
            let spoken = if asserted {
                self.given(switch)
            } else {
                self.refused(switch)
            };
            if spoken {
                propagated.push(OsString::from(spelling));
            }
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

    /// Whether this invocation answers a question rather than carrying out a
    /// build.
    ///
    /// Runner-only compatibility flags are accepted but do not change the
    /// Ninja question operation.
    const fn questioning(&self) -> bool {
        self.given(Switch::Question)
    }
}

/// What `-l` settled on, and whether GNU Make carries that spelling onward.
///
/// A bare `-l` lifts an inherited ceiling but disappears from `MAKEFLAGS`.
/// `-l0` has the same scheduler value and remains visible, so one `f64` cannot
/// represent both interface states.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LoadLimit {
    ceiling: f64,
    propagated: bool,
}

/// What `-O` synchronises, in GNU Make's own four names.
#[derive(Clone, Copy)]
enum OutputSync {
    None,
    Line,
    Target,
    Recurse,
}

impl OutputSync {
    /// The type named after `-O` or `--output-sync=`, where naming none at all
    /// is `target`. Only ever attached: `make -O line` is grouped output and a
    /// goal called `line`.
    fn parse(name: &[u8]) -> Result<Self, Error> {
        Ok(match name {
            b"" | b"target" => Self::Target,
            b"none" => Self::None,
            b"line" => Self::Line,
            b"recurse" => Self::Recurse,
            _ => return Err(CliError::InvalidParameter { option: "-O" }.into()),
        })
    }

    /// How a sub-make is told, which is always the resolved name: GNU Make
    /// writes `-Otarget` into `MAKEFLAGS` for a bare `-O`.
    const fn spelling(self) -> &'static str {
        match self {
            Self::None => "-Onone",
            Self::Line => "-Oline",
            Self::Target => "-Otarget",
            Self::Recurse => "-Orecurse",
        }
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
    Debug,
    DryRun,
    EnvironmentOverrides,
    IgnoreErrors,
    KeepGoing,
    NoBuiltinRules,
    NoBuiltinVariables,
    PrintDirectory,
    Question,
    Silent,
    Touch,
    Trace,
}

impl Switch {
    /// The switch one letter names, which is how a cluster like `-ki` reads.
    const fn short(letter: u8) -> Option<Self> {
        Some(match letter {
            b'B' => Self::AlwaysMake,
            b'd' => Self::Debug,
            b'e' => Self::EnvironmentOverrides,
            b'i' => Self::IgnoreErrors,
            b'k' => Self::KeepGoing,
            b'n' => Self::DryRun,
            b'q' => Self::Question,
            b'r' => Self::NoBuiltinRules,
            b'R' => Self::NoBuiltinVariables,
            b's' => Self::Silent,
            b't' => Self::Touch,
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
            b"--no-builtin-variables" => Self::NoBuiltinVariables,
            b"--print-directory" => Self::PrintDirectory,
            b"--question" => Self::Question,
            b"--silent" | b"--quiet" => Self::Silent,
            b"--touch" => Self::Touch,
            b"--trace" => Self::Trace,
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
            "  -I DIR   also look in DIR for an include, once per option\n",
            "  -W FILE  build as though FILE had just been modified\n",
            "  -j [N]   run N recipes at once, or as many as are ready\n",
            "  -l [N]   start no recipe while the load average is above N\n",
            "  -O[TYPE] hold each recipe's output: none, line, target, recurse\n",
            "  -B       rebuild every target, whatever its timestamps say\n",
            "  -e       let the environment outrank the makefile's assignments\n",
            "  -i       ignore the status of every recipe line\n",
            "  -k       keep going after a recipe fails\n",
            "  -n       print the recipes without running them\n",
            "  -q       report whether the goals are up to date, and build nothing\n",
            "  -r       do not read the built-in rules\n",
            "  -R       do not define the built-in variables, and imply -r\n",
            "  -s       do not report what is being built\n",
            "  -t       touch the targets that are out of date, running no recipe\n",
            "  -w       announce the directory the build runs in\n",
            "  -S       stop at the first failure, taking back -k\n",
            "  -d       report what Make is doing, as --debug=a does\n",
            "  --debug[=FLAGS]  report the workings FLAGS names: a b i j m n p v w\n",
            "  --trace  report why each recipe is running, as --debug=p,w does\n",
            "  --shuffle[=MODE]  build in another order: SEED, random, reverse, none\n",
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
fn load_value(
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
) -> Result<LoadLimit, Error> {
    let numeric = |argument: &&BString| {
        !argument.is_empty()
            && argument
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b'.')
    };
    let digits = if attached.is_empty() {
        let Some(next) = arguments.get(*index + 1).filter(numeric) else {
            return Ok(LoadLimit {
                ceiling: 0.0,
                propagated: false,
            });
        };
        *index += 1;
        next.clone()
    } else {
        BString::from(attached)
    };
    let ceiling = digits
        .to_str()
        .ok()
        .and_then(|digits| digits.parse::<f64>().ok())
        .ok_or(CliError::InvalidParameter { option: "-l" })?;
    Ok(LoadLimit {
        ceiling,
        propagated: true,
    })
}

/// Consume a deliberately ignored long option, including its required value.
///
/// Options with a compiler or Ninja mapping are handled explicitly. This
/// fallback is the executable meaning of [`OptionClass::NoOp`]: accepting a
/// spelling never silently turns it into Make runtime behavior.
fn accept_noop_long(
    option: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> Result<bool, Error> {
    for declared in MAKE_OPTION_SURFACE
        .iter()
        .filter(|declared| declared.class == OptionClass::NoOp)
    {
        for spelling in declared
            .spellings
            .iter()
            .filter(|spelling| spelling.starts_with("--"))
        {
            let spelling = spelling.as_bytes();
            if option == spelling {
                if declared.argument == ArgumentShape::Required {
                    let name = String::from_utf8_lossy(spelling);
                    let _ = value(arguments, index, b"", &name)?;
                }
                return Ok(true);
            }
            if option.starts_with(spelling)
                && option.get(spelling.len()) == Some(&b'=')
                && matches!(
                    declared.argument,
                    ArgumentShape::Required | ArgumentShape::OptionalAttached
                )
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Consume a deliberately ignored short option from a cluster.
fn accept_noop_short(
    option: u8,
    argument: &[u8],
    short: &mut usize,
    arguments: &[BString],
    index: &mut usize,
) -> Result<bool, Error> {
    let spelling = [b'-', option];
    let Some(declared) = MAKE_OPTION_SURFACE.iter().find(|declared| {
        declared.class == OptionClass::NoOp
            && declared
                .spellings
                .iter()
                .any(|candidate| candidate.as_bytes() == spelling.as_slice())
    }) else {
        return Ok(false);
    };
    if declared.argument == ArgumentShape::Required {
        let name = String::from_utf8_lossy(&spelling);
        let _ = value(arguments, index, &argument[*short..], &name)?;
        *short = argument.len();
    }
    Ok(true)
}

/// A long option carrying its value after an `=`, in the spellings that take
/// one at all. Says whether the option was one of them.
fn attached_long(
    invocation: &mut Invocation,
    source: ArgumentSource,
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
    } else if let Some(named) = option.strip_prefix(b"--include-dir=") {
        invocation.include_dir(named)?;
    } else if option
        .strip_prefix(b"--what-if=")
        .or_else(|| option.strip_prefix(b"--new-file="))
        .or_else(|| option.strip_prefix(b"--assume-new="))
        .is_some()
    {
    } else if let Some(eval) = option.strip_prefix(b"--eval=") {
        invocation.evals.push(Bytes::from(eval.to_vec()));
    } else if let Some(count) = option.strip_prefix(b"--jobs=") {
        invocation.set_jobs(source, jobs_value(arguments, index, count)?);
    } else if let Some(load) = option
        .strip_prefix(b"--load-average=")
        .or_else(|| option.strip_prefix(b"--max-load="))
    {
        invocation.load = Some(load_value(arguments, index, load)?);
    } else if let Some(kind) = option.strip_prefix(b"--output-sync=") {
        invocation.output_sync = Some(OutputSync::parse(kind)?);
    } else {
        return Ok(false);
    }
    Ok(true)
}

/// Which of Make's two option streams one word came from.
#[derive(Clone, Copy)]
enum ArgumentSource {
    Inherited,
    CommandLine,
}

/// Read one Make command line, over whatever a parent make put in `MAKEFLAGS`.
// [spec:ronin:req:product.make-identity]
// [spec:ronin:req:make.interface-compatibility]
fn parse(arguments: &[BString], inherited: Option<&str>) -> Result<Action, Error> {
    let mut invocation = Invocation::new();
    if let Some(inherited) = inherited {
        let inherited = makeflags_arguments(inherited);
        if let Some(action) =
            parse_arguments(&mut invocation, &inherited, ArgumentSource::Inherited)?
        {
            return Ok(action);
        }
    }
    if let Some(action) = parse_arguments(&mut invocation, arguments, ArgumentSource::CommandLine)?
    {
        return Ok(action);
    }
    // GNU Make forgets -d once the words have had their say, so a `--debug=n`
    // after it leaves nothing for a sub-make to inherit either.
    if invocation.debugging() == 0 {
        invocation.switches &= !Switch::Debug.bit();
    }
    Ok(Action::Execute(Box::new(invocation)))
}

/// Read one argv-shaped option source into an invocation.
fn parse_arguments(
    invocation: &mut Invocation,
    arguments: &[BString],
    source: ArgumentSource,
) -> Result<Option<Action>, Error> {
    let mut index = 1;
    let mut options_enabled = true;
    while index < arguments.len() {
        let argument = arguments[index].as_bytes();
        if !options_enabled || !argument.starts_with(b"-") || argument == b"-" {
            classify_word(invocation, &arguments[index]);
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
            b"--version" | b"-v" => return Ok(Some(Action::Immediate(reported(version())))),
            b"--help" => return Ok(Some(Action::Immediate(reported(usage())))),
            b"--output-sync" => invocation.output_sync = Some(OutputSync::Target),
            // GNU Make's argument is optional and its default is the basic
            // level, which is also what the group of letters is read against.
            b"--debug" => invocation.debug.push(BString::from(&b"basic"[..])),
            option if option.starts_with(b"--debug=") => {
                let spec = &option["--debug=".len()..];
                if debug_facets(0, spec).is_none() {
                    return Ok(Some(refuse(format_args!(
                        "unknown debug level specification '{}'",
                        spec.to_str_lossy()
                    ))));
                }
                invocation.debug.push(BString::from(spec));
            }
            // GNU Make's argument is optional and its default is `random`.
            option if option == b"--shuffle" || option.starts_with(b"--shuffle=") => {
                let spec = option.strip_prefix(b"--shuffle=").unwrap_or(b"random");
                let Some(mode) = Shuffle::requested(spec) else {
                    return Ok(Some(refuse(format_args!(
                        "invalid shuffle mode: Invalid value: '{}'",
                        spec.to_str_lossy()
                    ))));
                };
                invocation.shuffle = mode;
            }
            b"--jobs" => {
                invocation.set_jobs(source, jobs_value(arguments, &mut index, b"")?);
            }
            b"--load-average" | b"--max-load" => {
                invocation.load = Some(load_value(arguments, &mut index, b"")?);
            }
            b"--file" | b"--makefile" => {
                let named = value(arguments, &mut index, b"", "--file")?;
                invocation.makefile = Some(path_of(named.as_bytes())?);
            }
            b"--directory" => {
                let named = value(arguments, &mut index, b"", "--directory")?;
                invocation.directories.push(path_of(named.as_bytes())?);
            }
            b"--include-dir" => {
                let named = value(arguments, &mut index, b"", "--include-dir")?;
                invocation.include_dir(named.as_bytes())?;
            }
            b"--eval" => {
                let eval = value(arguments, &mut index, b"", "--eval")?;
                invocation.evals.push(Bytes::from(eval.to_vec()));
            }
            b"--what-if" | b"--new-file" | b"--assume-new" => {
                let _ = value(arguments, &mut index, b"", "--what-if")?;
            }
            option if option.starts_with(b"--") => {
                if !attached_long(invocation, source, option, arguments, &mut index)?
                    && !accept_noop_long(option, arguments, &mut index)?
                {
                    return Ok(Some(refuse(format_args!(
                        "unrecognized option '{}'",
                        option.to_str_lossy()
                    ))));
                }
            }
            _ => {
                if let Some(immediate) =
                    read_cluster(invocation, source, argument, arguments, &mut index)?
                {
                    return Ok(Some(immediate));
                }
            }
        }
        index += 1;
    }
    Ok(None)
}

/// One `-abc` group, which is several switches unless one of them takes an
/// argument — after which the rest of the word is that argument.
fn read_cluster(
    invocation: &mut Invocation,
    source: ArgumentSource,
    argument: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> Result<Option<Action>, Error> {
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
            b'h' => return Ok(Some(Action::Immediate(reported(usage())))),
            b'E' => {
                let eval = value(arguments, index, &argument[short..], "-E")?;
                short = argument.len();
                invocation.evals.push(Bytes::from(eval.to_vec()));
            }
            b'j' => {
                invocation.set_jobs(source, jobs_value(arguments, index, &argument[short..])?);
                short = argument.len();
            }
            b'l' => {
                invocation.load = Some(load_value(arguments, index, &argument[short..])?);
                short = argument.len();
            }
            b'W' => {
                let _ = value(arguments, index, &argument[short..], "-W")?;
                short = argument.len();
            }
            b'O' => {
                invocation.output_sync = Some(OutputSync::parse(&argument[short..])?);
                short = argument.len();
            }
            b'f' | b'C' | b'I' => {
                let named = value(
                    arguments,
                    index,
                    &argument[short..],
                    match option {
                        b'f' => "-f",
                        b'I' => "-I",
                        _ => "-C",
                    },
                )?;
                short = argument.len();
                if option == b'I' {
                    invocation.include_dir(named.as_bytes())?;
                } else if option == b'f' {
                    invocation.makefile = Some(path_of(named.as_bytes())?);
                } else {
                    invocation.directories.push(path_of(named.as_bytes())?);
                }
            }
            _ => {
                if !accept_noop_short(option, argument, &mut short, arguments, index)? {
                    return Ok(Some(refuse(format_args!(
                        "invalid option -- '{}'",
                        char::from(option)
                    ))));
                }
            }
        }
    }
    Ok(None)
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
// [spec:ronin:req:make.recursive-invocation+1]
fn session_for(
    invocation: &Invocation,
    makefile: &Path,
    jobs: usize,
    invoked_as: &Path,
) -> Session {
    let mut session = Session::new();
    let compiler_flags = compiler_flag_variables(invocation);
    session.flags = Flags {
        makefile: Some(makefile.as_os_str().to_owned()),
        num_jobs: jobs,
        num_cpus: jobs,
        is_silent_mode: invocation.given(Switch::Silent),
        // The three options whose whole effect is on evaluation rather than on
        // the build: what the makefile starts with, what outranks it, and
        // whether a recipe line's status is worth stopping for.
        no_builtin_rules: invocation.given(Switch::NoBuiltinRules),
        no_builtin_variables: invocation.given(Switch::NoBuiltinVariables),
        environment_overrides: invocation.given(Switch::EnvironmentOverrides),
        ignore_errors: invocation.given(Switch::IgnoreErrors),
        // A parent's assignments and this invocation's own, in that order,
        // which is the order Make applies them.
        cl_vars: invocation.variables.clone(),
        makeflags: Some(Bytes::from(compiler_flags.base.into_bytes())),
        make_overrides: Some(Bytes::from(compiler_flags.overrides.into_bytes())),
        // One word, and that word is a path. GNU Make answers `$(MAKE)` this
        // way and a great deal of software execs the answer rather than running
        // it through a shell — upstream's own suite adopts it as the program for
        // every later invocation. Nothing has to ride along, because the path
        // already names Make mode: that is the whole point of selecting the
        // front end by name. Switches and assignments travel in MAKEFLAGS.
        subkati_args: vec![invoked_as.as_os_str().to_owned()],
        // Compiler diagnostics retain their Makefile source, but never acquire
        // a recursive Make runner identity.
        program_name: PRODUCT_NAME.to_owned(),
        // The evaluator declares what a Makefile may assume of the interface.
        // An inherited jobserver can still bound the outer Ninja scheduler;
        // Make's output-sync feature is not advertised because `-O` is a no-op.
        extra_features: vec!["jobserver".to_owned(), "jobserver-fifo".to_owned()],
        include_dirs: invocation.include_dirs.clone(),
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
/// The evaluator imports this session's environment snapshot. Recording the
/// invocation there makes the same mechanism work for the process entry point
/// and for a semantic subninja compiled inside this process.
// [spec:ronin:req:make.recursive-invocation+1]
fn record_invocation(session: &mut Session, name: &'static str, value: String) {
    let environment = session
        .invocation_environment
        .get_or_insert_with(|| std::env::vars_os().collect());
    environment.retain(|(candidate, _)| candidate != name);
    environment.push((OsString::from(name), OsString::from(value)));
}

fn remove_invocation(session: &mut Session, name: &'static str) {
    let environment = session
        .invocation_environment
        .get_or_insert_with(|| std::env::vars_os().collect());
    environment.retain(|(candidate, _)| candidate != name);
}

/// The scheduler settings this invocation maps onto Ninja's controls.
// [spec:ronin:req:make.narration]
// [spec:ronin:req:make.jobserver+1]
fn build_options(
    invocation: &Invocation,
    runner: &Runner,
    working_directory: crate::os::WorkingDirectory,
) -> Result<BuildOptions, Error> {
    let mut options = BuildOptions {
        // Only this argv's `-j` is an explicit override. A count inherited in
        // MAKEFLAGS is the fallback below when no outer jobserver can constrain
        // the one Ninja scheduler.
        jobs: invocation.jobs.unwrap_or(JobLimit::Auto),
        // GNU Make's -k has no count: it stops when nothing is left that could
        // run, which is what an unbounded failure limit means here.
        maxfail: if invocation.given(Switch::KeepGoing) {
            usize::MAX
        } else {
            1
        },
        dryrun: invocation.given(Switch::DryRun),
        // Make's `-n` maps onto Ninja's dry run and therefore uses Ninja's
        // ordinary verbose dry-run rendering. Debug and trace are accepted
        // interface no-ops and never install a Make narrator.
        verbose: invocation.given(Switch::DryRun),
        quiet: invocation.given(Switch::Silent),
        // Make's `-l` and Ninja's are one ceiling: the scheduler starts nothing
        // further while the load average is above it, and zero is no ceiling.
        maxload: invocation.load.map_or(0.0, |load| load.ceiling),
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
        invocation
            .inherited_jobs
            .unwrap_or(JobLimit::Fixed(std::num::NonZeroUsize::MIN)),
    )?;
    // Recursive Make invocations have already compiled into this graph. Its
    // fixed limit therefore belongs directly to the Ninja scheduler; creating
    // a GNU Make token server beside it would be a second scheduling mechanism.
    // An inherited outer transport remains attached above and may bound the
    // same scheduler.
    options.serve_jobserver = false;
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
// [spec:ronin:req:make.recursive-invocation+1]
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

struct RootCompilation<'a> {
    invocation: &'a Invocation,
    makefile: &'a Path,
    invoked_as: &'a Path,
    directory: &'a Path,
    options: &'a BuildOptions,
    level: usize,
}

enum PreparedGraph {
    Ready(Box<BuildGraph>),
    Finished(RunResult),
}

/// Compile a stable graph, building source includes through Ninja between
/// attempts when the provisional graph says how to produce them.
fn prepare_graph(
    root: &RootCompilation<'_>,
    reported: &mut String,
    output: &mut Option<&mut dyn Write>,
    diagnostics: &mut Option<&mut dyn Write>,
) -> Result<PreparedGraph, Error> {
    for _ in 0..100 {
        let mut session = session_for(
            root.invocation,
            root.makefile,
            job_count(root.options),
            root.invoked_as,
        );
        record_invocation_variables(&mut session, root.invocation, root.level);
        let compilation = compilation_context(
            root.invocation,
            root.directory.to_owned(),
            job_count(root.options),
            root.level,
            &session,
        );
        let loaded = match evaluated(
            session,
            &root.invocation.evals,
            root.invocation.shuffle,
            compilation,
            reported,
        ) {
            Ok(loaded) => loaded,
            Err(result) => return Ok(PreparedGraph::Finished(result)),
        };
        if loaded.regeneration_targets().is_empty() {
            return Ok(PreparedGraph::Ready(Box::new(loaded.graph)));
        }

        let regeneration_targets = loaded.regeneration_targets().to_vec();
        let mut graph = loaded.graph;
        let (mut persistence, warning) = Persistence::open(&mut graph, root.directory)?;
        reported.push_str(warning.as_deref().unwrap_or_default());
        let mut build = Build::with_options(&mut graph, &mut persistence, root.options.clone());
        if let Some(sink) = output.as_deref_mut() {
            build = build.output(sink);
        }
        if let Some(sink) = diagnostics.as_deref_mut() {
            build = build.diagnostics(sink);
        }
        let planned = build.plan(&regeneration_targets);
        if root.invocation.questioning() {
            let question = planned.map(|planned| planned.already_up_to_date());
            let flushed = persistence.finish();
            let question = question.and_then(|up_to_date| flushed.map(|()| up_to_date));
            return Ok(PreparedGraph::Finished(answered(
                std::mem::take(reported),
                question,
            )));
        }
        let outcome = planned.and_then(|planned| {
            let disposable = planned.disposable();
            planned.run().map(|outcome| (disposable, outcome))
        });
        let flushed = persistence.finish();
        let (disposable, outcome) = match outcome {
            Ok(outcome) => outcome,
            Err(failure) => {
                return Ok(PreparedGraph::Finished(abandoned(
                    std::mem::take(reported),
                    failure,
                )));
            }
        };
        flushed?;
        discard_intermediates(&disposable, root.invocation.given(Switch::DryRun));
        if outcome.exit_code() != 0 || root.invocation.given(Switch::DryRun) {
            return Ok(PreparedGraph::Finished(finished(
                std::mem::take(reported),
                false,
                &outcome,
                root.invocation.given(Switch::Silent),
            )));
        }
        reported.push_str(&String::from_utf8_lossy(outcome.output()));
    }

    let path = BString::from(root.makefile.as_os_str().as_encoded_bytes().to_vec());
    Ok(PreparedGraph::Finished(abandoned(
        std::mem::take(reported),
        CliError::ManifestRetryLimit {
            path,
            attempts: 100,
        }
        .into(),
    )))
}

/// Run one Make invocation to its end.
// [spec:ronin:req:product.make-identity]
// [spec:ronin:req:make.recursive-invocation+1]
// [spec:ronin:req:make.narration]
pub(crate) fn run(
    runner: &Runner,
    arguments: &[BString],
    mut output: Option<&mut dyn Write>,
    mut diagnostics: Option<&mut dyn Write>,
) -> Result<RunResult, Error> {
    let invocation = match parse(arguments, runner.makeflags.as_deref())? {
        Action::Immediate(result) => return Ok(result),
        Action::Execute(invocation) => *invocation,
    };
    let invoked_as = make_named_invocation(arguments, &runner.executable);
    let mut reported = String::new();
    let directory = enter_directories(&invocation.directories)?;
    let working_directory = crate::os::WorkingDirectory::new(&directory)
        .map_err(|source| CliError::CurrentDirectory { source })?;
    let level = runner.makelevel.as_deref().unwrap_or_default();
    let level: usize = level.trim().parse().unwrap_or(0);
    let options = build_options(&invocation, runner, working_directory)?;

    let Some(makefile) = invocation
        .makefile
        .clone()
        .or_else(|| default_makefile(&directory))
    else {
        return Ok(no_makefile());
    };

    // Missing included Makefiles are source dependencies. Kati emits their
    // rules into a provisional graph; the ordinary Ninja scheduler builds
    // those roots, then the frontend recompiles from a fresh session. No Make
    // provenance or restart behavior crosses into the executor.
    let root = RootCompilation {
        invocation: &invocation,
        makefile: &makefile,
        invoked_as: &invoked_as,
        directory: &directory,
        options: &options,
        level,
    };
    let mut graph = match prepare_graph(&root, &mut reported, &mut output, &mut diagnostics)? {
        PreparedGraph::Ready(graph) => *graph,
        PreparedGraph::Finished(result) => return Ok(result),
    };
    let (mut persistence, warning) = Persistence::open(&mut graph, &directory)?;
    reported.push_str(warning.as_deref().unwrap_or_default());
    let targets = graph.default_targets();
    let mut build = Build::with_options(&mut graph, &mut persistence, options);
    if let Some(sink) = output {
        build = build.output(sink);
    }
    if let Some(sink) = diagnostics {
        build = build.diagnostics(sink);
    }
    let planned = build.plan(&targets);
    if invocation.questioning() {
        let question = planned.map(|planned| planned.already_up_to_date());
        let flushed = persistence.finish();
        let question = question.and_then(|up_to_date| flushed.map(|()| up_to_date));
        return Ok(answered(reported, question));
    }
    let outcome = planned.and_then(|planned| {
        let ending = (planned.already_up_to_date(), planned.disposable());
        planned.run().map(|outcome| (ending, outcome))
    });
    let flushed = persistence.finish();
    let ((up_to_date, disposable), outcome) = match outcome {
        Ok(outcome) => outcome,
        Err(failure) => {
            return Ok(abandoned(reported, failure));
        }
    };
    flushed?;
    let silent = invocation.given(Switch::Silent);
    discard_intermediates(&disposable, invocation.given(Switch::DryRun));
    Ok(finished(reported, up_to_date, &outcome, silent))
}

/// What the makefile is told about the invocation reading it.
///
/// The same switches the children are told, so a Makefile that branches on
/// `$(findstring s,$(MAKEFLAGS))` is asking about this invocation and not about
/// the one that spawned it, and the depth it sits at.
// [spec:ronin:req:make.recursive-invocation+1]
fn record_invocation_variables(session: &mut Session, invocation: &Invocation, level: usize) {
    record_invocation(session, MAKELEVEL, level.to_string());
    // Kati installs MAKEFLAGS as a file-origin recursive compiler variable.
    // Leaving an inherited environment binding beside it would make `-e`
    // incorrectly outrank that built-in definition.
    remove_invocation(session, "MAKEFLAGS");
    let flags = compiler_flag_variables(invocation);
    record_invocation(session, "MFLAGS", flags.mflags);
}

/// The compiler context that a recursive recipe inherits from this unit.
fn compilation_context(
    invocation: &Invocation,
    directory: PathBuf,
    jobs: usize,
    level: usize,
    session: &Session,
) -> crate::make::CompilationContext {
    let recipe_environment = vec![(
        OsString::from(MAKELEVEL),
        Some(OsString::from(level.saturating_add(1).to_string())),
    )];
    crate::make::CompilationContext {
        root_directory: directory.clone(),
        directory,
        path_prefix: PathBuf::new(),
        makeflags: propagated_makeflags(invocation),
        level,
        jobs,
        environment: session
            .invocation_environment
            .clone()
            .unwrap_or_else(|| std::env::vars_os().collect()),
        recipe_environment,
    }
}

/// The exact MAKEFLAGS value this compilation unit hands to a semantic child.
fn propagated_makeflags(invocation: &Invocation) -> String {
    compiler_flag_variables(invocation).makeflags
}

fn compilation_key(directory: &Path, makefile: &[u8], makeflags: &str) -> Vec<u8> {
    let mut key = directory.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(makefile);
    key.push(0);
    key.extend_from_slice(makeflags.as_bytes());
    key
}

/// The graph a makefile describes, or the result of not getting one.
///
/// A makefile that will not evaluate is a build abandoned, so it leaves with the
/// same status as any other. The diagnostic is passed through as the evaluator
/// wrote it, because the evaluator already put the program's name in front of
/// it; naming it again here would say it twice.
// [spec:ronin:req:make.recursive-invocation+1]
fn evaluated(
    mut session: Session,
    evals: &[Bytes],
    shuffle: Shuffle,
    context: crate::make::CompilationContext,
    reported: &str,
) -> Result<crate::make::Loaded, RunResult> {
    if let Err(failure) = prepend_command_line_evals(&mut session, evals) {
        return Err(RunResult {
            stdout: terminated(reported),
            stderr: ordinary_diagnostic(failure),
            exit_code: ABANDONED,
        });
    }
    let makefile = session
        .flags
        .makefile
        .as_ref()
        .map(|makefile| makefile.as_encoded_bytes())
        .unwrap_or_default();
    let cache_key = compilation_key(&context.directory, makefile, &context.makeflags);
    let compilation = crate::make::Compilation {
        session,
        shuffle,
        context,
        cache_key,
    };
    crate::make::load_with_subninjas(compilation, compile_subninja).map_err(|failure| RunResult {
        stdout: terminated(reported),
        stderr: ordinary_diagnostic(failure),
        exit_code: ABANDONED,
    })
}

#[cfg(test)]
mod interface_tests;

#[cfg(test)]
mod tests {
    use super::interface_tests::{parsed, parsed_under, refused};
    use super::{Invocation, OutputSync, Shuffle, Switch};
    use crate::build::JobLimit;
    use crate::util::BString;
    use std::path::PathBuf;

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn maps_make_options_to_ninja_controls() {
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
    fn parses_clusters_and_directories() {
        let invocation = parsed(&["make", "-kn", "-C", "a", "-C", "b"]);
        assert!(invocation.given(Switch::KeepGoing) && invocation.given(Switch::DryRun));
        assert_eq!(
            invocation.directories,
            vec![PathBuf::from("a"), PathBuf::from("b")]
        );
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn parses_optional_jobs_count() {
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
    // [spec:ronin:req:make.recursive-invocation+1/test]
    #[test]
    fn make_variable_is_invoked_path() {
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
    ///
    /// It is read before the command line, which is what settles the two
    /// against each other: what was typed has the last word on a switch the
    /// parent had already spoken about, in either direction, and a switch only
    /// the parent named still takes effect.
    // [spec:ronin:req:make.recursive-invocation+1/test]
    #[test]
    fn inherits_parent_switches() {
        let under = parsed_under;

        let invocation = under(Some("ks -- FOO=bar"), &["make", "all"]);
        assert!(invocation.given(Switch::KeepGoing));
        assert!(invocation.given(Switch::Silent));
        assert_eq!(
            invocation.variables,
            [kati::bytes::Bytes::from_static(b"FOO=bar")]
        );
        assert_eq!(
            super::compiler_flag_variables(&invocation).overrides,
            "FOO=bar"
        );

        let invocation = under(Some("w -j4 --jobserver-auth=fifo:/tmp/x"), &["make", "all"]);
        assert!(invocation.given(Switch::PrintDirectory));
        assert!(!invocation.given(Switch::Silent));
        assert!(!invocation.given(Switch::NoBuiltinRules));
        assert_eq!(invocation.jobs, None);
        assert_eq!(invocation.inherited_jobs, JobLimit::fixed(4));
        assert_eq!(invocation.effective_jobs(), JobLimit::fixed(4));

        let invocation = under(Some(" -j4 -l2.5"), &["make", "-j2", "-l4"]);
        assert_eq!(invocation.jobs, JobLimit::fixed(2));
        assert_eq!(invocation.inherited_jobs, JobLimit::fixed(4));
        assert_eq!(invocation.effective_jobs(), JobLimit::fixed(2));
        assert_eq!(
            super::compiler_flag_variables(&invocation).makeflags,
            " -j2 -l4"
        );

        // Asserted here, withdrawn there, and the other way about.
        assert!(under(Some("w"), &["make", "--no-print-directory"]).refused(Switch::PrintDirectory));
        assert!(under(Some("--no-print-directory"), &["make", "-w"]).given(Switch::PrintDirectory));
        assert!(under(Some("k"), &["make", "-S"]).refused(Switch::KeepGoing));
        assert!(under(Some("S"), &["make", "-k"]).given(Switch::KeepGoing));

        // Only there, only here, and both agreeing.
        assert!(under(Some("i"), &["make"]).given(Switch::IgnoreErrors));
        assert!(under(None, &["make", "-i"]).given(Switch::IgnoreErrors));
        assert!(under(Some("i"), &["make", "-i"]).given(Switch::IgnoreErrors));

        // The switches carrying a value follow the same rule: the last word on
        // one wins, and `--debug` accumulates in the order the two were read,
        // so a `--debug=n` typed here takes back what the parent asked for.
        let sync = |invocation: Invocation| invocation.output_sync.map(OutputSync::spelling);
        assert_eq!(sync(under(Some("-Oline"), &["make"])), Some("-Oline"));
        assert_eq!(
            sync(under(Some("-Oline"), &["make", "-Otarget"])),
            Some("-Otarget")
        );
        assert_eq!(
            under(Some("--shuffle=reverse"), &["make", "--shuffle=none"]).shuffle,
            Shuffle::None
        );
        assert_eq!(
            under(Some("--debug=a"), &["make", "--debug=n"]).debugging(),
            0
        );
        assert_ne!(
            under(Some("--debug=n"), &["make", "--debug=b"]).debugging(),
            0
        );
    }

    /// Switches and assignments go to MAKEFLAGS, not into `$(MAKE)`. MFLAGS
    /// spells the switches as a command line and carries no assignments, which
    /// is where GNU Make puts the two apart.
    // [spec:ronin:req:make.recursive-invocation+1/test]
    #[test]
    fn exports_makeflags_and_mflags() {
        let invocation = parsed(&[
            "make",
            "-k",
            "-s",
            "FIRST=a b",
            "SECOND=a\\b",
            "FIRST=last",
            "all",
        ]);
        let variables = super::compiler_flag_variables(&invocation);
        assert_eq!(variables.base, "ks");
        assert_eq!(variables.overrides, r"SECOND=a\\b FIRST=last");
        assert_eq!(variables.makeflags, r"ks -- SECOND=a\\b FIRST=last");
        assert_eq!(variables.mflags, "-ks");
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn accepts_short_and_long_switches() {
        for (letter, name) in [
            ("-B", "--always-make"),
            ("-e", "--environment-overrides"),
            ("-i", "--ignore-errors"),
            ("-k", "--keep-going"),
            ("-n", "--dry-run"),
            ("-q", "--question"),
            ("-r", "--no-builtin-rules"),
            ("-s", "--silent"),
            ("-t", "--touch"),
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
    fn negation_withdraws_switch() {
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
        assert!(parsed(&["make", "-w", "--no-print-directory"]).refused(Switch::PrintDirectory));
        assert!(parsed(&["make", "--no-print-directory", "-w"]).given(Switch::PrintDirectory));
    }

    // [spec:ronin:req:make.recursive-invocation+1/test]
    #[test]
    fn propagates_switch_negations() {
        let makeflags = |arguments: &[&str]| {
            let mut all = vec!["make"];
            all.extend_from_slice(arguments);
            super::compiler_flag_variables(&parsed(&all)).makeflags
        };
        // Read off GNU Make 4.4.1, once per spelling.
        assert_eq!(makeflags(&["-S"]), "S");
        assert_eq!(makeflags(&["-k", "-S"]), "S");
        // In the group at its own place, not after it.
        assert_eq!(makeflags(&["-i", "-S", "-w", "-B", "-r"]), "BirSw");
        assert_eq!(
            makeflags(&["--no-print-directory"]),
            " --no-print-directory"
        );
        assert_eq!(
            makeflags(&["-k", "--no-print-directory"]),
            "k --no-print-directory"
        );
        assert_eq!(makeflags(&["-k", "--no-silent", "-S"]), "S --no-silent");
        // `-d` is a letter in the group; the two options that carry a facet
        // follow it, before the withdrawals, which is where GNU Make's switch
        // table puts them. A `--debug` said twice is handed on once.
        // `--shuffle` is last of all, and carries the seed this run settled on
        // rather than the word that asked for one.
        assert_eq!(makeflags(&["--shuffle=reverse"]), " --shuffle=reverse");
        assert_eq!(makeflags(&["--shuffle=identity"]), " --shuffle=identity");
        assert_eq!(makeflags(&["--shuffle=12345"]), " --shuffle=12345");
        assert_eq!(makeflags(&["--shuffle=none"]), "");
        assert_eq!(
            makeflags(&["-k", "--shuffle=reverse"]),
            "k --shuffle=reverse"
        );
        assert_eq!(
            makeflags(&["--no-print-directory", "--shuffle=reverse"]),
            " --no-print-directory --shuffle=reverse"
        );
        let seeded = makeflags(&["--shuffle"]);
        assert!(
            seeded
                .strip_prefix(" --shuffle=")
                .is_some_and(|seed| seed.parse::<u32>().is_ok()),
            "{seeded}"
        );

        assert_eq!(makeflags(&["-d"]), "d");
        assert_eq!(makeflags(&["--debug"]), " --debug=basic");
        assert_eq!(
            makeflags(&["--debug=b", "--debug=j"]),
            " --debug=b --debug=j"
        );
        assert_eq!(makeflags(&["-k", "--trace"]), "k --trace");
        assert_eq!(makeflags(&["-d", "--debug=n"]), " --debug=n");
        assert_eq!(
            makeflags(&["--trace", "--no-print-directory"]),
            " --trace --no-print-directory"
        );

        let adopted = parsed_under(Some("S --no-print-directory --no-silent"), &["make", "all"]);
        assert!(adopted.refused(Switch::KeepGoing));
        assert!(adopted.refused(Switch::PrintDirectory));
        assert!(adopted.refused(Switch::Silent));

        // And the two long spellings back again, which the letterwise read
        // would otherwise lose.
        let adopted = parsed_under(
            Some("k --debug=b --trace --shuffle=12345"),
            &["make", "--debug=b"],
        );
        assert_eq!(adopted.shuffle, Shuffle::Seed(12345));
        assert_eq!(
            adopted.debugging(),
            super::DB_BASIC | super::DB_PRINT | super::DB_WHY
        );
        assert_eq!(
            super::compiler_flag_variables(&adopted).makeflags,
            "k --debug=b --trace --shuffle=12345"
        );
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn parses_extended_switch_cluster() {
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
    fn parses_optional_load_limit() {
        // A bare -l lifts the limit rather than imposing one, and the word
        // after it is still a goal.
        let lifted = parsed(&["make", "-l", "all"]);
        assert_eq!(
            lifted.load,
            Some(super::LoadLimit {
                ceiling: 0.0,
                propagated: false,
            })
        );
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
            assert_eq!(
                invocation.load,
                Some(super::LoadLimit {
                    ceiling: 2.5,
                    propagated: true,
                }),
                "{spelling:?}"
            );
            assert_eq!(invocation.goals, vec![BString::from("all")], "{spelling:?}");
        }
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn maps_load_limit_to_scheduler() {
        let directory = std::env::temp_dir();
        let runner = crate::cli::Runner::new(&directory).unwrap();
        let options = |arguments: &[&str]| {
            let working = crate::os::WorkingDirectory::new(&directory).unwrap();
            super::build_options(&parsed(arguments), &runner, working).unwrap()
        };
        assert!((options(&["make", "-l", "2.5"]).maxload - 2.5).abs() < f64::EPSILON);
        // Zero is what the scheduler reads as no ceiling, and it is what an
        // invocation that never mentioned one leaves behind.
        assert!(options(&["make"]).maxload.abs() < f64::EPSILON);
        assert!(options(&["make", "-l"]).maxload.abs() < f64::EPSILON);
    }

    /// `-j` is a limit on the one scheduler that runs the compiled graph. Even
    /// when an outer jobserver token is accepted as interface data, an explicit
    /// child limit cannot make Make mode serve another token pool.
    // [spec:ronin:req:make.jobserver+1/test]
    #[test]
    fn make_jobs_use_one_ninja_scheduler() {
        let directory = std::env::temp_dir();
        let options = |runner: &crate::cli::Runner, arguments: &[&str]| {
            let working = crate::os::WorkingDirectory::new(&directory).unwrap();
            let invocation = parsed_under(runner.makeflags.as_deref(), arguments);
            super::build_options(&invocation, runner, working).unwrap()
        };

        let root = crate::cli::Runner::new(&directory).unwrap();
        let root_options = options(&root, &["make", "-j4"]);
        assert_eq!(root_options.jobs, JobLimit::fixed(4).unwrap());
        assert!(root_options.jobserver.is_none());
        assert!(!root_options.serve_jobserver);

        let mut under_parent = crate::cli::Runner::new(&directory).unwrap();
        under_parent.makeflags = Some(" -j8".to_owned());
        let inherited_options = options(&under_parent, &["make"]);
        assert_eq!(inherited_options.jobs, JobLimit::fixed(8).unwrap());
        assert!(inherited_options.jobserver.is_none());

        under_parent.makeflags = Some(" -j8 --jobserver-auth=fifo:/tmp/parent".to_owned());
        let child_options = options(&under_parent, &["make", "-j2"]);
        assert_eq!(child_options.jobs, JobLimit::fixed(2).unwrap());
        assert!(child_options.jobserver.is_none());
        assert!(!child_options.serve_jobserver);
    }

    /// The group and its order are GNU Make 4.4.1's own, read off a makefile
    /// printing `$(MAKEFLAGS)` rather than reasoned about. `-W` is missing
    /// from it because GNU Make's option table leaves it out: a sub-make is
    /// told which switches were asked for, not which files to pretend about.
    // [spec:ronin:req:make.recursive-invocation+1/test]
    #[test]
    fn propagates_inherited_switches() {
        let invocation = parsed(&["make", "-Beikqrstw", "-W", "b.x", "all"]);
        assert_eq!(
            super::compiler_flag_variables(&invocation).makeflags,
            "Beikqrstw"
        );
    }

    /// Every spelling GNU Make gives `-W`, and every shape its argument
    /// arrives in.
    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn accepts_what_if_aliases() {
        for spelling in [
            ["-W", "b.x"].as_slice(),
            ["-Wb.x"].as_slice(),
            ["--what-if", "b.x"].as_slice(),
            ["--what-if=b.x"].as_slice(),
            ["--new-file=b.x"].as_slice(),
            ["--assume-new=b.x"].as_slice(),
        ] {
            let mut arguments = vec!["make"];
            arguments.extend_from_slice(spelling);
            arguments.push("all");
            let invocation = parsed(&arguments);
            assert_eq!(invocation.goals, vec![BString::from("all")], "{spelling:?}");
        }
    }

    /// Accepted runner no-ops do not change Ninja's question operation.
    // [spec:ronin:req:make.question-status/test]
    #[test]
    fn touch_does_not_override_question_mode() {
        assert!(parsed(&["make", "-q"]).questioning());
        assert!(parsed(&["make", "-q", "-t"]).questioning());
    }

    /// Read off GNU Make 4.4.1's own `decode_debug_flags`: only the first
    /// letter of each word is looked at, three letters imply the basic level as
    /// well as their own, and `n` takes back whatever preceded it.
    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn parses_debug_facets() {
        let level = |arguments: &[&str]| {
            let mut all = vec!["make"];
            all.extend_from_slice(arguments);
            parsed(&all).debugging()
        };
        assert_eq!(level(&["-d"]), super::DB_ALL);
        assert_eq!(level(&["--debug=a"]), super::DB_ALL);
        assert_eq!(level(&["--trace"]), super::DB_PRINT | super::DB_WHY);
        assert_eq!(level(&["--debug=print,why"]), level(&["--trace"]));
        assert_eq!(level(&["--debug"]), super::DB_BASIC);
        assert_eq!(level(&["--debug=basic"]), level(&["--debug=b"]));
        assert_eq!(
            level(&["--debug=i"]),
            super::DB_BASIC | super::DB_IMPLICIT,
            "an implicit-rule account carries the basic one"
        );
        assert_eq!(level(&["--debug=n"]), 0);
        assert_eq!(level(&["-d", "--debug=n"]), 0);
        assert_eq!(level(&["--debug=n", "-d"]), 0);
        assert_eq!(
            level(&["--debug=b", "--debug=j"]),
            super::DB_BASIC | super::DB_JOBS
        );

        let diagnostic = refused(&["make", "--debug=x"]).expect("a level Make does not have");
        assert!(
            diagnostic.starts_with("ronin: unknown debug level specification 'x'"),
            "{diagnostic}"
        );
    }
}
