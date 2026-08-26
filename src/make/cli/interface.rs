//! Inputs shared by Make's command-line parser and kati compilation.

use super::jobserver_style::{carried_switches, unknown_jobserver_style};
use super::{Action, Invocation, JobLimit, Switch, parse_arguments, refuse};
use crate::Error;
use crate::build::BuildOptions;
use crate::cli::{PRODUCT_NAME, RunResult};
use crate::error::CliError;
use crate::make::Shuffle;
use crate::make::report::ABANDONED;
use crate::util::{BString, ByteSlice, diagnostic, terminated};
use kati::bytes::Bytes;
use kati::diagnostics::Diagnostics;
use kati::session::Session;
use std::sync::Arc;

/// Which of Make's option streams one word came from.
///
/// GNU Make decodes the command line and the inherited `MAKEFLAGS` at
/// `o_command`, and a makefile's own `MAKEFLAGS` at `o_env` once the read is
/// over. Only the command-line origin dies for a word it cannot read —
/// `if (bad && origin == o_command) print_usage (bad)` in main.c
/// `decode_switches`, with `opterr` set the same way so nothing is printed
/// either. A switch a newer or older Make wrote into a makefile is skipped and
/// the build goes on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ArgumentSource {
    Inherited,
    CommandLine,
    Makefile,
    /// The command line and inherited environment read a second time, over a
    /// Makefile's write to `MAKEFLAGS`, so that a switch typed on the command
    /// line outranks one the Makefile took away.
    Protection,
}

/// What a `--shuffle` reaching Make through one stream does.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ShuffleEffect {
    /// Names the order this run builds in, so the word has to name a mode and
    /// what travels onward is the permutation it settled on.
    Settles,
    /// Lands in the switch table and nothing looks at it: republished exactly
    /// as written, reordering nothing.
    Republishes,
    /// Passes over it. The stream is a re-assertion of switches that act, and
    /// by this point the shuffle is not one of them.
    Ignored,
}

impl ArgumentSource {
    /// Whether a switch this stream got wrong ends the run.
    ///
    /// GNU Make's `decode_switches` has one `bad` flag for every way a word can
    /// be wrong — a switch it does not know, and an empty argument to one it
    /// does — and one place it answers for them, so both questions are this one.
    pub(super) const fn refuses_a_bad_switch(self) -> bool {
        matches!(self, Self::Inherited | Self::CommandLine)
    }

    /// What a `--shuffle` from this stream does.
    ///
    /// GNU Make settles the mode once, in `main`, after the command line and
    /// the inherited environment and before the first makefile is read. A
    /// makefile's own write to `MAKEFLAGS` is decoded long after that block and
    /// never reaches it again, so the word is stored, republished and otherwise
    /// unexamined — which is why a value naming no mode is not an error there.
    /// Nothing re-applies the command line afterwards, so the makefile's word
    /// is the one the table ends up holding.
    pub(super) const fn shuffle_effect(self) -> ShuffleEffect {
        match self {
            Self::Inherited | Self::CommandLine => ShuffleEffect::Settles,
            Self::Makefile => ShuffleEffect::Republishes,
            Self::Protection => ShuffleEffect::Ignored,
        }
    }
}

/// Read `--shuffle`'s argument, which the streams answer differently.
///
/// Answers with the refusal, when there is one. See
/// [`ArgumentSource::shuffle_effect`] for why only one stream can produce one.
pub(super) fn read_shuffle(
    invocation: &mut Invocation,
    source: ArgumentSource,
    spec: &[u8],
) -> Option<Action> {
    // Before the mode is looked at, and at every origin, because GNU Make's
    // empty-string check is `decode_switches`' own and runs before the value
    // reaches anything that could judge it. `--shuffle` with no `=` arrives
    // here as `random` and never sees this.
    if !invocation.non_empty(source, "--shuffle", spec) {
        return None;
    }
    match source.shuffle_effect() {
        ShuffleEffect::Settles => {
            let Some(mode) = Shuffle::requested(spec) else {
                return Some(refuse(format_args!(
                    "invalid shuffle mode: Invalid value: '{}'",
                    spec.to_str_lossy()
                )));
            };
            invocation.shuffle = mode;
            // The permutation rather than the word that asked for one, so a
            // child reproduces the order this run used.
            invocation.shuffle_spelling = mode.spelling();
        }
        // An empty argument leaves the table entry empty, and a switch with an
        // empty string is one `define_makeflags` writes nothing for.
        ShuffleEffect::Republishes => {
            invocation.shuffle_spelling =
                (!spec.is_empty()).then(|| spec.to_str_lossy().into_owned());
        }
        ShuffleEffect::Ignored => {}
    }
    None
}

