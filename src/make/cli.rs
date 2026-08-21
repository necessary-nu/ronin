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

use crate::Error;
use crate::build::{BuildOptions, JobLimit};
use crate::cli::{PRODUCT_NAME, RunResult, Runner};
use crate::error::CliError;
use crate::frontend::{Build, BuildGraph, Persistence};
use crate::make::Shuffle;
use crate::make::report::{
    ABANDONED, abandoned, answered, discard_intermediates, duplicate_standard_input, finished,
    no_makefile, ordinary_diagnostic,
};
use crate::util::{BString, ByteSlice, terminated};
use kati::bytes::Bytes;
use kati::flags::Flags;
use kati::session::Session;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod diagnostics;
mod interface;
mod jobserver_style;
mod option_values;
mod remake;
mod selection;
mod subninja;
mod switch_table;
use diagnostics::{emit_raised, led_by_raised};
use interface::{
    ArgumentSource, compiler_flag_variables, decode_makefile_makeflags, evaluated_build_options,
    evaluated_invocation, makeflags_arguments, prepend_command_line_evals, read_shuffle,
};
use jobserver_style::{carried_switches, read_jobserver_style, unknown_jobserver_style};
use option_values::{jobs_value, load_value, value};
use remake::{CompilerInputBuild, Settlement, build_compiler_inputs};
use selection::{DEFAULT_MAKEFILES, STANDARD_INPUT, is_standard_input, named_makefiles};
pub(super) use subninja::compile as compile_subninja;
use switch_table::path_of;

/// How deep in a recursive Make tree this invocation is.
///
/// GNU Make counts recursion in the environment: the value that arrives there
/// is this invocation's depth, and the value handed to its own recipes is one
/// deeper.
const MAKELEVEL: &str = "MAKELEVEL";

/// The environment's second option stream.
///
/// GNU Make decodes it exactly as it decodes `MAKEFLAGS`, immediately before
/// it, and then empties the name so a child does not read the same switches a
/// second time — `decode_env_switches (STRING_SIZE_TUPLE (GNUMAKEFLAGS_NAME),
/// o_command)` followed by `define_variable_cname (GNUMAKEFLAGS_NAME, "",
/// o_env, 0)` in `main` (main.c). Everything the switches asked for is carried
/// onward by `MAKEFLAGS`, which is where they end up.
///
/// The name is a distribution's and a user's place to say what every Make in a
/// tree should do without writing it into `MAKEFLAGS`, where a Makefile
/// reading `$(MAKEFLAGS)` would then be told about it before Make has had a
/// chance to fold it in.
pub(crate) const GNUMAKEFLAGS: &str = "GNUMAKEFLAGS";

/// How many times the read has started over to pick up a remade Makefile.
///
/// Absent on the first read and `1` after one restart, which is what a Makefile
/// branching on it is asking about.
const MAKE_RESTARTS: &str = "MAKE_RESTARTS";

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
        class: OptionClass::NinjaControl,
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
        class: OptionClass::NinjaControl,
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
        class: OptionClass::NinjaControl,
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
    /// The Makefiles `-f` named, in the order it named them. GNU Make reads
    /// every one of them as though they had been concatenated, so this is a
    /// list and the order is the semantics: the default goal comes from the
    /// first file that declares an eligible target, not the last.
    makefiles: Vec<PathBuf>,
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
    ///
    /// Settled once, from the streams GNU Make has read by the time it reaches
    /// `main`'s shuffle block, and never again: a makefile's own write to
    /// `MAKEFLAGS` is decoded long after that point and so reorders nothing.
    shuffle: Shuffle,
    /// The word `MAKEFLAGS` republishes for `--shuffle`, which is GNU Make's
    /// switch table entry rather than the mode this run is using.
    ///
    /// The two part company at every origin that is not the command line. The
    /// table holds whatever was last decoded into it, unexamined, so a
    /// makefile's `MAKEFLAGS += --shuffle=random` publishes `random` where the
    /// command line's would have published the seed it settled on — and a
    /// value naming no mode at all is published exactly as written, because
    /// nothing ever looked at it.
    shuffle_spelling: Option<String>,
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
    /// The last `--jobserver-style` written, kept only to be checked.
    ///
    /// `make-single-ninja-scheduler` settled that one Ninja scheduler replaces
    /// the recursive jobserver, so the style selects nothing here. It is still
    /// read, because an argument GNU Make would have rejected must not be taken
    /// for one it would have accepted: a makefile guarding on the refusal would
    /// otherwise take the wrong branch.
    jobserver_style: Option<BString>,
    /// The files `-W` named, canonicalised, in the order they were given.
    ///
    /// GNU Make's `new_files`, which `main` stamps `NEW_MTIME` on rather than
    /// touching: the file reads as present and newer than everything
    /// downstream of it, and its own rule sees nothing to do.
    assumed_new: Vec<BString>,
    /// The first switch this stream could not read, kept until the whole word
    /// has been consumed.
    ///
    /// GNU Make's `decode_switches` (main.c) sets a `bad` flag rather than
    /// dying where it stands, so that the argument is still consumed and the
    /// parse stays in step. Only the command line and the environment's
    /// `MAKEFLAGS` — which GNU Make decodes at `o_command` as well — answer with
    /// the usage message afterwards; a makefile's own write loses the switch and
    /// the read goes on.
    bad: Option<String>,
    /// What this stream had to say about a word it dropped rather than died
    /// of.
    ///
    /// GNU Make's `decode_switches` prints its own complaint about an empty
    /// string argument or a job count that is not a positive integer whatever
    /// origin the word came from, and only the dying afterwards is the command
    /// line's alone. A stream that refuses answers with the first of these
    /// through `bad`; a stream that forgives carries them out to whoever
    /// decoded it.
    complaints: Vec<String>,
}

