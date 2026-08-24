//! What `sh`'s own argument vector means, parsed byte for byte.
//!
//! This is the shell's command line rather than a build's, and it lives here
//! because nsh no longer parses one. The library's surface is closed around a
//! *typed* request — [`nsh::Startup`] plus the option table — and its own
//! documentation says why: "the command-line crate turns raw process arguments
//! into this value. The library therefore receives a typed execution request
//! and never parses an invocation or decides whether the host process should
//! exit." A shell that ended its embedder's process, or that read an argument
//! vector the embedder had already read, would be a library nobody could
//! embed. So a frontend owns this, and Ronin invoked as `sh` is a frontend.
//!
//! It is therefore a port of `crates/nsh-cli/src/invocation.rs` at nsh
//! 8aa0728, and it is deliberately a close one: the shell's option grammar is
//! POSIX's, the diagnostics are dash's byte for byte, and a paraphrase would
//! be a second shell language wearing this one's syntax. Two things are
//! dropped rather than copied, both because Ronin answers to one name only
//! (see [`super::SHELL_NAMES`]):
//!
//!   * `bash` mode inferred from `argv[0]`, which Ronin cannot reach — the
//!     name never selects the shell, so the inference could only ever be
//!     false. `-o bash` still works, through the ordinary option path.
//!   * `--help` and `--version`, which are nsh-cli's own and not a shell's.
//!     `/bin/sh` is dash here and dash has neither; Ronin's `--version` is the
//!     build tool's, reached under the build tool's name.
//!
//! Everything else is the same parse, and `tests/shell.rs` is what says so.

use crate::util::{BStr, BString};
use nsh::{ShellOption, Startup};

/// Why an invocation could not be run at all.
///
/// Not a shell error: the shell has not started. dash reports these against
/// line 0 — `sh: 0: Illegal option -Q` — because there is no script line to
/// blame, and leaves with 2.
pub(super) enum ParseError {
    /// The argument vector is not one the shell can act on. Carries the
    /// diagnostic's own bytes, without the `argv[0]: 0: ` prefix.
    Invocation(Vec<u8>),
    /// `-o` with no operand asked for the option table and the write failed.
    Output,
}

/// Where the shell reads its commands, which is the whole of what `-c`, a
/// script operand and bare `sh` differ by.
enum Input {
    /// `sh -c COMMAND`.
    Command(BString),
    /// `sh -s -c COMMAND`: the command, and then standard input.
    CommandThenStdin(BString),
    /// `sh SCRIPT`, which is the shape Ronin's own response files arrive in.
    Script(BString),
    /// Bare `sh`.
    Stdin,
}

/// The shell option table as the command line leaves it.
///
/// A `Vec` of pairs rather than a set keyed by discriminant, because
/// [`ShellOption`] is the library's type and offers no map: `ALL` is 23 long
/// and a linear scan over 23 is not worth a dependency on its internals.
pub(super) struct Options {
    values: Vec<(ShellOption, bool)>,
}

impl Options {
    fn new() -> Self {
        Self {
            values: ShellOption::ALL
                .into_iter()
                .map(|option| (option, false))
                .collect(),
        }
    }

    /// Whether the command line left `option` on.
    pub(super) fn enabled(&self, option: ShellOption) -> bool {
        self.values
            .iter()
            .find(|(candidate, _)| *candidate == option)
            .is_some_and(|(_, enabled)| *enabled)
    }

    /// Set `option`, and clear the one it excludes.
    ///
    /// `vi` and `emacs` are the only pair that cannot both stand: they name
    /// one editing discipline, so enabling either is a statement about the
    /// other. Nothing else in the table interacts.
    fn set(&mut self, option: ShellOption, enabled: bool) {
        if let Some((_, value)) = self
            .values
            .iter_mut()
            .find(|(candidate, _)| *candidate == option)
        {
            *value = enabled;
        }
        if !enabled {
            return;
        }
        let counterpart = match option {
            ShellOption::Vi => ShellOption::Emacs,
            ShellOption::Emacs => ShellOption::Vi,
            _ => return,
        };
        if let Some((_, value)) = self
            .values
            .iter_mut()
            .find(|(candidate, _)| *candidate == counterpart)
        {
            *value = false;
        }
    }

