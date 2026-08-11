//! Inputs shared by Make's command-line parser and kati compilation.

use super::{Action, ArgumentSource, Invocation, JobLimit, Switch, parse, parse_arguments};
use crate::Error;
use crate::build::BuildOptions;
use crate::error::CliError;
use crate::util::{BString, ByteSlice};
use kati::bytes::Bytes;
use kati::session::Session;

/// The Make interface variables one compiler invocation presents to source.
pub(super) struct CompilerFlagVariables {
    /// `MAKEFLAGS` before its `MAKEOVERRIDES` reference is expanded.
    pub(super) base: String,
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

/// Quote one command-line assignment for Make's environment-variable syntax.
fn quote_makeflags_assignment(assignment: &str) -> String {
    let mut quoted = String::with_capacity(assignment.len());
    for character in assignment.chars() {
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
        .map(|(_, assignment)| quote_makeflags_assignment(assignment))
        .collect::<Vec<_>>()
        .join(" ")
}

/// How a Make invocation describes itself to this compilation unit and every
/// semantic child.
///
/// Job and load limits remain compiler-visible because Makefiles branch on
/// them, while execution still has one Ninja scheduler. The jobserver
/// authorization itself is deliberately absent: no recursive Make runtime is
/// created, and an inherited outer jobserver is consumed only by that one
/// scheduler.
// [spec:ronin:req:make.recursive-invocation+1]
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
    let mut long = Vec::new();
    for spec in &invocation.debug {
        let option = format!(" --debug={}", spec.to_str_lossy());
        if !long.contains(&option) {
            long.push(option);
        }
    }
    if invocation.given(Switch::Trace) {
        long.push(" --trace".to_owned());
    }
    for option in long {
        base.push_str(&option);
    }
    for withdrawn in invocation.withdrawn() {
        append(&mut base, withdrawn);
    }
    // Last, where GNU Make's switch table puts it, and carrying the seed this
    // run settled on so that a child reproduces the same order.
    if let Some(mode) = invocation.shuffle.spelling() {
        append(&mut base, &format!("--shuffle={mode}"));
    }
    let overrides = command_overrides(invocation);
    let mut makeflags = base.clone();
    if !overrides.is_empty() {
        // GNU Make ends the switches at a `--` before the assignments, so that
        // one beginning with a dash cannot be read as another switch.
        makeflags.push_str(" -- ");
        makeflags.push_str(&overrides);
    }
    let mflags = if letters.is_empty() {
        base.trim_start().to_owned()
    } else {
        format!("-{base}")
    };
    CompilerFlagVariables {
        base,
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
pub(super) fn makeflags_arguments(inherited: &str) -> Vec<BString> {
    let mut arguments = vec![BString::from("make")];
    for (position, word) in makeflags_words(inherited).into_iter().enumerate() {
        if position == 0 && word != "--" && !word.starts_with('-') && !word.contains('=') {
            arguments.push(BString::from(format!("-{word}")));
        } else {
            arguments.push(BString::from(word));
        }
    }
    arguments
}

/// Turn the evaluated right-hand side of a Makefile `MAKEFLAGS` assignment
/// back into an option stream.
///
/// The value may already contain `--` and command-line assignments, followed
/// by newly appended switches. GNU Make does not turn those switches into
/// goals: it removes assignments, decodes every remaining word as an option,
/// then renders one fresh `--` before the override table.
fn assigned_makeflags_arguments(value: &str) -> Vec<BString> {
    let mut arguments = vec![BString::from("make")];
    for word in makeflags_words(value) {
        if word == "--" || (!word.starts_with('-') && word.contains('=')) {
            continue;
        }
        if arguments.len() == 1 && !word.starts_with('-') {
            arguments.push(BString::from(format!("-{word}")));
        } else {
            arguments.push(BString::from(word));
        }
    }
    arguments
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
    for value in [previous, assigned, protected] {
        let value = std::str::from_utf8(value)
            .map_err(|_| "MAKEFLAGS contains non-UTF-8 option bytes".to_owned())?;
        let arguments = assigned_makeflags_arguments(value);
        if let Some(action) =
            parse_arguments(&mut invocation, &arguments, ArgumentSource::Inherited)
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
        if !invocation.goals.is_empty() {
            let goal = invocation.goals.remove(0);
            return Err(format!(
                "MAKEFLAGS contains non-option word '{}'",
                goal.to_str_lossy()
            ));
        }
    }
    if invocation.debugging() == 0 {
        invocation.switches &= !Switch::Debug.bit();
    }
    let flags = compiler_flag_variables(&invocation);
    Ok(kati::flags::DecodedMakeflags {
        makeflags: Bytes::from(flags.base.into_bytes()),
        mflags: Bytes::from(flags.mflags.into_bytes()),
        is_dry_run: invocation.given(Switch::DryRun),
        is_silent_mode: invocation.given(Switch::Silent),
        ignore_errors: invocation.given(Switch::IgnoreErrors),
        environment_overrides: invocation.given(Switch::EnvironmentOverrides),
        no_builtin_rules: invocation.given(Switch::NoBuiltinRules),
        no_builtin_variables: invocation.given(Switch::NoBuiltinVariables),
    })
}

/// Parse the canonical value left by Makefile assignments back into the state
/// that controls this unit's one Ninja scheduler.
pub(super) fn evaluated_invocation(makeflags: &str) -> Result<Invocation, Error> {
    match parse(&[BString::from("make")], Some(makeflags))? {
        Action::Execute(invocation) => Ok(*invocation),
        Action::Immediate(_) => Err(CliError::InvalidParameter {
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
fn makeflags_words(inherited: &str) -> Vec<String> {
    let bytes = inherited.as_bytes();
    let mut words = Vec::new();
    let mut word = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if index + 1 < bytes.len() => {
                index += 1;
                word.push(bytes[index]);
            }
            b'$' if bytes.get(index + 1) == Some(&b'$') => {
                index += 1;
                word.push(b'$');
            }
            byte if byte.is_ascii_whitespace() => {
                if !word.is_empty() {
                    words.push(String::from_utf8(word).expect("MAKEFLAGS started as UTF-8"));
                    word = Vec::new();
                }
            }
            byte => word.push(byte),
        }
        index += 1;
    }
    if !word.is_empty() {
        words.push(String::from_utf8(word).expect("MAKEFLAGS started as UTF-8"));
    }
    words
}

/// Parse `-E`/`--eval` fragments as Makefile source before the selected file.
///
/// Kati caches a Makefile's parsed statements in its owned session. Prepending
/// the fragments there makes them ordinary compiler input while leaving the
/// selected Makefile's identity, include base, and `MAKEFILE_LIST` unchanged.
// [spec:ronin:req:make.interface-compatibility]
pub(super) fn prepend_command_line_evals(
    session: &mut Session,
    evals: &[Bytes],
) -> Result<(), kati::anyhow::Error> {
    if evals.is_empty() {
        return Ok(());
    }

    let Some(makefile_name) = session.flags.makefile.clone() else {
        return Ok(());
    };
    // A Makefile that is absent or will not open is not this function's failure
    // to report: evaluation reads it again in a moment and says which file and
    // why. Here the fragments simply have nothing to go in front of.
    let kati::file::Source::Read(makefile) = session.get_makefile(&makefile_name)? else {
        return Ok(());
    };
    let filename = session.intern("*command line eval*");
    let mut statements = Vec::new();
    for source in evals {
        let parsed =
            kati::parser::parse_buf(session, source, kati::loc::Loc { filename, line: 0 })?;
        statements.extend(parsed.lock().iter().cloned());
    }
    makefile.stmts.lock().splice(0..0, statements);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::makeflags_arguments;
    use crate::util::BString;

    #[test]
    fn makeflags_quoting_round_trips() {
        assert_eq!(
            makeflags_arguments(r"ks -- SPACE=two\ words SLASH=a\\b DOLLAR=a$$b"),
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
}
