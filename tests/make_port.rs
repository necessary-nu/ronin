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
//! build the way the manifest front end does — `[spec:ronin:req:make.narration+1]`
//! — so stdout cannot be compared without deciding, case by case, which lines
//! were narration. That decision is what a classifier is, and a classifier
//! reports rather than fails. Observable effect is comparable without any such
//! judgement: either the file is there with those bytes or it is not.
//!
//! A case that means to test output writes it to a file. The corpus is ours, so
//! that is a property of how a case is written rather than a limitation.
//!
//! Expectations are recorded from GNU Make and never written by hand:
//!
//!   `MAKE_PORT_RECORD=1` on a `--test make_port` run
//!
//! which needs `/usr/bin/make`. Running the test does not.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// The oracle. Pinned so that re-recording on a host with a different Make is a
/// deliberate act rather than an accident.
const ORACLE_VERSION: &str = "GNU Make 4.4.1";

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
const DISCOVERY_ONLY_CASES: [&str; 10] = [
    "always-make-option",
    "dry-run-skips-a-make-reference-line",
    "dry-run-skips-a-plus-line",
    "makeflags-keep-going-precedence",
    "makeflags-outranked-by-command-line",
    "makeflags-value-switch-precedence",
    "makeflags-withdrawal-outranked-by-command-line",
    "phony-runs-though-the-file-is-current",
    "touch-option",
    "what-if-option",
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
        record(&cases);
        return;
    }

    let front_end = make_named_ronin(root);
    let mut failures = Vec::new();
    let mut repaired = Vec::new();
    for case in &cases {
        let observed = run(case, &front_end);
        let expected = read_expected(case);
        let difference = difference(&expected, &observed);
        match (difference, known_divergence(case)) {
            (Some(difference), None) => failures.push(format!("{}: {difference}", case.id)),
            (None, Some(reason)) => repaired.push(format!("{}: {reason}", case.id)),
            _ => {}
        }
    }
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
    fs::read_to_string(case.directory.join("divergence"))
        .ok()
        .map(|reason| reason.trim().to_owned())
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
fn make_named_ronin(root: &Path) -> PathBuf {
    // Cargo supplies the binary built for this exact test invocation. Never
    // use target/release here: it may be absent or, worse, left over from an
    // older source tree while a debug test appears to pass.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_ronin"));
    assert!(
        binary.is_file(),
        "Cargo did not build the Ronin binary at {}",
        binary.display()
    );
    let directory = root.join("target/make-port-bin");
    fs::create_dir_all(&directory).expect("a directory for the link");
    let link = directory.join("make");
    let _ = fs::remove_file(&link);
    #[cfg(unix)]
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

    let output = Command::new(program)
        .args(&arguments)
        .current_dir(&scratch)
        .env("LC_ALL", "C")
        .env_remove("MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .output()
        .unwrap_or_else(|error| panic!("{}: running {}: {error}", case.id, program.display()));

    Observed {
        succeeded: output.status.success(),
        files: listing(&scratch, before),
    }
}

/// A clean copy of the case, so a run never sees the last one's leavings.
fn scratch_for(case: &Case) -> PathBuf {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/make-port-work")
        .join(&case.id);
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("a scratch directory");
    copy_into(&case.directory, &scratch);
    scratch
}

fn copy_into(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("reading a case") {
        let entry = entry.expect("a case entry");
        let name = entry.file_name();
        // The recording is the test's own, not the build's input.
        if name == "expected" || name == "divergence" {
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
        if name.starts_with('.') || name == "setup" || name == "args" || name == "divergence" {
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

fn read_words(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// The recording format: readable, diffable, and written only by --record.

fn render(observed: &Observed) -> String {
    let outcome = if observed.succeeded {
        "success"
    } else {
        "failure"
    };
    let mut text = format!("oracle {ORACLE_VERSION}\noutcome {outcome}\n");
    for (path, entry) in &observed.files {
        let mark = if entry.touched { "touched" } else { "kept" };
        match &entry.content {
            Content::Text(content) => {
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

fn parse(text: &str, id: &str) -> Observed {
    let mut succeeded = None;
    let mut files = BTreeMap::new();
    let mut pending: Option<(String, bool, Vec<String>)> = None;

    let flush = |pending: &mut Option<(String, bool, Vec<String>)>,
                 files: &mut BTreeMap<String, Entry>| {
        if let Some((path, touched, lines)) = pending.take() {
            let mut content = lines.join("\n");
            if !content.is_empty() {
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
            let (_, _, lines) = pending
                .as_mut()
                .unwrap_or_else(|| panic!("{id}: a content line with no file"));
            lines.push(rest.to_owned());
            continue;
        }
        flush(&mut pending, &mut files);
        let mut words = line.split(' ');
        match words.next() {
            // The oracle line is a header for the reader; the version it names
            // is enforced when recording, not when replaying.
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
                pending = Some((path, touched, Vec::new()));
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
fn record(cases: &[Case]) {
    let oracle = Path::new("/usr/bin/make");
    assert!(oracle.exists(), "recording needs {}", oracle.display());
    let reported = Command::new(oracle)
        .arg("--version")
        .output()
        .expect("asking the oracle its version");
    let banner = String::from_utf8_lossy(&reported.stdout);
    let first = banner.lines().next().unwrap_or_default();
    assert!(
        first.starts_with(ORACLE_VERSION),
        "the oracle is {first:?}, and the corpus is recorded against {ORACLE_VERSION}"
    );

    for case in cases {
        let observed = run(case, oracle);
        fs::write(case.directory.join("expected"), render(&observed))
            .unwrap_or_else(|error| panic!("{}: writing the recording: {error}", case.id));
        println!("recorded {}", case.id);
    }
}
