//! Reductions of what vim and zsh do that the conformance corpus does not.
//!
//! The corpus is a Makefile per feature; these are the shapes that appear when
//! a hand-written build system has been maintained for thirty years and uses
//! recursion the way a generator never would. Each case here was found by
//! building the real package with Ronin's Make mode beside GNU Make 4.4.1 and
//! reducing the first divergence, so each one is a package that does not
//! build rather than a Makefile that reads oddly.
//!
//! The tree each case runs is checked in under `tests/make-project-reductions`
//! and copied into a scratch directory to run, so the reproduction a reader
//! wants to do by hand is the one the test does.
//!
//! What is asserted is build intent: the exit code, and the file the recipe
//! was for. Not the narration — the runtime speaks Ninja, and a progress line
//! where GNU echoes a recipe is not a divergence.
//!
//! One case here is the exception, and it is deliberate. A quiet command is a
//! Makefile writing its own description, so what Ronin prints for it IS the
//! deliverable and nothing else can stand in for it. That case asserts on
//! Ronin's own `[N/M]` line and never on GNU's text.
#![cfg(all(unix, feature = "make"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Copy the named reduction into a scratch directory of its own.
fn reduction(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("ronin-make-project-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/make-project-reductions")
            .join(name),
        &path,
    );
    path
}

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

/// Make mode is reached by the invoked name and by nothing else.
fn make_command(directory: &Path) -> Command {
    let link = directory.join("make");
    if !link.exists() {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
    }
    let mut command = Command::new(link);
    command
        .current_dir(directory)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL");
    command
}

/// vim's top-level Makefile hands every goal to `src/` and then asks, in two
/// guards that are false for every goal but `test` and `clean`, whether more
/// recursion is wanted. GNU runs the guards and nothing happens.
///
/// Ronin lifts `cd src && $(MAKE) all` into a child compilation unit, but the
/// guarded line carries a `$(MAKE)` that cannot be lifted — it is not in
/// command position — and splitting was all-or-nothing, so the recipe
/// compiled to no children at all while still counting as recursive, and the
/// build was refused before anything ran.
///
/// Note what Ronin does when *no* line is liftable: it runs the script and
/// lets the nested Make start. The refusal was therefore stricter than the
/// path already taken for the same construct on its own, and vim is the tree
/// that shows the difference.
#[test]
fn a_guard_holds_an_unliftable_make() {
    let directory = reduction("recipe-mixes-liftable-and-unliftable-recursion");

    let output = make_command(&directory).arg("all").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        directory.join("src/built.stamp").exists(),
        "the child unit's recipe did not run"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// zsh's generated `Src/Makemod` builds each module header through a chain of
/// three targets, two of which re-invoke the same makefile: `X.mdh` needs
/// `X.mdhi`, `X.mdhi` needs `X.mdhs`, and both `X.mdh` and `X.mdhs` run
/// `$(MAKE) -f Makemod X.mdh.tmp`.
///
/// The two recursive wrappers are related only through `X.mdhi`, which has an
/// ordinary recipe. Ronin ordered held recursive edges by matching each one's
/// evaluation inputs against the other's outputs, and that comparison was
/// direct rather than transitive, so `X.mdh` was composed first: its input
/// `X.mdhi` was handed to a provisional build that had not yet been given the
/// edge which makes `X.mdhs`.
///
/// Naming `X.mdhs` as a direct prerequisite of `X.mdh` made the build pass,
/// which is the ordering pass saying what it could and could not see.
#[test]
fn a_wrapper_behind_a_middleman() {
    let directory = reduction("recursive-wrapper-reached-through-an-ordinary-target");

    let output = make_command(&directory).arg("mdh").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(directory.join("mdhs").exists(), "mdhs was never made");
    assert!(directory.join("mdh").exists(), "mdh was never made");
    fs::remove_dir_all(directory).unwrap();
}

/// zsh compiles a module's object with `.c.$(OBJ):` where `OBJ` is `.o`, so
/// the rule it writes is `.c..o:` and the `.SUFFIXES` line beside it names
/// `..o`. GNU Make writes every declared suffix in front of every other and
/// looks the name up, so `.c..o` is `%..o: %.c` because those are the two
/// suffixes on the list.
///
/// Ronin decided by counting dots, and a third dot meant "not a suffix rule",
/// so every `X..o` in zsh had no rule and none of the twenty-seven
/// dynamically loaded modules was built. The binary still links, which is why
/// this has to assert the object rather than the exit code alone.
#[test]
fn a_declared_suffix_holds_a_dot() {
    let directory = reduction("suffix-rule-target-suffix-holds-a-dot");

    let output = make_command(&directory).arg("all").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("foo..o")).unwrap(),
        "dyn from foo.c\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// The Linux kernel's build, reduced to one object per shape.
///
/// kbuild silences every recipe and echoes a short line in its place, which is
/// a Makefile writing a description in Make's vocabulary. Ronin had no way to
/// read it: the description fell back to the whole expanded script, the echo
/// still ran inside that script, and an operator building the kernel saw every
/// compile twice —
///
/// ```text
/// [1/2] set -e; echo '  CC      misc.o'; cc -c -o misc.o misc.c
///   CC      misc.o
/// ```
///
/// — which was reported as "a horrid mix of make and ninja". Both shapes the
/// pattern is written in are here: fused onto one line behind `set -e`, as
/// kbuild's `cmd` macro writes it, and split across two silenced lines.
///
/// This is the one case in this file that asserts narration, because for a
/// quiet command the narration is the whole of what was wrong. It asserts
/// Ronin's own progress line — the `[N/M]` counter carrying the text the
/// Makefile chose — and not GNU's, which says the same thing in its own voice.
#[test]
fn a_quiet_command_is_said_once() {
    let directory = reduction("kbuild-quiet-command");

    // One job, so the counter counts in the order the makefile named.
    let output = make_command(&directory)
        .args(["-j1", "all"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["[1/2]   CC      fused.o", "[2/2]   CC      split.o"],
        "a quiet command is one line per object, in Ronin's voice"
    );
    assert!(directory.join("fused.o").exists(), "fused.o was never made");
    assert!(directory.join("split.o").exists(), "split.o was never made");
    fs::remove_dir_all(directory).unwrap();
}