/// The Make interface variables one compiler invocation presents to source.
pub(super) struct CompilerFlagVariables {
    /// The switch table alone: what a Makefile write is settled against, and
    /// what MFLAGS is spelled from. It holds no `--eval` fragment, for the
    /// reason [`compiler_flag_variables`] gives.
    pub(super) base: String,
    /// This invocation's `--eval` fragments, quoted as `MAKEFLAGS` carries
    /// them and joined by one space, or empty where there are none.
    ///
    /// `MAKEFLAGS` names them rather than containing them, so this is what the
    /// name resolves to. See [`compiler_flag_variables`] for why they are not
    /// in the switch table.
    pub(super) eval_flags: String,
    /// The expanded value inherited by a semantic subninja.
    pub(super) makeflags: String,
    /// The same options in command-line spelling, without assignments.
    pub(super) mflags: String,
    /// Escaped command-line assignments, in Make's variable-table order.
    pub(super) overrides: String,
}

/// The variable name an assignment binds, excluding its operator.
fn command_variable_name(assignment: &str) -> Option<&str> {
    let (before_equals, _) = assignment.split_once('=')?;
    for operator in [":::", "::", ":", "+", "?", "!"] {
        if let Some(name) = before_equals.strip_suffix(operator) {
            return Some(name);
        }
    }
    Some(before_equals)
}

/// Quote one word so `MAKEFLAGS` carries it back as the single word it was.
///
/// GNU Make's `quote_for_env`: a `$` is doubled so reading the variable does
/// not expand it, and a backslash or a blank is backslash-escaped so word
/// splitting does not end the word there. It serves command-line assignments
/// and `--eval` fragments alike, because both are arbitrary text that has to
/// survive being read back as a command line.
///
/// One byte is treated differently from GNU Make's, and deliberately: GNU
/// escapes only space and tab, leaving a newline raw, because its own reader
/// splits on blanks alone and a raw newline therefore stays inside the word.
/// [`makeflags_words`] splits on any ASCII whitespace, so escaping the newline
/// here is what keeps the same word whole on the way back. The two choices are
/// one choice — change either and the round trip breaks.
fn quote_for_makeflags(word: &str) -> String {
    let mut quoted = String::with_capacity(word.len());
    for character in word.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '$' => quoted.push_str("$$"),
            character if character.is_ascii_whitespace() => {
                quoted.push('\\');
                quoted.push(character);
            }
            character => quoted.push(character),
        }
    }
    quoted
}

/// Keep the last value for each command-line name and publish the table in the
/// reverse of first-introduction order, as GNU Make 4.4.1 does.
fn command_overrides(invocation: &Invocation) -> String {
    let mut assignments: Vec<(&str, &str)> = Vec::new();
    for assignment in &invocation.variables {
        let Some(assignment) = assignment.to_str().ok() else {
            continue;
        };
        let Some(name) = command_variable_name(assignment) else {
            continue;
        };
        if let Some((_, value)) = assignments
            .iter_mut()
            .find(|(candidate, _)| *candidate == name)
        {
            *value = assignment;
        } else {
            assignments.push((name, assignment));
        }
    }
    assignments
        .into_iter()
        .rev()
        .map(|(_, assignment)| quote_for_makeflags(assignment))
        .collect::<Vec<_>>()
        .join(" ")
}

