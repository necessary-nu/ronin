//! Synthetic Makefile shapes for the Make-mode wall-time baseline.
//!
//! The two real trees the Make gate measures — vim and zsh — are the workloads
//! that matter, and they are also the ones that answer nothing on their own: a
//! ratio measured on vim mixes graph construction, recursion, `$(shell)` and
//! stat volume into one number, and a number like that names no cost. These
//! two shapes exist to split it. `wide` is one directory, one Makefile, four
//! thousand explicit rules and no recursion at all, so what it times is
//! building a graph. `recursive` is a tree of directories whose every Makefile
//! holds almost no graph and one `$(MAKE) -C` per child, so what it times is
//! reading a Makefile and composing a child — two hundred and fifty-nine times.
//!
//! Both are left up to date, because a no-op is the shape a developer spends
//! their day in and the shape GNU Make is fastest at: its no-op is a read and a
//! stat walk, where Ronin's is a read, a compile, a graph and a stat walk.
//!
//! The catalog is frozen at version 1 the way the Ninja workloads are: changing
//! a shape changes what the recorded ratios mean, so it takes a new version and
//! fresh recorded numbers.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

pub(crate) const MAKE_WORKLOAD_VERSION: u32 = 1;

/// Explicit rules in the wide Makefile. Large enough that graph construction
/// dominates the process start both tools pay, small enough that GNU Make's
/// no-op stays inside a few hundred milliseconds.
pub(crate) const WIDE_RULES: usize = 4_000;

/// Children each recursive directory dispatches into.
pub(crate) const RECURSION_FANOUT: usize = 6;

/// Directory levels below the root that recurse further.
pub(crate) const RECURSION_DEPTH: usize = 3;

/// Up-to-date file targets in each leaf directory, so that a composed child is
/// a Makefile with a graph in it rather than an empty one.
pub(crate) const RECURSION_LEAF_TARGETS: usize = 8;

/// Total Makefiles the recursive shape reads: the root, plus a level per
/// fan-out.
pub(crate) const fn recursion_units() -> usize {
    let mut total = 1;
    let mut level = 1;
    let mut power = RECURSION_FANOUT;
    while level <= RECURSION_DEPTH {
        total += power;
        power *= RECURSION_FANOUT;
        level += 1;
    }
    total
}

/// Write a source file and the output built from it, in that order, so the
/// output is never older than its prerequisite and the tree reads up to date.
fn up_to_date_pair(directory: &Path, source: &Path, output: &Path) -> io::Result<()> {
    fs::write(directory.join(source), b"baseline\n")?;
    fs::write(directory.join(output), b"baseline\n")
}

/// One directory, one Makefile, `WIDE_RULES` explicit rules, no recursion.
pub(crate) fn wide(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory.join("src"))?;
    fs::create_dir_all(directory.join("build"))?;

    let mut makefile = String::from(".PHONY: all\nall:");
    for index in 0..WIDE_RULES {
        let _ = write!(makefile, " build/{index}");
    }
    makefile.push('\n');
    for index in 0..WIDE_RULES {
        let _ = write!(
            makefile,
            "build/{index}: src/{index}\n\t@cp src/{index} build/{index}\n"
        );
    }
    for index in 0..WIDE_RULES {
        up_to_date_pair(
            directory,
            Path::new(&format!("src/{index}")),
            Path::new(&format!("build/{index}")),
        )?;
    }
    fs::write(directory.join("Makefile"), makefile)
}

/// A tree of directories, each dispatching into its children with `$(MAKE) -C`
/// and each leaf holding a small up-to-date graph.
pub(crate) fn recursive(directory: &Path) -> io::Result<()> {
    write_recursive_level(directory, 0)
}

fn write_recursive_level(directory: &Path, depth: usize) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    if depth == RECURSION_DEPTH {
        return write_recursive_leaf(directory);
    }

    let mut makefile = String::from("SUBDIRS =");
    for child in 0..RECURSION_FANOUT {
        let _ = write!(makefile, " sub{child}");
    }
    makefile.push_str(
        "\n.PHONY: all $(SUBDIRS)\nall: $(SUBDIRS)\n\
         $(SUBDIRS):\n\t@$(MAKE) --no-print-directory -C $@ all\n",
    );
    fs::write(directory.join("Makefile"), makefile)?;
    for child in 0..RECURSION_FANOUT {
        write_recursive_level(&directory.join(format!("sub{child}")), depth + 1)?;
    }
    Ok(())
}

fn write_recursive_leaf(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory.join("src"))?;
    fs::create_dir_all(directory.join("build"))?;

    let mut makefile = String::from(".PHONY: all\nall:");
    for index in 0..RECURSION_LEAF_TARGETS {
        let _ = write!(makefile, " build/{index}");
    }
    makefile.push('\n');
    for index in 0..RECURSION_LEAF_TARGETS {
        let _ = write!(
            makefile,
            "build/{index}: src/{index}\n\t@cp src/{index} build/{index}\n"
        );
    }
    for index in 0..RECURSION_LEAF_TARGETS {
        up_to_date_pair(
            directory,
            Path::new(&format!("src/{index}")),
            Path::new(&format!("build/{index}")),
        )?;
    }
    fs::write(directory.join("Makefile"), makefile)
}
