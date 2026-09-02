//! What Ronin says while it builds, against what GNU Make 4.4.1 says.
//!
//! `[spec:ronin:req:make.narration+2]` fixes an edge's description as exactly
//! the bytes GNU Make would echo for its recipe, so the check that means
//! anything is the differential one: run both over the same makefile and
//! compare the text.
//!
//! The two do not print it in the same *order*, and cannot. GNU Make runs a
//! recipe line at a time and echoes each just before running it, so an echo, a
//! line's output, the next echo and its output interleave. Ronin compiles the
//! recipe to one edge, and one edge is narrated once — every echoed line first,
//! then everything the recipe wrote. The comparison is therefore over the lines
//! each printed rather than their sequence, which is the strongest statement
//! that is true of both: no line of GNU's is missing, no line Ronin invented is
//! there, and every shared line is byte-identical. The per-edge order is
//! asserted byte-for-byte in `tests/cli.rs` instead, where it is Ronin's own.
//!
//! What Ronin adds is the progress token and nothing else. `[N/M] ` is prefixed
//! to the first line of an edge's narration; the lines after it are the further
//! lines GNU echoed, unprefixed and unaltered. An edge that echoes nothing
//! prints the token alone. That is the whole of the difference, and this file
//! is what says so.
//!
//! Everything here runs through a pipe, where every line is written whole. On
//! a terminal the same lines are overprinted as Ninja overprints them, so a
//! silent edge is a counter advancing in place and leaves nothing behind;
//! `tests/terminal.rs` is where that is asserted, against stock Ninja.
#![cfg(all(unix, feature = "make"))]

use std::path::Path;
use std::process::Command;

/// The oracle module for the one thing this suite needs of it: which Make to
/// compare against. It was written for the recorder, which reads a whole
/// provenance record out of that Make; this suite only runs the binary, so
/// most of the module is dead here and live there — the same arrangement, and
/// the same reason, as `examples/make_conformance.rs`.
#[path = "support/oracle.rs"]
#[allow(dead_code, unreachable_pub)]
mod oracle;
#[path = "support/scratch.rs"]
mod scratch_directory;

use scratch_directory::Scratch;

/// Kbuild's own narration vocabulary, reduced to the shapes the kernel build
/// puts on screen: `$(Q)` silencing every recipe, `filechk` writing a whole
/// shell program into one silenced line, `quiet_cmd`/`kecho` narrating with a
/// silenced echo, an `@:` rule that exists only to be depended on, and a
/// recipe left loud so the echoing path is exercised beside the silent one.
const KBUILD_SHAPED: &str = r#"
Q = @
kecho := echo

define filechk
	$(Q)set -e; mkdir -p $(dir $@); trap "rm -f $(dir $@).tmp_$(notdir $@)" EXIT; { $(filechk_$(1)); } > $(dir $@).tmp_$(notdir $@); if [ ! -r $@ ] || ! cmp -s $@ $(dir $@).tmp_$(notdir $@); then $(kecho) '  UPD     $@'; mv -f $(dir $@).tmp_$(notdir $@) $@; fi
endef

filechk_version = echo 'define V 1'

quiet_cmd_cc = CC      $@
      cmd_cc = cp $< $@

all: generated/version.h built.o checked stamped loud

generated/version.h:
	$(call filechk,version)

built.o: built.c
	@echo '  $(quiet_cmd_cc)'; $(cmd_cc)

built.c:
	@echo body > $@

checked:
	@:

stamped:
	@echo '  STAMP   $@'
	@:

loud:
	echo louder
	@echo hidden
	echo loudest

.PHONY: all checked stamped loud
"#;

