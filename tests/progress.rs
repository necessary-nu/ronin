//! What the `[N/M]` line counts, over both kinds of graph Ronin builds.
//!
//! The total is not the size of the plan. A `restat` that leaves its output
//! alone takes every consumer whose inputs are now clean out of the work, and
//! Ninja's total shrinks as that happens: a line printed before the prune
//! carries the larger number and every line after it carries the smaller one.
//! The expectations here are `reference/ninja-build/ninja`'s own output on the
//! manifest below, so they are a comparison rather than an assertion.

#![cfg(unix)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

#[path = "support/scratch.rs"]
mod scratch_directory;

use scratch_directory::Scratch;

/// The two lines the reference ninja prints for the second build of the tree
/// below, and what Ronin has to print for either language of it.
const SECOND_BUILD: &str = "[1/10] touch -m -d @1000000000 base\n[2/2] touch z\n";

/// A scratch directory of this test's own, which goes away with the test.
fn test_directory(label: &str) -> Scratch {
    Scratch::named(&format!("ronin-{label}-"))
}

/// Set a file's content and its modification time, so freshness is decided by
/// the fixture rather than by how long the test took.
fn write_at(directory: &Path, name: &str, contents: &str, seconds: u64) {
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

/// Bring the tree up to date, then age its two sources past everything.
///
/// `base` is rewritten with the timestamp it already had, so on the second run
/// the eight edges that read it stop being work while `z`, which has a newer
/// prerequisite of its own, stays work.
fn run_twice(directory: &Path, mut build: impl FnMut() -> std::process::Output) -> String {
    write_at(directory, "src", "one\n", 100);
    write_at(directory, "zsrc", "two\n", 100);
    let first = build();
    assert!(first.status.success(), "{first:?}");

    write_at(directory, "src", "one\n", 2_000_000_000);
    write_at(directory, "zsrc", "two\n", 2_000_000_000);
    let second = build();
    assert!(second.status.success(), "{second:?}");
    String::from_utf8(second.stdout).unwrap()
}

// [spec:ronin:sem:build.nodedone-fn/test]
#[test]
fn a_prune_leaves_the_total() {
    let directory = test_directory("restat-progress-total");
    let mut manifest = String::from(
        "rule stamp\n  command = touch -m -d @1000000000 $out\n  restat = 1\n\
         rule copy\n  command = touch $out\n\
         build base: stamp src\n",
    );
    for index in 1..=8 {
        let _ = writeln!(manifest, "build o{index}: copy base");
    }
    manifest.push_str("build z: copy o1 o2 o3 o4 o5 o6 o7 o8 zsrc\ndefault z\n");
    fs::write(directory.join("build.ninja"), manifest).unwrap();

    let reported = run_twice(&directory, || {
        Command::new(env!("CARGO_BIN_EXE_ronin"))
            .current_dir(&directory)
            .arg("-j1")
            .output()
            .unwrap()
    });
    assert_eq!(reported, SECOND_BUILD);
}

/// The same shape written as a Makefile, which is where it was reported from:
/// an install pass over a built tree read `[1/79]` … `[4/79]` and stopped. Make
/// mode narrates a Ninja graph Ninja's way, so the total shrinks here too.
///
/// The recipes are loud so that the whole line can be compared and not just its
/// counter: what GNU Make echoes for `touch z` is the same text the manifest
/// above binds as that rule's command, so both languages of this tree print the
/// reference ninja's own two lines byte for byte. Silencing them would leave
/// the counters to compare and nothing else.
// [spec:ronin:req:make.narration+2/test]
#[cfg(feature = "make")]
#[test]
fn make_mode_prunes_the_total_too() {
    let directory = test_directory("make-progress-total");
    fs::write(
        directory.join("Makefile"),
        "all: z\n\
         z: o1 o2 o3 o4 o5 o6 o7 o8 zsrc\n\
         \ttouch z\n\
         o1 o2 o3 o4 o5 o6 o7 o8: base\n\
         \ttouch $@\n\
         base: src\n\
         \ttouch -m -d @1000000000 base\n",
    )
    .unwrap();
    let make = directory.join("make");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &make).unwrap();

    let reported = run_twice(&directory, || {
        Command::new(&make)
            .current_dir(&directory)
            .arg("-j1")
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("MAKELEVEL")
            .output()
            .unwrap()
    });
    assert_eq!(reported, SECOND_BUILD);
}
