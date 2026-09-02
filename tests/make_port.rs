//! GNU Make's behaviour, ported into cases this repository owns.
//!
//! Each build-intent case under `tests/make/` is a makefile written here, and an
//! `expected` file holding what GNU Make did with it: whether the build
//! succeeded, and for every file the build could have touched, its contents and
//! whether the build touched it. The test runs Ronin's Make mode over the same
//! case and asserts the same build intent and effects. Numeric runner status
//! distinctions are deliberately not part of this gate.
//!
//! Why this rather than diffing the two tools' output. Make mode narrates a
//! build the way the manifest front end does — `[spec:ronin:req:make.narration+2]`
//! — so stdout cannot be compared without deciding, case by case, which lines
//! were narration. That decision is what a classifier is, and a classifier
//! reports rather than fails. Observable effect is comparable without any such
//! judgement: either the file is there with those bytes or it is not.
//!
//! A case that means to test output writes it to a file. The corpus is ours, so
//! that is a property of how a case is written rather than a limitation.
//!
//! Beside the makefile a case may write `args` (the invocation's words), `env`
//! (the environment it must be given, and the names it must not carry) and
//! `setup` (a shell script run in the scratch copy first). All three are inputs
//! to the run, so the recording and the replay are asked the same question.
//!
//! Expectations are recorded from GNU Make and never written by hand:
//!
//!   `MAKE_PORT_RECORD=1 MAKE_PORT_ORACLE=<make>` on a `--test make_port` run
//!
//! which needs the Make `tests/make/oracle.provenance` names. Running the test
//! does not. A second Make can be measured against the recording without
//! becoming it:
//!
//!   `MAKE_PORT_COMPARE=1 MAKE_PORT_ORACLE=<make>`
//!
//! The gate runs the executable under the name `make`, which is the only way to
//! reach the Make front end, so the file needs the `make` feature. The
//! recording-format tests beside it would build without the feature, but they
//! are self-tests of this gate's own format and there is no Make port to gate
//! in a build that has no Make front end.

#![cfg(all(unix, feature = "make"))]

#[path = "support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Runs the corpus with the selected Make and reports how it differs from the
/// recording, instead of comparing Ronin against it.
const COMPARE_VARIABLE: &str = "MAKE_PORT_COMPARE";

/// Where the comparison report is left, so a classification can be read after
/// the run rather than scrolled back to.
const COMPARISON_REPORT: &str = "target/make-port-comparison.txt";

/// Where every run's scratch lives, one directory per run underneath.
const WORK_ROOT: &str = "target/make-port-work";

/// Cases retained as evaluator and interface discovery, but excluded from the
/// build-intent gate because their asserted result belongs to GNU Make's
/// executor rather than to the graph the invocation describes.
///
/// The option cases assert `MAKEFLAGS` propagation/precedence or the execution
/// effects of flags Ronin accepts as interface-compatible no-ops. Recursive
/// Make invocations compile into one graph, so there is no child Make runner in
/// which that choreography could occur.
///
/// The two dry-run cases record what GNU Make 4.4.1 runs while told to run
/// nothing: a `+`-prefixed line, and a line whose unexpanded text names
/// `$(MAKE)`. It runs them because starting the child is the only way it can
/// learn what the child would do. Ronin compiled the child instead, so its
/// `-n` is Ninja's and writes nothing at all. The recordings stay because the
/// classification they demonstrate is the one Ronin's compiler reads —
/// `recursive-dry-run-writes-nothing` is the same question where the two tools
/// agree, and it gates.
///
/// The `-W` case is where a refusal falls rather than whether it happens. GNU
/// Make updates a double-colon chain entry by entry and meets the recipe-less
/// entry the switch appended last, so a `::` rule with work to do RUNS and the
/// run is refused afterwards. A compiled graph is planned before any of it
/// runs, so the refusal comes first and that recipe never does. Both refuse,
/// both leave the dependent alone, and the recording holds the file GNU Make
/// wrote on its way to the same answer. The shape where the chain has nothing
/// to do agrees exactly and gates —
/// `a-what-if-file-a-double-colon-declares-refuses-the-run`.
///
/// `-t` used to be here beside `-W` and is not any more: it brings the goals up
/// to date without making them, which is a filesystem effect this harness can
/// see, so it gates on what the touch did rather than being read for discovery.
/// `-B` left for the same reason and it is the plainer of the two: what it
/// decides is which recipes run, and therefore which files the build writes.
const DISCOVERY_ONLY_CASES: [&str; 8] = [
    "a-what-if-file-that-is-double-colon-refuses-after-the-chain-ran",
    "dry-run-skips-a-make-reference-line",
    "dry-run-skips-a-plus-line",
    "makeflags-keep-going-precedence",
    "makeflags-outranked-by-command-line",
    "makeflags-value-switch-precedence",
    "makeflags-withdrawal-outranked-by-command-line",
    "phony-runs-though-the-file-is-current",
];

