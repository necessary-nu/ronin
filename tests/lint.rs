//! What `-t lint` says, which is the whole of what it delivers.
//!
//! A lint has no build to check afterwards and no oracle to compare against:
//! the report IS the product, so these tests assert on the bytes it writes and
//! the status it leaves with, the way the narration-contract tests do.

use std::fs;
use std::path::Path;
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

/// The two lines kati raises for a target declared twice, which lint passes on
/// as the compiler wrote them: a finding that is not a census entry, above the
/// census and above the missing child.
#[cfg(all(unix, feature = "make"))]
const OVERRIDDEN: &str = concat!(
    "Makefile:26: warning: overriding commands for target 'noise'\n",
    "Makefile:23: warning: ignoring old commands for target 'noise'\n",
);

/// The composition written above the missing child.
#[cfg(all(unix, feature = "make"))]
const COMPOSED_PRESENT: &str =
    "Makefile:12: note: recursion composed into the graph: cd present && $(MAKE) all\n";

/// The missing child's own composition: the compiler decided to lift it out,
/// which is what makes the line below it a finding rather than a refusal.
#[cfg(all(unix, feature = "make"))]
const COMPOSED_MISSING: &str =
    "Makefile:15: note: recursion composed into the graph: cd nomakefile/ && $(MAKE) all\n";

/// The finding this reduction exists for.
#[cfg(all(unix, feature = "make"))]
const NO_MAKEFILE: &str = concat!(
    "Makefile:15: warning: the invocation composes and its makefile is missing: ",
    "nothing a Make reads is in `nomakefile`\n",
);

/// What it says would answer it.
#[cfg(all(unix, feature = "make"))]
const WRITE_A_MAKEFILE: &str = concat!(
    "Makefile:15: note: a composed invocation is compiled where it points, so that directory ",
    "needs a makefile under one of the names a Make looks for, or the invocation needs a ",
    "`-f` naming one\n",
);

/// What it says would compose that one instead.
#[cfg(all(unix, feature = "make"))]
const WRITE_IT_AS_THE_COMMAND: &str = concat!(
    "note: a line whose own command is the invocation composes: `$(MAKE) ...`, or ",
    "`cd DIR && $(MAKE) ...`, with any test settled where the Makefile can answer it\n",
);

/// A scratch directory of this test's own, which goes away with the test.
///
/// Held rather than returned as a path, because the directory lives exactly as
/// long as this value does: a test that took the path and dropped the handle
/// would be reading a directory that had already been removed. It stands in
/// for a `&Path` everywhere a path is wanted, so a case reads the same as it
/// did when the directory was named and left behind.
struct Scratch(tempfile::TempDir);

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

