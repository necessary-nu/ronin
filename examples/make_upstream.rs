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

const FAMILIES: [Family; 18] = [
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
        reason: "Make echoes the recipe line it is about to run; Ronin prints a progress line instead. Recognised by reading the makefile the case ran, not by guessing.",
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
        name: "recipe-interleave",
        class: Class::Narration,
        reason: "the same lines in a different order: Make interleaves each recipe line with the output of running it, and Ronin runs the recipe as one script.",
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

/// What a line contributes: the family it belongs to, and what is left of it.
///
/// Narration is not simply deleted. A diagnostic keeps its body and loses only
/// the name in front, so two tools saying the same thing about the same problem
/// cancel out while two tools saying *different* things still show up. Deleting
/// the whole line would hide a wrong message behind a right prefix.
fn normalise(line: &str, side: Side, recipe: &[String]) -> (Option<&'static str>, Option<String>) {
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
        if is_progress_line(body) {
            return (Some("ninja-progress"), None);
        }
        // Ninja's failure block is three lines: the banner, the command it ran,
        // and Ronin's own stopped line. The middle one is the command echoed
        // back, not anything the recipe printed.
        if body.starts_with("FAILED:")
            || body.contains("build stopped:")
            || body.starts_with("/bin/sh -c \"")
        {
            return (Some("ninja-failure-block"), None);
        }
        if body.contains("no work to do") {
            return (Some("no-work-line"), None);
        }
    } else {
        // Make's counterpart to the failure block, naming the makefile line and
        // the target and giving the recipe's own status: `*** [Makefile:3: all]
        // Error 1`, or the same with `(ignored)` when `-` or `-i` withdrew it.
        if recipe_error_line(body) {
            return (Some("recipe-error-line"), None);
        }
        if body.contains("Nothing to be done") {
            return (Some("no-work-line"), None);
        }
        // The recipe, echoed. Make prints each line before running it and Ronin
        // prints a progress line instead, so this is the counterpart of
        // `ninja-progress` and the reason it was invisible: nothing in a diff
        // says an unmatched line is the command's text rather than its output.
        // The makefile the test wrote does say, and it is on disk beside the
        // diff, so it is read rather than guessed at.
        if recipe.iter().any(|line| echoes(line, body)) {
            return (Some("recipe-echo"), None);
        }
    }
    if body.contains("Entering directory") || body.contains("Leaving directory") {
        return (Some("directory-announce"), None);
    }
    (named, Some(body.to_owned()))
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

/// `[3/7] ` — Ninja's counter, which a Makefile never asked for.
fn is_progress_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let Some((counter, _)) = rest.split_once("] ") else {
        return false;
    };
    match counter.split_once('/') {
        Some((finished, total)) => {
            !finished.is_empty()
                && !total.is_empty()
                && finished.bytes().all(|b| b.is_ascii_digit())
                && total.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
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

/// Whether a line is only switches and command-line assignments.
///
/// What `$(MAKEFLAGS)` expands to, and nothing a recipe is likely to print on
/// its own: every word is either a switch or holds an `=`, and there is at
/// least one word, so an empty line does not qualify.
fn is_flag_list(line: &str) -> bool {
    let mut words = line.split_ascii_whitespace().peekable();
    words.peek().is_some()
        && words.all(|word| word == "--" || word.starts_with('-') || word.contains('='))
}

/// The recipe lines of the makefile a case ran, as the Makefile wrote them.
///
/// The driver leaves a `.run` file beside each `.diff` holding the exact command
/// line, which names the makefile with `-f`. That is how the makefile is found
/// rather than guessed at from the case's name: one script writes several
/// makefiles and the numbering of the two does not have to agree.
fn recipe_of(diff: &Path, tests: &Path) -> Vec<String> {
    let run = diff.to_string_lossy().replace(".diff", ".run");
    let Ok(command) = fs::read_to_string(&run) else {
        return Vec::new();
    };
    let mut words = command.split_ascii_whitespace();
    let mut makefile = None;
    while let Some(word) = words.next() {
        if word == "-f" || word == "--file" || word == "--makefile" {
            makefile = words.next();
            break;
        }
    }
    let Some(makefile) = makefile else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(tests.join(makefile)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| line.strip_prefix('\t'))
        // The prefixes are Make's own and never reach the echoed line: `@`
        // suppresses it, `-` ignores the status, `+` runs it even under -n.
        .map(|line| line.trim_start_matches(['@', '-', '+']).to_owned())
        .filter(|line| !line.trim().is_empty())
        .collect()
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

fn classify(divergence: &Divergence, recipe: &[String]) -> Verdict {
    let mut families = Vec::new();
    let mut note = |family: &'static str| {
        if !families.contains(&family) {
            families.push(family);
        }
    };

    let mut residual_actual = Vec::new();
    let mut refused = false;
    for line in &divergence.actual {
        if line.contains("invalid option")
            || line.contains("unrecognized option")
            || line.starts_with("usage: ronin")
        {
            refused = true;
            continue;
        }
        let (family, kept) = normalise(line, Side::Ours, recipe);
        if let Some(family) = family {
            note(family);
        }
        if let Some(kept) = kept {
            residual_actual.push(kept);
        }
    }
    if refused {
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

    let mut residual_expected = Vec::new();
    for line in &divergence.expected {
        let (family, kept) = normalise(line, Side::Theirs, recipe);
        if let Some(family) = family {
            note(family);
        }
        if let Some(kept) = kept {
            residual_expected.push(kept);
        }
    }

    // Name what the residue is, where it has a recognisable shape. Each of
    // these was a slice of `evaluation` — the family that means nothing more
    // than "not recognised" — and naming one turns a share of that number into
    // a work item somebody can pick up.
    let residue = || residual_actual.iter().chain(residual_expected.iter());
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
        if residue().any(|line| line.contains(marker)) {
            note(family);
            named_residue = true;
        }
    }
    if let Some(refusal) = refusal_of(&divergence.actual) {
        note(match refusal_target(&divergence.expected) {
            None => "no-rule-to-make",
            Some(theirs) if theirs == refusal => "shared-refusal",
            Some(_) => "refusal-attribution",
        });
        named_residue = true;
    }
    // A residue that is nothing but switches and assignments is a makefile
    // having printed MAKEFLAGS and got a different answer. Recognised by shape
    // rather than by the case's name, so it holds wherever it happens.
    if !named_residue && residue().next().is_some() && residue().all(|line| is_flag_list(line)) {
        note("makeflags-content");
        named_residue = true;
    }

    if named_residue {
        // Already explained. Adding `evaluation` on top would put the case back
        // in the bucket that means "not recognised" and undo the naming.
    } else if residual_actual == residual_expected {
        // Nothing left once both narrations are accounted for.
    } else if sorted(&residual_actual) == sorted(&residual_expected) {
        // The same lines in a different order, which is Make interleaving each
        // recipe line with what running it printed.
        note("recipe-interleave");
    } else {
        note("evaluation");
    }

    if families.is_empty() {
        // Both sides narrated and neither left a residue, so the whole
        // difference was narration even though no single line named a family.
        families.push("ninja-progress");
    }
    let class = families
        .iter()
        .filter_map(|name| FAMILIES.iter().find(|f| f.name == *name))
        .map(|family| family.class)
        .max()
        .unwrap_or(Class::Narration);
    Verdict {
        families,
        class,
        expected: residual_expected,
        actual: residual_actual,
    }
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
        let recipe = recipe_of(path, &config.work.join(".."));
        let verdict = classify(&read_divergence(&text), &recipe);
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
/// `recipe-interleave` says the same lines came out in a different order. Some
/// of the suite's cases arrange for exactly that — features/parallelism has
/// three recipes rendezvous through files under -j4, and they have to overlap
/// to finish at all — so which lands first is the scheduler's and not a
/// property of the code being observed. Recording a coin flip and then failing
/// when it lands the other way makes the discovery inventory report noise.
///
/// Only this family, and only within a class: a case that changed what it is
/// still fails. This was taken out once when .WAIT stopped being the only
/// source of it, which was wrong — the cause is any case that asks for
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
        classify, echoes, normalise, read_divergence, record, settled, Class, Divergence, Side,
    };

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
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
                &[],
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
        let (family, kept) = normalise("ronin: [Makefile:3: all] Error 1", Side::Theirs, &[]);
        assert_eq!(family, Some("recipe-error-line"));
        assert_eq!(kept, None);

        let (family, kept) = normalise(
            "ronin: *** No rule to make target 'x'.  Stop.",
            Side::Ours,
            &[],
        );
        assert_eq!(family, Some("product-name"));
        assert_eq!(
            kept.as_deref(),
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
        assert_eq!(classify(&same, &[]).class, Class::Narration);

        let different = Divergence {
            expected: lines(&["make: *** No rule to make target 'x'.  Stop."]),
            actual: lines(&["ronin: *** something else entirely.  Stop."]),
        };
        assert_eq!(classify(&different, &[]).class, Class::Unclassified);
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
        let verdict = classify(&refused, &[]);
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
        let verdict = classify(&shuffled, &[]);
        assert_eq!(verdict.class, Class::Narration);
        assert!(verdict.families.contains(&"recipe-interleave"));
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
            normalise("[1/2] build all", Side::Ours, &[]).0,
            Some("ninja-progress")
        );
        let (family, kept) = normalise("[1/2] build all", Side::Theirs, &[]);
        assert_eq!(family, None);
        assert_eq!(kept.as_deref(), Some("[1/2] build all"));
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
        let verdict = classify(&unsupported, &[]);
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
        let verdict = classify(&flags, &[]);
        assert_eq!(verdict.class, Class::Interface);
        assert!(verdict.families.contains(&"makeflags-content"));

        assert!(super::is_flag_list("-S"));
        assert!(super::is_flag_list("-ks -- FOO=bar"));
        // A recipe's output is not a flag list just for starting with a dash.
        assert!(!super::is_flag_list("-n is what we printed"));
        assert!(!super::is_flag_list(""));
        // The bare letter group MAKEFLAGS leads with is deliberately not
        // recognised: `ks` is indistinguishable from a word a recipe printed,
        // and guessing there would swallow real output.
        assert!(!super::is_flag_list("ks -- FOO=bar"));
    }
}
