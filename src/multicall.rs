//! Which build language an invocation speaks.
//!
//! Ronin has two front ends over one engine, and the name the executable was
//! invoked under is the only thing that says which of them an invocation
//! wanted. `make` and `gmake` select Make mode; every other name, Ronin's own
//! included, selects Ninja mode. The name is an explicit statement of intent in
//! a way that sniffing the directory for a `Makefile` or a `build.ninja` is
//! not, and it is how every multi-call binary has ever done this.
//!
//! The name is the *only* way in: there is no flag that selects a front end.
//! A second door had to answer the same question the name already answers, and
//! it answered it worse. `$(MAKE)` is the case that settled it — a sub-make
//! reached through a flag needs the flag carried in `MAKE`, which makes `MAKE`
//! more than one word, and a great deal of software treats that value as the
//! path of the make program and execs it. GNU Make's own test suite is the most
//! demanding example, and it dies on the second word. Reached through the name,
//! `MAKE` is a path, which is all it ever needed to be.
//!
//! A third name is not a front end at all. Invoked as `sh`, Ronin is the
//! shell — the one it hands commands to for the rest of the time — and that
//! question is answered before a build is set up, because a shell wants the
//! process's own signal dispositions and its own streams rather than the ones
//! a build arranges. See [`run_as_shell`].
//!
//! See `plan/decisions/multicall-identity.md` and
//! `plan/decisions/builtin-shell.md`.
// [spec:ronin:req:product.make-identity]

use crate::Error;
use crate::cli::RunResult;
use crate::util::{BString, ByteSlice};
use std::ffi::OsString;
use std::path::Path;

/// The shell's own command line, which nsh's closed surface leaves to a
/// frontend. Unix-only because the shell is.
#[cfg(unix)]
mod shell_invocation;

/// The build language one invocation speaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrontEnd {
    /// Ninja manifests, which is what every name but Make's selects.
    Ninja,
    /// Makefiles, evaluated by the vendored fork into the same graph.
    Make,
}

/// The program names that select Make mode, and nothing else does.
const MAKE_NAMES: [&str; 2] = ["make", "gmake"];

/// Whether a program name selects the Make front end.
///
/// The whole file name has to be one of the two, not merely start with it: a
/// `make.old` left behind by a package upgrade is not a request for Make mode.
/// The one exception is the executable suffix, which is part of the name on
/// Windows without being part of what the user typed.
pub(crate) fn is_make_name(name: &str) -> bool {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    MAKE_NAMES.contains(&name)
}

/// The program names that select the shell, and nothing else does.
///
/// One name, spelled as POSIX spells it. `dash` is deliberately absent: what
/// Ronin stands in for is the shell a build resolved, which is `/bin/sh`, and
/// answering to a second name would put Ronin in the way of a shell somebody
/// installed on purpose.
const SHELL_NAMES: [&str; 1] = ["sh"];

/// Whether a program name asks for the shell rather than for a build.
///
/// Read the same way [`is_make_name`] reads its own: the whole file name, so
/// an `sh.old` is not a shell, and the executable suffix does not count.
pub(crate) fn is_shell_name(name: &str) -> bool {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    SHELL_NAMES.contains(&name)
}