impl Invocation {
    const fn new() -> Self {
        Self {
            directories: Vec::new(),
            include_dirs: Vec::new(),
            makefiles: Vec::new(),
            goals: Vec::new(),
            variables: Vec::new(),
            evals: Vec::new(),
            debug: Vec::new(),
            shuffle: Shuffle::None,
            shuffle_spelling: None,
            jobs: None,
            inherited_jobs: None,
            load: None,
            switches: 0,
            negated: 0,
            output_sync: None,
            jobserver_style: None,
            assumed_new: Vec::new(),
            bad: None,
            complaints: Vec::new(),
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
            ArgumentSource::Inherited | ArgumentSource::Makefile | ArgumentSource::Protection => {
                self.inherited_jobs = Some(jobs);
            }
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
    fn parse(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"" | b"target" => Self::Target,
            b"none" => Self::None,
            b"line" => Self::Line,
            b"recurse" => Self::Recurse,
            _ => return None,
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

/// Record `-O`'s type, or say why it is not one.
///
/// GNU Make stores the word where it reads it and judges it afterwards, in
/// `decode_output_sync_flags` — which runs once every option stream has been
/// read and calls `fatal`. So a type Make does not have ends the run whichever
/// stream wrote it, unlike the switches the origin forgives.
fn read_output_sync(invocation: &mut Invocation, kind: &[u8]) {
    if let Some(output_sync) = OutputSync::parse(kind) {
        invocation.output_sync = Some(output_sync);
        return;
    }
    invocation.undecodable(format!("invalid argument to -O: '{}'", kind.to_str_lossy()));
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

/// Consume a deliberately ignored long option, including its required value.
///
/// Options with a compiler or Ninja mapping are handled explicitly. This
/// fallback is the executable meaning of [`OptionClass::NoOp`]: accepting a
/// spelling never silently turns it into Make runtime behavior.
fn accept_noop_long(
    invocation: &mut Invocation,
    source: ArgumentSource,
    option: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> bool {
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
            // GNU Make names the switch rather than the spelling that reached
            // it, and the switch's name is its short letter wherever it has
            // one: `--old-file=` complains about `-o`.
            let name = declared
                .spellings
                .first()
                .filter(|first| !first.starts_with("--"))
                .map_or_else(
                    || String::from_utf8_lossy(spelling),
                    |first| (*first).into(),
                );
            if option == spelling {
                // The spelling this word wrote, because getopt names the
                // option it was given: `--old-file` is not reported as `-o`,
                // where the empty-argument complaint below names the switch.
                if declared.argument == ArgumentShape::Required
                    && let Some(named) = value(
                        invocation,
                        source,
                        arguments,
                        index,
                        b"",
                        &String::from_utf8_lossy(spelling),
                    )
                {
                    invocation.discarded_argument(source, &name, named.as_bytes());
                }
                return true;
            }
            if option.starts_with(spelling)
                && option.get(spelling.len()) == Some(&b'=')
                && matches!(
                    declared.argument,
                    ArgumentShape::Required | ArgumentShape::OptionalAttached
                )
            {
                if declared.argument == ArgumentShape::Required {
                    invocation.discarded_argument(source, &name, &option[spelling.len() + 1..]);
                }
                return true;
            }
        }
    }
    false
}

/// Consume a deliberately ignored short option from a cluster.
fn accept_noop_short(
    invocation: &mut Invocation,
    source: ArgumentSource,
    option: u8,
    argument: &[u8],
    short: &mut usize,
    arguments: &[BString],
    index: &mut usize,
) -> bool {
    let spelling = [b'-', option];
    let Some(declared) = MAKE_OPTION_SURFACE.iter().find(|declared| {
        declared.class == OptionClass::NoOp
            && declared
                .spellings
                .iter()
                .any(|candidate| candidate.as_bytes() == spelling.as_slice())
    }) else {
        return false;
    };
    if declared.argument == ArgumentShape::Required {
        let name = String::from_utf8_lossy(&spelling);
        if let Some(named) = value(
            invocation,
            source,
            arguments,
            index,
            &argument[*short..],
            &name,
        ) {
            invocation.discarded_argument(source, &name, named.as_bytes());
        }
        *short = argument.len();
    }
    true
}

/// A long option whose value is the word after it, in the spellings that take
/// one that way.
///
/// The option and its value are two words, so a stream that ends on the option
/// has no value to give it — which is getopt's missing-argument case, recorded
/// against the invocation rather than raised, and the switch is then dropped.
fn separated_long(
    invocation: &mut Invocation,
    source: ArgumentSource,
    option: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> Result<(), Error> {
    let spelling = String::from_utf8_lossy(option);
    let Some(named) = value(invocation, source, arguments, index, b"", &spelling) else {
        return Ok(());
    };
    match option {
        b"--file" | b"--makefile" => invocation.makefile(source, named.as_bytes())?,
        b"--directory" => invocation.directory(source, named.as_bytes())?,
        b"--include-dir" => invocation.include_dir(source, named.as_bytes())?,
        b"--eval" => invocation.eval_statement(source, named.as_bytes()),
        _ => invocation.assume_new(source, named.as_bytes()),
    }
    Ok(())
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
        invocation.makefile(source, named)?;
    } else if let Some(named) = option.strip_prefix(b"--directory=") {
        invocation.directory(source, named)?;
    } else if let Some(named) = option.strip_prefix(b"--include-dir=") {
        invocation.include_dir(source, named)?;
    } else if let Some(named) = option
        .strip_prefix(b"--what-if=")
        .or_else(|| option.strip_prefix(b"--new-file="))
        .or_else(|| option.strip_prefix(b"--assume-new="))
    {
        invocation.assume_new(source, named);
    } else if let Some(eval) = option.strip_prefix(b"--eval=") {
        invocation.eval_statement(source, eval);
    } else if let Some(count) = option.strip_prefix(b"--jobs=") {
        // Written with an `=`, so the argument is whatever follows it and no
        // later word can stand in: `--jobs=` is an argument GNU Make has and
        // cannot read, where a bare `-j` is one it never had.
        if count.is_empty() {
            invocation.complain(
                source,
                "the '-j' option requires a positive integer argument".to_owned(),
            );
        } else if let Some(jobs) = jobs_value(invocation, source, arguments, index, count) {
            invocation.set_jobs(source, jobs);
        }
    } else if let Some(load) = option
        .strip_prefix(b"--load-average=")
        .or_else(|| option.strip_prefix(b"--max-load="))
    {
        invocation.load = Some(load_value(arguments, index, load));
    } else if let Some(kind) = option.strip_prefix(b"--output-sync=") {
        if invocation.non_empty(source, "-O", kind) {
            read_output_sync(invocation, kind);
        }
    } else {
        return Ok(false);
    }
    Ok(true)
}

/// Read one Make command line, over whatever a parent make put in the
/// environment's two option streams.
///
/// `GNUMAKEFLAGS` is read first and `MAKEFLAGS` second, which is the order GNU
/// Make's `main` calls `decode_env_switches` in and the order that decides
/// which spelling of a switch taking an argument is the one left standing.
/// Both are decoded at the same origin — `o_command`, so a word either of them
/// got wrong ends the run — and the words the two streams contribute are
/// indistinguishable afterwards: everything published is published as
/// `MAKEFLAGS`.
// [spec:ronin:req:product.make-identity]
// [spec:ronin:req:make.interface-compatibility]
fn parse(
    arguments: &[BString],
    inherited: Option<&str>,
    gnumakeflags: Option<&str>,
) -> Result<Action, Error> {
    let mut invocation = Invocation::new();
    for stream in [gnumakeflags, inherited] {
        let Some(stream) = stream else {
            continue;
        };
        let stream = makeflags_arguments(stream);
        if let Some(action) = parse_arguments(&mut invocation, &stream, ArgumentSource::Inherited)?
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
    if let Some(message) = unknown_jobserver_style(&invocation) {
        // Stated on its own rather than through `refuse`: this is not a
        // malformed option, so GNU prints no usage after it.
        return Ok(Action::Immediate(RunResult {
            stdout: Vec::new(),
            stderr: format!("{}\n", crate::util::diagnostic(PRODUCT_NAME, message)).into_bytes(),
            exit_code: ABANDONED,
        }));
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
            b"--debug" => invocation.debug_spec(source, b"basic"),
            option if option.starts_with(b"--debug=") => {
                let spec = &option["--debug=".len()..];
                if !spec.is_empty() && debug_facets(0, spec).is_none() {
                    return Ok(Some(refuse(format_args!(
                        "unknown debug level specification '{}'",
                        spec.to_str_lossy()
                    ))));
                }
                invocation.debug_spec(source, spec);
            }
            // GNU Make's argument is optional and its default is `random`.
            option if option == b"--shuffle" || option.starts_with(b"--shuffle=") => {
                let spec = option.strip_prefix(b"--shuffle=").unwrap_or(b"random");
                if let Some(action) = read_shuffle(invocation, source, spec) {
                    return Ok(Some(action));
                }
            }
            b"--jobs" => {
                if let Some(jobs) = jobs_value(invocation, source, arguments, &mut index, b"") {
                    invocation.set_jobs(source, jobs);
                }
            }
            option
                if option == b"--jobserver-style" || option.starts_with(b"--jobserver-style=") =>
            {
                read_jobserver_style(invocation, source, option, arguments, &mut index);
            }
            b"--load-average" | b"--max-load" => {
                invocation.load = Some(load_value(arguments, &mut index, b""));
            }
            b"--file" | b"--makefile" | b"--directory" | b"--include-dir" | b"--eval"
            | b"--what-if" | b"--new-file" | b"--assume-new" => {
                separated_long(invocation, source, argument, arguments, &mut index)?;
            }
            option if option.starts_with(b"--") => {
                if let Some(action) =
                    read_other_long(invocation, source, option, arguments, &mut index)?
                {
                    return Ok(Some(action));
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
        // Read here rather than where it is set, because GNU Make consumes the
        // whole switch before it gives up on it: the argument of an `-I ''` is
        // still the argument, and a parse that abandoned the word where the
        // emptiness was noticed would read the next one as a goal.
        if let Some(message) = invocation.bad.take() {
            return Ok(Some(refuse(message)));
        }
        index += 1;
    }
    Ok(None)
}

/// A long option none of the spellings above claimed: one carrying its value
/// after an `=`, one Make accepts and does nothing with, or one it does not
/// know at all.
fn read_other_long(
    invocation: &mut Invocation,
    source: ArgumentSource,
    option: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> Result<Option<Action>, Error> {
    if attached_long(invocation, source, option, arguments, index)?
        || accept_noop_long(invocation, source, option, arguments, index)
        || !source.refuses_a_bad_switch()
    {
        return Ok(None);
    }
    Ok(Some(refuse(format_args!(
        "unrecognized option '{}'",
        option.to_str_lossy()
    ))))
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
                let eval = value(
                    invocation,
                    source,
                    arguments,
                    index,
                    &argument[short..],
                    "-E",
                );
                short = argument.len();
                if let Some(eval) = eval {
                    invocation.eval_statement(source, eval.as_bytes());
                }
            }
            b'j' => {
                if let Some(jobs) =
                    jobs_value(invocation, source, arguments, index, &argument[short..])
                {
                    invocation.set_jobs(source, jobs);
                }
                short = argument.len();
            }
            b'l' => {
                invocation.load = Some(load_value(arguments, index, &argument[short..]));
                short = argument.len();
            }
            b'W' => {
                let named = value(
                    invocation,
                    source,
                    arguments,
                    index,
                    &argument[short..],
                    "-W",
                );
                short = argument.len();
                if let Some(named) = named {
                    invocation.assume_new(source, named.as_bytes());
                }
            }
            b'O' => {
                read_output_sync(invocation, &argument[short..]);
                short = argument.len();
            }
            b'f' | b'C' | b'I' => {
                let named = value(
                    invocation,
                    source,
                    arguments,
                    index,
                    &argument[short..],
                    match option {
                        b'f' => "-f",
                        b'I' => "-I",
                        _ => "-C",
                    },
                );
                short = argument.len();
                let Some(named) = named else { continue };
                if option == b'I' {
                    invocation.include_dir(source, named.as_bytes())?;
                } else if option == b'f' {
                    invocation.makefile(source, named.as_bytes())?;
                } else {
                    invocation.directory(source, named.as_bytes())?;
                }
            }
            _ => {
                if !accept_noop_short(
                    invocation, source, option, argument, &mut short, arguments, index,
                ) && source.refuses_a_bad_switch()
                {
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

/// A word that is not an option: a variable assignment, or a goal.
///
/// Make's own test, and the vendored evaluator's: a word with an `=` anywhere
/// in it assigns, and nothing else does. A command-line assignment is not an
/// environment variable — it outranks the makefile's own, which is why it
/// travels as an assignment rather than as an exported value.
///
/// Two words are neither, and GNU Make's `handle_non_switch_argument` (main.c)
/// passes over both. A zero-length word fails the goal branch's own
/// `arg[0] != '\0'` guard, so `make ""` builds the default goal and leaves
/// `MAKECMDGOALS` empty — the shape a wrapper hands Make whenever it expands
/// `make "$TARGET"` with `TARGET` unset. A lone `-` is returned on before
/// anything else, under a comment reading `Ignore plain '-' for
/// compatibility.`. Reading either as a path instead abandons a build GNU Make
/// runs.
///
/// This is the right place for the dash and the only one: a bare `-` means a
/// directory to `-I`, which forgets the ones accumulated before it, so the
/// clause has to sit where a word has already been ruled not to be a switch
/// and not to be a switch's argument.
fn classify_word(invocation: &mut Invocation, word: &BString) {
    if word.is_empty() || word == "-" {
        return;
    }
    if word.contains(&b'=') {
        invocation.variables.push(Bytes::from(word.to_vec()));
    } else {
        invocation.goals.push(word.clone());
    }
}

/// The evaluation session one Make invocation describes.
// [spec:ronin:req:make.recursive-invocation+2]
fn session_for(
    invocation: &Invocation,
    makefiles: &[PathBuf],
    jobs: usize,
    invoked_as: &Path,
    diagnostics: &Arc<kati::diagnostics::Diagnostics>,
    census: &Arc<kati::census::Census>,
) -> Session {
    let mut session = Session::new();
    // Every session of one invocation writes what it has to say to the same
    // descriptor, which is the invocation's rather than the process's: a
    // warning raised while a Makefile is read is part of what the compilation
    // answered, and a caller that collects a run's output has to be able to see
    // it. See [`crate::make::cli::run`], which drains it.
    session.diagnostics = Arc::clone(diagnostics);
    // And into the same ledger, for the same reason: a recursive `$(MAKE)`
    // composed into this graph is classified by a session of its own, and what
    // it classified belongs to the invocation that asked.
    session.census = Arc::clone(census);
    let compiler_flags = compiler_flag_variables(invocation);
    let carried = Bytes::from(carried_switches(&compiler_flags.base, invocation).into_bytes());
    // The switch table alone. What `MAKEFLAGS` reads back is this plus the two
    // references it names, which the evaluator assembles: GNU Make's
    // `define_makeflags` writes the fragments and the assignments as
    // `$(-*-eval-flags-*-)` and `$(MAKEOVERRIDES)` rather than inline.
    let makeflags = Bytes::from(compiler_flags.base.into_bytes());
    let eval_flags = Bytes::from(compiler_flags.eval_flags.into_bytes());
    let has_evals = !eval_flags.is_empty();
    let make_overrides = Bytes::from(compiler_flags.overrides.into_bytes());
    session.flags = Flags {
        makefiles: makefiles
            .iter()
            .map(|makefile| makefile.as_os_str().to_owned())
            .collect(),
        num_jobs: jobs,
        num_cpus: jobs,
        // The two the compiler reads for narration: under `-s` nothing is
        // narrated and under `-n` nothing is run, so in both the command line
        // is the whole of what the build shows and a recipe's own echo stays
        // inside it rather than becoming an edge's description.
        is_silent_mode: invocation.given(Switch::Silent),
        is_dry_run: invocation.given(Switch::DryRun),
        // The three options whose whole effect is on evaluation rather than on
        // the build: what the makefile starts with, what outranks it, and
        // whether a recipe line's status is worth stopping for.
        no_builtin_rules: invocation.given(Switch::NoBuiltinRules),
        no_builtin_variables: invocation.given(Switch::NoBuiltinVariables),
        environment_overrides: invocation.given(Switch::EnvironmentOverrides),
        ignore_errors: invocation.given(Switch::IgnoreErrors),
        // A fourth, and its effect on evaluation is one thing only: whether the
        // first required makefile nothing can make is the last one the update
        // considers. `complain()` chooses `error` over `fatal` on it
        // (remake.c:422), so the update walks on and refuses over every one of
        // them rather than dying inside the first.
        keep_going: invocation.given(Switch::KeepGoing),
        // A parent's assignments and this invocation's own, in that order,
        // which is the order Make applies them.
        cl_vars: invocation.variables.clone(),
        makeflags: Some(makeflags),
        eval_flags,
        make_overrides: Some(make_overrides.clone()),
        makeflags_assignment: Some(kati::flags::MakeflagsAssignment {
            decoder: decode_makefile_makeflags,
            protected: carried.clone(),
            effective: carried,
            has_overrides: !make_overrides.is_empty(),
            has_evals,
        }),
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
        // An inherited jobserver can still bound the outer Ninja scheduler.
        // Ninja execution publishes a command edge's captured output as one
        // unit, which is target-style output synchronization even though
        // Make's `-O` selector does not install a second reporting path.
        extra_features: vec![
            "archives".to_owned(),
            "jobserver".to_owned(),
            "jobserver-fifo".to_owned(),
            "output-sync".to_owned(),
        ],
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
// [spec:ronin:req:make.recursive-invocation+2]
fn record_invocation(session: &mut Session, name: &'static str, value: String) {
    let environment = session
        .invocation_environment
        .get_or_insert_with(|| std::env::vars_os().collect());
    environment.retain(|(candidate, _)| candidate != name);
    environment.push((OsString::from(name), OsString::from(value)));
}

/// The scheduler settings this invocation maps onto Ninja's controls.
// [spec:ronin:req:make.narration+1]
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
        assumed_new: invocation.assumed_new.clone(),
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
    // 130 is Ninja's `ExitInterrupted` and nothing at all to Make: GNU Make
    // reports a recipe that exits 130 as `Error 130` and goes on under -k
    // exactly as it would for any other status.
    options.command_status_interrupts = false;
    // GNU Make reaps a recipe its own signal killed and reports it like any
    // other failure — `*** [Makefile:2: out] Terminated`, and on under -k. Only
    // a signal delivered to Make itself ends the build.
    options.recipe_signal_fails = true;
    // `lib.a(member.o)` is a target GNU Make reads as a member of an archive,
    // and its timestamp comes out of the archive's index rather than off a
    // file of that name. A manifest build has no such shape and must keep
    // reading parentheses as ordinary bytes in a path.
    options.archive_members = true;
    // `-t` brings the goals up to date without making them. It is not a
    // narration switch: the same edges are planned, reported and counted, and
    // only what an edge does changes — a fresh date on each output in place of
    // the recipe. `-n` keeps its precedence over it, as it does over everything
    // that would write, and `-q` never reaches here because a question runs
    // nothing at all.
    options.touch = invocation.given(Switch::Touch);
    // `-B` decides what the run writes to disk rather than how it is narrated,
    // so it belongs here beside `-t` and not among the interface no-ops. Every
    // edge with a recipe is out of date and every prerequisite is one of the
    // new inputs, which is the whole of what GNU Make's `always_make_flag`
    // does. The makefile-remaking pass turns it off again after a restart —
    // see `remake_makefiles` — and that is the one place the two disagree.
    options.always_make = invocation.given(Switch::AlwaysMake);
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
// [spec:ronin:req:make.recursive-invocation+2]
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

/// Whether a compilation is being run to make the build or to report on it,
/// and where the report goes.
///
/// The two travel together because they are one decision. A census is gathered
/// only for a report, and only a report survives a composition whose child
/// directory holds no makefile — a build refuses there, because the child graph
/// does not exist and the recipe line that would have started a Make of its own
/// was lifted out of the recipe.
struct Purpose<'a> {
    /// Where every session this compilation opens records what it classified
    /// about a recursive invocation.
    census: &'a Arc<kati::census::Census>,
    /// Whether a report is what was asked for.
    reporting: bool,
}

struct RootCompilation<'a> {
    invocation: &'a Invocation,
    /// Where every session this compilation opens writes its warnings.
    diagnostics: &'a Arc<kati::diagnostics::Diagnostics>,
    /// Where every one of them records what it classified, for a caller that
    /// asked for a census rather than a build.
    census: &'a Arc<kati::census::Census>,
    /// Every Makefile this invocation reads, in order.
    makefiles: &'a [PathBuf],
    invoked_as: &'a Path,
    directory: &'a Path,
    options: &'a BuildOptions,
    /// What standard input held, when one of `makefiles` is `-`.
    makefile_contents: Option<&'a [u8]>,
    level: usize,
    /// Whether this compilation is a report rather than a build. See
    /// [`crate::make::CompilationContext::reporting`].
    reporting: bool,
}

enum PreparedGraph {
    Ready {
        graph: Box<BuildGraph>,
        /// The recipes the graph left for the build to expand as it runs
        /// them, with the evaluation session they belong to.
        recipes: Option<Box<crate::make::recipe::PendingRecipes>>,
        /// The logs already opened over that graph, when the compiler-input
        /// build opened them. Ninja's logs are opened once per graph, so the
        /// pass that settled hands its own on rather than leaving the goal
        /// build to open a second set over the same nodes.
        persistence: Option<Persistence>,
        invocation: Box<Invocation>,
        options: Box<BuildOptions>,
    },
    Finished(RunResult),
}

/// Compile a stable graph, building source includes through Ninja between
/// attempts when the provisional graph says how to produce them.
///
/// GNU Make reads the Makefiles, brings every one of them up to date the way it
/// would any other target, and starts over from the beginning if that changed
/// one of them — counting each such start in `MAKE_RESTARTS`. This is that
/// loop: each pass evaluates from a fresh session, builds what the provisional
/// graph knows how to build, and goes around only when the build left the
/// read's own inputs different from how it found them.
///
/// It also goes around for a reason GNU Make does not have, which is why only
/// one of the two kinds is counted. A `$(MAKE)` recipe's child cannot be
/// compiled until the files the parent would have made before running it are
/// on the ground, so the read ends and starts again with that boundary
/// settled. The text it reads is the same text; nothing it consulted moved.
/// Counting that in `MAKE_RESTARTS` would show a Makefile a restart that, in
/// the terms the variable is defined in, did not happen.
// [spec:ronin:req:make.semantics+1]
fn prepare_graph(
    root: &RootCompilation<'_>,
    reported: &mut String,
    output: &mut Option<&mut dyn Write>,
    diagnostics: &mut Option<&mut dyn Write>,
    held: &mut Vec<u8>,
) -> Result<PreparedGraph, Error> {
    // What the passes before each one settled: the compiler-input boundaries
    // whose staged work is on the ground, the recursive recipes an earlier pass
    // carried to their end, and which units a pass is repeating rather than
    // performing.
    //
    // A recipe cut into segments has to stop being called begun once it is
    // over. One that keeps saying it leaves its target dirty for the whole
    // invocation, so a Makefile made FROM that target is remade on every pass
    // and starts the read over for ever — where GNU Make restarts once, its
    // re-exec keeping nothing but the disk.
    //
    // A staging pass re-reads over text that has not moved, so what the read
    // does on the way through — `$(info)`, `$(warning)`, `$(file >)` — belongs
    // to the first read of each unit and not to the repeats; and what the read
    // is TOLD cannot be held back at all, so the first read's answers are
    // handed back instead. The ground has moved by then and GNU Make's single
    // read never saw it move. `build_compiler_inputs` both fills the journals
    // and empties them, because the pass that read those units is also the pass
    // that decides whether the next one is repeating them.
    let mut settled = crate::make::Groundwork::default();
    let mut restarts = 0_usize;
    for _ in 0..100 {
        // Each pass reads the whole compilation again, so what an earlier one
        // classified is the same lines classified twice. The last pass is the
        // one the build is made from, and it is the one a census describes.
        let _ = root.census.take();
        let mut session = session_for(
            root.invocation,
            root.makefiles,
            job_count(root.options),
            root.invoked_as,
            root.diagnostics,
            root.census,
        );
        if let Some(contents) = root.makefile_contents {
            session.supply_makefile(STANDARD_INPUT.into(), contents.to_vec());
        }
        record_invocation_variables(&mut session, root.invocation, root.level, restarts);
        let compilation = compilation_context(
            root.invocation,
            root.directory.to_owned(),
            job_count(root.options),
            root.level,
            &session,
            root.reporting,
            restarts,
        );
        let mut loaded = match evaluated(
            session,
            &root.invocation.evals,
            root.invocation.shuffle,
            compilation,
            reported,
            &settled,
        ) {
            Ok(loaded) => loaded,
            Err(refusal) => return led_by_raised(refusal, root.diagnostics, diagnostics, held),
        };
        // Taken here rather than after the build, because `recipe_begun` is
        // consulted while compiling: what is recorded now reaches the pass
        // after this one and not this one.
        settled.recipes.extend(loaded.take_recipes_carried_whole());
        emit_raised(root.diagnostics, diagnostics, held)?;
        let effective_invocation = evaluated_invocation(loaded.makeflags())?;
        let effective_options = evaluated_build_options(root.options, &effective_invocation);
        if loaded.regeneration_targets().is_empty() {
            return Ok(crate::make::cli::remake::read_with_nothing_to_remake(
                loaded,
                reported,
                effective_invocation,
                effective_options,
            ));
        }
        let compiler_inputs = CompilerInputBuild {
            loaded,
            invocation: &effective_invocation,
            options: effective_options.clone(),
            directory: root.directory,
            goals: &root.invocation.goals,
            restarts,
        };
        let settlement =
            build_compiler_inputs(compiler_inputs, reported, output, diagnostics, &mut settled);
        emit_raised(root.diagnostics, diagnostics, held)?;
        match settlement? {
            Settlement::Finished(result) => return Ok(PreparedGraph::Finished(result)),
            Settlement::Restart => restarts = restarts.saturating_add(1),
            Settlement::Staged => {}
            Settlement::Settled {
                graph,
                persistence,
                recipes,
            } => {
                return Ok(PreparedGraph::Ready {
                    graph,
                    recipes,
                    persistence: Some(persistence),
                    invocation: Box::new(effective_invocation),
                    options: Box::new(effective_options),
                });
            }
        }
    }

    let path = BString::from(
        root.makefiles
            .first()
            .map(|makefile| makefile.as_os_str().as_encoded_bytes())
            .unwrap_or_default(),
    );
    Ok(PreparedGraph::Finished(abandoned(
        std::mem::take(reported),
        CliError::ManifestRetryLimit {
            path,
            attempts: 100,
        }
        .into(),
    )))
}

/// What reading a Makefile without building it produced.
pub(crate) struct MakefileRead {
    /// Every diagnostic the compile raised, in the words and the located shape
    /// the compiler that raised them wrote. A lint passes them on rather than
    /// re-rendering them: each one already points at its own Makefile line.
    pub(crate) raised: Vec<u8>,
    /// What the read narrated on its way, which is a remade Makefile's build
    /// output and nothing else.
    pub(crate) reported: String,
    /// Every recursive invocation the compile classified, in the order it
    /// classified them, with the disposition it acted on.
    pub(crate) census: Vec<kati::census::Invocation>,
    /// The result the read ended with, when it ended before producing a graph.
    /// A refusal is the usual reason, and its status is the one the invocation
    /// would have left with.
    pub(crate) stopped: Option<RunResult>,
}

/// Read a Makefile the way a build's read phase reads it, and stop there.
///
/// The evaluation is a build's own: `$(shell)` runs, `$(warning)` and
/// `$(info)` print, and a Makefile the read must remake is remade, because
/// GNU Make's read phase does all three. A report gathered from a quieter read
/// would describe a Makefile nobody builds.
// [spec:ronin:req:tools.lint]
pub(crate) fn read_without_building(
    runner: &Runner,
    arguments: &[BString],
) -> Result<MakefileRead, Error> {
    let raised = Arc::new(kati::diagnostics::Diagnostics::collected());
    let census = Arc::new(kati::census::Census::collected());
    let mut held = Vec::new();
    let compiled = compile_invocation(
        runner,
        arguments,
        &mut None,
        &mut None,
        &raised,
        &Purpose {
            census: &census,
            reporting: true,
        },
        &mut held,
    )?;
    held.extend(raised.take());
    let stopped = match compiled.prepared {
        // The logs a compiler-input build opened are flushed rather than
        // dropped: that build ran real commands, and what it recorded about
        // them belongs in the log whether or not a lint asked for it.
        PreparedGraph::Ready { persistence, .. } => {
            if let Some(persistence) = persistence {
                persistence.finish()?;
            }
            None
        }
        PreparedGraph::Finished(result) => Some(result),
    };
    Ok(MakefileRead {
        raised: held,
        reported: compiled.reported,
        census: census.take(),
        stopped,
    })
}

/// Run one Make invocation to its end.
// [spec:ronin:req:product.make-identity]
// [spec:ronin:req:make.recursive-invocation+2]
// [spec:ronin:req:make.narration+1]
pub(crate) fn run(
    runner: &Runner,
    arguments: &[BString],
    output: Option<&mut dyn Write>,
    diagnostics: Option<&mut dyn Write>,
) -> Result<RunResult, Error> {
    // Where the compiler writes what it has to say short of a refusal. It is
    // this invocation's descriptor rather than the process's standard error, so
    // that a caller holding a run as a value holds its warnings too — the same
    // arrangement the compiler's refusals have always had, where the error is
    // returned and the caller decides where it goes.
    let raised = Arc::new(kati::diagnostics::Diagnostics::collected());
    // What was raised with no sink to stream it to, which leads the result's
    // standard error exactly as an ordinary warning leads Ninja's.
    let mut held = Vec::new();
    let mut result = reported_run(runner, arguments, output, diagnostics, &raised, &mut held);
    // Anything raised after the last drain — the build owns the descriptor
    // while it runs and empties it as it binds each recipe, so this is the
    // residue of a run that ended somewhere else.
    held.extend(raised.take());
    if held.is_empty() {
        return result;
    }
    if let Ok(result) = result.as_mut() {
        held.append(&mut result.stderr);
        result.stderr = std::mem::take(&mut held);
    }
    result
}

/// One Make invocation read and compiled, with the build it was compiled for
/// not yet started.
///
/// This is the seam a lint reads at. Everything above it is the read phase,
/// which a lint performs in full because a report gathered from a lesser read
/// would be about a different Makefile; everything below it is the build,
/// which a lint does not perform at all.
struct CompiledInvocation {
    prepared: PreparedGraph,
    /// What the read narrated on its way here, which the run that follows
    /// leads its own output with.
    reported: String,
    /// Where the invocation ended up after its `-C` options were applied.
    directory: PathBuf,
}

/// Read and compile one Make invocation, stopping where a build would begin.
// [spec:ronin:req:tools.lint]
fn compile_invocation(
    runner: &Runner,
    arguments: &[BString],
    output: &mut Option<&mut dyn Write>,
    diagnostics: &mut Option<&mut dyn Write>,
    raised: &Arc<kati::diagnostics::Diagnostics>,
    purpose: &Purpose<'_>,
    held: &mut Vec<u8>,
) -> Result<CompiledInvocation, Error> {
    let mut reported = String::new();
    let invocation = match parse(
        arguments,
        runner.makeflags.as_deref(),
        runner.gnumakeflags.as_deref(),
    )? {
        Action::Immediate(result) => {
            return Ok(CompiledInvocation {
                prepared: PreparedGraph::Finished(result),
                reported,
                directory: runner.working_directory.as_path().to_owned(),
            });
        }
        Action::Execute(invocation) => *invocation,
    };
    let invoked_as = make_named_invocation(arguments, &runner.executable);
    let directory = enter_directories(&invocation.directories)?;
    let working_directory = crate::os::WorkingDirectory::new(&directory)
        .map_err(|source| CliError::CurrentDirectory { source })?;
    let level = runner.makelevel.as_deref().unwrap_or_default();
    let level: usize = level.trim().parse().unwrap_or(0);
    let options = build_options(&invocation, runner, working_directory)?;

    let finished = |result| {
        Ok(CompiledInvocation {
            prepared: PreparedGraph::Finished(result),
            reported: String::new(),
            directory: directory.clone(),
        })
    };
    let makefiles = named_makefiles(&invocation, &directory);
    if makefiles.is_empty() {
        return finished(no_makefile());
    }
    // Standard input is drained once and replayed into every read, because a
    // Makefile remade between reads sends the whole read around again and the
    // pipe is gone by then. Two of them cannot be told apart afterwards, so
    // the refusal comes before the draining rather than after it.
    let named_stdin = makefiles
        .iter()
        .filter(|named| is_standard_input(named))
        .count();
    if named_stdin > 1 {
        return finished(duplicate_standard_input());
    }
    let makefile_contents = if named_stdin == 1 {
        let mut contents = Vec::new();
        std::io::stdin()
            .read_to_end(&mut contents)
            .map_err(|source| CliError::ReadInput { source })?;
        Some(contents)
    } else {
        None
    };

    // Missing included Makefiles are source dependencies. Kati emits their
    // rules into a provisional graph; the ordinary Ninja scheduler builds
    // those roots, then the frontend recompiles from a fresh session. No Make
    // provenance or restart behavior crosses into the executor.
    let root = RootCompilation {
        invocation: &invocation,
        diagnostics: raised,
        census: purpose.census,
        makefiles: &makefiles,
        invoked_as: &invoked_as,
        directory: &directory,
        options: &options,
        makefile_contents: makefile_contents.as_deref(),
        level,
        reporting: purpose.reporting,
    };
    let prepared = prepare_graph(&root, &mut reported, output, diagnostics, held)?;
    Ok(CompiledInvocation {
        prepared,
        reported,
        directory,
    })
}

/// One Make invocation, with the descriptor its compiler diagnostics reach.
fn reported_run(
    runner: &Runner,
    arguments: &[BString],
    mut output: Option<&mut dyn Write>,
    mut diagnostics: Option<&mut dyn Write>,
    raised: &Arc<kati::diagnostics::Diagnostics>,
    held: &mut Vec<u8>,
) -> Result<RunResult, Error> {
    let compiled = compile_invocation(
        runner,
        arguments,
        &mut output,
        &mut diagnostics,
        raised,
        &Purpose {
            // A build acts on each classification as it is made and has no use
            // for it afterwards, so it keeps none of them.
            census: &Arc::new(kati::census::Census::ignored()),
            // And a composition it cannot read is a refusal rather than a
            // finding, because the work would simply not happen.
            reporting: false,
        },
        held,
    )?;
    let mut reported = compiled.reported;
    let directory = compiled.directory;
    let (mut graph, mut recipes, opened, invocation, options) = match compiled.prepared {
        PreparedGraph::Ready {
            graph,
            recipes,
            persistence,
            invocation,
            options,
        } => (*graph, recipes, persistence, *invocation, *options),
        PreparedGraph::Finished(result) => return Ok(result),
    };
    let mut persistence = if let Some(persistence) = opened {
        persistence
    } else {
        let (persistence, warning) = Persistence::open(&mut graph, &directory)?;
        reported.push_str(warning.as_deref().unwrap_or_default());
        persistence
    };
    let targets = graph.default_targets();
    let mut build = Build::with_options(&mut graph, &mut persistence, options);
    if let Some(recipes) = recipes.as_deref_mut() {
        build = build.late_commands(recipes);
    }
    if let Some(sink) = output {
        build = build.output(sink);
    }
    if let Some(sink) = diagnostics {
        build = build.diagnostics(sink);
    }
    let planned = build.plan(&targets);
    if invocation.questioning() {
        let question = planned.and_then(|mut planned| planned.interrogate());
        let flushed = persistence.finish();
        let question = question.and_then(|up_to_date| flushed.map(|()| up_to_date));
        return Ok(answered(
            reported,
            question,
            invocation.given(Switch::KeepGoing),
        ));
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
// [spec:ronin:req:make.recursive-invocation+2]
fn record_invocation_variables(
    session: &mut Session,
    invocation: &Invocation,
    level: usize,
    restarts: usize,
) {
    record_invocation(session, MAKELEVEL, level.to_string());
    // GNU Make re-executes itself to read a remade Makefile and hands the new
    // process a `MAKE_RESTARTS` count in its environment, which is why the
    // variable's origin is the environment and why the first read has no such
    // variable at all rather than a zero. Ronin reads again in place, so the
    // count is recorded here instead of survived across an exec.
    if restarts > 0 {
        record_invocation(session, MAKE_RESTARTS, restarts.to_string());
    }
    // Kati installs MAKEFLAGS as a file-origin recursive compiler variable.
    // Leaving an inherited environment binding beside it would make `-e`
    // incorrectly outrank that built-in definition.
    let environment = session
        .invocation_environment
        .get_or_insert_with(|| std::env::vars_os().collect());
    environment.retain(|(candidate, _)| candidate != "MAKEFLAGS");
    // GNU Make empties `GNUMAKEFLAGS` rather than withdrawing it: `main` writes
    // `define_variable_cname (GNUMAKEFLAGS_NAME, "", o_env, 0)` the instant its
    // switches have been decoded, so a Makefile reads an empty value at the
    // environment's own rank and a child is handed the name with nothing in it.
    // Emptying and withdrawing are two different things here, and the
    // difference is what upstream's own case asks about: a Make that was given
    // no `GNUMAKEFLAGS` does not invent one for its children.
    if environment.iter().any(|(name, _)| name == GNUMAKEFLAGS) {
        record_invocation(session, GNUMAKEFLAGS, String::new());
    }
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
    reporting: bool,
    restarts: usize,
) -> crate::make::CompilationContext {
    let mut recipe_environment = vec![(
        OsString::from(MAKELEVEL),
        Some(OsString::from(level.saturating_add(1).to_string())),
    )];
    // A recipe's environment is a delta over the process's, so an emptied
    // `GNUMAKEFLAGS` has to be said again here: the process still holds the
    // switches this invocation already folded into `MAKEFLAGS`, and a child
    // reading them a second time would apply them twice. Taken from what the
    // invocation was recorded with, so a run that was given no second stream
    // says nothing rather than inventing an empty one.
    recipe_environment.extend(
        session
            .invocation_environment
            .iter()
            .flatten()
            .find(|(name, _)| name == GNUMAKEFLAGS)
            .map(|(name, value)| (name.clone(), Some(value.clone()))),
    );
    crate::make::CompilationContext {
        root_directory: directory.clone(),
        directory,
        path_prefix: PathBuf::new(),
        diagnostics: Arc::clone(&session.diagnostics),
        census: Arc::clone(&session.census),
        reporting,
        makeflags: propagated_makeflags(invocation),
        always_make: invocation.given(Switch::AlwaysMake),
        restarted: restarts > 0,
        assumed_new: invocation.assumed_new.clone(),
        level,
        jobs,
        // Everything this unit was evaluated with except how many times it has
        // been read. GNU Make marks `MAKE_RESTARTS` no-export precisely so a
        // child never sees it, and here it would do more than be visible: a
        // recursive child's compilation is identified by the environment it
        // inherits, so a count that rises with every restart would give the
        // same child a new identity each time and the work staged for it would
        // never be recognised as done.
        environment: descendant_environment(session),
        recipe_environment,
    }
}

/// The environment a recursive child of this unit is compiled with.
fn descendant_environment(session: &Session) -> Vec<(OsString, OsString)> {
    let mut environment = session
        .invocation_environment
        .clone()
        .unwrap_or_else(|| std::env::vars_os().collect());
    environment.retain(|(name, _)| name != MAKE_RESTARTS);
    environment
}

/// The exact MAKEFLAGS value this compilation unit hands to a semantic child.
fn propagated_makeflags(invocation: &Invocation) -> String {
    compiler_flag_variables(invocation).makeflags
}

fn compilation_key(directory: &Path, makefiles: &[PathBuf], makeflags: &str) -> Vec<u8> {
    let mut key = directory.as_os_str().as_encoded_bytes().to_vec();
    for makefile in makefiles {
        key.push(0);
        key.extend_from_slice(makefile.as_os_str().as_encoded_bytes());
    }
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
// [spec:ronin:req:make.recursive-invocation+2]
fn evaluated(
    mut session: Session,
    evals: &[Bytes],
    shuffle: Shuffle,
    context: crate::make::CompilationContext,
    reported: &str,
    settled: &crate::make::Groundwork,
) -> Result<crate::make::Loaded, RunResult> {
    if let Err(failure) = prepend_command_line_evals(&mut session, evals) {
        return Err(RunResult {
            stdout: terminated(reported),
            stderr: ordinary_diagnostic(failure),
            exit_code: ABANDONED,
        });
    }
    let makefiles: Vec<PathBuf> = session.flags.makefiles.iter().map(PathBuf::from).collect();
    let cache_key = compilation_key(&context.directory, &makefiles, &context.makeflags);
    let compilation = crate::make::Compilation {
        session,
        shuffle,
        context,
        cache_key,
    };
    crate::make::load_with_subninjas(
        compilation,
        compile_subninja,
        settled,
        // Make mode runs the graph it compiles, in the process that compiled
        // it, so a recipe is expanded when its edge is about to run.
        kati::build_sink::RecipeExpansion::Launch,
    )
    .map_err(|failure| RunResult {
        stdout: terminated(reported),
        stderr: ordinary_diagnostic(failure),
        exit_code: ABANDONED,
    })
}

#[cfg(test)]
mod interface_tests;

#[cfg(test)]
mod tests {
    use super::interface_tests::{parsed, parsed_under, parsed_with_environment, refused};
    use super::{Invocation, MAKE_RESTARTS, OutputSync, Shuffle, Switch, descendant_environment};
    use crate::build::JobLimit;
    use crate::util::BString;
    use kati::session::Session;
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// A recursive child is identified by the environment it inherits, and how
    /// many times the parent has read its own Makefile is not part of what the
    /// child is — GNU Make marks the count no-export for the same reason.
    #[test]
    fn child_does_not_inherit_restart_count() {
        let mut session = Session::from_args(vec![OsString::from("make")]);
        session.invocation_environment = Some(vec![
            (OsString::from(MAKE_RESTARTS), OsString::from("2")),
            (OsString::from("PATH"), OsString::from("/bin")),
        ]);
        let environment = descendant_environment(&session);
        assert!(environment.iter().all(|(name, _)| name != MAKE_RESTARTS));
        assert!(environment.iter().any(|(name, _)| name == "PATH"));
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn maps_make_options_to_ninja_controls() {
        let invocation = parsed(&[
            "make", "-j8", "-k", "-n", "-f", "other.mk", "all", "FOO=bar",
        ]);
        assert_eq!(invocation.jobs, JobLimit::fixed(8));
        assert!(invocation.given(Switch::KeepGoing));
        assert!(invocation.given(Switch::DryRun));
        assert_eq!(invocation.makefiles, vec![PathBuf::from("other.mk")]);
        assert_eq!(invocation.goals, vec![BString::from("all")]);
        assert_eq!(
            invocation.variables,
            vec![kati::bytes::Bytes::from_static(b"FOO=bar")]
        );
    }

    /// GNU Make's `handle_non_switch_argument` will not make a goal of a
    /// zero-length word, so neither does this: the word is passed over
    /// wherever it stands and the goals either side of it are the whole list.
    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn an_empty_word_is_passed_over() {
        let alone = parsed(&["make", ""]);
        assert!(alone.goals.is_empty());
        assert!(alone.variables.is_empty());

        let surrounded = parsed(&["make", "", "all", "", "FOO=bar", ""]);
        assert_eq!(surrounded.goals, vec![BString::from("all")]);
        assert_eq!(
            surrounded.variables,
            vec![kati::bytes::Bytes::from_static(b"FOO=bar")]
        );

        // `--` stops option parsing and the same guard has to hold after it.
        let after_terminator = parsed(&["make", "--", ""]);
        assert!(after_terminator.goals.is_empty());
    }

    /// The other word `handle_non_switch_argument` will not make a goal of:
    /// it returns on a lone `-` before it tests anything, "for compatibility".
    /// A dash naming a directory to `-I` is a different word in a different
    /// position and is still read as one.
    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn a_plain_dash_is_passed_over() {
        let alone = parsed(&["make", "-"]);
        assert!(alone.goals.is_empty());
        assert!(alone.variables.is_empty());

        let surrounded = parsed(&["make", "-", "all", "-", "FOO=bar", "-"]);
        assert_eq!(surrounded.goals, vec![BString::from("all")]);
        assert_eq!(
            surrounded.variables,
            vec![kati::bytes::Bytes::from_static(b"FOO=bar")]
        );

        let after_terminator = parsed(&["make", "--", "-"]);
        assert!(after_terminator.goals.is_empty());

        // The dash `-I` takes is a list entry rather than a goal, and it stays
        // in the list at the position it was written: what it forgets and what
        // it turns off are read off there.
        let forgets = parsed(&["make", "-I", "one", "-I", "-", "-I", "two"]);
        assert!(forgets.goals.is_empty());
        assert_eq!(
            forgets.include_dirs,
            vec![
                PathBuf::from("one"),
                PathBuf::from("-"),
                PathBuf::from("two")
            ]
        );
        // A second one is a duplicate and is dropped, so it resets nothing —
        // and a directory named on both sides of the first is dropped too, by
        // the copy the reset was supposed to have thrown away.
        let once = parsed(&["make", "-I", "one", "-I", "-", "-I", "one", "-I", "-"]);
        assert_eq!(
            once.include_dirs,
            vec![PathBuf::from("one"), PathBuf::from("-")]
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
    // [spec:ronin:req:make.recursive-invocation+2/test]
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
    // [spec:ronin:req:make.recursive-invocation+2/test]
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
        assert!(
            under(Some("w"), &["make", "--no-print-directory"]).refused(Switch::PrintDirectory)
        );
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
    // [spec:ronin:req:make.recursive-invocation+2/test]
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

    /// An `--eval` fragment travels in MAKEFLAGS and nowhere else, because
    /// `$(MAKE)` is a path and carries nothing. Every value below is read off
    /// GNU Make 4.4.1 rather than reasoned about: a `$` doubled, a blank
    /// backslashed, the fragments after every switch and before the `--`, and
    /// MFLAGS carrying none of them because GNU Make spells MFLAGS from the
    /// switch string before it appends them.
    ///
    /// `makeflags` is the whole of it, which is what descends and what a child
    /// decodes; `eval_flags` is the part `MAKEFLAGS` names rather than holds,
    /// and `base` is the switch table with neither. The three are asserted
    /// together because their relationship is the thing: `base`, one space and
    /// `eval_flags` is exactly the text the reference resolves to.
    // [spec:ronin:req:make.recursive-invocation+2/test]
    #[test]
    fn propagates_eval_fragments() {
        let variables = |arguments: &[&str]| super::compiler_flag_variables(&parsed(arguments));

        // The leading space is GNU Make's: with no switch letters the group is
        // empty and the first thing after it still gets its separator.
        let one = variables(&["make", "--eval=$(info eval)"]);
        assert_eq!(one.makeflags, r" --eval=$$(info\ eval)");
        assert_eq!(one.eval_flags, r"--eval=$$(info\ eval)");
        assert_eq!(one.base, "");
        assert_eq!(one.mflags, "");

        // The short spelling is the same switch, and two fragments keep the
        // order they were written in.
        let two = variables(&["make", "-E", "A=1", "--eval=B=2"]);
        assert_eq!(two.makeflags, " --eval=A=1 --eval=B=2");
        assert_eq!(two.eval_flags, "--eval=A=1 --eval=B=2");

        // Beside a switch group, a long option and an assignment, which is
        // where the position is decided.
        let placed = variables(&[
            "make",
            "-k",
            "--no-print-directory",
            "--eval=X := 1",
            "FOO=bar",
        ]);
        assert_eq!(
            placed.makeflags,
            r"k --no-print-directory --eval=X\ :=\ 1 -- FOO=bar"
        );
        assert_eq!(placed.base, "k --no-print-directory");
        assert_eq!(placed.eval_flags, r"--eval=X\ :=\ 1");
        assert_eq!(placed.mflags, "-k --no-print-directory");

        // And nothing at all where no fragment was given, which is what makes
        // `MAKEFLAGS` name no variable rather than name an empty one.
        let none = variables(&["make", "-k", "FOO=bar"]);
        assert_eq!(none.eval_flags, "");
        assert_eq!(none.makeflags, "k -- FOO=bar");

        // A backslash is escaped too, so the fragment comes back as one word.
        let escaped = variables(&["make", r"--eval=P := a\b"]);
        assert_eq!(escaped.makeflags, r" --eval=P\ :=\ a\\b");
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

    // [spec:ronin:req:make.recursive-invocation+2/test]
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

    /// The environment's two option streams, read in GNU Make's order.
    ///
    /// Pinned at the parser because what distinguishes them is only the order
    /// and not the grammar: everything either stream contributes is published
    /// as `MAKEFLAGS`, so a later gate cannot say which of them a switch came
    /// from, and the one thing the order decides is which spelling of a switch
    /// taking an argument is left standing.
    // [spec:ronin:req:make.interface-compatibility/test]
    #[test]
    fn gnumakeflags_is_read_before_makeflags() {
        let flags = |gnumakeflags: Option<&str>, inherited: Option<&str>| {
            let invocation = parsed_with_environment(gnumakeflags, inherited, &["make"]);
            super::compiler_flag_variables(&invocation).makeflags
        };

        // The stream alone, with the leading cluster's dash prepended exactly
        // as `decode_env_switches` prepends it to `argv[1]`.
        assert_eq!(flags(Some("-k"), None), "k");
        assert_eq!(flags(Some("k"), None), "k");
        assert_eq!(
            flags(Some("--no-print-directory -e -r -R --trace"), None),
            "erR --trace --no-print-directory"
        );

        // Beside `MAKEFLAGS`, which is read second and therefore last.
        assert_eq!(flags(Some("-k"), Some("s")), "ks");
        assert_eq!(
            flags(Some("-I first"), Some("-I second")),
            " -Ifirst -Isecond"
        );
        assert_eq!(
            flags(Some("--debug=b"), Some("--debug=j")),
            " --debug=b --debug=j"
        );

        // A word holding an `=` is a command-line assignment in either stream,
        // and is the one first word the prepended dash is withheld from.
        assert_eq!(flags(Some("FOO=bar"), None), " -- FOO=bar");

        // Nothing at all is not an empty stream: neither reads a switch, and
        // an empty one contributes no word rather than an empty one.
        assert_eq!(flags(None, None), "");
        assert_eq!(flags(Some(""), None), "");
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
    // [spec:ronin:req:make.recursive-invocation+2/test]
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