/// How a Make invocation describes itself to this compilation unit, to every
/// semantic child, and to whatever an invocation nothing could compose starts
/// for itself — the last of those reads these switches out of the environment,
/// which is the only way they reach it.
///
/// Job and load limits remain compiler-visible because Makefiles branch on
/// them, while execution still has one Ninja scheduler. The jobserver
/// authorization itself is deliberately absent: Ronin stands up no GNU Make
/// jobserver at any level, and an inherited outer jobserver is consumed by the
/// one scheduler that inherited it.
// [spec:ronin:req:make.recursive-invocation+2]
pub(super) fn compiler_flag_variables(invocation: &Invocation) -> CompilerFlagVariables {
    let letters: String = invocation
        .propagated()
        .iter()
        .filter_map(|switch| switch.to_str().and_then(|switch| switch.strip_prefix('-')))
        .collect();
    let mut base = letters.clone();
    let append = |base: &mut String, option: &str| {
        base.push(' ');
        base.push_str(option);
    };
    // Between the letter group and `-j`, which is where GNU Make's switch table
    // puts `-I`, and one word per entry rather than one for the list. The
    // entries are the table's own, so `-I -` travels as `-I-` and a directory
    // that is not there travels too: the table holds what was written, and it
    // is `construct_include_path` on the far side that decides which of them a
    // search reaches. That is the only way a child learns the search path at
    // all, and the only way a makefile's own `MAKEFLAGS += -I dir` survives
    // being read back.
    for dir in &invocation.include_dirs {
        append(
            &mut base,
            &format!("-I{}", quote_for_makeflags(&dir.to_string_lossy())),
        );
    }
    match invocation.effective_jobs() {
        Some(JobLimit::Fixed(jobs)) => append(&mut base, &format!("-j{}", jobs.get())),
        Some(JobLimit::Unlimited) => append(&mut base, "-j"),
        Some(JobLimit::Auto) | None => {}
    }
    if let Some(load) = invocation.load.filter(|load| load.propagated) {
        append(&mut base, &format!("-l{}", load.ceiling));
    }
    // Between the letter group and the long options, which is where GNU Make
    // writes it: `k -Oline --debug=b --trace --no-print-directory`.
    if let Some(sync) = invocation.output_sync {
        append(&mut base, sync.spelling());
    }
    // The requests with no letter to travel as, which GNU Make writes after the
    // group and before the withdrawals, in its own switch table's order.
    // Deduplicated as GNU Make deduplicates them: the same `--debug` can arrive
    // from the command line and from a parent's `MAKEFLAGS` at once, and a
    // letter in the group cannot say a thing twice.
    //
    // Quoted like every other switch argument, because `define_makeflags` runs
    // one `quote_for_env` over `flags->arg` and does not ask which switch it
    // belongs to. `--debug` is the second of the two switches whose argument is
    // arbitrary text — `-I` is the other — so it is the second place a
    // backslash, a blank or a `$` has to survive being read back as a command
    // line. Leaving it raw agreed with `$(MAKEFLAGS)` only by cancelling GNU
    // Make's own halving, and disagreed with `$(value MAKEFLAGS)` and
    // `$(MFLAGS)`, which read the stored text.
    let mut long = Vec::new();
    for spec in &invocation.debug {
        let option = format!(" --debug={}", quote_for_makeflags(&spec.to_str_lossy()));
        if !long.contains(&option) {
            long.push(option);
        }
    }
    if invocation.given(Switch::Trace) {
        long.push(" --trace".to_owned());
    }
    if invocation.given(Switch::WarnUndefinedVariables) {
        long.push(" --warn-undefined-variables".to_owned());
    }
    for option in long {
        base.push_str(&option);
    }
    for withdrawn in invocation.withdrawn() {
        append(&mut base, withdrawn);
    }
    // Last, where GNU Make's switch table puts it, and carrying what the table
    // holds rather than what this run is shuffling by — the two agree only
    // when the command line is what wrote it.
    if let Some(mode) = &invocation.shuffle_spelling {
        append(&mut base, &format!("--shuffle={mode}"));
    }
    let mflags = if letters.is_empty() {
        base.trim_start().to_owned()
    } else {
        format!("-{base}")
    };
    // The fragments go after every switch and before the assignments, read off
    // GNU Make's own `MAKEFLAGS` rather than reasoned about. Carrying them is
    // the only way one reaches a child at all: `$(MAKE)` is one word and holds
    // nothing, so a child — composed here, or started by a recipe line nothing
    // could compose — learns what it was invoked under from this variable and
    // from nowhere else.
    //
    // They sit beside the switch table rather than in it, which is where GNU
    // Make keeps them too: its `MAKEFLAGS` holds a reference to a separate
    // `-*-eval-flags-*-` variable, and the switch table proper never contains a
    // fragment. The distinction is load-bearing here for one reason. A switch
    // is a bit and an assignment is keyed by name, so reading either back twice
    // settles to the same table; a fragment is neither, and reading one back
    // appends it again. `decode_makefile_makeflags` reads its three inputs into
    // one invocation, so a fragment left in the protected table would multiply
    // every time a Makefile wrote to MAKEFLAGS.
    let eval_flags = invocation
        .evals
        .iter()
        .map(|eval| format!("--eval={}", quote_for_makeflags(&eval.to_str_lossy())))
        .collect::<Vec<_>>()
        .join(" ");
    let overrides = command_overrides(invocation);
    let mut makeflags = base.clone();
    if !eval_flags.is_empty() {
        append(&mut makeflags, &eval_flags);
    }
    if !overrides.is_empty() {
        // GNU Make ends the switches at a `--` before the assignments, so that
        // one beginning with a dash cannot be read as another switch.
        makeflags.push_str(" -- ");
        makeflags.push_str(&overrides);
    }
    CompilerFlagVariables {
        base,
        eval_flags,
        makeflags,
        mflags,
        overrides,
    }
}