/// A scratch directory of this test's own. The name is the prefix rather than
/// the whole of it, so two cases of one name cannot collide and nothing has to
/// clear the directory before using it.
fn scratch(name: &str) -> Scratch {
    Scratch(
        tempfile::Builder::new()
            .prefix(&format!("ronin-lint-{name}-"))
            .tempdir()
            .unwrap(),
    )
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

/// The harness's own promise, gated because nothing else would notice it
/// breaking: a suite that leaves a directory per case per run accumulates
/// them by the hundred on a developer's machine, and the only symptom is a
/// `/tmp` that looks like evidence of a leak in the tool under test.
#[test]
fn a_scratch_goes_with_its_test() {
    let path = {
        let directory = scratch("self-removing");
        assert!(directory.is_dir(), "{}", directory.display());
        directory.to_path_buf()
    };
    assert!(
        !path.exists(),
        "the scratch outlived the test that made it: {}",
        path.display()
    );
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
fn reduction(name: &str) -> Scratch {
    reduction_at(name, name)
}

/// The same, into a scratch directory the caller names, so two tests can read
/// one reduction without sharing a directory or racing each other in it.
#[cfg(all(unix, feature = "make"))]
fn reduction_at(name: &str, label: &str) -> Scratch {
    let directory = scratch(label);
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/make-project-reductions")
            .join(name),
        &directory,
    );
    directory
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
// [spec:ronin:req:make.nesting-census+2/test]
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

/// A composition whose child directory holds no makefile is a FINDING and not
/// the end of the report.
///
/// The tree is `lang-sme`'s `devtools/testing-from-old-infra/make-dictindex` in
/// miniature: a recipe line whose own command is the invocation, pointed at a
/// directory that exists and holds nothing a Make reads. Compiling it is a
/// refusal — there is no child graph — and reporting on it is not, so the
/// census here has to carry findings written above the missing child, findings
/// written below it, and findings from the child that does exist.
// [spec:ronin:req:make.nesting-census+2/test]
// [spec:ronin:req:tools.lint/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_census_survives_a_missing_child() {
    let directory = reduction("recursion-into-a-directory-with-no-makefile");
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    // A warning and not an error: the report is complete, and what it found is
    // a problem with the build rather than with the reading.
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(
        stdout(&output),
        format!(
            "{OVERRIDDEN}{COMPOSED_PRESENT}{COMPOSED_MISSING}{NO_MAKEFILE}{WRITE_A_MAKEFILE}\
             Makefile:18: {THROUGH_A_CONSTRUCT}\
             Makefile:18: {WRITE_IT_AS_THE_COMMAND}\
             present/Makefile:5: {THROUGH_A_CONSTRUCT}\
             present/Makefile:5: {WRITE_IT_AS_THE_COMMAND}\
             ronin: 4 recursive invocations: 2 composed, 2 nested; 1 of them found no makefile\n"
        )
    );
}

/// Building the same tree is still refused, because the graph genuinely cannot
/// be compiled: there is no child to compose and the recipe line that would
/// have started a Make of its own was lifted out of the recipe.
///
/// The report's tolerance is the report's alone. GNU Make 4.4.1 also fails this
/// tree, at the child it starts rather than at the compile.
// [spec:ronin:req:tools.lint/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_build_refuses_the_missing_child() {
    let directory = reduction_at(
        "recursion-into-a-directory-with-no-makefile",
        "no-makefile-built",
    );
    // Make mode is reached by the invoked name and by nothing else.
    let make = directory.join("make");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &make).unwrap();
    let output = Command::new(&make)
        .current_dir(&directory)
        .args(["-f", "Makefile", "all"])
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let said = String::from_utf8_lossy(&output.stderr).into_owned()
        + &String::from_utf8_lossy(&output.stdout);
    assert!(
        said.contains("no makefile found for recursive compilation in "),
        "the build must still refuse, and say why: {said}"
    );
}

/// A `.ONESHELL` recipe of several lines nests an invocation that would have
/// composed on its own, and the census says which of the three shapes that is
/// rather than reporting one nesting for all of them.
// [spec:ronin:req:make.nesting-census+2/test]
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
// [spec:ronin:req:make.nesting-census+2/test]
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

