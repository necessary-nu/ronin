//! What `-t lint` says, which is the whole of what it delivers.
//!
//! A lint has no build to check afterwards and no oracle to compare against:
//! the report IS the product, so these tests assert on the bytes it writes and
//! the status it leaves with, the way the narration-contract tests do.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The composed line a census writes for the checked-in reduction.
#[cfg(all(unix, feature = "make"))]
const COMPOSED: &str = "Makefile:5: note: recursion composed into the graph: cd src && $(MAKE) all";

/// The two lines a census writes for an invocation a shell construct hides.
#[cfg(all(unix, feature = "make"))]
const THROUGH_A_CONSTRUCT: &str = concat!(
    "warning: recursion nests at run time: the invocation is not the recipe line's own ",
    "command, so a shell construct stands between them\n",
);

/// What it says would compose that one instead.
#[cfg(all(unix, feature = "make"))]
const WRITE_IT_AS_THE_COMMAND: &str = concat!(
    "note: a line whose own command is the invocation composes: `$(MAKE) ...`, or ",
    "`cd DIR && $(MAKE) ...`, with any test settled where the Makefile can answer it\n",
);

/// A scratch directory of this test's own.
fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ronin-lint-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn write(directory: &Path, name: &str, contents: &str) {
    fs::write(directory.join(name), contents).unwrap();
}

