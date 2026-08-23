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

    /// What a recipe of this unit would carry under `name`, the target's own
    /// `export` having the last word over the unit's.
    fn held(&self, name: &[u8], scoped: &[kati::export::EnvironmentChange]) -> Option<Vec<u8>> {
        scoped
            .iter()
            .rev()
            .find(|(candidate, _)| candidate.as_ref() == name)
            .map(|(_, value)| value.as_ref().map(|value| value.to_vec()))
            .or_else(|| {
                self.recipe_environment
                    .iter()
                    .rev()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, value)| value.clone())
            })
            .flatten()
    }

    /// This unit's `MAKEFLAGS` and `MFLAGS` as the makefile update hands them
    /// to a recipe's child.
    ///
    /// Appended to what the recipe already carries rather than replacing it,
    /// because `env` reads its arguments in order and the last word on a name
    /// is the one that stands — the same thing that lets a target's own
    /// `export` win over its unit's. Empty when neither value would move, so a
    /// recipe with nothing to hide is launched exactly as it always was.
    pub(crate) fn while_remaking_makefiles(
        &self,
        scoped: &[kati::export::EnvironmentChange],
    ) -> Vec<kati::export::EnvironmentChange> {
        let mut changes = Vec::new();
        if let Some(value) = self.held(b"MAKEFLAGS", scoped) {
            let remade = makeflags_while_remaking(&value);
            if remade != value {
                changes.push((
                    Bytes::copy_from_slice(b"MAKEFLAGS"),
                    Some(Bytes::from(remade)),
                ));
            }
        }
        if let Some(value) = self.held(b"MFLAGS", scoped) {
            let remade = mflags_while_remaking(&value);
            if remade != value {
                changes.push((Bytes::copy_from_slice(b"MFLAGS"), Some(Bytes::from(remade))));
            }
        }
        changes
    }

    /// Where this unit writes the response file for the edge producing
    /// `output`, which is per edge because the output is.
    pub(super) fn response_file(&self, output: &[u8]) -> Vec<u8> {
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
            // The flags come too, less the letter that says the next word is
            // the command: here the next word is a file name, and everything
            // else the flags say is about the shell rather than about how the
            // script reached it. Dropping them was how a `.POSIX:` recipe lost
            // its `-e` on crossing the threshold.
            let flags = kati::ninja::script_file_flags(shell_flags);
            if !flags.is_empty() {
                command.extend_from_slice(&flags);
                command.push(b' ');
            }
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

    /// The launch that runs a whole assembled script.
    ///
    /// The same substitution [`Self::launch_step`] makes for one recipe line,
    /// for a recipe that could not be handed over as its lines. The command
    /// line built by [`Self::launch`] cannot make it: its own work is to `exec
    /// env`, and `env` execs the shell spelled inside it, which is the
    /// machine's however the line itself was launched.
    ///
    /// `None` where the recipe names a shell of its own — the substitution
    /// boundary [`Self::launch_step`] draws in the same place. There is no
    /// executable to stand in for a shell that is not the default one, so the
    /// composed command line runs it exactly as it always did.
    pub(crate) fn launch_script(
        &self,
        shell: &[u8],
        shell_flags: &[u8],
        script: Script<'_>,
        scoped: &[kati::export::EnvironmentChange],
    ) -> Option<crate::subprocess::Launch> {
        if shell != kati::simple_command::DEFAULT_SHELL {
            return None;
        }
        let mut argv = vec![BString::from(shell.to_vec())];
        match script {
            // What the command line spells as `<shell> <flags> "<script>"`.
            Script::Argument(text) => {
                argv.extend(Self::shell_flag_words(shell_flags));
                argv.push(BString::from(text.to_vec()));
            }
            // And what it spells as `<shell> <flags> <path>`: a script the
            // shell reads out of a file is a file operand, so the letter that
            // would have taken it for the command comes off — and every other
            // letter stays, because they are about the shell rather than about
            // how the script reached it. A `.POSIX:` recipe that crossed the
            // length threshold used to lose its `-e` right here.
            Script::File(path) => {
                argv.extend(Self::shell_flag_words(&kati::ninja::script_file_flags(
                    shell_flags,
                )));
                argv.push(BString::from(path.to_vec()));
            }
        }
        Some(self.direct_launch(argv, scoped))
    }

    /// `.SHELLFLAGS` as the words a launch passes.
    ///
    /// An empty `.SHELLFLAGS` is why they are words rather than one: the
    /// command line spelled `sh  "script"`, which a shell splits into `sh` and
    /// a file operand, so an empty word must vanish here too rather than become
    /// an empty argument. A `.SHELLFLAGS` of several words splits for the same
    /// reason.
    fn shell_flag_words(shell_flags: &[u8]) -> impl Iterator<Item = BString> {
        shell_flags
            .split(u8::is_ascii_whitespace)
            .filter(|word| !word.is_empty())
            .map(|word| BString::from(word.to_vec()))
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
        // A `.ONESHELL` step's shell is a path and not a command line — see
        // [`kati::ninja::RecipeStep::shell_is_a_path`] — so it is named as the
        // program whatever is in it, and a value of more than one word is a
        // program of more than one word that nothing can start. That is the
        // same launch the default shell gets, and asking about the default
        // first is what keeps the builtin substitution: `/bin/sh` under
        // `.ONESHELL` is still `/bin/sh`.
        if step.shell == kati::simple_command::DEFAULT_SHELL || step.shell_is_a_path {
            let mut argv = vec![BString::from(step.shell.to_vec())];
            // A `.ONESHELL` launch reads its flags the way GNU Make's
            // one-shell branch does, with the shell's own tokenizer, so a
            // quoted flag with a space in it stays one word. Every other
            // launch splits on whitespace because the flags it carries were
            // going to be words on a command line either way.
            if step.shell_is_a_path {
                argv.extend(
                    kati::simple_command::shell_flag_argv(&step.shell_flags)
                        .into_iter()
                        .map(|word| BString::from(word.to_vec())),
                );
            } else {
                argv.extend(Self::shell_flag_words(&step.shell_flags));
            }
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

/// A whole assembled script one rule runs, waiting for the edge that runs it.
///
/// A recipe handed over as its command lines needs nothing from the edge: each
/// line is a finished launch by the time the rule is declared. One handed over
/// whole may be read out of a response file instead, and that file is named
/// after the edge's output, so the launch cannot be finished until there is an
/// edge to name it after.
///
/// The layout travels with it because the unit that declared the rule is not
/// the current one by the time a recursive recipe's segments reach their edges:
/// the children have been compiled in between, each with a layout of its own.
pub(crate) struct SettledScript {
    layout: CommandLayout,
    shell: Vec<u8>,
    shell_flags: Vec<u8>,
    /// The script itself, or `None` where it is the response file's content and
    /// the shell reads it from there.
    script: Option<Vec<u8>>,
    /// What the target's own scope changes about the environment, over what the
    /// layout already carries for the unit.
    scoped: Vec<kati::export::EnvironmentChange>,
    ignore_errors: bool,
}

impl SettledScript {
    /// One rule's assembled script, held until there is an edge to launch it
    /// for.
    pub(crate) fn held(
        layout: CommandLayout,
        rule: &kati::build_sink::SinkRule<'_>,
        command: kati::build_sink::SinkCommand<'_>,
        ignore_errors: bool,
    ) -> Self {
        Self {
            layout,
            shell: rule.shell.to_vec(),
            shell_flags: rule.shell_flags.to_vec(),
            script: match command {
                kati::build_sink::SinkCommand::Inline(script) => Some(script.to_vec()),
                kati::build_sink::SinkCommand::ResponseFile(_) => None,
            },
            scoped: rule.recipe_environment.to_vec(),
            ignore_errors,
        }
    }

    /// The launch, once `output` says what the edge's response file is called.
    ///
    /// `None` for a recipe naming a shell of its own, which the composed
    /// command line runs exactly as it always did.
    pub(crate) fn launch(&self, output: &[u8]) -> Option<SettledSteps> {
        let ordinary = self.launched(output, &self.scoped)?;
        let remaking = self.layout.while_remaking_makefiles(&self.scoped);
        if remaking.is_empty() {
            return Some(SettledSteps::same(vec![ordinary]));
        }
        let mut scoped = self.scoped.clone();
        scoped.extend(remaking);
        Some(SettledSteps {
            ordinary: vec![ordinary],
            while_remaking: self.launched(output, &scoped).map(|step| vec![step]),
        })
    }

    /// The launch this script becomes under one environment.
    fn launched(
        &self,
        output: &[u8],
        scoped: &[kati::export::EnvironmentChange],
    ) -> Option<crate::build::LateStep> {
        // A rule holding no script of its own is one whose script was written
        // out for the shell to read, and this edge's output is what that file
        // is named after.
        let response_file = if self.script.is_some() {
            Vec::new()
        } else {
            self.layout.response_file(output)
        };
        let script = self
            .script
            .as_deref()
            .map_or(Script::File(&response_file), Script::Argument);
        self.layout
            .launch_script(&self.shell, &self.shell_flags, script, scoped)
            .map(|launch| crate::build::LateStep {
                launch,
                ignore_errors: self.ignore_errors,
                // What the command line this replaces was given, unchanged. A
                // recipe of several lines has no single answer — GNU Make runs
                // the marked lines of one and skips the rest, and a script
                // assembled into one process can do neither — so naming the
                // program says nothing new about it.
                runs_while_pretending: false,
            })
    }
}

/// One recipe's launches, in the shapes GNU Make's two phases need.
///
/// A recipe the compiler had to read for itself has its `MAKEFLAGS` written
/// into the command line by the time the graph is built, and which value
/// belongs there is not a fact about the edge: the makefile update hands a
/// `$(MAKE)` a value without the pretending switches and the goals hand it one
/// with them, over the same graph and the same edge. So both are built where
/// the environment is still in hand, and the pass that runs chooses.
#[derive(Clone)]
pub(crate) struct SettledSteps {
    ordinary: Vec<crate::build::LateStep>,
    /// The same launches without the pretending switches, and nothing when the
    /// two would be the same command — which is every invocation carrying none
    /// of `-n`, `-t` and `-q`, so the ordinary run pays nothing for this.
    while_remaking: Option<Vec<crate::build::LateStep>>,
}

impl SettledSteps {
    /// Launches the two phases share, because nothing in them would move.
    pub(crate) const fn same(steps: Vec<crate::build::LateStep>) -> Self {
        Self {
            ordinary: steps,
            while_remaking: None,
        }
    }

    /// One recipe's launches under each of the two environments.
    pub(crate) const fn split(
        ordinary: Vec<crate::build::LateStep>,
        while_remaking: Vec<crate::build::LateStep>,
    ) -> Self {
        Self {
            ordinary,
            while_remaking: Some(while_remaking),
        }
    }

    /// The launches for the phase now running.
    pub(crate) fn during(&self, remaking_makefiles: bool) -> &[crate::build::LateStep] {
        if remaking_makefiles && let Some(steps) = &self.while_remaking {
            return steps;
        }
        &self.ordinary
    }
}

/// How a shell is given the script it is to run.
///
/// The two ways [`CommandLayout::launch`] writes it into a command line, said
/// to a launch instead.
#[derive(Clone, Copy)]
pub(crate) enum Script<'a> {
    /// Passed as an argument, which is what a script short enough to be one
    /// is.
    Argument(&'a [u8]),
    /// Read out of this file, which is where a script too long to be an
    /// argument was written.
    File(&'a [u8]),
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

/// The switches GNU Make keeps out of what the makefile update hands a
/// recipe's child.
///
/// These three and nothing else: `n`, `q` and `t` are the only entries in GNU
/// Make's switch table carrying `no_makefile`, and `define_makeflags (1)` —
/// called once ahead of the update and undone by `define_makeflags (0)` after
/// it (main.c) — is what leaves them out. The reason is the update's own: a
/// Makefile only pretended to be remade is one whose contents the read would
/// then have to guess, and a child pretending on the update's behalf leaves
/// exactly that behind.
const NOT_WHILE_REMAKING: &[u8] = b"nqt";

/// `MAKEFLAGS` without the pretending switches.
///
/// The value opens with the group of single-letter switches — empty when there
/// are none, in which case what follows is preceded by a space — so taking a
/// letter out is taking it out of the first word and leaving the rest alone.
/// Everything else the invocation propagates it still propagates: `-j`, `-k`,
/// `-B`, the long options and the `--` assignments reach the child exactly as
/// they would have.
fn makeflags_while_remaking(value: &[u8]) -> Vec<u8> {
    let end = value
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(value.len());
    let (group, rest) = value.split_at(end);
    let mut kept = group
        .iter()
        .copied()
        .filter(|letter| !NOT_WHILE_REMAKING.contains(letter))
        .collect::<Vec<_>>();
    kept.extend_from_slice(rest);
    kept
}

/// `MFLAGS` without the pretending switches.
///
/// The same switches under GNU Make's other spelling: the letter group carries
/// a leading `-`, the `--` assignments are not there at all, and a group that
/// empties takes its whole word with it — `-t -j3` becomes `-j3` rather than
/// `- -j3`, and `-t` alone becomes nothing.
fn mflags_while_remaking(value: &[u8]) -> Vec<u8> {
    let Some(after) = value.strip_prefix(b"-") else {
        return value.to_vec();
    };
    let end = after
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(after.len());
    let (group, rest) = after.split_at(end);
    // A word carrying anything but letters is a switch with an argument, which
    // the group never holds and which nothing here takes out.
    if !group.iter().all(u8::is_ascii_alphabetic) {
        return value.to_vec();
    }
    let kept = group
        .iter()
        .copied()
        .filter(|letter| !NOT_WHILE_REMAKING.contains(letter))
        .collect::<Vec<_>>();
    if kept.is_empty() {
        return rest.strip_prefix(b" ").unwrap_or(rest).to_vec();
    }
    let mut remade = vec![b'-'];
    remade.extend_from_slice(&kept);
    remade.extend_from_slice(rest);
    remade
}
