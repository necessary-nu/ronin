//! Classify what GNU Make's own test suite says about Ronin's Make mode.
//!
//! The suite compares stdout byte for byte against GNU Make's, and Ronin is not
//! trying to produce GNU Make's output — it narrates a build the way Ninja does,
//! because that is the product. So its pass rate is not the measure of anything
//! and chasing it would be chasing a number we have decided not to want.
//!
//! What the suite is good for is the other question: given the same Makefile,
//! did we evaluate it to the same thing, run the same recipes, and finish with
//! the same status? That is a semantic question, and the narration sits on top
//! of it as noise. This separates the two.
//!
//! The method is subtraction rather than pattern-matching the whole diff. Each
//! side has its own narration — ours is Ninja's progress line and `FAILED:`
//! block, Make's is its own name in front of a diagnostic — so both are removed
//! and whatever remains is compared. A case whose residue matches differs only
//! in how the two tools talk. A case whose residue does not is a defect, and
//! that count is the number worth reporting.
//!
//! Usage: `make_upstream --work DIR [--inventory FILE] [--update]`

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// How much a difference matters, worst first.
///
/// A case is classified by the worst thing in it: a diff that shows both a
/// progress line and a wrong variable value is a defect, because the wrong
/// value is still wrong once the progress line is accounted for.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Class {
    /// Ronin says it differently on purpose. Not a defect and never will be.
    Narration,
    /// A Make feature Ronin does not have yet. A defect, but a known-shaped one
    /// that is somebody's node rather than a mystery.
    Capability,
    /// The Makefile evaluated to something else, or the build did something
    /// else. This is the number that matters.
    Semantic,
}

impl Class {
    const fn name(self) -> &'static str {
        match self {
            Self::Narration => "narration",
            Self::Capability => "capability",
            Self::Semantic => "semantic",
        }
    }
}

/// One recognisable reason a case differs.
struct Family {
    name: &'static str,
    class: Class,
    reason: &'static str,
}

const FAMILIES: [Family; 9] = [
    Family {
        name: "recipe-error-line",
        class: Class::Narration,
        reason: "Make names the makefile line, the target and the recipe's status in one line; Ronin reports a failure Ninja's way.",
    },
    Family {
        name: "ninja-progress",
        class: Class::Narration,
        reason: "Ronin prints Ninja's [N/M] progress line where Make echoes the recipe.",
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
        class: Class::Capability,
        reason: "Ronin refused an option the test passed and GNU Make accepted.",
    },
    Family {
        name: "evaluation",
        class: Class::Semantic,
        reason: "the Makefile evaluated to something else, or the build did something else: a different value, a different rule, a different file, a different status.",
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

/// What a line contributes: the family it belongs to, and what is left of it.
///
/// Narration is not simply deleted. A diagnostic keeps its body and loses only
/// the name in front, so two tools saying the same thing about the same problem
/// cancel out while two tools saying *different* things still show up. Deleting
/// the whole line would hide a wrong message behind a right prefix.
fn normalise(line: &str) -> (Option<&'static str>, Option<String>) {
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
    if is_progress_line(body) {
        return (Some("ninja-progress"), None);
    }
    // Ninja's failure block is three lines: the banner, the command it ran, and
    // Ronin's own stopped line. The middle one is the command echoed back, not
    // anything the recipe printed.
    if body.starts_with("FAILED:")
        || body.contains("build stopped:")
        || body.starts_with("/bin/sh -c \"")
    {
        return (Some("ninja-failure-block"), None);
    }
    // Make's counterpart, which names the makefile line and the target and
    // gives the recipe's own status: `*** [Makefile:3: all] Error 1`, or the
    // same with `(ignored)` when `-` or `-i` withdrew the failure.
    if recipe_error_line(body) {
        return (Some("recipe-error-line"), None);
    }
    // The same fact in two vocabularies: Ronin reports having found nothing to
    // do, Make reports having nothing to be done.
    if body.contains("no work to do") || body.contains("Nothing to be done") {
        return (Some("no-work-line"), None);
    }
    if body.contains("Entering directory") || body.contains("Leaving directory") {
        return (Some("directory-announce"), None);
    }
    (named, Some(body.to_owned()))
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

/// Every family a case exhibits, and the class it therefore belongs to.
///
/// Narration is established by subtraction: strip each side's own narration and
/// compare what is left. Anything remaining is a real difference, whatever the
/// narration around it looked like — which is the whole point, because a case
/// can show a progress line and a wrong value at once and only the second one
/// is worth anybody's time.
struct Verdict {
    families: Vec<&'static str>,
    class: Class,
    /// What GNU Make said that narration does not account for.
    expected: Vec<String>,
    /// What Ronin said that narration does not account for.
    actual: Vec<String>,
}

fn classify(divergence: &Divergence) -> Verdict {
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
        let (family, kept) = normalise(line);
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
            class: Class::Capability,
            expected: Vec::new(),
            actual: Vec::new(),
        };
    }

    let mut residual_expected = Vec::new();
    for line in &divergence.expected {
        let (family, kept) = normalise(line);
        if let Some(family) = family {
            note(family);
        }
        if let Some(kept) = kept {
            residual_expected.push(kept);
        }
    }

    if residual_actual == residual_expected {
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
    /// Show the residue of this many semantic cases: what was left over once
    /// both narrations were subtracted, which is the evidence for calling them
    /// defects and the raw material for splitting `evaluation` into families
    /// that name something.
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
    println!("failing cases: {cases}");
    println!("by class:");
    for class in [Class::Semantic, Class::Capability, Class::Narration] {
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

/// The inventory as it should be on disk: the families, then a row per case.
fn record(rows: &[String]) -> String {
    use std::fmt::Write as _;
    let mut inventory = String::new();
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
        let verdict = classify(&read_divergence(&text));
        for family in &verdict.families {
            *by_family.entry(*family).or_default() += 1;
        }
        *by_class.entry(verdict.class).or_default() += 1;
        rows.push(format!(
            "case\t{id}\t{}\t{}",
            verdict.class.name(),
            verdict.families.join("+")
        ));
        if verdict.class == Class::Semantic && explained < config.explain {
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
    if recorded == inventory {
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

#[cfg(test)]
mod tests {
    use super::{classify, normalise, read_divergence, Class, Divergence};

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    /// The suite writes the name it probed for into its own expectations, so a
    /// shape has to be recognised through the name rather than around it. This
    /// was wrong first time: the name was peeled last, so a recipe-failure line
    /// wearing one was kept as though it were a recipe's output.
    #[test]
    fn a_name_is_peeled_before_a_shape_is_read() {
        let (family, kept) = normalise("ronin: [Makefile:3: all] Error 1");
        assert_eq!(family, Some("recipe-error-line"));
        assert_eq!(kept, None);

        let (family, kept) = normalise("ronin: *** No rule to make target 'x'.  Stop.");
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
        assert_eq!(classify(&same).class, Class::Narration);

        let different = Divergence {
            expected: lines(&["make: *** No rule to make target 'x'.  Stop."]),
            actual: lines(&["ronin: *** something else entirely.  Stop."]),
        };
        assert_eq!(classify(&different).class, Class::Semantic);
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
        let verdict = classify(&refused);
        assert_eq!(verdict.class, Class::Capability);
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
        let verdict = classify(&shuffled);
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
}
