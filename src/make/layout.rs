//! Everything a compilation unit wraps a recipe in before it is launched.
//!
//! GNU Make runs a recipe where the Makefile that wrote it was read and under
//! the environment that Makefile exports, and it decides per command line
//! whether a shell is needed at all. Ronin's executor runs the whole graph
//! from one directory with one environment, so what Make expresses by being
//! somewhere else has to travel with the command: as a `cd` and an `env` in
//! front of a shell command line, and as values beside an argument list where
//! there is no shell to read them.
//!
//! The graph sink builds a command line here as it declares each rule, and a
//! recipe expanded later — when its edge is launched — is wrapped by the same
//! value, so the two paths cannot drift into two different wrappers.

use crate::util::BString;
use kati::bytes::Bytes;
use kati::strutil::escape_shell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Everything one compilation unit wraps a recipe in.
#[derive(Clone, Default)]
pub(crate) struct CommandLayout {
    pub(super) command_directory: PathBuf,
    pub(super) recipe_environment: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    pub(super) root_directory: PathBuf,
    /// The root unit, whose auxiliary paths are already the ones the build
    /// reads: a child's are qualified by where its Makefile was read.
    pub(super) root: bool,
}

impl CommandLayout {
    /// A layout for one compilation unit.
    #[cfg(test)]
    pub(crate) const fn new(
        command_directory: PathBuf,
        recipe_environment: Vec<(Vec<u8>, Option<Vec<u8>>)>,
        root_directory: PathBuf,
        root: bool,
    ) -> Self {
        Self {
            command_directory,
            recipe_environment,
            root_directory,
            root,
        }
    }

    fn push_shell_word(command: &mut Vec<u8>, word: &[u8]) {
        command.push(b'\'');
        for byte in word {
            if *byte == b'\'' {
                command.extend_from_slice(b"'\\''");
            } else {
                command.push(*byte);
            }
        }
        command.push(b'\'');
    }

    /// The `cd` and `env` that put a command where Make would have run it.
    ///
    /// `scoped` is what one target's own `export` changes about the unit's
    /// answer. It is applied after the unit's, and `env` reads its arguments
    /// in order, so the target's word is the last one on any name both name.
    ///
    /// `exec` sits between the two because that is the last point where the
    /// launching shell still has something of its own to do. `cd` is the
    /// shell's own builtin and has to run in the shell that is about to be
    /// replaced; `env` is a program, and one that replaces itself with what it
    /// was given, so everything from there on is a single process. What that
    /// buys is in [`Self::launch`].
    pub(super) fn prefix(&self, scoped: &[kati::export::EnvironmentChange]) -> Vec<u8> {
        let mut command = Vec::new();
        if !self.command_directory.as_os_str().is_empty() {
            command.extend_from_slice(b"cd ");
            Self::push_shell_word(&mut command, self.command_directory.as_os_str().as_bytes());
            command.extend_from_slice(b" && ");
        }
        command.extend_from_slice(b"exec ");
        let unit = self
            .recipe_environment
            .iter()
            .map(|(name, value)| {
                (
                    Bytes::copy_from_slice(name),
                    value.as_ref().map(|value| Bytes::copy_from_slice(value)),
                )
            })
            .chain(scoped.iter().cloned())
            .collect::<Vec<_>>();
        command.extend_from_slice(&kati::export::environment_prefix(&unit));
        command
    }

    /// Where this unit writes the response file for the edge producing
    /// `output`, which is per edge because the output is.
    fn response_file(&self, output: &[u8]) -> Vec<u8> {
        let mut path = Vec::new();
        if !self.root && !self.root_directory.as_os_str().is_empty() {
            path.extend_from_slice(self.root_directory.as_os_str().as_bytes());
            path.extend_from_slice(std::path::MAIN_SEPARATOR_STR.as_bytes());
        }
        path.extend_from_slice(output);
        path.extend_from_slice(b".rsp");
        path
    }

    /// The command line that runs `script`, and the response file it needs.
    ///
    /// The same choice the sink makes while emitting: a script short enough to
    /// pass as an argument is quoted into one, and one too long reaches the
    /// shell as a file instead.
    ///
    /// Either way the command runs in place of the shell that launched it,
    /// which is what the `exec` from [`Self::prefix`] is for and the only
    /// reason it is there. GNU Make waits on the recipe's own shell, so a
    /// recipe a signal killed is a signalled child and Make can see that it
    /// was; a launcher that stayed alive to report the death would report
    /// `128 + signal` and exit normally, which is what a recipe that ran
    /// `exit 143` looks like too. Replacing the launcher leaves one process
    /// where Make has one, and the two answers stay apart.
    pub(crate) fn launch(
        &self,
        shell: &[u8],
        shell_flags: &[u8],
        script: &[u8],
        output: &[u8],
        scoped: &[kati::export::EnvironmentChange],
    ) -> LaunchedScript {
        let mut command = self.prefix(scoped);
        command.extend_from_slice(shell);
        command.push(b' ');
        if script.len() > RESPONSE_FILE_THRESHOLD {
            let path = self.response_file(output);
            match crate::graph::shell_escape_path(&path) {
                Some(quoted) => command.extend_from_slice(&quoted),
                None => command.extend_from_slice(&path),
            }
            return LaunchedScript {
                command,
                response_file: Some((path, script.to_vec())),
            };
        }
        command.extend_from_slice(shell_flags);
        command.extend_from_slice(b" \"");
        command.extend_from_slice(&escape_shell(&Bytes::copy_from_slice(script)));
        command.push(b'"');
        LaunchedScript {
            command,
            response_file: None,
        }
    }