/// Turn `MAKEFLAGS` into an argv-shaped list.
///
/// GNU Make omits the dash from its leading cluster (`ks`, not `-ks`). Every
/// later word already has command-line shape, and `--` still separates
/// assignments. Parsing this list before argv gives argv normal last-word-wins
/// precedence while keeping one option grammar for both inputs.
pub(super) fn makeflags_arguments(
    inherited: &str,
    diagnostics: &Arc<Diagnostics>,
) -> Result<Vec<BString>, RunResult> {
    Ok(option_arguments(expanded_option_words(
        inherited,
        diagnostics,
    )?))
}

/// Split the canonical `MAKEFLAGS` a makefile's own assignments left back into
/// an argv-shaped list, without expanding it.
///
/// This is Ronin's own switch table read a second time, not a stream from the
/// environment: GNU Make keeps the table it built and never hands it back to
/// `variable_expand`, so a `$$` here is unquoted once and nothing is resolved.
pub(super) fn evaluated_option_arguments(makeflags: &str) -> Vec<BString> {
    option_arguments(makeflags_words(makeflags, true))
}

/// Assemble already-split option words into an argv-shaped list, applying the
/// leading-cluster rule GNU Make gives `argv[1]` alone.
fn option_arguments(words: Vec<String>) -> Vec<BString> {
    let mut arguments = vec![BString::from("make")];
    for (position, word) in words.into_iter().enumerate() {
        if position == 0 && word != "--" && !word.starts_with('-') && !word.contains('=') {
            arguments.push(BString::from(format!("-{word}")));
        } else {
            arguments.push(BString::from(word));
        }
    }
    arguments
}

/// Expand an environment option stream as makefile text, then split the result
/// into words — the two halves of GNU Make's `decode_env_switches`, in that
/// order.
///
/// `decode_env_switches` (main.c) never splits the environment's bytes: it
/// hands `$(NAME)` to `variable_expand` and splits the RESULT, so
/// `MAKEFLAGS='$(subst X,-,Xk)'` is `-k` and a `$(FOO)` resolves against the
/// environment. The expansion is the bootstrap evaluator standing before switch
/// decode — everything a session needs to construct itself (`-C`, `-f`, `-r`,
/// `-R`, `--eval`) comes out of this same stream, so the names in scope are the
/// environment's alone. A `$(warning)` reports through `diagnostics`; a
/// `$(error)` and a malformed reference end the run with a refusal, as GNU
/// Make's do here before `-C` has moved.
///
/// A stream with no `$` cannot expand to anything but itself, so it skips the
/// evaluator and its whole environment load — the overwhelmingly common case
/// stays exactly the split it always was. When the stream IS expanded, the
/// split no longer halves `$$`: the expansion already did, and halving a second
/// time would turn a directory's doubled dollar into a lost one. This is where
/// the recorded literal-text divergence stays put — a value a Make WROTE
/// travels as before, because Ronin still exports the stored text; only a
/// hand-written reference decodes differently now, exactly as GNU Make's does.
fn expanded_option_words(
    inherited: &str,
    diagnostics: &Arc<Diagnostics>,
) -> Result<Vec<String>, RunResult> {
    if !inherited.contains('$') {
        return Ok(makeflags_words(inherited, true));
    }
    let expanded = kati::evaluate::expand_environment_option_stream(
        None,
        inherited.as_bytes(),
        Arc::clone(diagnostics),
    )
    .map_err(|failure| RunResult {
        stdout: Vec::new(),
        stderr: terminated(diagnostic(PRODUCT_NAME, failure)),
        exit_code: ABANDONED,
    })?;
    Ok(makeflags_words(&String::from_utf8_lossy(&expanded), false))
}

