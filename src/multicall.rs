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
//! See `plan/decisions/multicall-identity.md`.
// [spec:ronin:req:product.make-identity]

use crate::Error;
use crate::cli::RunResult;
use crate::util::{BString, ByteSlice};
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

/// The front end `arguments` ask for.
///
/// The invoked program name decides it and nothing else does, so a path and a
/// symlink read the same and no option anywhere can change the answer. See the
/// module documentation for why there is no flag.
// [spec:ronin:req:product.make-identity]
pub(crate) fn select(arguments: &[BString]) -> FrontEnd {
    arguments.first().map_or(FrontEnd::Ninja, |program| {
        let program = program.to_os_str_lossy();
        let name = Path::new(&*program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if is_make_name(&name) {
            FrontEnd::Make
        } else {
            FrontEnd::Ninja
        }
    })
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
    use super::{FrontEnd, select};
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
}