/// What a case's build left behind.
#[derive(PartialEq, Eq)]
struct Observed {
    succeeded: bool,
    /// Path relative to the case directory, in sorted order, with the contents
    /// after the build and whether the build wrote to it.
    files: BTreeMap<String, Entry>,
}

#[derive(PartialEq, Eq)]
struct Entry {
    touched: bool,
    content: Content,
}

#[derive(PartialEq, Eq)]
enum Content {
    Text(String),
    /// Length only: a case whose output is not text is testing that it exists.
    Opaque(usize),
}

/// One case on disk.
struct Case {
    id: String,
    directory: PathBuf,
}

#[test]
// [spec:ronin:req:make.semantics+1/test]
fn make_build_intent_matches_oracle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = root.join("tests/make");
    let cases = collect(&corpus);
    assert!(!cases.is_empty(), "no cases under {}", corpus.display());

    if std::env::var_os("MAKE_PORT_RECORD").is_some() {
        record(&corpus, &cases);
        discard_run_scratch(false);
        return;
    }

    if std::env::var_os(COMPARE_VARIABLE).is_some() {
        report_second_make(&corpus, &cases);
        discard_run_scratch(false);
        return;
    }

    let front_end = make_named_ronin();
    let mut failures = Vec::new();
    let mut repaired = Vec::new();
    for case in &cases {
        let observed = run(case, &front_end);
        let expected = read_expected(case);
        let difference = difference(&expected, &observed);
        match (difference, known_divergence(case)) {
            (Some(difference), None) => failures.push(failure(case, &difference)),
            (None, Some(reason)) => repaired.push(format!("{}: {reason}", case.id)),
            _ => {}
        }
    }
    discard_run_scratch(!failures.is_empty() || !repaired.is_empty());
    assert!(
        failures.is_empty(),
        "{} of {} ported cases diverge from GNU Make's build intent:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
    assert!(
        repaired.is_empty(),
        "{} case(s) match GNU Make and are still recorded as diverging. \
         Delete the `divergence` file:\n\n{}",
        repaired.len(),
        repaired.join("\n\n")
    );
}

/// A defect this case is known to reach, and the node that owns it.
///
/// Recorded so the gate stays a gate in both directions: an unrecorded
/// divergence fails, and so does a recorded one that has been fixed — which is
/// what stops the list becoming a place differences go to be forgotten.
fn known_divergence(case: &Case) -> Option<String> {
    sidecar(case, "divergence")
}

/// How this case failed, with whatever the corpus recorded about why it is one
/// of the delicate ones.
///
/// A case whose recording departs from a distribution's Make fails here first
/// on a host that has one, so what the reader needs at that moment is the note
/// beside the case rather than a search for which build the answer came from.
fn failure(case: &Case, difference: &str) -> String {
    let note = sidecar(case, "note").map_or_else(String::new, |note| format!("\n  {note}"));
    format!("{}: {difference}{note}", case.id)
}