/// Turn the evaluated right-hand side of a Makefile `MAKEFLAGS` assignment
/// back into an option stream.
///
/// The value may already contain `--` and command-line assignments, followed
/// by newly appended switches. GNU Make does not turn those switches into
/// goals: it removes assignments, decodes every remaining word as an option,
/// then renders one fresh `--` before the override table.
///
/// An assignment binding no name is the one such word that ends the build
/// instead of being set aside. GNU Make reads every non-switch word through
/// `handle_non_switch_argument` whatever origin it arrived from, and that
/// reaches `parse_variable_definition`, which is fatal on an empty name — so
/// `MAKEFLAGS += -k := 2` abandons exactly as `make '=1'` does.
///
/// A word that names something is set aside for the evaluator instead of being
/// dropped: `handle_non_switch_argument` hands it to `try_variable_definition`
/// at the origin the assignment carried, which for a Makefile's own write is
/// `o_file`. So it is an ordinary Makefile assignment made where the write
/// stands, not a command-line variable — measured, `$(origin FOO)` answers
/// `file` and a later `FOO = mine` outranks it.
///
/// The leading-cluster rule is positional, and GNU Make applies it to the
/// first word of the value and to no other (`decode_env_switches` examines
/// `argv[1]` alone). Counting words already kept would let an assignment in
/// front of it hand the dash to the word behind: `MAKEFLAGS += FOO=bar ran`
/// would silently turn on `-r -a -n`, which is a dry run.
fn assigned_makeflags_arguments(value: &str) -> Result<MakeflagsWords, String> {
    let mut words = MakeflagsWords {
        arguments: vec![BString::from("make")],
        assignments: Vec::new(),
    };
    for (position, word) in makeflags_words(value, true).into_iter().enumerate() {
        if word == "--" {
            continue;
        }
        if !word.starts_with('-') && word.contains('=') {
            if command_variable_name(&word).is_some_and(str::is_empty) {
                return Err("empty variable name".to_owned());
            }
            words.assignments.push(word);
            continue;
        }
        if position == 0 && !word.starts_with('-') {
            words.arguments.push(BString::from(format!("-{word}")));
        } else {
            words.arguments.push(BString::from(word));
        }
    }
    Ok(words)
}

/// One `MAKEFLAGS` value split the way GNU Make splits it: the words the
/// option grammar reads, and the words that bind a name.
struct MakeflagsWords {
    arguments: Vec<BString>,
    assignments: Vec<String>,
}

