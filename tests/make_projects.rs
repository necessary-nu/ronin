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