    /// The table as `-o` prints it with no operand, in dash's two shapes.
    ///
    /// `-o` gives the human-readable column form and `+o` gives the form that
    /// can be fed back to a shell. The 16-column pad is dash's, and the names
    /// are the library's, so a new option appears here without this file
    /// changing.
    fn report(&self, enabled_form: bool) -> Vec<u8> {
        let mut output = Vec::new();
        if enabled_form {
            output.extend_from_slice(b"Current option settings\n");
            for option in ShellOption::ALL {
                let name = option.name();
                output.extend_from_slice(name);
                output.resize(output.len() + 16usize.saturating_sub(name.len()), b' ');
                output.extend_from_slice(if self.enabled(option) {
                    b"on\n"
                } else {
                    b"off\n"
                });
            }
        } else {
            for option in ShellOption::ALL {
                output.extend_from_slice(if self.enabled(option) {
                    b"set -o "
                } else {
                    b"set +o "
                });
                output.extend_from_slice(option.name());
                output.push(b'\n');
            }
        }
        output
    }
}

/// What the option words at the head of the command line left behind.
struct Scan {
    /// The option table as those words set it.
    options: Options,
    /// The options a word named, which is not the same as the options that
    /// are on: two defaults below apply only where the command line was
    /// silent, and only this can tell silence from an explicit `+o`.
    explicit: Vec<ShellOption>,
    /// `-c` was given, so the first operand is a command rather than a file.
    command: bool,
    /// The shell reads its profiles first.
    login: bool,
    /// Where the operands begin.
    operands: usize,
}