/// A build statement's binding that no rule it reaches ever expands. The
/// parser takes it, stores it and never looks at it again, so the build runs
/// exactly as it would with the line deleted — which is what makes a
/// misspelling of a name that WOULD have been read so easy to leave in.
// [spec:ronin:req:tools.manifest-lint/test]
#[test]
fn an_unexpanded_binding_is_reported() {
    let directory = scratch("unread-binding");
    write(
        &directory,
        "build.ninja",
        "rule cc\n  command = gcc $cflags -c $in -o $out\n\nbuild a.o: cc a.c\n  cflag = -O2\n",
    );
    let output = ronin(&directory, &["-t", "lint"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "ronin: warning: build `a.o`: the binding `cflag` is expanded by nothing its rule \
         writes, so the build runs as it would without the line\n\
         ronin: read 1 manifest\n"
    );
}

/// A phony statement runs nothing, and a binding on one is dispatched by rule
/// identity rather than by the absence of a command — so a `command` there is
/// accepted, stored, and never run.
// [spec:ronin:req:tools.manifest-lint/test]
#[test]
fn a_phony_binding_is_reported() {
    let directory = scratch("phony-binding");
    write(
        &directory,
        "build.ninja",
        "build all: phony a.c\n  command = rm -rf /\n",
    );
    let output = ronin(&directory, &["-t", "lint"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "ronin: warning: build `all`: the binding `command` runs nothing, because a phony \
         statement runs nothing\n\
         ronin: read 1 manifest\n"
    );
}

/// Ninja dispatches its built-in phony by identity, so a rule of that name in
/// a `subninja` scope is an ordinary rule that runs its command — the
/// opposite of what its name tells every reader of the statements using it.
// [spec:ronin:req:tools.manifest-lint/test]
#[test]
fn a_shadowed_phony_is_reported() {
    let directory = scratch("shadowed-phony");
    write(
        &directory,
        "sub.ninja",
        "rule phony\n  command = echo not really phony\nbuild fake: phony\n",
    );
    write(
        &directory,
        "build.ninja",
        "rule cc\n  command = gcc $in -o $out\nbuild a: cc a.c\nsubninja sub.ninja\n",
    );
    let output = ronin(&directory, &["-t", "lint"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "ronin: warning: a rule of its own named `phony` shadows the built-in one: a build \
         statement using it runs its command, where the name says it runs nothing\n\
         ronin: read 1 manifest\n"
    );
}

/// A cycle the build would never walk into, because no target it was asked
/// for reaches it. Every existing tool loads the manifest and says nothing
/// about it; a report about the manifest itself is the one place it shows.
// [spec:ronin:req:tools.manifest-lint/test]
#[test]
fn an_unreached_cycle_is_reported() {
    let directory = scratch("unreachable-cycle");
    write(
        &directory,
        "build.ninja",
        "rule cc\n  command = gcc $in -o $out\n\nbuild a: cc b\nbuild b: cc a\nbuild z: cc y\n",
    );
    let output = ronin(&directory, &["-t", "lint", "--ninja"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stdout(&output),
        "ronin: dependency cycle: a -> b -> a\n\
         ronin: read 1 manifest\n"
    );
}

/// The checks find nothing in a manifest a generator wrote, which is the
/// property that makes them worth running: a lint that fired on every real
/// build system would be a lint nobody reads.
// [spec:ronin:req:tools.manifest-lint/test]
#[test]
fn a_generated_manifest_lints_clean() {
    let directory = scratch("generated");
    write(
        &directory,
        "build.ninja",
        "cflags = -O2\n\
         rule cc\n  command = gcc $cflags -MD -MF $out.d -c $in -o $out\n           depfile = $out.d\n  deps = gcc\n  description = CC $out\n\
         rule link\n  command = gcc $in -o $out $libs\n  description = LINK $out\n\
         build a.o: cc a.c\n  cflags = -O2 -Wall\n\
         build prog: link a.o\n  libs = -lm\n\
         build all: phony prog\n\
         default all\n",
    );
    let output = ronin(&directory, &["-t", "lint"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_eq!(stdout(&output), "ronin: read 1 manifest\n");
}

/// A read that has to remake an include runs the recipe that remakes it, and a
/// `$(MAKE)` in that recipe starts the Make the report is about.
///
/// The one place a lint's own contract could fail to be true of itself: the
/// read is the read phase a build would perform, and a build's `$(MAKE)` is the
/// path of the Make that is running. Gated on what the child left behind rather
/// than on what either tool printed — a build log in the child's own directory
/// is a thing only a Make of ours writes.
// [spec:ronin:req:tools.lint/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_remade_include_recurses_here() {
    let directory = scratch("remade-include");
    fs::create_dir_all(directory.join("sub")).unwrap();
    write(
        &directory,
        "sub/Makefile",
        "marker: ; @printf '%s\\n' '$(MAKE)' > marker.out\n",
    );
    // The `;` is what keeps the invocation off the recipe line's own command,
    // so it stays in the recipe as something to run rather than being composed
    // into the graph — zsh's shape, and the one the read has to start.
    write(
        &directory,
        "Makefile",
        "all: ; @printf '%s\\n' '$(GENERATED)' > out\n\
         \n\
         include gen.mk\n\
         \n\
         gen.mk:\n\
         \t@printf 'GENERATED := from-generated\\n' > $@; $(MAKE) -C sub marker\n",
    );
    let output = ronin(&directory, &["-f", "Makefile", "-t", "lint", "all"]);
    // The nested invocation is a finding, so the report closes with one.
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).ends_with("ronin: 1 recursive invocation: 0 composed, 1 nested\n"),
        "{}",
        stdout(&output)
    );
    // The child ran, and it was one of ours: nothing else writes a build log.
    assert!(
        directory.join("sub").join(".ninja_log").is_file(),
        "the child left no build log, so it was not this Make"
    );
    // And what it was reached by: a path, ending in the name that selects the
    // front end, which is the only way a string a recipe runs can name this
    // executable and get a Make.
    let named = fs::read_to_string(directory.join("sub").join("marker.out")).unwrap();
    let named = Path::new(named.trim());
    assert!(named.is_absolute(), "{}", named.display());
    assert_eq!(named.file_name(), Some(std::ffi::OsStr::new("make")));
    // The link is this read's own and goes with it, which is also why the
    // child's own answer cannot be resolved from here any more.
    assert!(
        !named.exists(),
        "the staged make outlived the read that staged it"
    );
}