    /// The launch that runs one recipe line.
    ///
    /// GNU Make asks `construct_command_argv` per line and gets one of two
    /// answers back: an argument list to exec itself, or nothing, meaning the
    /// line is the shell's errand. kati has already asked; this puts the
    /// answer where Ronin can act on it.
    ///
    /// The shell-free answer takes the unit's directory and environment as
    /// values rather than as a `cd` and an `env` in front of a command line,
    /// because there is no shell here to read either — which is the whole
    /// point of it, and why a program that is not there is reported against
    /// its own name.
    pub(crate) fn launch_step(
        &self,
        step: &kati::ninja::RecipeStep,
        scoped: &[kati::export::EnvironmentChange],
    ) -> crate::subprocess::Launch {
        if let Some(argv) = &step.direct {
            return self.direct_launch(
                argv.iter()
                    .map(|word| BString::from(word.to_vec()))
                    .collect(),
                scoped,
            );
        }
        // The shell is a program, not a word in a command line, whenever it is
        // the default one — which is exactly when Ronin has a shell of its own
        // to put there. `cd` and `env` in front of a command line say the same
        // thing a launch says with a directory and an environment, and a
        // command line cannot say the one thing that matters here: that the
        // program is this executable while `argv[0]` stays `/bin/sh`.
        //
        // An empty `.SHELLFLAGS` is why the flags are words rather than one:
        // the command line spelled `sh  "script"`, which a shell splits into
        // `sh` and a file operand, so an empty word must vanish here too
        // rather than become an empty argument. A `.SHELLFLAGS` of several
        // words splits for the same reason.
        if step.shell == kati::simple_command::DEFAULT_SHELL {
            let mut argv = vec![BString::from(step.shell.to_vec())];
            argv.extend(
                step.shell_flags
                    .split(u8::is_ascii_whitespace)
                    .filter(|word| !word.is_empty())
                    .map(|word| BString::from(word.to_vec())),
            );
            argv.push(BString::from(step.text.to_vec()));
            return self.direct_launch(argv, scoped);
        }
        let mut command = self.prefix(scoped);
        command.extend_from_slice(&step.shell);
        command.push(b' ');
        command.extend_from_slice(&step.shell_flags);
        command.extend_from_slice(b" \"");
        command.extend_from_slice(&escape_shell(&step.text));
        command.push(b'"');
        crate::subprocess::Launch::Shell(BString::from(command))
    }

    /// A launch of `argv` with the unit's directory and environment as values.
    ///
    /// What a `cd` and an `env` in front of a command line would have said,
    /// said to the spawn instead — which is the only form that can name a
    /// program and its `argv[0]` separately.
    fn direct_launch(
        &self,
        argv: Vec<BString>,
        scoped: &[kati::export::EnvironmentChange],
    ) -> crate::subprocess::Launch {
        crate::subprocess::Launch::Direct(Box::new(crate::subprocess::DirectLaunch {
            argv,
            directory: self.command_directory.clone(),
            environment: self
                .recipe_environment
                .iter()
                .map(|(name, value)| (name.as_slice(), value.as_deref()))
                .chain(
                    scoped
                        .iter()
                        .map(|(name, value)| (name.as_ref(), value.as_deref())),
                )
                .map(|(name, value)| {
                    (
                        OsStr::from_bytes(name).to_owned(),
                        value.map(|value| OsStr::from_bytes(value).to_owned()),
                    )
                })
                .collect(),
            diagnostic_prefix: format!("{}: ", crate::cli::PRODUCT_NAME),
        }))
    }

    /// Whether every line of this recipe can be launched on its own.
    ///
    /// A line long enough to need a response file cannot: the file is named
    /// per edge, so several of them would want the same name. Such a recipe
    /// keeps the assembled script and the single launch that goes with it,
    /// which is what it had before there were several.
    pub(crate) fn launches_line_by_line(steps: &[kati::ninja::RecipeStep]) -> bool {
        !steps.is_empty()
            && steps
                .iter()
                .all(|step| step.text.len() <= RESPONSE_FILE_THRESHOLD)
    }
}

/// A command line, and the response file it needs when the script was too long
/// to be an argument.
pub(crate) struct LaunchedScript {
    pub(crate) command: Vec<u8>,
    pub(crate) response_file: Option<(Vec<u8>, Vec<u8>)>,
}

/// How long a script has to be before it reaches the shell as a file rather
/// than as an argument. kati's own threshold, kept in step with it.
const RESPONSE_FILE_THRESHOLD: usize = 100 * 1000;