/// Decode a Makefile assignment with the same option grammar as argv.
///
/// `previous` is GNU Make's persistent switch table, while `protected` is the
/// environment/argv state that outranks every Makefile write. The evaluated
/// assignment sits between them, so ordinary last-spelling-wins parsing gives
/// exactly the special precedence GNU Make applies here.
pub(super) fn decode_makefile_makeflags(
    previous: &[u8],
    assigned: &[u8],
    protected: &[u8],
) -> Result<kati::flags::DecodedMakeflags, String> {
    let mut invocation = Invocation::new();
    let mut assignments = Vec::new();
    for (value, source, is_the_write) in [
        (previous, ArgumentSource::Makefile, false),
        (assigned, ArgumentSource::Makefile, true),
        (protected, ArgumentSource::Protection, false),
    ] {
        let value = std::str::from_utf8(value)
            .map_err(|_| "MAKEFLAGS contains non-UTF-8 option bytes".to_owned())?;
        let words = assigned_makeflags_arguments(value)?;
        // Only the write itself binds anything. GNU Make decodes one value —
        // the whole of `MAKEFLAGS` as the Makefile left it — while the switch
        // table and the protected state are read back here as two more streams
        // of the same grammar, and a name bound out of either of those would be
        // bound again on every write.
        if is_the_write {
            assignments = words
                .assignments
                .into_iter()
                .map(|word| Bytes::from(word.into_bytes()))
                .collect();
        }
        if let Some(action) = parse_arguments(&mut invocation, &words.arguments, source)
            .map_err(|error| error.to_string())?
        {
            let diagnostic = match action {
                Action::Immediate(result) => String::from_utf8_lossy(&result.stderr).into_owned(),
                Action::Execute(_) => unreachable!("parse_arguments never executes"),
            };
            return Err(if diagnostic.is_empty() {
                "MAKEFLAGS requests an immediate command-line action".to_owned()
            } else {
                diagnostic
            });
        }
        // A word that is not a switch names a goal, and GNU Make enters a goal
        // only for the command line — `handle_non_switch_argument` guards that
        // half with `origin == o_command`, so a word left over here is dropped.
        // It is what a switch this Make does not know left behind.
        invocation.goals.clear();
    }
    if invocation.debugging() == 0 {
        invocation.switches &= !Switch::Debug.bit();
    }
    // GNU Make reaches `jobserver_setup` after the whole read, so a style named
    // here is checked against the job count the read finally settles on — which
    // is why the style is carried from assignment to assignment rather than
    // judged where it is written.
    if let Some(refusal) = unknown_jobserver_style(&invocation) {
        return Err(refusal);
    }
    let flags = compiler_flag_variables(&invocation);
    Ok(kati::flags::DecodedMakeflags {
        assignments,
        carried: Bytes::from(carried_switches(&flags.base, &invocation).into_bytes()),
        makeflags: Bytes::from(flags.base.into_bytes()),
        eval_flags: Bytes::from(flags.eval_flags.into_bytes()),
        mflags: Bytes::from(flags.mflags.into_bytes()),
        is_dry_run: invocation.given(Switch::DryRun),
        is_silent_mode: invocation.given(Switch::Silent),
        ignore_errors: invocation.given(Switch::IgnoreErrors),
        environment_overrides: invocation.given(Switch::EnvironmentOverrides),
        no_builtin_rules: invocation.given(Switch::NoBuiltinRules),
        no_builtin_variables: invocation.given(Switch::NoBuiltinVariables),
        warn_undefined_variables: invocation.given(Switch::WarnUndefinedVariables),
        include_dirs: invocation.include_dirs.clone(),
        // What this write said about a word it dropped. GNU Make prints these
        // where the decode reaches them, which is inside the assignment, so
        // they are handed back for the evaluator to raise there.
        complaints: invocation
            .complaints
            .iter()
            .map(|complaint| Bytes::from(complaint.clone().into_bytes()))
            .collect(),
    })
}

/// Parse the canonical value left by Makefile assignments back into the state
/// that controls this unit's one Ninja scheduler.
///
/// Read at [`ArgumentSource::Makefile`], because that is what this text is: the
/// switch table as the makefiles left it. GNU Make keeps the table itself and
/// never re-reads it, so a word it accepted there without examining — a
/// `--shuffle` naming no mode — has to survive being read back here too.
pub(super) fn evaluated_invocation(makeflags: &str) -> Result<Invocation, Error> {
    let mut invocation = Invocation::new();
    let arguments = evaluated_option_arguments(makeflags);
    match parse_arguments(&mut invocation, &arguments, ArgumentSource::Makefile)? {
        None => Ok(invocation),
        Some(_) => Err(CliError::InvalidParameter {
            option: "MAKEFLAGS",
        }
        .into()),
    }
}

/// Apply the switches a Makefile named without disturbing the runtime
/// facilities (terminal, jobserver transport, working directory) already
/// selected for this invocation.
pub(super) fn evaluated_build_options(
    initial: &BuildOptions,
    invocation: &Invocation,
) -> BuildOptions {
    let mut options = initial.clone();
    options.maxfail = if invocation.given(Switch::KeepGoing) {
        usize::MAX
    } else {
        1
    };
    options.dryrun = invocation.given(Switch::DryRun);
    options.verbose = options.dryrun;
    options.quiet = invocation.given(Switch::Silent);
    options.maxload = invocation.load.map_or(0.0, |load| load.ceiling);
    if options.jobserver.is_none()
        && let Some(jobs) = invocation.effective_jobs()
    {
        options.jobs = jobs;
    }
    options
}