/// The file name an invocation arrived under, which is what selects everything.
fn invoked_name(program: &std::ffi::OsStr) -> String {
    Path::new(program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The front end `arguments` ask for.
///
/// The invoked program name decides it and nothing else does, so a path and a
/// symlink read the same and no option anywhere can change the answer. See the
/// module documentation for why there is no flag.
// [spec:ronin:req:product.make-identity]
pub(crate) fn select(arguments: &[BString]) -> FrontEnd {
    arguments.first().map_or(FrontEnd::Ninja, |program| {
        let program = program.to_os_str_lossy();
        if is_make_name(&invoked_name(std::ffi::OsStr::new(&*program))) {
            FrontEnd::Make
        } else {
            FrontEnd::Ninja
        }
    })
}

/// Runs Ronin as the shell, when the shell is the name it was invoked under.
///
/// Returns the status the shell left with, or `None` when this invocation is a
/// build and the caller should carry on. It is the process entry point's first
/// act, ahead of signal handlers and buffered streams, because a shell must
/// present the process it inherited: dash sets its own dispositions and writes
/// through the descriptors it was given, and a build's arrangements are
/// visible to every child if they are still standing.
///
/// The shell is a whole process rather than a call into the build, and that is
/// load-bearing rather than convenient. A shell reaps any child of the process
/// it runs in, which a scheduler holding its own children cannot allow; this
/// way it reaps its own and nobody else's. See
/// `plan/decisions/builtin-shell.md`.
///
/// This is the frontend nsh's own `nsh-cli` is, and it is a frontend's worth of
/// code rather than one call because nsh's surface is closed around a *typed*
/// request: the library never parses an argument vector and never ends a
/// process, so the invocation parse in [`shell_invocation`] and the
/// `std::process::exit` the caller makes of this return value are both a
/// frontend's to own. See that module for what is ported and what is declined.
// [spec:ronin:req:product.shell-identity]
#[must_use]
#[cfg(unix)]
pub fn run_as_shell(arguments: &[OsString]) -> Option<i32> {
    use std::io::{IsTerminal as _, Write as _};
    use std::os::unix::ffi::OsStrExt;

    if !is_shell_name(&invoked_name(arguments.first()?)) {
        return None;
    }
    // Rust's runtime does work between `_start` and `main` that C's does not,
    // and a shell sits close enough to the operating system that all of it
    // shows: SIGPIPE ignored, `/dev/null` opened over any closed standard
    // descriptor, a SIGSEGV handler on an alternate stack. Every one of them
    // is inherited or observable — a recipe's `cmd | head` wants SIGPIPE's
    // default disposition — so every one of them is undone here.
    //
    // Before the terminal questions below, and that ordering is load-bearing:
    // one of the three arrangements this undoes opens `/dev/null` over a closed
    // standard descriptor, and `/dev/null` is not a terminal but a closed
    // descriptor is not one either — asking first would answer about the
    // runtime's stand-in rather than about the process the shell inherited.
    nsh_platform::restore_shell_process_runtime_state();
    // The operating system's representation, kept intact: an argument need not
    // be valid UTF-8, and dash passes such bytes through untouched.
    let argv = arguments
        .iter()
        .map(|argument| argument.as_bytes().to_vec())
        .collect::<Vec<_>>();

    let stdin_is_terminal = std::io::stdin().is_terminal();
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let parsed = {
        let mut stdout = std::io::stdout().lock();
        shell_invocation::CommandLine::parse(
            &argv,
            stdin_is_terminal,
            stderr_is_terminal,
            |bytes| stdout.write_all(bytes),
        )
    };
    let invocation = match parsed {
        Ok(invocation) => invocation,
        // dash blames line 0 for a command line, there being no script line to
        // blame, and prefixes the spelling it was reached under. `sh: 0:
        // Illegal option -Q`, and 2, which is the shell's own error status.
        Err(shell_invocation::ParseError::Invocation(message)) => {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(argv.first().map_or(b"sh".as_slice(), Vec::as_slice));
            let _ = stderr.write_all(b": 0: ");
            let _ = stderr.write_all(&message);
            let _ = stderr.write_all(b"\n");
            return Some(2);
        }
        Err(shell_invocation::ParseError::Output) => return Some(1),
    };

    let parameters = invocation
        .parameters
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect::<Vec<&crate::util::BStr>>();
    let mut builder = nsh::Shell::builder()
        .invocation_name(invocation.invocation_name.as_ref())
        .argument_zero(invocation.argument_zero.as_ref())
        .args(&parameters)
        .inherit_env()
        // The frontend is the thing entitled to the process's standard
        // descriptors, so it hands them over rather than letting the shell
        // assume them; and it is the process's own shell, so it gets the host
        // that says so. A library shell defaults to refusing `exec` and
        // installing no signal handler, which is right for a library and wrong
        // for this.
        .streams(nsh::Streams::INHERIT)
        .host(nsh::ProcessHost);
    for option in nsh::ShellOption::ALL {
        builder = builder.shell_option(option, invocation.options.enabled(option));
    }
    let startup = invocation.startup();
    let mut shell = match builder.build() {
        Ok(shell) => shell,
        // A shell that could not be built has nothing to recover to, and the
        // status it refused with is the one to leave with.
        Err(error) => return Some(error.status().code().into()),
    };
    Some(shell.run_to_completion(startup).code().into())
}

/// Runs Ronin as the shell, which no build on this platform has.
///
/// Windows has no shell in the position Ronin would stand in — Ninja hands the
/// whole command line to `CreateProcess` — so every invocation here is a build.
#[must_use]
#[cfg(not(unix))]
pub fn run_as_shell(_arguments: &[OsString]) -> Option<i32> {
    None
}

/// Runs Ronin as its executable does.
///
/// This is the process entry point rather than the library one. It selects the
/// front end from the invoked program name, and Make mode reaches
/// `-C` by changing the process working directory, because Make evaluation
/// reads that directory directly. [`crate::Runner`] is the library path and never
/// moves it; see `[spec:ronin:req:runtime.explicit-invocation-boundary]`.
///
/// # Errors
///
/// Returns an [`Error`] when argument conversion, front-end evaluation, or
/// execution fails.
// [spec:ronin:req:product.make-identity]
pub fn run_process(
    arguments: &[OsString],
    output: &mut dyn std::io::Write,
    diagnostics: &mut dyn std::io::Write,
) -> Result<RunResult, Error> {
    let runner = crate::cli::process_runner()?;
    let arguments = crate::cli::byte_arguments(arguments)?;
    let result = match select(&arguments) {
        FrontEnd::Ninja => {
            crate::cli::run_bytes(&runner, &arguments, Some(output), Some(diagnostics))
        }
        #[cfg(all(unix, feature = "make"))]
        FrontEnd::Make => {
            crate::make::cli::run(&runner, &arguments, Some(output), Some(diagnostics))
        }
        // The `make` feature is on by default and off for a Ninja-only build,
        // where asking for Make mode is a mistake worth naming rather than a
        // manifest nobody wrote.
        #[cfg(not(all(unix, feature = "make")))]
        FrontEnd::Make => Ok(RunResult {
            stdout: Vec::new(),
            stderr: format!(
                "{}: this build has no Make front end; it was compiled without the \
                 'make' feature\n",
                crate::cli::PRODUCT_NAME
            )
            .into_bytes(),
            exit_code: 1,
        }),
    };
    result.map_err(Error::at_process_boundary)
}

#[cfg(test)]
mod tests {
    use super::{FrontEnd, invoked_name, is_shell_name, select};
    use crate::util::BString;

    fn selected(arguments: &[&str]) -> FrontEnd {
        let arguments = arguments
            .iter()
            .map(|argument| BString::from(*argument))
            .collect::<Vec<_>>();
        select(&arguments)
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn the_invoked_name_selects_the_front_end_through_a_path_or_a_symlink() {
        assert_eq!(selected(&["make"]), FrontEnd::Make);
        assert_eq!(selected(&["gmake"]), FrontEnd::Make);
        assert_eq!(selected(&["/usr/local/bin/make"]), FrontEnd::Make);
        assert_eq!(selected(&["make.exe"]), FrontEnd::Make);
        assert_eq!(selected(&["ronin"]), FrontEnd::Ninja);
        assert_eq!(selected(&["samu"]), FrontEnd::Ninja);
        assert_eq!(selected(&["./out/ninja"]), FrontEnd::Ninja);
        // Every other name is Ninja's, which is what Ronin is. `make.old` and
        // `cmake` are here because the old spelling compared the file *stem*
        // and so answered Make for both.
        assert_eq!(selected(&["build-tool"]), FrontEnd::Ninja);
        assert_eq!(selected(&["make.old"]), FrontEnd::Ninja);
        assert_eq!(selected(&["/usr/bin/gmake.dpkg-dist"]), FrontEnd::Ninja);
        assert_eq!(selected(&["cmake"]), FrontEnd::Ninja);
    }

    /// The name is the whole answer, so the options are just words to whichever
    /// front end it chose.
    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn the_name_decides_whatever_the_options_say() {
        assert_eq!(selected(&["make", "--ninja"]), FrontEnd::Make);
        assert_eq!(selected(&["ronin", "--make"]), FrontEnd::Ninja);
        assert_eq!(selected(&["gmake", "-j4", "all"]), FrontEnd::Make);
    }

    /// Read through a path or a symlink like Make's names, and just as narrow:
    /// the shell answers to what POSIX calls it and to nothing that merely
    /// looks like it.
    ///
    /// Nothing here calls [`super::run_as_shell`] with a shell's name, because
    /// a call that matched would replace this test process with a shell.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn the_shells_own_name_selects_it() {
        let name = |program| invoked_name(std::ffi::OsStr::new(program));
        assert!(is_shell_name(&name("sh")));
        assert!(is_shell_name(&name("/bin/sh")));
        assert!(is_shell_name(&name("/usr/bin/sh")));
        assert!(is_shell_name(&name("sh.exe")));
        // A shell somebody installed on purpose is not this one, and neither
        // is a name that merely ends in the shell's.
        assert!(!is_shell_name(&name("dash")));
        assert!(!is_shell_name(&name("bash")));
        assert!(!is_shell_name(&name("ssh")));
        assert!(!is_shell_name(&name("sh.old")));
        assert!(!is_shell_name(&name("ronin")));
        assert!(!is_shell_name(&name("make")));
    }

    /// The shell is not a front end, so an option cannot ask for it any more
    /// than an option can ask for Make mode.
    // [spec:ronin:req:product.shell-identity/test]
    #[test]
    fn no_option_reaches_the_shell() {
        use std::ffi::OsString;

        assert_eq!(super::run_as_shell(&[]), None);
        assert_eq!(
            super::run_as_shell(&[OsString::from("ronin"), OsString::from("--sh")]),
            None
        );
        assert_eq!(
            super::run_as_shell(&[OsString::from("ronin"), OsString::from("-c")]),
            None
        );
    }
}