/// A case's prose beside it, if the case wrote any.
fn sidecar(case: &Case, name: &str) -> Option<String> {
    fs::read_to_string(case.directory.join(name))
        .ok()
        .map(|text| text.trim().to_owned())
}

fn collect(corpus: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    let entries = fs::read_dir(corpus).unwrap_or_else(|error| {
        panic!("reading {}: {error}", corpus.display());
    });
    for entry in entries {
        let entry = entry.expect("a corpus entry");
        if !entry.file_type().expect("a file type").is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if DISCOVERY_ONLY_CASES.contains(&id.as_str()) {
            continue;
        }
        cases.push(Case {
            id,
            directory: entry.path(),
        });
    }
    cases.sort_by(|left, right| left.id.cmp(&right.id));
    cases
}

/// Make mode is reached by the invoked name and by nothing else, so pointing a
/// harness at it means a make-named link rather than a flag.
fn make_named_ronin() -> PathBuf {
    // Cargo supplies the binary built for this exact test invocation. Never
    // use target/release here: it may be absent or, worse, left over from an
    // older source tree while a debug test appears to pass.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_ronin"));
    assert!(
        binary.is_file(),
        "Cargo did not build the Ronin binary at {}",
        binary.display()
    );
    // Inside this run's own directory, like everything else the run writes: the
    // link used to live at a fixed `target/make-port-bin/make`, which two runs
    // unlink and recreate under each other while both are resolving it.
    let directory = run_directory().join("bin");
    fs::create_dir_all(&directory).expect("a directory for the link");
    let link = directory.join("make");
    std::os::unix::fs::symlink(&binary, &link).expect("a make-named link");
    link
}

/// Run one case in a scratch copy and read what it left behind.
fn run(case: &Case, program: &Path) -> Observed {
    let scratch = scratch_for(case);
    let arguments = read_words(&case.directory.join("args"));

    if case.directory.join("setup").exists() {
        let status = Command::new("sh")
            .arg("setup")
            .current_dir(&scratch)
            .status()
            .expect("running the case's setup");
        assert!(status.success(), "{}: setup failed", case.id);
    }

    // Everything the build could have touched is what was there before it, and
    // a build that creates a file is the interesting case, so the mark has to
    // predate the run rather than be taken from it.
    let before = SystemTime::now();
    std::thread::sleep(std::time::Duration::from_millis(10));

    let mut command = Command::new(program);
    command
        .args(&arguments)
        .current_dir(&scratch)
        .env("LC_ALL", "C")
        .env_remove("MAKEFLAGS")
        // The environment's second option stream, which decodes exactly as
        // MAKEFLAGS does. A host that exports one would be handing every case
        // in the corpus switches the case never asked for, and the recording
        // would carry them.
        .env_remove("GNUMAKEFLAGS")
        .env_remove("MAKELEVEL");
    for (name, value) in read_environment(&case.directory.join("env")) {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{}: running {}: {error}", case.id, program.display()));

    Observed {
        succeeded: output.status.success(),
        files: listing(&scratch, before),
    }
}

/// A clean copy of the case, inside the directory this run owns.
fn scratch_for(case: &Case) -> PathBuf {
    let scratch = run_directory().join(&case.id);
    fs::create_dir(&scratch)
        .unwrap_or_else(|error| panic!("{}: making {}: {error}", case.id, scratch.display()));
    copy_into(&case.directory, &scratch);
    scratch
}

/// The directory this RUN owns, made once and shared by every case in it.
///
/// A case's scratch used to be `target/make-port-work/<case id>` and nothing
/// else. The name carried no run, so two harness processes in one checkout
/// were one directory: each removed the other's tree mid-run, copied its own
/// makefile over it, and read back a listing made of both. Measured, two runs
/// started together: one read `"src\nsrc\ngen\ngen\n"` where GNU Make left
/// `"src\ngen\n"`, and one failed in a case's setup because the other process
/// had just removed the tree under it. A case failing that way reads exactly
/// like a product defect, and one of them was investigated as one.
///
/// The pid alone would not settle it, which is the other half of that finding:
/// under `scripts/sandboxed` a run has its own pid namespace, so
/// `process::id()` is a small number that repeats exactly from one run to the
/// next. The clock goes in the name beside it, and the directory is made with
/// `create_dir` rather than `create_dir_all` — a name that already exists is
/// one to walk away from, never one to build in.
fn run_directory() -> &'static Path {
    static DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIRECTORY
        .get_or_init(|| {
            let work = Path::new(env!("CARGO_MANIFEST_DIR")).join(WORK_ROOT);
            fs::create_dir_all(&work)
                .unwrap_or_else(|error| panic!("making {}: {error}", work.display()));
            sweep_stale_runs(&work);
            loop {
                let stamp = SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_nanos());
                let directory = work.join(format!("run-{}-{stamp}", std::process::id()));
                match fs::create_dir(&directory) {
                    Ok(()) => return directory,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("making {}: {error}", directory.display()),
                }
            }
        })
        .as_path()
}