/// Split the words GNU Make writes into `MAKEFLAGS`.
///
/// Command-line assignments are quoted for an environment variable rather
/// than for a shell: a backslash protects the following byte and a doubled
/// dollar represents one literal dollar. Plain whitespace still separates
/// options. Resolving that quoting here gives the ordinary argv parser the
/// exact assignment a parent invocation received.
///
/// A VALUE THAT ENDS IN WHITESPACE ENDS IN AN EMPTY WORD, which is not a
/// rounding error in `decode_env_switches` (main.c) but the thing it does:
/// leading blanks are skipped once and a blank run between words is consumed
/// whole, but the last word is terminated wherever the value ran out. So
/// `MAKEFLAGS=-I ` hands `-I` an empty argument and is refused, while
/// `MAKEFLAGS=-I  -R` hands it `-R`. Dropping the empty word instead would turn
/// the first into a switch missing its argument, which is a different
/// complaint and, from a makefile's own write, a different outcome.
///
/// `halve_dollars` is whether a doubled `$` collapses to one here. It does for
/// a value that has not been through the evaluator — the halving is the inverse
/// of the doubling `quote_for_makeflags` wrote — and it does NOT for one an
/// environment stream already expanded, where the evaluator halved it once and
/// a second pass would lose the byte a directory's `$$` stood for.
fn makeflags_words(inherited: &str, halve_dollars: bool) -> Vec<String> {
    let bytes = inherited.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index == bytes.len() {
        return Vec::new();
    }
    let settled = |word: Vec<u8>| String::from_utf8(word).expect("MAKEFLAGS started as UTF-8");
    let mut words = Vec::new();
    let mut word = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if index + 1 < bytes.len() => {
                index += 1;
                word.push(bytes[index]);
            }
            b'$' if halve_dollars && bytes.get(index + 1) == Some(&b'$') => {
                index += 1;
                word.push(b'$');
            }
            byte if byte.is_ascii_whitespace() => {
                words.push(settled(std::mem::take(&mut word)));
                while bytes.get(index + 1).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
            }
            byte => word.push(byte),
        }
        index += 1;
    }
    words.push(settled(word));
    words
}

/// Hand the `-E`/`--eval` fragments to the read that is about to happen.
///
/// They are given to the read rather than spliced into a file, because that is
/// what they are: GNU Make evaluates each fragment as its own buffer above
/// `read_all_makefiles` (main.c). So they precede `MAKEFILES` as well as the
/// named files, they leave `MAKEFILE_LIST` alone, and a run with no makefile on
/// disk still has whatever rules they carry.
// [spec:ronin:req:make.interface-compatibility]
pub(super) fn carry_command_line_evals(session: &mut Session, evals: &[Bytes]) {
    session.flags.command_line_evals = evals.to_vec();
}

#[cfg(test)]
mod tests {
    use super::makeflags_arguments;
    use crate::util::BString;
    use kati::diagnostics::Diagnostics;
    use std::sync::Arc;

    #[test]
    fn makeflags_quoting_round_trips() {
        let diagnostics = Arc::new(Diagnostics::collected());
        assert_eq!(
            makeflags_arguments(
                r"ks -- SPACE=two\ words SLASH=a\\b DOLLAR=a$$b",
                &diagnostics
            )
            .unwrap(),
            [
                BString::from("make"),
                BString::from("-ks"),
                BString::from("--"),
                BString::from("SPACE=two words"),
                BString::from(r"SLASH=a\b"),
                BString::from("DOLLAR=a$b"),
            ]
        );
    }

    /// The filed reproducer: a `$(subst)` in an environment stream is makefile
    /// text, decoded to switches only after it is expanded.
    #[test]
    fn a_stream_reference_expands_before_the_split() {
        let diagnostics = Arc::new(Diagnostics::collected());
        assert_eq!(
            makeflags_arguments("$(subst X,-,Xk)", &diagnostics).unwrap(),
            [BString::from("make"), BString::from("-k")]
        );
    }
}
