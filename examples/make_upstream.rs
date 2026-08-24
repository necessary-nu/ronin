//! Classify what GNU Make's own test suite can teach us about Ronin's Make
//! compiler.
//!
//! The suite compares stdout byte for byte against GNU Make's, and Ronin is not
//! trying to produce GNU Make's output — it narrates a build the way Ninja does,
//! because that is the product. Its pass rate and exact runner residue are
//! therefore discovery data, not a conformance result.
//!
//! What the suite is good for is finding places to investigate. Known parser,
//! evaluator, rule-search and Makefile-selection shapes become compiler
//! candidates. Refused options and `MAKEFLAGS` differences are interface
//! observations. Product narration is identified separately. Everything else
//! stays explicitly unclassified, because an exact-output diff cannot decide
//! whether its residue came from graph intent or from the GNU Make runner.
//!
//! A discovery becomes a compatibility failure only after a focused reproducer
//! compares graph shape, selected work, normal outcome, or filesystem effects.
//! Inventory drift still fails this program so every newly-shaped observation
//! is reviewed and recorded; it does not make the inventory itself a gate on
//! GNU Make runner parity.
//!
//! Usage: `make_upstream --work DIR [--inventory FILE] [--update]`

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const ORACLE_VERSION: &str = "GNU Make 4.4.1";
const ORACLE_COMMIT: &str = "d66a65ad5a0e31b287f53930b0f09e31801f1613";

/// What kind of follow-up an exact-output difference can justify.
///
/// A case is classified by its strongest evidence. `Unclassified` sorts last
/// so residue that still needs human triage cannot disappear under a narration
/// family, not because it is presumed to be a defect.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Class {
    /// Ronin says it differently on purpose.
    Narration,
    /// An accepted/refused invocation spelling or compatibility variable.
    Interface,
    /// A known parser, evaluator, rule-search, or Makefile-selection shape.
    Compiler,
    /// Residue whose graph-vs-runner origin has not been established.
    Unclassified,
}

impl Class {
    const fn name(self) -> &'static str {
        match self {
            Self::Narration => "narration",
            Self::Interface => "interface",
            Self::Compiler => "compiler",
            Self::Unclassified => "unclassified",
        }
    }
}

/// One recognisable reason a case differs.
struct Family {
    name: &'static str,
    class: Class,
    reason: &'static str,
}

const FAMILIES: [Family; 28] = [
    Family {
        name: "fatal-decoration",
        class: Class::Narration,
        reason: "Make wraps a fatal diagnostic in `*** ` and `  Stop.`; Ronin states the same thing plainly, so the decoration is taken off before the two are compared.",
    },
    Family {
        name: "not-remade-line",
        class: Class::Narration,
        reason: "Make's keep-going summary, naming a goal it gave up on after a prerequisite failed; Ronin's counterpart is its own stopped line.",
    },
    Family {
        name: "debug-trace",
        class: Class::Narration,
        reason: "GNU Make's --debug trace, which Ronin does not produce. Recognised only when the run asked for debug output, because the suite writes these expectations as bare regexes.",
    },
    Family {
        name: "refusal-not-made",
        class: Class::Compiler,
        reason: "compiler candidate: GNU Make refused the run outright and Ronin went on to build. The refusal is the whole of Make's output, so what Ronin built is work Make never authorised.",
    },
    Family {
        name: "recipe-error-line",
        class: Class::Narration,
        reason: "Make named the makefile line, the target and the recipe's status in one line, and Ronin's own line for that failure is missing or names something else.",
    },
    Family {
        name: "ninja-progress",
        class: Class::Narration,
        reason: "Ronin prints Ninja's [N/M] progress line where Make echoes the recipe.",
    },
    Family {
        name: "recipe-echo",
        class: Class::Narration,
        reason: "Make echoes the recipe line it is about to run; Ronin names the same command in its progress counter. Recognised from the counter's own payload, or from the makefile the case ran — not by guessing.",
    },
    Family {
        name: "ninja-failure-block",
        class: Class::Narration,
        reason: "Ronin reports a failed command with Ninja's FAILED: block and its own stopped line.",
    },
    Family {
        name: "product-name",
        class: Class::Narration,
        reason: "the diagnostic is Ronin's by name, where the suite expects the name it probed for.",
    },
    Family {
        name: "directory-announce",
        class: Class::Narration,
        reason: "the two tools bracket a -C build with Entering and Leaving at different moments.",
    },
    Family {
        name: "no-work-line",
        class: Class::Narration,
        reason: "Ronin says it had nothing to do; Make says nothing at all.",
    },
    Family {
        name: "up-to-date-line",
        class: Class::Narration,
        reason: "Make announced a goal was already up to date; Ronin either said nothing or counted the goal as a build step and printed its progress line.",
    },
    Family {
        name: "intermediate-sweep",
        class: Class::Narration,
        reason: "Make announces the intermediate files it deletes at the end of a build with one `rm` line; Ronin sweeps the same files and says nothing. Recognised from the sweep's own payload — every name on the line has to be one this run's own commands named — not from the word `rm`.",
    },
    Family {
        name: "recipe-interleave",
        class: Class::Narration,
        reason: "the same lines in a different order: Make interleaves each recipe line with the output of running it, and Ronin reports an edge's output once the whole recipe has finished.",
    },
    Family {
        name: "option-refused",
        class: Class::Interface,
        reason: "interface observation: Ronin refused an option spelling the test passed and GNU Make accepted.",
    },
    Family {
        name: "unsupported-feature",
        class: Class::Compiler,
        reason: "compiler candidate: the evaluator says outright that it does not support a Makefile construct.",
    },
    Family {
        name: "no-rule-to-make",
        class: Class::Compiler,
        reason: "compiler candidate: Ronin found no rule for a target GNU Make built; reproduce the rule-search graph before treating it as a defect.",
    },
    Family {
        name: "shared-refusal",
        class: Class::Narration,
        reason: "both tools refused to build the same target for the same reason and worded it differently.",
    },
    Family {
        name: "refusal-attribution",
        class: Class::Narration,
        reason: "both tools refused, naming a different link of the same broken chain: Make walks further in before giving up.",
    },
    Family {
        name: "no-makefile-found",
        class: Class::Compiler,
        reason: "compiler candidate: Ronin found no Makefile to read where GNU Make found one.",
    },
    Family {
        name: "command-not-found-text",
        class: Class::Narration,
        reason: "both tools failed to run the same missing command and said so differently: Make execs it itself and reports the errno, Ronin goes through a shell and reports what the shell said. Only where the line holds shell syntax — a line that needs no shell is now exec'd directly by both.",
    },
    Family {
        name: "io-error-text",
        class: Class::Narration,
        reason: "both tools refused for the same reason and worded it differently; ours carries Rust's \"(os error N)\" suffix.",
    },
    Family {
        name: "makeflags-content",
        class: Class::Interface,
        reason: "interface observation: a Makefile read MAKEFLAGS and saw different switches; runner-only flags may deliberately be absent.",
    },
    Family {
        name: "parse-failure",
        class: Class::Compiler,
        reason: "compiler candidate: Ronin could not parse a Makefile GNU Make read.",
    },
    Family {
        name: "pattern-peer-warning",
        class: Class::Narration,
        reason: "GNU Make's success-path `pattern recipe did not update peer target 'X'` warning, which make-narration-contract-audit retired as silent by operator decision (2026-08-17): emitting it would emulate Make rather than compile to a Ninja graph. The build the two tools agree on is gated separately.",
    },
    Family {
        name: "delete-announce",
        class: Class::Narration,
        reason: "GNU Make's `*** Deleting file 'X'` announcement on the .DELETE_ON_ERROR path. make-delete-on-error-cleanup decided the announcement is not owed: Ronin withdraws the failed output silently, which is the same act the interrupt path already performs without a word.",
    },
    Family {
        name: "jobserver-narration",
        class: Class::Narration,
        reason: "GNU Make's jobserver-mode runner messages — `-jN forced in submake: resetting jobserver mode`, `jobserver unavailable: using -j1`, `cannot open jobserver` — which a single-scheduler Ronin (make-single-ninja-scheduler) never emits, because recursive Make invocations compile into one graph with one scheduler and there is no jobserver transport between separate processes.",
    },
    Family {
        name: "evaluation",
        class: Class::Unclassified,
        reason: "unclassified residue: exact output cannot distinguish graph intent from runner behavior; reproduce it through the build-intent gate.",
    },
];

/// The two sides of one failing test, reduced to the lines that differ.
struct Divergence {
    /// What GNU Make produced and Ronin did not.
    expected: Vec<String>,
    /// What Ronin produced and GNU Make did not.
    actual: Vec<String>,
}

