//! Which build language an invocation speaks.
//!
//! Ronin has two front ends over one engine, and the name the executable was
//! invoked under says which of them an invocation wanted: `make` and `gmake`
//! select Make mode, `ninja`, `samu`, and `ronin` select Ninja mode. The name
//! is an explicit statement of intent in a way that sniffing the directory for
//! a `Makefile` or a `build.ninja` is not, and it is how every multi-call
//! binary has ever done this.
//!
//! A name is not the only way in. `--make` and `--ninja` select a front end
//! outright, in both directions, so neither mode is reachable only through a
//! symlink somebody remembered to install. `--make` is also what makes
//! recursion work: `$(MAKE)` names this executable by its real path, which is
//! `ronin`, and a name that selects Ninja mode has to be overridden for the
//! sub-make to speak Make.
//!
//! See `plan/decisions/multicall-identity.md`.
// [spec:ronin:req:product.make-identity]

use crate::cli::RunResult;
use crate::util::{BString, ByteSlice};
use crate::Error;
use std::ffi::OsString;
use std::path::Path;

/// The build language one invocation speaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrontEnd {
    /// Ninja manifests, which is what every name but Make's selects.
    Ninja,
    /// Makefiles, evaluated by the vendored fork into the same graph.
    Make,
}

/// The program names that select Make mode.
const MAKE_NAMES: [&str; 2] = ["make", "gmake"];

/// Select Make mode whatever the executable is called.
const MAKE_OPTION: &[u8] = b"--make";

/// Select Ninja mode whatever the executable is called.
const NINJA_OPTION: &[u8] = b"--ninja";

/// The front end `arguments` ask for.
///
/// The file stem of the program name decides it, so a path and a symlink read
/// the same, and an explicit `--make` or `--ninja` anywhere in the options
/// overrides that. Both flags are ordinary options to whichever front end then
/// runs, which is what lets the last one win rather than the first.
// [spec:ronin:req:product.make-identity]
/// Whether a program name selects the Make front end.
pub(crate) fn is_make_name(stem: &str) -> bool {
    MAKE_NAMES.contains(&stem)
}

pub(crate) fn select(arguments: &[BString]) -> FrontEnd {
    let invoked = arguments.first().map_or(FrontEnd::Ninja, |program| {
        let program = program.to_os_str_lossy();
        let stem = Path::new(&*program)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if is_make_name(&stem) {
            FrontEnd::Make
        } else {
            FrontEnd::Ninja
        }
    });
    arguments
        .iter()
        .skip(1)
        // Ninja and Make both end their options at a bare `--`; a front end
        // named after it is a target, not a request.
        .take_while(|argument| argument.as_bytes() != b"--")
        .fold(invoked, |selected, argument| match argument.as_bytes() {
            MAKE_OPTION => FrontEnd::Make,
            NINJA_OPTION => FrontEnd::Ninja,
            _ => selected,
        })
}

/// Whether `argument` is one of the two front-end selectors.
///
/// Both front ends have already been told which one was chosen by the time
/// they read their own command line, so both accept and ignore the flag.
pub(crate) fn is_selector(argument: &[u8]) -> bool {
    argument == MAKE_OPTION || argument == NINJA_OPTION
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
    match select(&arguments) {
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
    }
}

#[cfg(test)]
mod tests {
    use super::{select, FrontEnd};
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
        assert_eq!(selected(&["ronin"]), FrontEnd::Ninja);
        assert_eq!(selected(&["samu"]), FrontEnd::Ninja);
        assert_eq!(selected(&["./out/ninja"]), FrontEnd::Ninja);
        // A name nobody claimed is Ninja's, which is what Ronin is.
        assert_eq!(selected(&["build-tool"]), FrontEnd::Ninja);
    }

    // [spec:ronin:req:product.make-identity/test]
    #[test]
    fn either_front_end_is_reachable_from_the_command_line() {
        assert_eq!(selected(&["ronin", "--make"]), FrontEnd::Make);
        assert_eq!(selected(&["make", "--ninja"]), FrontEnd::Ninja);
        assert_eq!(selected(&["ronin", "--make", "--ninja"]), FrontEnd::Ninja);
        assert_eq!(selected(&["make", "--ninja", "--make"]), FrontEnd::Make);
        // Past `--` the word is a target both front ends will look up.
        assert_eq!(selected(&["ronin", "--", "--make"]), FrontEnd::Ninja);
    }
}