/// Read the option words, stopping at the first operand.
///
/// `login` arrives already decided by `argv[0]`, because a leading `-` on the
/// invoked name says so before any word does, and `-l` can only add to it.
fn scan_options(
    argv: &[Vec<u8>],
    login: bool,
    mut write_report: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<Scan, ParseError> {
    let mut scan = Scan {
        options: Options::new(),
        explicit: Vec::new(),
        command: false,
        login,
        operands: 1,
    };

    while let Some(word) = argv.get(scan.operands) {
        scan.operands += 1;
        let enabled = match word.first() {
            Some(b'-') => {
                // A bare `-` and a `--` both end the options. `-` is not an
                // option letter and `--` is the separator.
                if word.len() == 1 || word.as_slice() == b"--" {
                    break;
                }
                true
            }
            Some(b'+') => false,
            _ => {
                // The first operand. Put it back for the caller.
                scan.operands -= 1;
                break;
            }
        };

        for &letter in &word[1..] {
            let option = match letter {
                b'c' => {
                    scan.command = true;
                    continue;
                }
                b'l' => {
                    scan.login = true;
                    continue;
                }
                b'o' => {
                    let Some(name) = argv.get(scan.operands) else {
                        // `-o` at the end of the line asks what the options
                        // are rather than setting one.
                        write_report(&scan.options.report(enabled))
                            .map_err(|_| ParseError::Output)?;
                        continue;
                    };
                    scan.operands += 1;
                    let Some(option) = ShellOption::from_name(BStr::new(name)) else {
                        let mut message = b"Illegal option -o ".to_vec();
                        message.extend_from_slice(name);
                        return Err(ParseError::Invocation(message));
                    };
                    option
                }
                letter => {
                    let Some(option) = ShellOption::from_letter(letter) else {
                        let mut message = b"Illegal option -".to_vec();
                        message.push(letter);
                        return Err(ParseError::Invocation(message));
                    };
                    option
                }
            };
            scan.options.set(option, enabled);
            if !scan.explicit.contains(&option) {
                scan.explicit.push(option);
            }
        }
    }

    Ok(scan)
}

/// One parsed `sh` command line.
pub(super) struct CommandLine {
    /// `argv[0]` as written, which is the interpreter's own name.
    pub(super) invocation_name: BString,
    /// `$0`, which `-c COMMAND NAME` and a script operand both move.
    pub(super) argument_zero: BString,
    /// `$1`, `$2`, ….
    pub(super) parameters: Vec<BString>,
    /// The option table.
    pub(super) options: Options,
    /// Whether the profile files are read first.
    login: bool,
    /// Where the commands come from.
    input: Input,
}

impl CommandLine {
    /// Parse `argv` the way `sh` does.
    ///
    /// `stdin_is_terminal` and `stderr_is_terminal` are facts about the
    /// process, and they are arguments rather than reads because they must be
    /// taken *after* the Rust runtime's own arrangements have been undone —
    /// one of those arrangements opens `/dev/null` over a closed standard
    /// descriptor, which would answer this question wrongly. `write_report`
    /// takes the option table `-o` prints with no operand.
    ///
    /// # Errors
    ///
    /// [`ParseError::Invocation`] for an option the shell does not have or a
    /// `-c` with nothing after it, and [`ParseError::Output`] when the option
    /// table could not be written.
    pub(super) fn parse(
        argv: &[Vec<u8>],
        stdin_is_terminal: bool,
        stderr_is_terminal: bool,
        write_report: impl FnMut(&[u8]) -> std::io::Result<()>,
    ) -> Result<Self, ParseError> {
        let invocation_name = argv.first().cloned().unwrap_or_else(|| b"sh".to_vec());
        // A leading `-` on argv[0] is how login is signalled, and `-l` says it
        // outright. Ronin cannot be reached under `-sh` — the name that
        // selects the shell is the whole file name `sh` — so in practice only
        // the flag arrives, but the parse is the shell's and reads both.
        let login = invocation_name.first() == Some(&b'-');
        let Scan {
            mut options,
            explicit,
            command,
            login,
            operands,
        } = scan_options(argv, login, write_report)?;

        if operands >= argv.len() {
            if command {
                return Err(ParseError::Invocation(b"-c requires an argument".to_vec()));
            }
            // Nothing to read but the shell's own input, which is what `-s`
            // says and what its absence means when there is no operand.
            options.set(ShellOption::Stdin, true);
        }

        // A shell reading a terminal from a terminal is interactive unless it
        // was told otherwise, and an interactive shell takes the terminal's
        // process groups with it.
        if !explicit.contains(&ShellOption::Interactive)
            && options.enabled(ShellOption::Stdin)
            && stdin_is_terminal
            && stderr_is_terminal
        {
            options.set(ShellOption::Interactive, true);
        }
        if !explicit.contains(&ShellOption::Monitor) {
            // Interactivity forced over a pipe has no terminal process group
            // for monitor mode to take, so it does not get one. An explicit
            // `-m` is a different statement and is left standing.
            let monitor = if options.enabled(ShellOption::Stdin) && !stdin_is_terminal {
                false
            } else {
                options.enabled(ShellOption::Interactive)
            };
            options.set(ShellOption::Monitor, monitor);
        }

        let remaining = &argv[operands..];
        let mut argument_zero = BString::from(invocation_name.as_slice());
        let (input, parameters) = if command {
            // `sh -c COMMAND [NAME [ARG]...]`: the word after the command is
            // `$0`, not `$1`. This is the shape every recipe line arrives in.
            let text = BString::from(remaining[0].as_slice());
            let mut parameter_start = 1;
            if let Some(name) = remaining.get(parameter_start) {
                argument_zero = BString::from(name.as_slice());
                parameter_start += 1;
            }
            let input = if options.enabled(ShellOption::Stdin) {
                Input::CommandThenStdin(text)
            } else {
                Input::Command(text)
            };
            (input, owned_words(&remaining[parameter_start..]))
        } else if options.enabled(ShellOption::Stdin) {
            (Input::Stdin, owned_words(remaining))
        } else {
            // `sh SCRIPT [ARG]...`, which is the shape of a response file.
            argument_zero = BString::from(remaining[0].as_slice());
            (
                Input::Script(argument_zero.clone()),
                owned_words(&remaining[1..]),
            )
        };

        Ok(Self {
            invocation_name: BString::from(invocation_name),
            argument_zero,
            parameters,
            options,
            login,
            input,
        })
    }

    /// The typed execution request the library takes.
    pub(super) fn startup(&self) -> Startup {
        let startup = match &self.input {
            Input::Command(command) => Startup::command(command.clone()),
            Input::CommandThenStdin(command) => Startup::command_then_stdin(command.clone()),
            Input::Script(path) => Startup::script(path.clone()),
            Input::Stdin => Startup::standard_input(),
        };
        startup.login(self.login)
    }
}

fn owned_words(words: &[Vec<u8>]) -> Vec<BString> {
    words
        .iter()
        .map(|word| BString::from(word.as_slice()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&[u8]]) -> (CommandLine, Vec<u8>) {
        let argv: Vec<Vec<u8>> = argv.iter().map(|word| word.to_vec()).collect();
        let mut report = Vec::new();
        let invocation = CommandLine::parse(&argv, false, false, |bytes| {
            report.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap_or_else(|_| panic!("valid invocation"));
        (invocation, report)
    }

    fn error(argv: &[&[u8]]) -> Vec<u8> {
        let argv: Vec<Vec<u8>> = argv.iter().map(|word| word.to_vec()).collect();
        match CommandLine::parse(&argv, false, false, |_| Ok(())) {
            Err(ParseError::Invocation(message)) => message,
            _ => panic!("expected an invocation error"),
        }
    }

    /// The shape every recipe line arrives in: the command is one word, and
    /// the word after it is `$0` rather than `$1`.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_command_names_its_zero_then_its_parameters() {
        let (invocation, _) = parse(&[b"sh", b"-c", b"echo hi", b"name", b"one", b"two"]);
        assert_eq!(invocation.argument_zero, b"name"[..]);
        assert_eq!(invocation.parameters, [b"one".as_slice(), b"two"]);
        assert!(matches!(invocation.input, Input::Command(_)));
    }

    /// With no `NAME`, `$0` stays the spelling the shell was reached under —
    /// which is what makes a diagnostic say `/bin/sh` when a build wrote
    /// `/bin/sh`.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_nameless_command_keeps_the_invoked_spelling() {
        let (invocation, _) = parse(&[b"/bin/sh", b"-c", b"echo hi"]);
        assert_eq!(invocation.argument_zero, b"/bin/sh"[..]);
        assert!(invocation.parameters.is_empty());
    }

    /// A script operand moves `$0` to the script, and the rest are positional.
    /// This is the response-file shape.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_script_operand_becomes_zero_and_the_source() {
        let (invocation, _) = parse(&[b"sh", b"/tmp/recipe", b"a", b"b"]);
        assert_eq!(invocation.argument_zero, b"/tmp/recipe"[..]);
        assert_eq!(invocation.parameters, [b"a".as_slice(), b"b"]);
        assert!(matches!(invocation.input, Input::Script(_)));
    }

    /// `-ec` is one word and both letters count, which is the whole of
    /// `.POSIX:` reaching the shell.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn clustered_letters_are_each_an_option() {
        let (invocation, _) = parse(&[b"sh", b"-ec", b"false"]);
        assert!(invocation.options.enabled(ShellOption::Errexit));
        assert!(matches!(invocation.input, Input::Command(_)));
    }

    /// `+o` turns one off, and `-o` turns it on, by the library's own names.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_named_option_reads_both_ways() {
        let (invocation, _) = parse(&[b"sh", b"-o", b"errexit", b"-c", b":"]);
        assert!(invocation.options.enabled(ShellOption::Errexit));
        let (invocation, _) = parse(&[b"sh", b"-e", b"+o", b"errexit", b"-c", b":"]);
        assert!(!invocation.options.enabled(ShellOption::Errexit));
    }

    /// `-o` with nothing after it asks rather than sets, and it reports the
    /// table as it stands at that point in the scan.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_trailing_o_reports_the_table_reached() {
        let (_, report) = parse(&[b"sh", b"-eo"]);
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("Current option settings\n"), "{report}");
        assert!(report.contains("errexit         on\n"), "{report}");
        let (_, report) = parse(&[b"sh", b"+o"]);
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("set +o errexit\n"), "{report}");
    }

    /// Bare `sh` with no operand reads its own input, and nothing else does.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_bare_shell_reads_standard_input() {
        let (invocation, _) = parse(&[b"sh"]);
        assert!(invocation.options.enabled(ShellOption::Stdin));
        assert!(matches!(invocation.input, Input::Stdin));
    }

    /// The two facts about the process that decide interactivity, and the
    /// asymmetry monitor mode has against a pipe.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn terminal_facts_decide_interactivity_and_job_control() {
        let argv = vec![b"sh".to_vec()];
        let invocation = CommandLine::parse(&argv, true, true, |_| Ok(()))
            .ok()
            .unwrap();
        assert!(invocation.options.enabled(ShellOption::Interactive));
        assert!(invocation.options.enabled(ShellOption::Monitor));

        let invocation = CommandLine::parse(&argv, false, false, |_| Ok(()))
            .ok()
            .unwrap();
        assert!(!invocation.options.enabled(ShellOption::Interactive));
        assert!(!invocation.options.enabled(ShellOption::Monitor));
    }

    /// `--` ends the options, so what follows is an operand even when it
    /// begins with a dash.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_double_dash_ends_the_options() {
        let (invocation, _) = parse(&[b"sh", b"--", b"-not-an-option"]);
        assert_eq!(invocation.argument_zero, b"-not-an-option"[..]);
        assert!(matches!(invocation.input, Input::Script(_)));
    }

    /// Both refusals, in dash's bytes. The prefix and the status are added by
    /// the caller, which is where the shell's own name is known.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn an_unusable_line_is_refused_in_dashs_words() {
        assert_eq!(error(&[b"sh", b"-Q"]), b"Illegal option -Q");
        assert_eq!(
            error(&[b"sh", b"-o", b"nosuchoption"]),
            b"Illegal option -o nosuchoption"
        );
        assert_eq!(error(&[b"sh", b"-c"]), b"-c requires an argument");
    }

    /// An argument need not be valid UTF-8, and the parse must not be the
    /// place that decides otherwise.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn a_non_utf8_argument_survives_the_parse() {
        let (invocation, _) = parse(&[b"sh", b"-c", b":", b"name", b"\xff\xfe"]);
        assert_eq!(invocation.parameters, [b"\xff\xfe".as_slice()]);
    }
}