/// Read a context diff into the lines each side did not share.
///
/// The suite writes `diff -c`, whose two halves are separated by the `--- N,M
/// ----` banner: before it is the expected file with `-` and `!` markers, after
/// it is ours with `+` and `!`. Context lines carry two spaces and are dropped,
/// since a line both sides agree on explains nothing.
fn read_divergence(text: &str) -> Divergence {
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    let mut in_actual = false;
    for line in text.lines() {
        if line.starts_with("--- ") && line.ends_with("----") {
            in_actual = true;
            continue;
        }
        if line.starts_with("*** ") || line.starts_with("--- ") || line.starts_with("******") {
            continue;
        }
        let Some(body) = line.strip_prefix("! ").or_else(|| {
            if in_actual {
                line.strip_prefix("+ ")
            } else {
                line.strip_prefix("- ")
            }
        }) else {
            continue;
        };
        if in_actual {
            actual.push(body.to_owned());
        } else {
            expected.push(body.to_owned());
        }
    }
    Divergence { expected, actual }
}

/// The names a diagnostic can be attributed to on either side.
///
/// `ronin` appears on both. The suite asks the program under test what it is
/// called and writes the answer into every expected diagnostic, so a line the
/// suite expects reads `ronin:` as surely as one we produced — which is the
/// `#MAKE#` mechanism working, and the reason both sides get the same
/// treatment here rather than one rule each.
const PROGRAM_NAMES: [&str; 4] = ["ronin", "make", "gmake", "#MAKE#"];

/// Which tool produced a line, which decides what it is allowed to be.
///
/// Most narration can only come from one side, and saying so is what keeps the
/// subtraction honest. `[1/2] ` is Ninja's counter when Ronin prints it and is a
/// recipe's own output when GNU Make does — a test that echoes something
/// bracket-shaped would otherwise have its output silently deleted from the
/// expected side and the case would read as agreement.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Side {
    /// Ronin's output.
    Ours,
    /// GNU Make's, as the suite recorded it.
    Theirs,
}

/// What one line of a diff contributes.
///
/// Narration is not simply deleted. A diagnostic keeps its body and loses only
/// the name in front, so two tools saying the same thing about the same problem
/// cancel out while two tools saying *different* things still show up. Deleting
/// the whole line would hide a wrong message behind a right prefix.
#[derive(Default)]
struct Contribution {
    /// The family the line belongs to, where it has one.
    family: Option<&'static str>,
    /// What is left of the line once narration is accounted for.
    residue: Option<String>,
    /// Whether the tool marked the line as its own rather than a recipe's
    /// output: it wore a product name, or it opens with a source location.
    /// This is what tells a refusal apart from a build that ran.
    diagnostic: bool,
    /// The command a progress line named. Ronin prints `[1/2] cc -c foo.c`
    /// where Make echoes `cc -c foo.c`, so the counter's payload is the
    /// counterpart of the recipe line Make echoed — evidence from the same run,
    /// which is better than reading a makefile the suite may since have
    /// rewritten.
    payload: Option<String>,
}

impl Contribution {
    /// A line wholly explained by one narration family.
    fn narration(family: &'static str) -> Self {
        Self {
            family: Some(family),
            ..Self::default()
        }
    }
}

/// Whether a line opens with `path:line: `, the way both tools mark a
/// diagnostic they can attribute to a place in a makefile.
fn names_a_location(line: &str) -> bool {
    let Some((head, rest)) = line.split_once(':') else {
        return false;
    };
    if head.is_empty() || head.contains(' ') {
        return false;
    }
    let Some((number, _)) = rest.split_once(": ") else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn normalise(line: &str, side: Side, source: &Source) -> Contribution {
    let trimmed = line.trim_start();
    // The name comes off first, because every shape below can wear one and a
    // shape is not recognisable through it. The suite writes our name into its
    // expectations, so `ronin: [Makefile:3: all] Error 1` is a line GNU Make
    // produced and Ronin never does.
    let mut named = None;
    let mut body = trimmed;
    for name in PROGRAM_NAMES {
        if let Some(rest) = diagnostic_body(trimmed, name) {
            named = Some("product-name");
            body = rest;
            break;
        }
    }
    if side == Side::Ours {
        if let Some(payload) = progress_payload(body) {
            return Contribution {
                family: Some("ninja-progress"),
                payload: Some(payload.to_owned()),
                ..Contribution::default()
            };
        }
        // Ninja's failure block is three lines: the banner, the command it ran,
        // and Ronin's own stopped line. The middle one is the command echoed
        // back, not anything the recipe printed.
        if body.starts_with("FAILED:")
            || body.contains("build stopped:")
            || is_shell_invocation(body)
        {
            return Contribution::narration("ninja-failure-block");
        }
        if body.contains("no work to do") {
            return Contribution::narration("no-work-line");
        }
    } else {
        // Make's fatal diagnostics wear a decoration Ronin's do not: `*** ` in
        // front of the message and `  Stop.` after it. Taken off rather than
        // used to delete the line, for the same reason the product name is:
        // what the two tools *said* is the thing worth comparing, and once the
        // decoration is gone `missing separator.` is the same sentence on both
        // sides.
        if let Some(undecorated) = undecorated_fatal(body) {
            return Contribution {
                family: Some("fatal-decoration"),
                diagnostic: named.is_some() || names_a_location(&undecorated),
                residue: Some(undecorated),
                payload: None,
            };
        }
        // Make's counterpart to the failure block, naming the makefile line and
        // the target and giving the recipe's own status: `*** [Makefile:3: all]
        // Error 1`, or the same with `(ignored)` when `-` or `-i` withdrew it.
        if recipe_error_line(body) {
            return Contribution::narration("recipe-error-line");
        }
        if body.contains("Nothing to be done") {
            return Contribution::narration("no-work-line");
        }
        // What Make says under -k about a goal it abandoned. Ronin's stopped
        // line is its counterpart and is already narration.
        if body.starts_with("Target '") && body.ends_with("not remade because of errors.") {
            return Contribution::narration("not-remade-line");
        }
        // GNU Make's `*** Deleting file 'X'` on the .DELETE_ON_ERROR path. Ronin
        // withdraws the failed output silently, which make-delete-on-error-cleanup
        // decided is not owed. The fatal wrapper above does not catch it because
        // it carries no `  Stop.`.
        if body.contains("*** Deleting file '") {
            return Contribution::narration("delete-announce");
        }
        // GNU Make's success-path pattern-peer warning, retired as silent by the
        // 2026-08-17 operator ruling recorded on make-narration-contract-audit.
        if body.contains("pattern recipe did not update peer target '") {
            return Contribution::narration("pattern-peer-warning");
        }
        // GNU Make's jobserver-mode runner messages. A single-scheduler Ronin
        // (make-single-ninja-scheduler) composes recursive Make into one graph
        // and has no jobserver transport, so none of these has a counterpart.
        if body.contains("forced in submake: resetting jobserver mode.")
            || body.contains("jobserver unavailable: using -j1")
            || body.contains("cannot open jobserver ")
        {
            return Contribution::narration("jobserver-narration");
        }
        // GNU Make's debug trace. The suite writes these expectations as bare
        // regexes, which is too weak a shape to read on its own — `/Updating
        // makefiles/` is also something a recipe could print — so it counts
        // only when the run asked for debug output in the first place.
        if source.debug && body.len() > 2 && body.starts_with('/') && body.ends_with('/') {
            return Contribution::narration("debug-trace");
        }
        // Make's other way of saying it did nothing, for a goal that already
        // exists or whose recipe expanded to no command at all. Ronin has no
        // counterpart: it either says nothing or, having made an edge for the
        // goal, counts it and prints `[1/1] build all`.
        if up_to_date_line(body) {
            return Contribution::narration("up-to-date-line");
        }
        // The recipe, echoed. Make prints each line before running it and Ronin
        // prints a progress line instead, so this is the counterpart of
        // `ninja-progress` and the reason it was invisible: nothing in a diff
        // says an unmatched line is the command's text rather than its output.
        // The makefile the test wrote does say, and it is on disk beside the
        // diff, so it is read rather than guessed at.
        if source.recipe.iter().any(|line| echoes(line, body)) {
            return Contribution::narration("recipe-echo");
        }
    }
    if body.contains("Entering directory") || body.contains("Leaving directory") {
        return Contribution::narration("directory-announce");
    }
    Contribution {
        family: named,
        diagnostic: named.is_some() || names_a_location(body),
        residue: Some(body.to_owned()),
        payload: None,
    }
}

/// Make's fatal wrapper taken off a diagnostic, if it wore one.
///
/// `*** ` in front of the message and `  Stop.` behind it, with the location
/// Make already put ahead of the `***` left where it was.
fn undecorated_fatal(body: &str) -> Option<String> {
    let stopped = body.strip_suffix("  Stop.")?;
    let opening = stopped.find("*** ")?;
    let (location, message) = stopped.split_at(opening);
    Some(format!("{location}{}", &message["*** ".len()..]))
}

/// The command a Ninja progress line named, if the line is one.
fn progress_payload(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let (counter, payload) = rest.split_once("] ")?;
    let (finished, total) = counter.split_once('/')?;
    (!finished.is_empty()
        && !total.is_empty()
        && finished.bytes().all(|byte| byte.is_ascii_digit())
        && total.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some(payload)
}

/// Whether a line is the shell command Ninja's failure block echoes back.
///
/// `/bin/sh -c "…"`, and the same behind the two words the launcher line has
/// grown in front of it: the `env` that carries `MAKEFLAGS`, `MAKELEVEL` and
/// `MFLAGS` into a recipe's environment, and the `exec` that replaces the
/// intermediate shell so a signalled child is reported as a signal rather than
/// as `exit 143`. Each arrived with a change to how a recipe is launched and
/// each took the whole block out of this family until it was read here too, so
/// both are stripped in the order they are written rather than either being
/// folded into the other.
fn is_shell_invocation(line: &str) -> bool {
    let mut rest = line.strip_prefix("exec ").unwrap_or(line);
    if let Some(after) = rest.strip_prefix("env ") {
        rest = after;
        while let Some(assignment) = rest.strip_prefix('\'') {
            let Some((binding, tail)) = assignment.split_once('\'') else {
                return false;
            };
            if !binding.contains('=') {
                return false;
            }
            rest = tail.trim_start_matches(' ');
        }
    }
    rest.starts_with("/bin/sh -c \"")
}

/// The files a sweep line named, if the line is one.
///
/// `remove_intermediates` (file.c) writes `rm `, then each file's own name with
/// a single space between them, and nothing else: no options, no quoting and no
/// shell in the way. So a candidate is a payload of plain names, and whether it
/// really is the sweep is settled by the caller against what the build ran.
fn swept_intermediates(line: &str) -> Option<Vec<&str>> {
    let names = line.strip_prefix("rm ")?.split(' ').collect::<Vec<_>>();
    names
        .iter()
        .all(|name| {
            !name.is_empty()
                && !name.starts_with('-')
                && !name.contains(|character: char| "*?[]{}$;&|<>()'\"`\\\t".contains(character))
        })
        .then_some(names)
}

/// Whether a command names this file as a word of its own.
///
/// `cat inter.c > inter.b` names `inter.c`, and so does `cat inter.c>inter.b`;
/// `cat winter.city` names neither, which is the whole point of asking by word
/// rather than by substring.
fn names_file(command: &str, name: &str) -> bool {
    command
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '>' | '<' | '|' | ';' | '&' | '(' | ')' | '"' | '\''
                )
        })
        .any(|word| word == name)
}