/// Remove what earlier runs left behind, once per process.
///
/// A unique name means a leftover can no longer poison anything, so this is
/// housekeeping rather than correctness — but a run that fails keeps its
/// scratch on purpose, a run that panics keeps it by accident, and nothing
/// else would ever collect either. The whole of `target/make-port-work` is
/// this harness's, so everything under it is a candidate, including the
/// per-case directories the old layout left at the top level. Only entries
/// untouched for an hour go, so a run in progress beside this one is never one
/// of them.
fn sweep_stale_runs(work: &Path) {
    let stale = std::time::Duration::from_hours(1);
    let Ok(entries) = fs::read_dir(work) else {
        return;
    };
    for entry in entries.flatten() {
        let idle = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|when| SystemTime::now().duration_since(when).ok());
        if idle.is_some_and(|idle| idle > stale) {
            let path = entry.path();
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }
}

/// Take this run's scratch away, or say where it was left.
///
/// A green run's thousand case directories are worth nothing and cost twenty
/// megabytes; a red one's are the evidence, so they stay and the path is
/// printed rather than searched for.
fn discard_run_scratch(keep: bool) {
    let directory = run_directory();
    if keep {
        eprintln!(
            "make_port: this run's scratch is at {}",
            directory.display()
        );
    } else {
        let _ = fs::remove_dir_all(directory);
    }
}