/// The Make to compare against.
///
/// `scripts/build-make-oracle.sh` leaves upstream 4.4.1 here, and every other
/// gate reads it through that path, so this reads it too rather than trusting
/// whichever 4.4.1 the host installed — `docs/make-oracle-divergences.md` is
/// the list of things four builds of that version number disagree about.
/// `MAKE_PORT_ORACLE` still overrides, and a host with neither falls back to
/// its own Make, which for recipe echoing has not changed in decades.
fn oracle_make() -> std::path::PathBuf {
    if std::env::var_os(oracle::ORACLE_VARIABLE).is_none() {
        let pinned =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("reference/make-oracle/make-4.4.1/make");
        if pinned.is_file() {
            return pinned;
        }
    }
    oracle::selected()
}

fn scratch_with(makefile: &str, name: &str) -> Scratch {
    let directory = Scratch::named(&format!("ronin-make-narration-{name}-"));
    std::fs::write(directory.join("Makefile"), makefile).unwrap();
    directory
}

fn run(program: &Path, directory: &Path) -> String {
    let output = Command::new(program)
        .current_dir(directory)
        .arg("-j1")
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .output()
        .unwrap_or_else(|error| panic!("running {}: {error}", program.display()));
    assert!(
        output.status.success(),
        "{} failed: {}{}",
        program.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("both Makes write text here")
}

/// Ronin's output with the one thing it adds taken back off.
///
/// A line beginning with the progress token loses it; a line that was nothing
/// but the token is an edge that echoed nothing, which is a line GNU Make never
/// printed at all and so is dropped rather than compared against a blank.
fn without_progress_tokens(said: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    for line in said.lines() {
        let stripped = match line.split_once("] ") {
            Some((token, rest)) if token.starts_with('[') && counts(&token[1..]) => rest,
            _ => line,
        };
        if !(line.starts_with('[') && stripped.is_empty()) {
            lines.push(stripped);
        }
    }
    lines
}

/// Whether a token's body is the `finished/total` pair the default format
/// writes, so a recipe that echoed `[1/2] something` is not mistaken for one.
fn counts(body: &str) -> bool {
    body.split_once('/').is_some_and(|(finished, total)| {
        !finished.is_empty()
            && !total.is_empty()
            && finished.bytes().all(|b| b.is_ascii_digit())
            && total.bytes().all(|b| b.is_ascii_digit())
    })
}

fn sorted(mut lines: Vec<&str>) -> Vec<&str> {
    lines.sort_unstable();
    lines
}

/// Every line GNU Make prints, Ronin prints too — and no others.
// [spec:ronin:req:make.narration+2/test]
#[test]
fn ronin_says_what_gnu_make_says() {
    let make = oracle_make();
    let gnu = scratch_with(KBUILD_SHAPED, "gnu");
    let ronin = scratch_with(KBUILD_SHAPED, "ronin");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();

    let said_by_gnu = run(&make, &gnu);
    let said_by_ronin = run(&ronin.join("make"), &ronin);

    assert_eq!(
        sorted(said_by_gnu.lines().collect()),
        sorted(without_progress_tokens(&said_by_ronin)),
        "GNU said:\n{said_by_gnu}\nRonin said:\n{said_by_ronin}"
    );
}

/// The shell program a silenced recipe compiles to never reaches the screen.
///
/// This is the defect the rule was rewritten for: `filechk` is one `@`-silenced
/// line holding a `set -e`, a `trap`, a redirect and an `if`, GNU Make prints
/// not one byte of it, and Ronin used to print the lot as the edge's
/// description. Checked by its parts rather than by the whole line, so a
/// respelling of the same leak still trips it.
// [spec:ronin:req:make.narration+2/test]
#[test]
fn a_silenced_recipe_never_shows_its_script() {
    let ronin = scratch_with(KBUILD_SHAPED, "no-script");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    let said = run(&ronin.join("make"), &ronin);

    for leak in ["set -e", "trap ", "cmp -s", "mv -f", "set +e", "; ("] {
        assert!(
            !said.contains(leak),
            "{leak:?} is the script, not the narration:\n{said}"
        );
    }
    // The narration the recipe did write for itself is still said, once.
    assert_eq!(
        said.matches("  UPD     generated/version.h").count(),
        1,
        "{said}"
    );
    assert_eq!(said.matches("  CC      built.o").count(), 1, "{said}");
}

/// An edge whose recipe echoes nothing prints its progress token and stops.
///
/// The counter stays whole — every edge is still counted and still reported —
/// and the line carries exactly what the status format puts in it, which is
/// where the trailing blank comes from. Nothing is appended to that.
// [spec:ronin:req:make.narration+2/test]
#[test]
fn an_edge_echoing_nothing_prints_the_token_alone() {
    let ronin = scratch_with(
        "silent:\n\t@:\n\t@echo ran > out\n.PHONY: silent\n",
        "token-alone",
    );
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    assert_eq!(run(&ronin.join("make"), &ronin), "[1/1] \n");
    assert!(ronin.join("out").exists(), "the recipe still ran");
}

/// `-n` shows the command line, because a dry run has nothing else to show.
///
/// The narration contract is about what a build says while it works; under
/// `-n` no work happens and the command is the whole of the answer. That is
/// Ninja's `-n` on the graph the Makefile compiled to, and it is why the
/// silence above does not reach here.
// [spec:ronin:req:make.narration+2/test]
#[test]
fn a_dry_run_still_shows_the_command() {
    let ronin = scratch_with("silent:\n\t@echo ran\n.PHONY: silent\n", "dry-run");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    let output = Command::new(ronin.join("make"))
        .current_dir(&*ronin)
        .args(["-n"])
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(said.contains("echo ran"), "{said}");
}

/// A silenced recipe that fails is not a mystery: the token, then Ninja's
/// `FAILED:` block naming the target and then the command line, which GNU
/// Make's own `*** [Makefile:2: all] Error 1` never shows.
// [spec:ronin:req:make.narration+2/test]
// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn a_silenced_recipe_that_fails_shows_its_command() {
    let ronin = scratch_with("all:\n\t@false\n.PHONY: all\n", "silent-failure");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    let output = Command::new(ronin.join("make"))
        .current_dir(&*ronin)
        .arg("-j1")
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.starts_with("[1/1] \nFAILED: [code=1] all \n"),
        "{said:?}"
    );
    assert!(
        said.lines()
            .nth(2)
            .is_some_and(|line| line.contains("false")),
        "{said:?}"
    );
}