/// Ronin under its own name, which is the only name a tool is reached by.
fn ronin(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(directory)
        .args(arguments)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Lint is Ronin's own entry in a tool set Ninja otherwise owns, and it is
/// listed rather than hidden: a tool nobody can find is a tool nobody uses.
// [spec:ronin:req:tools.lint/test]
#[test]
fn the_tool_is_listed() {
    let directory = scratch("listed");
    let output = ronin(&directory, &["-t", "list"]);
    assert!(output.status.success());
    let listed = stdout(&output);
    assert!(
        listed
            .lines()
            .any(|line| line.split_whitespace().next() == Some("lint")),
        "lint is not listed among the subtools:\n{listed}"
    );
}

/// The help says the read runs the Makefile, because it does. A tool that let
/// a reader believe otherwise would be lying about what it had just done.
// [spec:ronin:req:tools.lint/test]
#[test]
fn the_help_states_the_read_cost() {
    let directory = scratch("help");
    let output = ronin(&directory, &["-t", "lint", "-h"]);
    assert_eq!(output.status.code(), Some(1));
    let help = stdout(&output);
    assert!(help.starts_with("usage: ronin -t lint"), "{help}");
    assert!(help.contains("$(shell) runs"), "{help}");
}

/// A manifest with nothing to say about it still says one thing, so a caller
/// gets a report to read rather than silence to interpret.
// [spec:ronin:req:tools.lint/test]
#[test]
fn a_clean_manifest_still_reports_something() {
    let directory = scratch("manifest-clean");
    write(
        &directory,
        "build.ninja",
        "rule cc\n  command = gcc $in -o $out\n\nbuild a.o: cc a.c\n",
    );
    let output = ronin(&directory, &["-t", "lint"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(stdout(&output), "ronin: read 1 manifest\n");
}

/// What the parser refuses outright is reported as the refusal it already is,
/// caret included: rewrapping a located manifest diagnostic would take the
/// caret off, and the caret is most of what makes it legible.
// [spec:ronin:req:tools.lint/test]
#[test]
fn an_unparsed_manifest_keeps_its_caret() {
    let directory = scratch("manifest-broken");
    write(&directory, "build.ninja", "build a: nope b\n");
    let output = ronin(&directory, &["-t", "lint"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stdout(&output),
        "error: build.ninja:1: unknown build rule 'nope'\n\
         build a: nope b\n\
         \x20        ^ near here\n\
         ronin: nothing further to report: the manifest did not parse\n"
    );
}

/// The kind comes from the file's own name, and `--ninja` overrides it. A
/// Makefile read as a manifest fails to parse, which is what proves the
/// selector was honoured rather than guessed around.
// [spec:ronin:req:tools.lint/test]
#[test]
fn the_named_kind_outranks_the_name() {
    let directory = scratch("kind");
    write(&directory, "Makefile", "all:\n\t@echo built\n");
    // The reading the name selects, which needs a Make front end to perform.
    #[cfg(all(unix, feature = "make"))]
    {
        let named = ronin(&directory, &["-f", "Makefile", "-t", "lint"]);
        assert_eq!(named.status.code(), Some(0), "{}", stdout(&named));
    }
    let forced = ronin(&directory, &["-f", "Makefile", "-t", "lint", "--ninja"]);
    assert_eq!(forced.status.code(), Some(2));
    assert!(
        stdout(&forced).starts_with("error: Makefile:1:"),
        "{}",
        stdout(&forced)
    );
}

/// An option lint does not know is refused rather than read as a target, so a
/// misspelling never quietly becomes a request for something else.
// [spec:ronin:req:tools.lint/test]
#[test]
fn an_unknown_option_is_refused() {
    let directory = scratch("unknown-option");
    write(&directory, "build.ninja", "");
    let output = ronin(&directory, &["-t", "lint", "--nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--nope"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A Makefile is read by the Make compiler because its name says so, without
/// the invocation being in Make mode at all: `[spec:ronin:req:product.make-identity]`
/// governs which front end BUILDS, and this builds nothing.
// [spec:ronin:req:tools.lint/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_makefile_is_read_by_name() {
    let directory = scratch("makefile");
    write(&directory, "Makefile", "all:\n\t@echo built\n");
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stdout(&output),
        "ronin: 0 recursive invocations: 0 composed, 0 nested\n"
    );
    assert!(
        !directory.join("built").exists(),
        "lint built something it was only asked to read"
    );
}

/// What the read raised on its way is passed on in the words the compiler
/// wrote, pointing at the Makefile line it is about — GNU Make 4.4.1 writes
/// this one without a severity word and so does Ronin, and lint does not add
/// one. It still counts as a finding: a warning the compile raised is exactly
/// the kind of thing a lint exists to put in front of someone.
// [spec:ronin:req:tools.lint/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_raised_warning_is_passed_on() {
    let directory = scratch("raised");
    write(
        &directory,
        "Makefile",
        "ifeq (a,a) careful\nendif\nall:\n\t@echo built\n",
    );
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "Makefile:1: extraneous text after 'ifeq' directive\n\
         ronin: 0 recursive invocations: 0 composed, 0 nested\n"
    );
}

/// Copy a checked-in reduction into a scratch directory of its own, so the
/// reproduction a reader does by hand is the one the test does.
#[cfg(all(unix, feature = "make"))]
fn reduction(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ronin-lint-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/make-project-reductions")
            .join(name),
        &path,
    );
    path
}

#[cfg(all(unix, feature = "make"))]
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The census is the headline, and what it PRINTS is the deliverable: every
/// recursive invocation the compile classified, where it was written, which
/// way it went, and — for one that nests — what would compose it instead.
///
/// The tree is vim's top-level Makefile in miniature, checked in and shared
/// with the build-intent gate that runs it: one liftable `cd src && $(MAKE)`
/// beside a guard holding a `$(MAKE)` the compiler cannot lift out.
// [spec:ronin:req:make.nesting-census+1/test]
// [spec:ronin:req:tools.lint/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn the_census_names_every_recursion() {
    let directory = reduction("recipe-mixes-liftable-and-unliftable-recursion");
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(
        stdout(&output),
        format!(
            "{COMPOSED}\n\
             Makefile:6: {THROUGH_A_CONSTRUCT}\
             Makefile:6: {WRITE_IT_AS_THE_COMMAND}\
             ronin: 2 recursive invocations: 1 composed, 1 nested\n"
        )
    );
    assert!(
        !directory.join("src/built.stamp").exists(),
        "a census built the recipe it was reporting on"
    );
}

/// A `.ONESHELL` recipe of several lines nests an invocation that would have
/// composed on its own, and the census says which of the three shapes that is
/// rather than reporting one nesting for all of them.
// [spec:ronin:req:make.nesting-census+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_shared_shell_names_itself() {
    let directory = scratch("oneshell");
    fs::create_dir_all(directory.join("sub")).unwrap();
    write(&directory, "sub/Makefile", "all:\n\t@echo one\n");
    write(
        &directory,
        "Makefile",
        ".ONESHELL:\nall:\n\tcd sub && $(MAKE) all\n\techo after\n",
    );
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "Makefile:3: warning: recursion nests at run time: a .ONESHELL recipe of several lines \
         shares one shell\n\
         Makefile:3: note: give the invocation a recipe line the rest of the recipe does not \
         share, and it composes\n\
         ronin: 1 recursive invocation: 0 composed, 1 nested\n"
    );
}

/// A line whose own command IS the invocation, written with an assignment in
/// front of it, is the third shape: the resolver reads an argument list and
/// this is not one.
// [spec:ronin:req:make.nesting-census+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_prefixed_invocation_names_itself() {
    let directory = scratch("prefixed");
    fs::create_dir_all(directory.join("sub")).unwrap();
    write(&directory, "sub/Makefile", "all:\n\t@echo one\n");
    write(&directory, "Makefile", "all:\n\tV=1 $(MAKE) -C sub all\n");
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "Makefile:2: warning: recursion nests at run time: the line's command is the invocation, \
         written as more than the argument list the resolver reads\n\
         Makefile:2: note: an invocation written as words alone composes: an assignment or `env` \
         prefix, a redirection, a glob, or an unsettled expansion is what stops this one\n\
         ronin: 1 recursive invocation: 0 composed, 1 nested\n"
    );
}