fn copy_into(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("reading a case") {
        let entry = entry.expect("a case entry");
        let name = entry.file_name();
        // The recording is the test's own, not the build's input.
        if name == "expected" || name == "divergence" || name == "note" {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        if entry.file_type().expect("a file type").is_dir() {
            fs::create_dir_all(&target).expect("a nested directory");
            copy_into(&source, &target);
        } else {
            fs::copy(&source, &target).expect("copying a case file");
        }
    }
}

/// Every file under the run directory, with what it holds and whether the build
/// wrote it.
fn listing(root: &Path, before: SystemTime) -> BTreeMap<String, Entry> {
    let mut files = BTreeMap::new();
    walk(root, root, before, &mut files);
    files
}

fn walk(root: &Path, directory: &Path, before: SystemTime, into: &mut BTreeMap<String, Entry>) {
    for entry in fs::read_dir(directory).expect("reading the run directory") {
        let entry = entry.expect("a run entry");
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Ronin's build log and the case's own setup are the harness's, not the
        // build's answer.
        if name.starts_with('.')
            || name == "setup"
            || name == "args"
            || name == "env"
            || name == "divergence"
            || name == "note"
        {
            continue;
        }
        if entry.file_type().expect("a file type").is_dir() {
            walk(root, &path, before, into);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("a path under the run directory")
            .to_string_lossy()
            .into_owned();
        let metadata = entry.metadata().expect("file metadata");
        let touched = metadata.modified().is_ok_and(|when| when > before);
        let bytes = fs::read(&path).expect("reading a produced file");
        let content = match String::from_utf8(bytes) {
            Ok(text) => Content::Text(text),
            Err(error) => Content::Opaque(error.into_bytes().len()),
        };
        into.insert(relative, Entry { touched, content });
    }
}

/// What a case's environment holds: `NAME=value` for a name the run carries,
/// `-NAME` for one it must not, one per line.
///
/// The ambient environment belongs to whoever is running the corpus, and some
/// of GNU Make's answers turn on it — `SHELL` stands at a different rank and a
/// different flavour depending on whether the environment had one at all. A
/// case that asks such a question has to say what the environment holds instead
/// of inheriting whatever the machine happened to export. Recording and replay
/// both come through here, so the oracle and Ronin are asked the same question.
fn read_environment(path: &Path) -> Vec<(String, Option<String>)> {
    let mut entries = Vec::new();
    for line in fs::read_to_string(path).unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('-') {
            entries.push((name.to_owned(), None));
        } else if let Some((name, value)) = line.split_once('=') {
            entries.push((name.to_owned(), Some(value.to_owned())));
        } else {
            panic!(
                "{}: `{line}` is neither NAME=value nor -NAME",
                path.display()
            );
        }
    }
    entries
}

/// A case's arguments, one word per whitespace-separated token.
///
/// `''` is a zero-length word — the shell's own spelling for one, and the only
/// argument whitespace cannot separate. `make ""` is a shape GNU Make builds
/// through, so a case has to be able to say it.
fn read_words(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(|word| {
            if word == "''" {
                String::new()
            } else {
                word.to_owned()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The recording format: readable, diffable, and written only by --record.

fn render(observed: &Observed, version: &str) -> String {
    let outcome = if observed.succeeded {
        "success"
    } else {
        "failure"
    };
    let mut text = format!("oracle {version}\noutcome {outcome}\n");
    for (path, entry) in &observed.files {
        let mark = if entry.touched { "touched" } else { "kept" };
        match &entry.content {
            Content::Text(content) => {
                // One greater than the number of content lines below exactly
                // when the last line carries no terminator: `lines()` yields
                // the same lines either way, so this count is the only place
                // that difference can live, and `parse` reads it back out.
                let lines = content.lines().count()
                    + usize::from(!content.is_empty() && !content.ends_with('\n'));
                writeln!(text, "file {path} {mark} {lines}").expect("writing to a String");
                for line in content.lines() {
                    writeln!(text, "| {line}").expect("writing to a String");
                }
            }
            Content::Opaque(length) => {
                writeln!(text, "opaque {path} {mark} {length}").expect("writing to a String");
            }
        }
    }
    text
}

/// What a `file` line said before its content lines: the path, whether the
/// build wrote it, and how many lines the file held.
type PendingFile = (String, bool, Option<usize>, Vec<String>);

fn parse(text: &str, id: &str) -> Observed {
    let mut succeeded = None;
    let mut files = BTreeMap::new();
    let mut pending: Option<PendingFile> = None;

    let flush = |pending: &mut Option<PendingFile>, files: &mut BTreeMap<String, Entry>| {
        if let Some((path, touched, recorded, lines)) = pending.take() {
            let mut content = lines.join("\n");
            // The recorded count is one greater than the lines that follow it
            // exactly when the file ended without a terminator, so a file that
            // ends `srr` reads back as `srr` rather than as `srr\n`, and one
            // holding a single newline reads back as `\n` rather than as
            // nothing. A recording made before the count was written falls back
            // to what the reader used to assume.
            let terminated = recorded.map_or_else(
                || !content.is_empty(),
                |recorded| !lines.is_empty() && recorded == lines.len(),
            );
            if terminated {
                content.push('\n');
            }
            files.insert(
                path,
                Entry {
                    touched,
                    content: Content::Text(content),
                },
            );
        }
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("| ") {
            let (_, _, _, lines) = pending
                .as_mut()
                .unwrap_or_else(|| panic!("{id}: a content line with no file"));
            lines.push(rest.to_owned());
            continue;
        }
        flush(&mut pending, &mut files);
        let mut words = line.split(' ');
        match words.next() {
            // The oracle line is a header for the reader. Which Make made the
            // recording is `tests/make/oracle.provenance`, and it is enforced
            // when recording rather than when replaying.
            Some("oracle" | "") | None => {}
            Some("status") => {
                // Backward-compatible reader for recordings made before the
                // gate stopped treating runner-specific exit numbers as build
                // semantics. New recordings use `outcome` below.
                succeeded = words
                    .next()
                    .and_then(|value| value.parse::<i32>().ok())
                    .map(|status| status == 0);
            }
            Some("outcome") => {
                succeeded = Some(match words.next() {
                    Some("success") => true,
                    Some("failure") => false,
                    Some(other) => panic!("{id}: unknown outcome `{other}`"),
                    None => panic!("{id}: no outcome value"),
                });
            }
            Some("file") => {
                let path = words.next().expect("a file path").to_owned();
                let touched = words.next() == Some("touched");
                let recorded = words.next().and_then(|count| count.parse().ok());
                pending = Some((path, touched, recorded, Vec::new()));
            }
            Some("opaque") => {
                let path = words.next().expect("a file path").to_owned();
                let touched = words.next() == Some("touched");
                let length = words.next().and_then(|n| n.parse().ok()).expect("a length");
                files.insert(
                    path,
                    Entry {
                        touched,
                        content: Content::Opaque(length),
                    },
                );
            }
            Some(other) => panic!("{id}: unknown record `{other}`"),
        }
    }
    flush(&mut pending, &mut files);

    Observed {
        succeeded: succeeded.unwrap_or_else(|| panic!("{id}: no outcome recorded")),
        files,
    }
}

/// Whatever a build leaves in a file, the recording of it reads back as itself.
///
/// This is the gate on the gate. A reader that put a terminator back on every
/// non-empty file made a case whose output ends without one impossible to
/// record: recording succeeded, and the next run failed the case against the
/// very oracle run that had recorded it. Two dispatches worked around it by
/// rewriting the fixture — `echo` for `echo -n`, a hidden input for a visible
/// one — which is how a harness defect gets read as a product one.
#[test]
fn make_recording_round_trips_every_ending() {
    for content in [
        "", "\n", "\n\n", "srr", "srr\n", "a\nb", "a\nb\n", "a\n\nb", "a\n\n", "a\n\n\n",
    ] {
        let observed = Observed {
            succeeded: true,
            files: BTreeMap::from([(
                "out".to_owned(),
                Entry {
                    touched: true,
                    content: Content::Text(content.to_owned()),
                },
            )]),
        };
        let text = render(&observed, "GNU Make 4.4.1");
        let Content::Text(read_back) = &parse(&text, "round-trip").files["out"].content else {
            panic!("{content:?} read back as something other than text");
        };
        assert_eq!(read_back, content, "recorded as:\n{text}");
    }
}

fn read_expected(case: &Case) -> Observed {
    let path = case.directory.join("expected");
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: no recording at {} ({error}). Record it with \
             MAKE_PORT_RECORD=1 cargo test --test make_port",
            case.id,
            path.display()
        )
    });
    parse(&text, &case.id)
}

/// The first way the run differs from the recording, in a sentence that names
/// the file and what about it.
fn difference(expected: &Observed, observed: &Observed) -> Option<String> {
    if expected.succeeded != observed.succeeded {
        return Some(format!(
            "build {} where GNU Make's build {}",
            if observed.succeeded {
                "succeeded"
            } else {
                "failed"
            },
            if expected.succeeded {
                "succeeded"
            } else {
                "failed"
            }
        ));
    }
    for (path, entry) in &expected.files {
        let Some(actual) = observed.files.get(path) else {
            return Some(format!("'{path}' was not produced"));
        };
        if actual.content != entry.content {
            return Some(match (&entry.content, &actual.content) {
                (Content::Text(wanted), Content::Text(got)) => {
                    format!("'{path}' holds {got:?} where GNU Make left {wanted:?}")
                }
                _ => format!("'{path}' differs from what GNU Make left"),
            });
        }
        if actual.touched != entry.touched {
            return Some(format!(
                "'{path}' was {} where GNU Make {}",
                if actual.touched {
                    "written"
                } else {
                    "left alone"
                },
                if entry.touched {
                    "wrote it"
                } else {
                    "left it alone"
                },
            ));
        }
    }
    for path in observed.files.keys() {
        if !expected.files.contains_key(path) {
            return Some(format!(
                "'{path}' was produced and GNU Make produced no such file"
            ));
        }
    }
    None
}

/// Re-derive every expectation from the oracle. Never inferred, never edited.
fn record(corpus: &Path, cases: &[Case]) {
    let make = oracle::selected();
    let provenance = pinned(corpus, &make);

    for case in cases {
        let observed = run(case, &make);
        fs::write(
            case.directory.join("expected"),
            render(&observed, &provenance.version),
        )
        .unwrap_or_else(|error| panic!("{}: writing the recording: {error}", case.id));
        println!("recorded {}", case.id);
    }
}

/// The identity this recording will carry, having established that the Make
/// about to produce it is the one the corpus is recorded against.
///
/// Moving the pin is its own act with its own name, and it rewrites the record
/// so that which Make the corpus now speaks for arrives as a diff to review.
fn pinned(corpus: &Path, make: &Path) -> oracle::Provenance {
    let observed = oracle::probe(make);
    let recorded = oracle::read(corpus);

    if let Some(build) = std::env::var_os(oracle::MOVE_VARIABLE) {
        let moved = oracle::Provenance {
            build: build.to_string_lossy().into_owned(),
            ..observed
        };
        if let Ok(recorded) = &recorded {
            for difference in oracle::differences(recorded, &moved) {
                println!("moved: {difference}");
            }
        }
        oracle::write(corpus, &moved);
        return moved;
    }

    let recorded = recorded.unwrap_or_else(|reason| {
        panic!(
            "{reason}. Build the oracle with scripts/build-make-oracle.sh and pin it with \
             {}=<what it was built from>",
            oracle::MOVE_VARIABLE
        )
    });
    let differences = oracle::differences(&recorded, &observed);
    assert!(
        differences.is_empty(),
        "{} is not the Make this corpus is recorded against ({}):\n\n{}\n\n\
         Build the oracle with scripts/build-make-oracle.sh and name it in {}, or move the \
         pin deliberately with {}=<what it was built from>",
        make.display(),
        recorded.build,
        differences.join("\n"),
        oracle::ORACLE_VARIABLE,
        oracle::MOVE_VARIABLE
    );

    oracle::Provenance {
        build: recorded.build,
        ..observed
    }
}

/// Run the corpus with a Make that is not the oracle and say how it differs
/// from the recording.
///
/// A report rather than a gate. Another build of 4.4.1 disagreeing with the
/// recording is the finding being sought, so there is nothing here to fail:
/// what the run produces is a classification of that build's departures.
fn report_second_make(corpus: &Path, cases: &[Case]) {
    let make = oracle::selected();
    let observed = oracle::probe(&make);
    let mut report = format!(
        "make {}\nversion {}\nhost {}\n",
        make.display(),
        observed.version,
        observed.host
    );
    match oracle::read(corpus) {
        Ok(recorded) => {
            let _ = writeln!(report, "recorded oracle {}", recorded.build);
            for difference in oracle::differences(&recorded, &observed) {
                let _ = writeln!(report, "identity {difference}");
            }
        }
        Err(reason) => {
            let _ = writeln!(report, "identity unrecorded: {reason}");
        }
    }

    let mut differing = 0;
    for case in cases {
        if let Some(difference) = difference(&read_expected(case), &run(case, &make)) {
            differing += 1;
            let _ = writeln!(report, "case {}: {difference}", case.id);
        }
    }
    let _ = writeln!(report, "{differing} of {} cases differ", cases.len());

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(COMPARISON_REPORT);
    fs::write(&path, &report).unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    println!("{report}\nwritten to {}", path.display());
}