/// Whether `printed` is what `recipe` looks like once Make expanded it.
///
/// A recipe line is written with variables in it and echoed with them expanded,
/// so the two are not equal and cannot be compared as strings. What survives
/// expansion is the literal text around each `$…`, in order, which is enough to
/// recognise a line and little enough to be sure: a recipe with no literal text
/// at all matches nothing rather than everything.
fn echoes(recipe: &str, printed: &str) -> bool {
    let mut rest = printed;
    let mut matched_any = false;
    for fragment in literal_fragments(recipe) {
        let Some(position) = rest.find(&fragment) else {
            return false;
        };
        rest = &rest[position + fragment.len()..];
        matched_any = true;
    }
    matched_any
}

/// The parts of a recipe line that expansion leaves alone.
fn literal_fragments(recipe: &str) -> Vec<String> {
    let mut fragments = Vec::new();
    let mut current = String::new();
    let mut characters = recipe.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '$' {
            current.push(character);
            continue;
        }
        // `$$` is a literal dollar the shell will see; anything else opens an
        // expansion whose result cannot be known here.
        match characters.peek() {
            Some('$') => {
                characters.next();
                current.push('$');
                continue;
            }
            Some('(' | '{') => {
                let opening = characters.next().unwrap_or('(');
                let closing = if opening == '(' { ')' } else { '}' };
                let mut depth = 1usize;
                for character in characters.by_ref() {
                    if character == opening {
                        depth += 1;
                    } else if character == closing {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            // `$@`, `$<` and friends: one character names the variable.
            Some(_) => {
                characters.next();
            }
            None => {}
        }
        if !current.trim().is_empty() {
            fragments.push(std::mem::take(&mut current));
        }
        current.clear();
    }
    if !current.trim().is_empty() {
        fragments.push(current);
    }
    fragments
}

/// `*** [Makefile:3: all] Error 1`, with or without a leading `***` and with or
/// without a trailing `(ignored)`.
fn recipe_error_line(line: &str) -> bool {
    let line = line.strip_prefix("*** ").unwrap_or(line);
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let Some((_, tail)) = rest.split_once("] Error ") else {
        return false;
    };
    let status = tail.strip_suffix(" (ignored)").unwrap_or(tail);
    !status.is_empty() && status.bytes().all(|byte| byte.is_ascii_digit())
}

/// What a diagnostic says once the name in front of it is taken off.
///
/// `name: ` or `name[2]: `, which is how both tools mark a line as their own
/// rather than a recipe's, and the level is part of the mark rather than part of
/// the message.
fn diagnostic_body<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?;
    if let Some(body) = rest.strip_prefix(": ") {
        return Some(body);
    }
    let (level, body) = rest.strip_prefix('[')?.split_once("]: ")?;
    (!level.is_empty() && level.bytes().all(|byte| byte.is_ascii_digit())).then_some(body)
}

/// `'all' is up to date.` — Make saying it found nothing to do for a goal.
fn up_to_date_line(line: &str) -> bool {
    line.starts_with('\'') && line.ends_with("' is up to date.")
}

/// The argument-less short switches GNU Make runs together into one cluster.
///
/// `switches[]` in main.c, every `flag` or `flag_off` entry with a single-letter
/// name that goes into the environment, in the order the table lists them:
/// `-h` and `-v` are excluded there by `toenv`, and `-b`/`-m` are ignored
/// switches with no state to report.
const CLUSTER_SWITCHES: &str = "BdeikLnpqrRsStw";

/// Whether a word could be that cluster.
///
/// Every letter is one of the switches and no letter repeats, which is what
/// keeps `rR` and `erR` apart from most words a recipe might print — `keep` has
/// the letters but says one twice. It is still a weak shape on its own, which
/// is why the caller only offers it a word from a makefile that reads
/// MAKEFLAGS.
fn is_switch_cluster(word: &str) -> bool {
    let mut seen = String::new();
    for letter in word.chars() {
        if !CLUSTER_SWITCHES.contains(letter) || seen.contains(letter) {
            return false;
        }
        seen.push(letter);
    }
    !seen.is_empty()
}

/// Whether a line is only switches and command-line assignments.
///
/// What `$(MAKEFLAGS)` expands to, and nothing a recipe is likely to print on
/// its own: every word is either a switch or holds an `=`, and there is at
/// least one word, so an empty line does not qualify.
///
/// GNU Make writes the value in two parts — `define_makeflags` in main.c, and
/// measured on 4.4.1 rather than read off it: the argument-less short switches
/// come first, run together with no dash in front of them, then the switches
/// that carry an argument or have only a long name, each with its own dash,
/// then ` -- ` and the command-line variable definitions. `-e -r -R FOO=bar`
/// comes back as `erR -- FOO=bar`, and `-w` alone as `w`. That leading cluster
/// is what this test used to reject, which sent the whole of variables/MAKEFLAGS
/// into the residue that means "not recognised". It cannot be recognised safely
/// on its own, so `cluster` is only true when the makefile the line came from
/// reads MAKEFLAGS.
fn is_flag_list(line: &str, cluster: bool) -> bool {
    let mut words = 0usize;
    for (index, word) in line.split_ascii_whitespace().enumerate() {
        words += 1;
        let recognised = word == "--"
            || word.starts_with('-')
            || word.contains('=')
            // Only the first word: the cluster is written once, in front.
            || (cluster && index == 0 && is_switch_cluster(word));
        if !recognised {
            return false;
        }
    }
    words > 0
}

/// What the makefile a case ran can tell the classifier about its output.
///
/// The driver leaves a `.run` file beside each `.diff` holding the exact command
/// line, which names the makefile with `-f`. That is how the makefile is found
/// rather than guessed at from the case's name: one script writes several
/// makefiles and the numbering of the two does not have to agree.
#[derive(Default)]
struct Source {
    /// Its recipe lines, as the makefile wrote them.
    recipe: Vec<String>,
    /// Whether it reads `MAKEFLAGS`, which is what lets a residue of
    /// option-shaped words be read as that variable's value rather than as
    /// something a recipe printed.
    reads_makeflags: bool,
    /// Whether the run asked for GNU Make's debug trace, from the command line
    /// or from a `MAKEFLAGS` the makefile sets. Without it a bare regex in the
    /// expected output is not evidence of anything.
    debug: bool,
    /// Whether the run asked for several recipes at once, from the command
    /// line or from a `MAKEFLAGS` the makefile sets. When it did, the order
    /// two lines came out in is the scheduler's answer rather than either
    /// tool's, so it cannot be read as evidence about them.
    concurrent: bool,
}

/// Whether a word is a `-j` spelling that asks for more than one recipe at a
/// time.
///
/// GNU's option takes its count attached — `-j4`, `--jobs=4` — so a separate
/// word after it is a target rather than a count and is not read as one. No
/// count at all is unbounded; `-j1` is the sequential default said out loud,
/// and a test that pins the job count to one is not asking for a race.
fn runs_concurrently(word: &str) -> bool {
    let count = if word == "-j" || word == "--jobs" {
        ""
    } else if let Some(rest) = word.strip_prefix("--jobs=") {
        rest
    } else if let Some(rest) = word.strip_prefix("-j") {
        rest
    } else {
        return false;
    };
    count
        .parse::<usize>()
        .map_or(count.is_empty(), |count| count > 1)
}

impl Source {
    fn read(diff: &Path, tests: &Path) -> Self {
        let run = diff.to_string_lossy().replace(".diff", ".run");
        let Ok(command) = fs::read_to_string(&run) else {
            return Self::default();
        };
        let invoked_with_debug = command
            .split_ascii_whitespace()
            .any(|word| word == "-d" || word == "--debug" || word.starts_with("--debug="));
        let invoked_concurrently = command.split_ascii_whitespace().any(runs_concurrently);
        let mut words = command.split_ascii_whitespace();
        let mut makefile = None;
        while let Some(word) = words.next() {
            if word == "-f" || word == "--file" || word == "--makefile" {
                makefile = words.next();
                break;
            }
        }
        let Some(makefile) = makefile else {
            return Self {
                debug: invoked_with_debug,
                concurrent: invoked_concurrently,
                ..Self::default()
            };
        };
        let Ok(text) = fs::read_to_string(tests.join(makefile)) else {
            return Self {
                debug: invoked_with_debug,
                concurrent: invoked_concurrently,
                ..Self::default()
            };
        };
        Self {
            debug: invoked_with_debug || text.contains("--debug"),
            // A makefile can ask for the jobs itself — `MAKEFLAGS += -j4` is
            // the shape the suite uses to check that it can — and a case that
            // does gets its concurrency from nowhere else. Read off a line
            // that names the variable rather than off the whole text, so a
            // recipe that happens to pass `-j` to something else is not
            // mistaken for the run's own job count.
            concurrent: invoked_concurrently
                || text.lines().any(|line| {
                    line.contains("MAKEFLAGS")
                        && line.split_ascii_whitespace().any(runs_concurrently)
                }),
            recipe: text
                .lines()
                .filter_map(|line| line.strip_prefix('\t'))
                // The prefixes are Make's own and never reach the echoed line:
                // `@` suppresses it, `-` ignores the status, `+` runs it even
                // under -n.
                .map(|line| line.trim_start_matches(['@', '-', '+']).to_owned())
                .filter(|line| !line.trim().is_empty())
                .collect(),
            reads_makeflags: text.contains("MAKEFLAGS"),
        }
    }
}

/// Every family a case exhibits, and the class it therefore belongs to.
///
/// Narration is established by subtraction: strip each side's own narration and
/// compare what is left. Anything remaining is discovery residue, whatever the
/// narration around it looked like. It is not called a compiler defect until a
/// graph/build-effect reproducer establishes that classification.
struct Verdict {
    families: Vec<&'static str>,
    class: Class,
    /// What GNU Make said that narration does not account for.
    expected: Vec<String>,
    /// What Ronin said that narration does not account for.
    actual: Vec<String>,
}

/// The target Ronin refused to build, if it refused one.
fn refusal_of(lines: &[String]) -> Option<&str> {
    lines
        .iter()
        .find(|line| line.contains("no known rule to make it"))
        .and_then(|line| quoted(line))
}

/// The target GNU Make refused to build, if it refused one.
fn refusal_target(lines: &[String]) -> Option<&str> {
    lines
        .iter()
        .find(|line| line.contains("No rule to make target"))
        .and_then(|line| quoted(line))
}

fn quoted(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once('\'')?;
    rest.split_once('\'').map(|(name, _)| name)
}

/// What Ronin's half of a diff came to.
struct OurSide {
    families: Vec<&'static str>,
    residue: Leftovers,
    /// The commands its progress counters named, which are what Make says by
    /// echoing a recipe line.
    commands: Vec<String>,
    /// Whether it refused an option, and so never ran the build at all.
    refused_an_option: bool,
}

/// Read Ronin's lines into narration, leftovers, and the commands it ran.
fn read_our_side(lines: &[String], source: &Source) -> OurSide {
    let mut side = OurSide {
        families: Vec::new(),
        residue: Leftovers::default(),
        commands: Vec::new(),
        refused_an_option: false,
    };
    for line in lines {
        if line.contains("invalid option")
            || line.contains("unrecognized option")
            || line.starts_with("usage: ronin")
        {
            side.refused_an_option = true;
            continue;
        }
        let contribution = normalise(line, Side::Ours, source);
        if let Some(family) = contribution.family
            && !side.families.contains(&family)
        {
            side.families.push(family);
        }
        if let Some(payload) = contribution.payload {
            side.commands.push(payload);
        }
        if let Some(kept) = contribution.residue {
            side.residue.push(kept, contribution.diagnostic);
        }
    }
    side
}

/// What GNU Make's half came to, read against what Ronin already said.
struct TheirSide {
    families: Vec<&'static str>,
    residue: Leftovers,
    /// Whether any of its diagnostics wore Make's fatal wrapper, which is how
    /// a refusal is told from a complaint it carried on past.
    refused_fatally: bool,
}

fn read_their_side(lines: &[String], source: &Source, ours: &OurSide) -> TheirSide {
    let mut side = TheirSide {
        families: Vec::new(),
        residue: Leftovers::default(),
        refused_fatally: false,
    };
    let note = |family: &'static str, families: &mut Vec<&'static str>| {
        if !families.contains(&family) {
            families.push(family);
        }
    };
    for line in lines {
        let contribution = normalise(line, Side::Theirs, source);
        if let Some(family) = contribution.family {
            side.refused_fatally |= family == "fatal-decoration";
            note(family, &mut side.families);
        }
        let Some(kept) = contribution.residue else {
            continue;
        };
        // Make echoed a recipe line and Ronin named the same command in its
        // progress counter. The two are each tool's way of saying it is about
        // to run that command, so they cancel — but only for a line Ronin did
        // not also print as output, which already cancels against itself.
        if !ours.residue.lines.contains(&kept)
            && ours
                .commands
                .iter()
                .any(|command| *command == kept || command.contains(&kept))
        {
            note("recipe-echo", &mut side.families);
            continue;
        }
        // The intermediate files Make swept once the build was over. Ronin
        // deletes the same files and announces nothing, so the line has no
        // counterpart to cancel against and is read from its own payload
        // instead: every name on it has to be one this run's commands named.
        // A recipe that runs `rm` is already accounted for above, and a build
        // that never made the files is not explained by a line about deleting
        // them.
        if let Some(swept) = swept_intermediates(&kept)
            && swept.iter().all(|name| {
                ours.commands
                    .iter()
                    .any(|command| names_file(command, name))
            })
        {
            note("intermediate-sweep", &mut side.families);
            continue;
        }
        side.residue.push(kept, contribution.diagnostic);
    }
    side
}

/// Add a family to the list once, whoever noticed it.
fn note(families: &mut Vec<&'static str>, family: &'static str) {
    if !families.contains(&family) {
        families.push(family);
    }
}

/// Name what the residue is, where it has a recognisable shape, and say whether
/// anything was named.
///
/// Lifted out of `classify` rather than left inline because it is a self
/// contained question — every one of these reads the two residues and the case's
/// own text, and none of them reads the comparison that follows.
fn name_residue(
    divergence: &Divergence,
    source: &Source,
    residual_actual: &Leftovers,
    residual_expected: &Leftovers,
    both_refused: bool,
    families: &mut Vec<&'static str>,
) -> bool {
    // Name what the residue is, where it has a recognisable shape. Each of
    // these was a slice of `evaluation` — the family that means nothing more
    // than "not recognised" — and naming one turns a share of that number into
    // a work item somebody can pick up.
    let residue = || {
        residual_actual
            .lines
            .iter()
            .chain(residual_expected.lines.iter())
    };
    let mut named_residue = false;
    for (marker, family) in [
        ("doesn't support", "unsupported-feature"),
        // The evaluator's other way of saying the same thing, about an
        // automatic variable rather than a directive.
        ("isn't support", "unsupported-feature"),
        ("makefile not found", "no-makefile-found"),
        ("missing separator", "parse-failure"),
        ("(os error ", "io-error-text"),
    ] {
        // Not when Make refused too: `parse-failure` and its neighbours mean
        // Ronin could not read something Make read, and a makefile Make also
        // rejected is not that.
        if !both_refused && residue().any(|line| line.contains(marker)) {
            note(families, family);
            named_residue = true;
        }
    }
    // Both tools tried to run a command that is not there. Make execs it and
    // reports the errno against the command's own name; Ronin hands it to a
    // shell, which reports it in the shell's words. Matched by the command
    // rather than by either sentence, so a case where only one side failed
    // still shows up.
    if residual_expected
        .lines
        .iter()
        .filter_map(|line| line.strip_suffix(": No such file or directory"))
        .any(|command| {
            residual_actual
                .lines
                .iter()
                .filter_map(|line| shell_could_not_find(line))
                .any(|missing| missing == command)
        })
    {
        note(families, "command-not-found-text");
        named_residue = true;
    }
    if let Some(refusal) = refusal_of(&divergence.actual) {
        note(
            families,
            match refusal_target(&divergence.expected) {
                None => "no-rule-to-make",
                Some(theirs) if theirs == refusal => "shared-refusal",
                Some(_) => "refusal-attribution",
            },
        );
        named_residue = true;
    }
    // A residue that is nothing but switches and assignments is a makefile
    // having printed MAKEFLAGS and got a different answer. Recognised by shape
    // rather than by the case's name, so it holds wherever it happens. A
    // makefile that reads the variable may also have written a sentence around
    // the value — `at parse time 'rR -- FOO=bar'` — so for one that does, a
    // quoted flag list counts as well as a bare one.
    if !named_residue
        && residue().next().is_some()
        && residue().all(|line| {
            is_flag_list(line, source.reads_makeflags)
                || (source.reads_makeflags && quotes_a_flag_list(line))
        })
    {
        note(families, "makeflags-content");
        named_residue = true;
    }

    named_residue
}

fn classify(divergence: &Divergence, source: &Source) -> Verdict {
    let ours = read_our_side(&divergence.actual, source);
    if ours.refused_an_option {
        // Exclusive, and deliberately so. Once Ronin has refused the option the
        // test passed, it did not run the build, so nothing else in the diff is
        // evidence about anything: the whole of the output is missing for one
        // known reason. Letting the absence also count as an evaluation
        // difference would file two hundred cases under the one family that is
        // supposed to mean "we do not know why".
        return Verdict {
            families: vec!["option-refused"],
            class: Class::Interface,
            expected: Vec::new(),
            actual: Vec::new(),
        };
    }
    let theirs = read_their_side(&divergence.expected, source, &ours);

    let mut families = ours.families;
    for family in theirs.families {
        note(&mut families, family);
    }
    let mut residual_actual = ours.residue;
    let mut residual_expected = theirs.residue;

    // Both tools refused, and every line either of them left is a diagnostic
    // rather than a build. A refusal is a refusal whatever it is worded like:
    // each tool decided to build nothing, which is the same decision, and
    // `make.narration` puts the sentence each chose outside the contract.
    let both_refused = theirs.refused_fatally
        && residual_expected.all_diagnostics()
        && residual_actual.all_diagnostics();
    // Make refused and Ronin built anyway. The opposite case, and not narration
    // at all: the refusal was the whole of Make's output, so anything Ronin ran
    // is work Make never authorised.
    let built_through_refusal = theirs.refused_fatally
        && !residual_expected.lines.is_empty()
        && !ours.commands.is_empty()
        && !residual_actual.diagnostics.iter().any(|marked| *marked);

    let mut named_residue = name_residue(
        divergence,
        source,
        &residual_actual,
        &residual_expected,
        both_refused,
        &mut families,
    );

    // Read after the families above so the more particular ones still get to
    // name the case; this only reports what is left.
    if both_refused || built_through_refusal {
        if !named_residue {
            note(
                &mut families,
                if both_refused {
                    "shared-refusal"
                } else {
                    "refusal-not-made"
                },
            );
        }
        named_residue = true;
        residual_expected.lines.clear();
        residual_actual.lines.clear();
    }

    let same_lines = residual_actual.lines == residual_expected.lines;
    let permuted = sorted(&residual_actual.lines) == sorted(&residual_expected.lines);
    if named_residue {
        // Already explained. Adding `evaluation` on top would put the case back
        // in the bucket that means "not recognised" and undo the naming.
    } else if same_lines || (permuted && source.concurrent) {
        // Nothing left once both narrations are accounted for. A concurrent
        // run reaches this by the second route as well: the same lines in a
        // different order is Make interleaving each recipe line with what
        // running it printed while Ronin runs the recipe as one script, which
        // is a property of the two tools only when the run was sequential.
        // Under `-j` the order is the scheduler's, so two runs of one case
        // landed either side of this test with nothing having changed and the
        // family became a coin flip the inventory recorded. Both readings say
        // the same lines came out of both tools; a concurrent run is simply
        // not entitled to the further claim about which came out first.
    } else if permuted {
        note(&mut families, "recipe-interleave");
    } else {
        note(&mut families, "evaluation");
    }

    if families.is_empty() {
        // Both sides narrated and neither left a residue, so the whole
        // difference was narration even though no single line named a family.
        families.push("ninja-progress");
    }
    Verdict {
        class: verdict_class(&families),
        families,
        expected: residual_expected.lines,
        actual: residual_actual.lines,
    }
}

/// One side's leftovers, and which of them that tool marked as its own.
///
/// The two travel together because the comparison that decides a case is over
/// the lines, while the question of whether a tool refused or built is over the
/// marks. Keeping the marks beside the lines rather than inside them leaves the
/// comparison exactly as it was.
#[derive(Default)]
struct Leftovers {
    lines: Vec<String>,
    diagnostics: Vec<bool>,
}

impl Leftovers {
    fn push(&mut self, line: String, diagnostic: bool) {
        self.lines.push(line);
        self.diagnostics.push(diagnostic);
    }

    /// Whether this side said something, and everything it said was a
    /// diagnostic — which is what a tool that refused looks like.
    fn all_diagnostics(&self) -> bool {
        !self.lines.is_empty() && self.diagnostics.iter().all(|marked| *marked)
    }
}

/// A case is classified by its strongest evidence.
fn verdict_class(families: &[&'static str]) -> Class {
    families
        .iter()
        .filter_map(|name| FAMILIES.iter().find(|family| family.name == *name))
        .map(|family| family.class)
        .max()
        .unwrap_or(Class::Narration)
}

/// The command a shell reported missing: `/bin/sh: 1: ./thing: not found`.
fn shell_could_not_find(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("/bin/sh: ")?;
    let (number, rest) = rest.split_once(": ")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    rest.strip_suffix(": not found")
}

/// Whether a line is a sentence with `MAKEFLAGS` quoted inside it.
fn quotes_a_flag_list(line: &str) -> bool {
    let mut quoted = line.split('\'').skip(1).step_by(2).peekable();
    quoted.peek().is_some() && quoted.all(|value| value.is_empty() || is_flag_list(value, true))
}

fn sorted(lines: &[String]) -> Vec<String> {
    let mut lines = lines.to_vec();
    lines.sort();
    lines
}

struct Config {
    work: PathBuf,
    inventory: PathBuf,
    update: bool,
    /// Show the residue of this many compiler-candidate or unclassified cases:
    /// raw material for focused graph/build-effect reproducers and for splitting
    /// `evaluation` into families that name something.
    explain: usize,
}

fn parse_arguments() -> Result<Config, String> {
    let mut config = Config {
        work: PathBuf::from("reference/gnumake/tests/work"),
        inventory: PathBuf::from("tests/make_upstream_inventory.tsv"),
        update: false,
        explain: 0,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--work" => {
                config.work = PathBuf::from(arguments.next().ok_or("--work needs a directory")?);
            }
            "--inventory" => {
                config.inventory =
                    PathBuf::from(arguments.next().ok_or("--inventory needs a file")?);
            }
            "--update" => config.update = true,
            "--explain" => {
                config.explain = arguments
                    .next()
                    .ok_or("--explain needs a count")?
                    .parse()
                    .map_err(|_| "--explain needs a number")?;
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(config)
}

/// Every `.diff` the run left, by the case it belongs to.
fn diffs(work: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    let categories =
        fs::read_dir(work).map_err(|error| format!("reading {}: {error}", work.display()))?;
    for category in categories {
        let category = category.map_err(|error| error.to_string())?.path();
        if !category.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&category).map_err(|error| error.to_string())?;
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.contains(".diff") {
                continue;
            }
            let group = category.file_name().unwrap_or_default().to_string_lossy();
            found.push((format!("{group}/{name}"), path.clone()));
        }
    }
    found.sort();
    Ok(found)
}

/// Say what the run found, worst class first and families ranked by weight.
fn report(cases: usize, by_class: &BTreeMap<Class, usize>, by_family: &BTreeMap<&str, usize>) {
    println!("discovery differences: {cases}");
    println!("(not a GNU Make runner-conformance result)");
    println!("by class:");
    for class in [
        Class::Unclassified,
        Class::Compiler,
        Class::Interface,
        Class::Narration,
    ] {
        println!(
            "  {:<12} {}",
            class.name(),
            by_class.get(&class).copied().unwrap_or(0)
        );
    }
    println!("by family:");
    let mut ranked = by_family.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    for (name, count) in ranked {
        let family = FAMILIES.iter().find(|family| family.name == *name);
        println!(
            "  {name:<22} {count:>5}  {}",
            family.map_or("", |family| family.reason)
        );
    }
}

/// The inventory as it should be on disk: its oracle, the families, then cases.
fn record(rows: &[String]) -> String {
    use std::fmt::Write as _;
    let mut inventory = String::new();
    let _ = writeln!(inventory, "oracle\t{ORACLE_VERSION}\t{ORACLE_COMMIT}");
    for family in &FAMILIES {
        let _ = writeln!(
            inventory,
            "family\t{}\t{}\t{}",
            family.name,
            family.class.name(),
            family.reason
        );
    }
    for row in rows {
        let _ = writeln!(inventory, "{row}");
    }
    inventory
}

fn main() -> std::process::ExitCode {
    let config = match parse_arguments() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("make_upstream: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let cases = match diffs(&config.work) {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("make_upstream: {error}");
            eprintln!(
                "Run scripts/check-make-upstream.sh first; this reads what it leaves behind."
            );
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut by_family: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_class: BTreeMap<Class, usize> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut explained = 0usize;
    for (id, path) in &cases {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let source = Source::read(path, &config.work.join(".."));
        let verdict = classify(&read_divergence(&text), &source);
        for family in &verdict.families {
            *by_family.entry(*family).or_default() += 1;
        }
        *by_class.entry(verdict.class).or_default() += 1;
        rows.push(format!(
            "case\t{id}\t{}\t{}",
            verdict.class.name(),
            verdict.families.join("+")
        ));
        if matches!(verdict.class, Class::Compiler | Class::Unclassified)
            && explained < config.explain
        {
            explained += 1;
            println!("--- {id}");
            for line in &verdict.expected {
                println!("  make  | {line}");
            }
            for line in &verdict.actual {
                println!("  ronin | {line}");
            }
        }
    }

    report(cases.len(), &by_class, &by_family);
    let inventory = record(&rows);

    if config.update {
        if let Err(error) = fs::write(&config.inventory, &inventory) {
            eprintln!(
                "make_upstream: writing {}: {error}",
                config.inventory.display()
            );
            return std::process::ExitCode::FAILURE;
        }
        println!("\nrecorded {} case(s)", rows.len());
        return std::process::ExitCode::SUCCESS;
    }

    let recorded = fs::read_to_string(&config.inventory).unwrap_or_default();
    if recorded == inventory || settled(&recorded) == settled(&inventory) {
        return std::process::ExitCode::SUCCESS;
    }
    // A classification that moved is the point of running this, so say what
    // moved rather than only that something did.
    let was = recorded
        .lines()
        .filter(|line| line.starts_with("case\t"))
        .count();
    println!(
        "\nthe classification moved: {was} recorded case(s), {} now.",
        rows.len()
    );
    println!("Re-record with --update once the change is understood.");
    std::process::ExitCode::FAILURE
}

/// The inventory without the one family a rerun can decide differently.
///
/// `recipe-interleave` says the same lines came out in a different order, and
/// the cases that used to flap on it were the ones that asked for concurrency:
/// features/parallelism has three recipes rendezvous through files under `-j4`
/// and they have to overlap to finish at all, so which lands first is the
/// scheduler's and not a property of the code being observed. That source is
/// now settled where the family is decided rather than here — `classify` will
/// not draw the family from a concurrent run's line order at all — so this is
/// a backstop rather than the answer, kept for order nondeterminism that has
/// some cause other than `-j` and would otherwise fail a gate on a rerun.
///
/// Only this family, and only within a class: a case that changed what it is
/// still fails. This was taken out once when .WAIT stopped being the only
/// source of it, which was wrong — the cause was any case that asked for
/// concurrency, and .WAIT was just the one that happened to be flapping.
fn settled(inventory: &str) -> String {
    inventory
        .lines()
        .map(|line| line.replace("+recipe-interleave", ""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        Class, Divergence, Leftovers, Side, Source, classify, echoes, name_residue, normalise,
        read_divergence, record, settled,
    };

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    /// A case whose makefile could not be read, which is what every test below
    /// gets unless it says otherwise.
    fn unread() -> Source {
        Source::default()
    }

    /// A case whose makefile reads MAKEFLAGS, with no recipe lines.
    fn reads_makeflags() -> Source {
        Source {
            reads_makeflags: true,
            ..Source::default()
        }
    }

    /// A case that asked for several recipes at once, so the order its lines
    /// came out in is the scheduler's answer rather than either tool's.
    fn concurrent() -> Source {
        Source {
            concurrent: true,
            ..Source::default()
        }
    }

    #[test]
    fn inventory_names_the_pinned_oracle() {
        assert!(record(&[]).starts_with(
            "oracle\tGNU Make 4.4.1\td66a65ad5a0e31b287f53930b0f09e31801f1613\nfamily\t"
        ));
    }

    /// A case that races decides its own family, so the comparison ignores that
    /// one — but only that one, and only within a class.
    #[test]
    fn an_interleaving_is_not_a_classification_that_moved() {
        let with = "case\tfeatures/parallelism\tnarration\tninja-progress+recipe-interleave";
        let without = "case\tfeatures/parallelism\tnarration\tninja-progress";
        assert_eq!(settled(with), settled(without));

        let changed = "case\tfeatures/parallelism\tcompiler\tninja-progress";
        assert_ne!(settled(with), settled(changed));
    }

    /// When both tools refused, the residue is narration. If GNU Make built the
    /// target, the missing rule is a compiler candidate pending reproduction.
    /// The residue-naming half of `classify`, asked directly now that it has a
    /// name: a marker in either side's leftovers names the family, and the same
    /// marker names nothing when Make refused too, because `parse-failure` means
    /// Ronin could not read something Make read.
    #[test]
    fn a_marker_names_an_unread_residue() {
        let unreadable = || {
            let mut leftovers = Leftovers::default();
            leftovers.push("ronin: f.mk:2: missing separator".to_owned(), true);
            leftovers
        };
        let nothing = Divergence {
            expected: lines(&[]),
            actual: lines(&[]),
        };

        let mut families = Vec::new();
        assert!(name_residue(
            &nothing,
            &unread(),
            &unreadable(),
            &Leftovers::default(),
            false,
            &mut families
        ));
        assert!(families.contains(&"parse-failure"), "{families:?}");

        let mut families = Vec::new();
        assert!(!name_residue(
            &nothing,
            &unread(),
            &unreadable(),
            &Leftovers::default(),
            true,
            &mut families
        ));
        assert!(families.is_empty(), "{families:?}");
    }

    #[test]
    fn shared_refusal_is_narration() {
        let refused = |theirs: &str| {
            classify(
                &Divergence {
                    expected: lines(&[theirs]),
                    actual: lines(&[
                        "ronin: 'hello.c', needed by 'hello', missing and no known rule to make it",
                    ]),
                },
                &unread(),
            )
        };

        let shared =
            refused("ronin: *** No rule to make target 'hello.c', needed by 'hello'.  Stop.");
        assert_eq!(shared.class, Class::Narration);
        assert!(shared.families.contains(&"shared-refusal"));

        // Make walked further into the chain before giving up. Still a refusal
        // on both sides; only the link named differs.
        let attributed =
            refused("ronin: *** No rule to make target 'hello.o', needed by 'hello'.  Stop.");
        assert_eq!(attributed.class, Class::Narration);
        assert!(attributed.families.contains(&"refusal-attribution"));

        let ours_alone = refused("built hello");
        assert_eq!(ours_alone.class, Class::Compiler);
        assert!(ours_alone.families.contains(&"no-rule-to-make"));
    }

    /// The suite writes the name it probed for into its own expectations, so a
    /// shape has to be recognised through the name rather than around it. This
    /// was wrong first time: the name was peeled last, so a recipe-failure line
    /// wearing one was kept as though it were a recipe's output.
    #[test]
    fn a_name_is_peeled_before_a_shape_is_read() {
        let contribution = normalise("ronin: [Makefile:3: all] Error 1", Side::Theirs, &unread());
        assert_eq!(contribution.family, Some("recipe-error-line"));
        assert_eq!(contribution.residue, None);

        let contribution = normalise(
            "ronin: *** No rule to make target 'x'.  Stop.",
            Side::Ours,
            &unread(),
        );
        assert_eq!(contribution.family, Some("product-name"));
        assert_eq!(
            contribution.residue.as_deref(),
            Some("*** No rule to make target 'x'.  Stop.")
        );
    }

    /// Two tools saying the same thing about the same problem cancel; two tools
    /// saying different things do not. Deleting a diagnostic outright would
    /// hide the second case behind the first.
    #[test]
    fn a_diagnostic_keeps_its_body_and_loses_only_its_name() {
        let same = Divergence {
            expected: lines(&["make: *** No rule to make target 'x'.  Stop."]),
            actual: lines(&["ronin: *** No rule to make target 'x'.  Stop."]),
        };
        assert_eq!(classify(&same, &unread()).class, Class::Narration);

        // Only one of them refused, so what the other said is not accounted
        // for and the case stays in the bucket that says so.
        let different = Divergence {
            expected: lines(&["make: *** No rule to make target 'x'.  Stop."]),
            actual: lines(&["something else entirely"]),
        };
        assert_eq!(classify(&different, &unread()).class, Class::Unclassified);
    }

    /// Two fatal refusals are one decision worded twice.
    ///
    /// This narrows the rule above, and deliberately. Make wraps a fatal
    /// diagnostic in `*** ` and `  Stop.`, so a run where both tools left
    /// nothing but diagnostics and one of Make's wore that wrapper is a run
    /// where each tool decided to build nothing — the same decision, whatever
    /// sentence either chose for it, and `make.narration` puts the sentence
    /// outside the contract. Measured on the suite: GNU Make calls
    /// `ifeq(a,b)` a missing separator on the line it appears and Ronin calls
    /// the `endif` below it extraneous, and neither builds anything.
    ///
    /// The reading is only available because Make refused too. When Make
    /// refused and Ronin went on to build, the same evidence says the opposite,
    /// and that is a compiler candidate rather than narration.
    #[test]
    fn two_refusals_are_one_decision() {
        let both = Divergence {
            expected: lines(&["f.mk:2: *** missing separator.  Stop."]),
            actual: lines(&["ronin: f.mk:4: extraneous `endif'."]),
        };
        let verdict = classify(&both, &unread());
        assert_eq!(verdict.class, Class::Narration);
        assert!(verdict.families.contains(&"shared-refusal"));
    }

    /// The same evidence read the other way. Make refused and Ronin went on to
    /// build, so what it built is work Make never authorised.
    #[test]
    fn an_ignored_refusal_is_a_gap() {
        let ignored = Divergence {
            expected: lines(&["make: *** No targets.  Stop."]),
            actual: lines(&["[1/1] printf hello", "hello"]),
        };
        let verdict = classify(&ignored, &unread());
        assert_eq!(verdict.class, Class::Compiler);
        assert!(verdict.families.contains(&"refusal-not-made"));
    }

    /// Make's fatal wrapper comes off so the sentence underneath can be
    /// compared, and stays off only where it was actually worn.
    #[test]
    fn a_fatal_wrapper_comes_off() {
        assert_eq!(
            super::undecorated_fatal("f.mk:2: *** missing separator.  Stop.").as_deref(),
            Some("f.mk:2: missing separator.")
        );
        assert_eq!(
            super::undecorated_fatal("*** No targets.  Stop.").as_deref(),
            Some("No targets.")
        );
        // A recipe's failure line carries the stars without the full stop, and
        // has its own family.
        assert_eq!(
            super::undecorated_fatal("*** [Makefile:3: all] Error 1"),
            None
        );
        assert_eq!(super::undecorated_fatal("cc -c foo.c"), None);
    }

    /// The counter's payload is the command Ronin is about to run, which is the
    /// same thing Make says by echoing the recipe line. Reading it off the run
    /// beats reading the makefile: one script writes a makefile several times
    /// over, and only the last of them is still on disk when this runs.
    #[test]
    fn a_counter_names_the_recipe() {
        let echoed = Divergence {
            expected: lines(&["touch hello.z"]),
            actual: lines(&["[1/1] touch hello.z"]),
        };
        let verdict = classify(&echoed, &unread());
        assert_eq!(verdict.class, Class::Narration);
        assert!(verdict.families.contains(&"recipe-echo"));

        // A command Ronin never named is still unexplained.
        let unexplained = Divergence {
            expected: lines(&["touch hello.z"]),
            actual: lines(&["[1/1] touch something.else"]),
        };
        assert_eq!(classify(&unexplained, &unread()).class, Class::Unclassified);
    }

    /// Make deletes the intermediate files it made and says so; Ronin deletes
    /// the same files and says nothing, which is the whole of the difference in
    /// features/vpathplus's intermediate cases. The line is read from its own
    /// payload — the names it lists — and explains only itself: a case with
    /// anything else left over is still unclassified.
    #[test]
    fn a_sweep_line_explains_only_itself() {
        let built = || {
            lines(&[
                "[1/3] cat vp/inter.d > inter.c",
                "[2/3] cat inter.c > inter.b 2>/dev/null || exit 1",
                "[3/3] cat inter.b > inter.a",
            ])
        };
        let swept = Divergence {
            expected: lines(&[
                "cat vp/inter.d > inter.c",
                "cat inter.c > inter.b 2>/dev/null || exit 1",
                "cat inter.b > inter.a",
                "rm inter.c",
            ]),
            actual: built(),
        };
        let verdict = classify(&swept, &unread());
        assert_eq!(verdict.class, Class::Narration);
        assert!(
            verdict.families.contains(&"intermediate-sweep"),
            "{:?}",
            verdict.families
        );

        // Explains only itself. Another unaccounted line and the case stays in
        // the bucket that says the residue is not understood.
        let mut expected = swept.expected.clone();
        expected.push("something else entirely".to_owned());
        let verdict = classify(
            &Divergence {
                expected,
                actual: built(),
            },
            &unread(),
        );
        assert_eq!(verdict.class, Class::Unclassified);
        assert!(
            verdict.families.contains(&"intermediate-sweep"),
            "{:?}",
            verdict.families
        );

        // A name this run's commands never mentioned is not this run's sweep.
        let unrelated = Divergence {
            expected: lines(&["rm elsewhere.o"]),
            actual: lines(&["[1/1] cat a > b"]),
        };
        let verdict = classify(&unrelated, &unread());
        assert_eq!(verdict.class, Class::Unclassified);
        assert!(
            !verdict.families.contains(&"intermediate-sweep"),
            "{:?}",
            verdict.families
        );
    }

    /// What the sweep can look like, read off `remove_intermediates`: `rm `, the
    /// names, single spaces, nothing else.
    #[test]
    fn a_sweep_line_is_only_names() {
        assert_eq!(
            super::swept_intermediates("rm inter.b inter.c"),
            Some(vec!["inter.b", "inter.c"])
        );
        assert_eq!(super::swept_intermediates("rm"), None);
        assert_eq!(super::swept_intermediates("rm "), None);
        assert_eq!(super::swept_intermediates("rm -f inter.c"), None);
        assert_eq!(super::swept_intermediates("rm inter.c && echo gone"), None);
        assert_eq!(super::swept_intermediates("rm $(FILES)"), None);
        assert_eq!(super::swept_intermediates("cat inter.b > inter.a"), None);

        assert!(super::names_file("cat inter.c > inter.b", "inter.c"));
        assert!(super::names_file("cat inter.c>inter.b", "inter.b"));
        assert!(!super::names_file("cat winter.city > x", "inter.c"));
    }

    /// The `exec` that replaces the intermediate shell and the `env` that
    /// carries MAKEFLAGS into a recipe's environment both sit in front of the
    /// shell Ninja's failure block echoes back. Reading the block through them
    /// is what keeps the block narration; it was not, first when those
    /// variables started being exported and again when the launcher was
    /// dropped in favour of `exec`.
    #[test]
    fn the_block_reads_through_its_launcher() {
        assert!(super::is_shell_invocation(r#"/bin/sh -c "exit 1""#));
        assert!(super::is_shell_invocation(
            r#"env 'MAKEFLAGS=i' 'MAKELEVEL=1' 'MFLAGS=-i' /bin/sh -c "exit 1""#
        ));
        assert!(super::is_shell_invocation(
            r#"exec env 'MAKEFLAGS=' 'MAKELEVEL=1' 'MFLAGS=' /bin/sh -c "exit 1""#
        ));
        // `exec` on its own, for a recipe with nothing to export.
        assert!(super::is_shell_invocation(r#"exec /bin/sh -c "exit 1""#));
        // Not anything at all that begins with either word.
        assert!(!super::is_shell_invocation("env | sort"));
        assert!(!super::is_shell_invocation("exec 3< thing"));
        assert!(!super::is_shell_invocation("echo /bin/sh -c \"x\""));
    }

    /// A refused option explains the whole of a diff, because the build never
    /// ran. Letting the missing output also count would file the entire family
    /// under the one that means "we do not know why".
    #[test]
    fn a_refused_option_explains_everything_after_it() {
        let refused = Divergence {
            expected: lines(&["hello", "world"]),
            actual: lines(&["ronin: invalid option -- 'I'", "usage: ronin [options]"]),
        };
        let verdict = classify(&refused, &unread());
        assert_eq!(verdict.class, Class::Interface);
        assert_eq!(verdict.families, vec!["option-refused"]);
    }

    /// Same lines, different order: Make interleaves each recipe line with what
    /// running it printed, and Ronin runs the recipe as one script.
    #[test]
    fn the_same_lines_in_another_order_are_an_interleaving() {
        let shuffled = Divergence {
            expected: lines(&["echo one", "one", "echo two", "two"]),
            actual: lines(&["echo one", "echo two", "one", "two"]),
        };
        let verdict = classify(&shuffled, &unread());
        assert_eq!(verdict.class, Class::Narration);
        assert!(verdict.families.contains(&"recipe-interleave"));
    }

    /// Under `-j` the order is the scheduler's, so the same case classified
    /// itself differently on consecutive runs with nothing changed. A
    /// concurrent run gets the same answer either way round: the lines are
    /// accounted for, and no family is drawn from which of them landed first.
    #[test]
    fn a_concurrent_run_names_no_interleaving() {
        let ordered = Divergence {
            expected: lines(&["one", "two"]),
            actual: lines(&["one", "two"]),
        };
        let shuffled = Divergence {
            expected: lines(&["one", "two"]),
            actual: lines(&["two", "one"]),
        };
        for divergence in [&ordered, &shuffled] {
            let verdict = classify(divergence, &concurrent());
            assert_eq!(verdict.class, Class::Narration);
            assert_eq!(verdict.families, vec!["ninja-progress"]);
        }
        // A residue that is not the same lines at all is still unexplained,
        // whatever the job count was.
        let different = Divergence {
            expected: lines(&["one", "two"]),
            actual: lines(&["one", "three"]),
        };
        assert_eq!(
            classify(&different, &concurrent()).class,
            Class::Unclassified
        );
    }

    /// The job count is read where one is written, so a case that pins itself
    /// to one recipe at a time keeps the order as evidence.
    #[test]
    fn a_pinned_job_count_is_sequential() {
        for word in ["-j", "-j4", "--jobs", "--jobs=10"] {
            assert!(super::runs_concurrently(word), "{word}");
        }
        for word in ["-j1", "--jobs=1", "-k", "hello", "-jobs", ""] {
            assert!(!super::runs_concurrently(word), "{word}");
        }
    }

    /// The suite writes `diff -c`, whose halves are split by the `--- N,M ----`
    /// banner rather than by the marker characters, which both halves share.
    #[test]
    fn a_context_diff_splits_at_its_banner() {
        let divergence = read_divergence(concat!(
            "*** work/a.base\tThu\n",
            "--- work/a.log\tThu\n",
            "***************\n",
            "*** 1 ****\n",
            "! expected\n",
            "--- 1,2 ----\n",
            "! actual\n",
            "+ extra\n",
        ));
        assert_eq!(divergence.expected, lines(&["expected"]));
        assert_eq!(divergence.actual, lines(&["actual", "extra"]));
    }

    /// A shape only counts as narration on the side that can produce it. Make
    /// never prints Ninja's counter, so a bracket-shaped line on its side is a
    /// recipe's own output and deleting it would fake an agreement.
    #[test]
    fn a_shape_is_narration_only_on_the_side_that_makes_it() {
        assert_eq!(
            normalise("[1/2] build all", Side::Ours, &unread()).family,
            Some("ninja-progress")
        );
        let contribution = normalise("[1/2] build all", Side::Theirs, &unread());
        assert_eq!(contribution.family, None);
        assert_eq!(contribution.residue.as_deref(), Some("[1/2] build all"));
    }

    /// A recipe is written with variables and echoed with them expanded, so the
    /// two are never equal. What survives is the literal text around each `$…`.
    #[test]
    fn a_recipe_is_recognised_through_its_expansion() {
        assert!(echoes("cc -c $(SRC) -o $@", "cc -c main.c -o main.o"));
        assert!(echoes("echo hi", "echo hi"));
        // A recipe that is nothing but an expansion matches nothing, rather
        // than matching everything for having no literal text to disagree with.
        assert!(!echoes("$(CMD)", "anything at all"));
        assert!(!echoes("cc -c $(SRC)", "ld -r main.o"));
    }

    /// A residue with a recognised shape is named and does not also fall into
    /// `evaluation`, which exists to mean "not recognised".
    #[test]
    fn a_named_residue_leaves_the_unknown_bucket() {
        let unsupported = Divergence {
            expected: lines(&["one"]),
            actual: lines(&["f.mk:2: kati doesn't support .SECONDEXPANSION"]),
        };
        let verdict = classify(&unsupported, &unread());
        assert_eq!(verdict.class, Class::Compiler);
        assert!(verdict.families.contains(&"unsupported-feature"));
        assert!(!verdict.families.contains(&"evaluation"));
    }

    /// A residue that is only switches and assignments is a makefile having
    /// printed MAKEFLAGS, recognised by its shape so it holds wherever it
    /// happens rather than only in the category named after the variable.
    #[test]
    fn a_residue_of_only_switches_is_a_makeflags_value() {
        let flags = Divergence {
            expected: Vec::new(),
            actual: lines(&["-S", "-k"]),
        };
        let verdict = classify(&flags, &unread());
        assert_eq!(verdict.class, Class::Interface);
        assert!(verdict.families.contains(&"makeflags-content"));

        assert!(super::is_flag_list("-S", false));
        assert!(super::is_flag_list("-ks -- FOO=bar", false));
        // A recipe's output is not a flag list just for starting with a dash.
        assert!(!super::is_flag_list("-n is what we printed", false));
        assert!(!super::is_flag_list("", false));
    }

    /// GNU Make writes the switches that take no argument as one dashless
    /// cluster in front of everything else, which is the shape that used to
    /// send the whole of variables/MAKEFLAGS into the unclassified residue.
    /// Measured on 4.4.1: `-e -r -R FOO=bar` reads back as `erR -- FOO=bar`.
    #[test]
    fn a_dashless_cluster_needs_the_makefile() {
        // Not on its own: `erR` is also a word, and guessing here would swallow
        // whatever a recipe happened to print.
        assert!(!super::is_flag_list("erR -- FOO=bar", false));
        assert!(super::is_flag_list("erR -- FOO=bar", true));
        assert!(super::is_flag_list("w", true));
        assert!(super::is_flag_list(
            "krR --no-print-directory -- hello:=world",
            true
        ));
        // Only in front. A cluster after a dashed switch is not how the value
        // is written, so a line like that is something else.
        assert!(!super::is_flag_list("--trace erR", true));
        // A word whose letters are not all switches, or that says one twice.
        assert!(!super::is_flag_list("hello", true));
        assert!(!super::is_flag_list("keep", true));

        let cluster = Divergence {
            expected: lines(&["erR -- hello:=world FOO=bar"]),
            actual: lines(&["erR -- hello:=world FOO=bar --no-print-directory"]),
        };
        assert_eq!(classify(&cluster, &unread()).class, Class::Unclassified);
        let verdict = classify(&cluster, &reads_makeflags());
        assert_eq!(verdict.class, Class::Interface);
        assert!(verdict.families.contains(&"makeflags-content"));
    }

    /// Make's other way of saying it did nothing. Measured on GNU Make 4.4.1: a
    /// goal whose recipe expands to no command at all draws `'all' is up to
    /// date.`, where Ronin makes an edge for it and prints `[1/1] build all`.
    #[test]
    fn up_to_date_is_narration() {
        let contribution = normalise("make: 'all' is up to date.", Side::Theirs, &unread());
        assert_eq!(contribution.family, Some("up-to-date-line"));
        assert_eq!(contribution.residue, None);

        // Only on Make's side, like every other shape here: Ronin does not say
        // it, so a line like this from us is something a recipe printed.
        let contribution = normalise("'all' is up to date.", Side::Ours, &unread());
        assert_eq!(contribution.family, None);
        assert_eq!(
            contribution.residue.as_deref(),
            Some("'all' is up to date.")
        );

        // It explains only itself. Output Ronin produced and Make did not still
        // leaves the case unclassified rather than disappearing behind it.
        let alone = Divergence {
            expected: lines(&["make: 'all' is up to date."]),
            actual: Vec::new(),
        };
        assert_eq!(classify(&alone, &unread()).class, Class::Narration);
        let with_residue = Divergence {
            expected: lines(&["make: 'all' is up to date."]),
            actual: lines(&["something we built"]),
        };
        assert_eq!(
            classify(&with_residue, &unread()).class,
            Class::Unclassified
        );
    }
}