/// `.SILENT` is the Makefile asking for what `@` asks for on every line, so
/// every edge is a token alone and the counter still counts them all.
// [spec:ronin:req:make.narration+2/test]
// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn a_silent_makefile_still_counts_every_edge() {
    let ronin = scratch_with(
        ".SILENT:\nall: one two\none:\n\techo one\ntwo:\n\techo two\n.PHONY: all one two\n",
        "dot-silent",
    );
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    assert_eq!(
        run(&ronin.join("make"), &ronin),
        "[1/2] \none\n[2/2] \ntwo\n"
    );
}

/// `--trace` and `-d` show every command line, silenced or not, as `-n` does:
/// GNU Make's print condition has `ISDB (DB_PRINT)` in it.
// [spec:ronin:req:make.narration+2/test]
#[test]
fn a_traced_run_shows_every_command_line() {
    let ronin = scratch_with("all:\n\t@echo traced\n.PHONY: all\n", "trace");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    let output = Command::new(ronin.join("make"))
        .current_dir(&*ronin)
        .args(["--trace"])
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(said.contains("echo traced"), "{said}");
}

/// Under `-s` nothing is narrated at all, token included.
// [spec:ronin:req:make.narration+2/test]
#[test]
fn silent_mode_narrates_nothing() {
    let ronin = scratch_with("loud:\n\techo spoken\n.PHONY: loud\n", "silent-mode");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), ronin.join("make")).unwrap();
    let output = Command::new(ronin.join("make"))
        .current_dir(&*ronin)
        .args(["-s"])
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "spoken\n",
        "the recipe's output and not a word about the recipe"
    );
}
