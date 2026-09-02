//! What a build shows a terminal, against what stock Ninja shows one.
//!
//! On a pipe every status line is written whole, and the conformance suite
//! compares those bytes. On a terminal Ninja overprints them — each status
//! line goes over the last, only a command's own output and a failure stay
//! on screen, and the build's end supplies the newline — and nothing in the
//! pipe-based suites can see that. This suite gives both tools a terminal and
//! compares what each wrote to it, byte for byte.
//!
//! The expectations are pinned as constants so the contract is readable here,
//! and where `reference/ninja-build/ninja` is present each is also checked
//! against that binary under the same terminal, so the constants are proven
//! current rather than trusted.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/pty.rs"]
mod pty;
#[path = "support/scratch.rs"]
mod scratch_directory;

use pty::{Transcript, run_under_terminal, scrollback};
use scratch_directory::Scratch;

/// Three edges: two with a description, one narrated by its command and
/// writing a line of output.
const MANIFEST: &str = "rule said\n  command = true\n  description = said $out\n\
                        rule noisy\n  command = echo out-of-$out\n\
                        build a: said\nbuild b: noisy a\nbuild c: said b\n";

/// A `console` command between ordinary ones.
const CONSOLE_MANIFEST: &str = "rule held\n  command = echo held\n  pool = console\n\
                                rule said\n  command = true\n  description = said $out\n\
                                build a: held\nbuild b: said a\n";

const FAILING_MANIFEST: &str = "rule fail\n  command = false\nbuild a: fail\n";

/// Stock Ninja's bytes for `MANIFEST` under a 40-column terminal: the start
/// of every edge and its finish each overprint the line, output goes below
/// on a line of its own, and the newline comes at the end.
const OVERPRINTED: &str = "\r[0/3] said a\x1b[K\r[1/3] said a\x1b[K\
                           \r[1/3] echo out-of-b\x1b[K\r[2/3] echo out-of-b\x1b[K\
                           \nout-of-b\n\
                           \r[2/3] said c\x1b[K\r[3/3] said c\x1b[K\n";

/// The same under `-n`: a dry run still overprints, and writes no output.
const OVERPRINTED_DRY_RUN: &str = "\r[0/3] said a\x1b[K\r[1/3] said a\x1b[K\
                                   \r[1/3] echo out-of-b\x1b[K\r[2/3] echo out-of-b\x1b[K\
                                   \r[2/3] said c\x1b[K\r[3/3] said c\x1b[K\n";

/// `-v` leaves the terminal alone: every command line, whole, once.
const VERBOSE: &str = "[1/3] true\n[2/3] echo out-of-b\nout-of-b\n[3/3] true\n";

/// `--quiet` shows a terminal nothing but what the commands wrote.
const QUIET: &str = "out-of-b\n";

/// A failure: the overprinted line, then the `FAILED:` block below it with
/// its prefix in red, then the command, then the stop — which Ninja's `Info`
/// writes to standard output, under the tool's own name.
const FAILED: &str = "\r[0/1] false\x1b[K\r[1/1] false\x1b[K\n\
                      \x1b[31mFAILED: [code=1] \x1b[0ma \nfalse\n\
                      ronin: build stopped: subcommand failed.\n";

/// A `console` command is announced as it starts and takes a line of its own
/// for what it writes straight to the terminal; nothing is said as it ends.
const CONSOLE: &str = "\r[0/2] echo held\x1b[K\nheld\n\r[1/2] said b\x1b[K\r[2/2] said b\x1b[K\n";

fn stock_ninja() -> Option<PathBuf> {
    let ninja = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference/ninja-build/ninja");
    ninja.is_file().then_some(ninja)
}

fn manifest_directory(label: &str, manifest: &str) -> Scratch {
    let directory = Scratch::named(&format!("ronin-terminal-{label}-"));
    fs::write(directory.join("build.ninja"), manifest).unwrap();
    directory
}

fn build_command(program: &Path, directory: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .arg("-j1")
        .args(arguments)
        .env_remove("NINJA_STATUS")
        .env_remove("MAKEFLAGS");
    command
}

fn screen(transcript: &Transcript) -> String {
    String::from_utf8_lossy(&transcript.screen).into_owned()
}

/// Stock Ninja's screen with the one difference that is not a difference
/// taken out: it speaks under its own name, and Ronin under Ronin's.
fn under_ronins_name(screen: &str) -> String {
    // A newline in front makes a first-line prefix the same case as the rest.
    let mut renamed = format!("\n{screen}").replace("\nninja: ", "\nronin: ");
    renamed.remove(0);
    renamed
}

/// Ronin's bytes for `manifest` under a 40-column terminal, checked against
/// stock Ninja's on the same manifest where that binary is present.
fn ninja_mode(label: &str, manifest: &str, arguments: &[&str]) -> Transcript {
    let directory = manifest_directory(label, manifest);
    let ronin = run_under_terminal(
        build_command(
            Path::new(env!("CARGO_BIN_EXE_ronin")),
            &directory,
            arguments,
        ),
        40,
    );
    if let Some(ninja) = stock_ninja() {
        let oracle_directory = manifest_directory(&format!("{label}-oracle"), manifest);
        let oracle = run_under_terminal(build_command(&ninja, &oracle_directory, arguments), 40);
        assert_eq!(
            screen(&ronin),
            under_ronins_name(&screen(&oracle)),
            "stock Ninja under the same terminal disagrees"
        );
        assert_eq!(ronin.status.code(), oracle.status.code());
    }
    ronin
}

// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn a_terminal_is_overprinted_as_ninja_overprints_it() {
    let transcript = ninja_mode("overprint", MANIFEST, &[]);
    assert!(transcript.status.success(), "{}", screen(&transcript));
    assert_eq!(screen(&transcript), OVERPRINTED);
    // What the terminal keeps: the output, under the status line that says
    // whose it is, and the last line of the counter. The two status lines
    // that no output followed are gone.
    assert_eq!(
        scrollback(&transcript.screen),
        ["[2/3] echo out-of-b", "out-of-b", "[3/3] said c"]
    );
}

// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn dry_run_overprints_verbose_and_quiet_do_not() {
    assert_eq!(
        screen(&ninja_mode("dry-run", MANIFEST, &["-n"])),
        OVERPRINTED_DRY_RUN
    );
    assert_eq!(screen(&ninja_mode("verbose", MANIFEST, &["-v"])), VERBOSE);
    assert_eq!(screen(&ninja_mode("quiet", MANIFEST, &["--quiet"])), QUIET);
}

// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn a_failure_goes_below_the_line_in_red() {
    let transcript = ninja_mode("failure", FAILING_MANIFEST, &[]);
    assert_eq!(transcript.status.code(), Some(1));
    assert_eq!(screen(&transcript), FAILED);
    assert!(transcript.stderr.is_empty(), "{:?}", transcript.stderr);
}

// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn a_console_command_owns_the_terminal_it_announces() {
    let transcript = ninja_mode("console", CONSOLE_MANIFEST, &[]);
    assert!(transcript.status.success(), "{}", screen(&transcript));
    assert_eq!(screen(&transcript), CONSOLE);
}

/// A status line wider than the terminal is cut in its middle rather than
/// wrapped, because a wrapped line cannot be taken back with one carriage
/// return.
// [spec:ronin:req:compat.terminal-status/test]
#[test]
fn a_status_line_is_cut_to_terminal_width() {
    let manifest = "rule said\n  command = true\n  description = 0123456789abcdefghijklmnopqrstuvwxyz\n\
                    build a: said\n";
    let transcript = ninja_mode("elided", manifest, &[]);
    assert!(transcript.status.success(), "{}", screen(&transcript));
    // Forty-two columns into forty: eighteen kept on the left of the
    // ellipsis, six of them the token, and nineteen on the right, as Ninja
    // splits them.
    assert_eq!(
        screen(&transcript),
        "\r[0/1] 0123456789ab...hijklmnopqrstuvwxyz\x1b[K\
         \r[1/1] 0123456789ab...hijklmnopqrstuvwxyz\x1b[K\n"
    );
}

/// Kbuild's own shapes, as in `tests/make_narration.rs`: silenced recipes,
/// `filechk` writing a whole shell program into one silenced line, a quiet
/// command narrated by a hoisted echo, an `@:` no-op, and one loud recipe.
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

/// Every silenced edge is a counter advancing in place; the hoisted
/// narrations are overprinted like any Ninja description; the one recipe
/// GNU Make echoes over two lines is written whole and once, as it finishes.
const KBUILD_OVERPRINTED: &str = "\r[0/6] \x1b[K\r[1/6] \x1b[K\n  UPD     generated/version.h\n\
                                  \r[1/6] \x1b[K\r[2/6] \x1b[K\
                                  \r[2/6]   CC      built.o\x1b[K\r[3/6]   CC      built.o\x1b[K\
                                  \r[3/6] \x1b[K\r[4/6] \x1b[K\
                                  \r[4/6]   STAMP   stamped\x1b[K\r[5/6]   STAMP   stamped\x1b[K\
                                  \r[6/6] echo louder\x1b[K\necho loudest\x1b[K\n\
                                  louder\nhidden\nloudest\n";

fn make_directory(label: &str, makefile: &str) -> Scratch {
    let directory = Scratch::named(&format!("ronin-terminal-make-{label}-"));
    fs::write(directory.join("Makefile"), makefile).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), directory.join("make")).unwrap();
    directory
}

fn make_command(directory: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new(directory.join("make"));
    command
        .current_dir(directory)
        .arg("-j1")
        .args(arguments)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("MAKELEVEL")
        .env_remove("CARGO_MAKEFLAGS");
    command
}

/// The kernel's shape on a terminal: what stays on screen is what GNU Make
/// prints, each piece of output under the status line that says whose it is
/// — a bare token only where a silent recipe wrote something — and the
/// counter's last line. Three silent edges that wrote nothing left nothing.
// [spec:ronin:req:compat.terminal-status/test]
#[cfg(feature = "make")]
#[test]
fn make_mode_leaves_a_terminal_what_gnu_prints() {
    let directory = make_directory("kbuild", KBUILD_SHAPED);
    let transcript = run_under_terminal(make_command(&directory, &[]), 80);
    assert!(
        transcript.status.success(),
        "{}{}",
        screen(&transcript),
        String::from_utf8_lossy(&transcript.stderr)
    );
    assert_eq!(screen(&transcript), KBUILD_OVERPRINTED);
    assert_eq!(
        scrollback(&transcript.screen),
        [
            "[1/6] ",
            "  UPD     generated/version.h",
            "[6/6] echo louder",
            "echo loudest",
            "louder",
            "hidden",
            "loudest",
        ]
    );
}

/// A silenced recipe that fails is still shown: the token, then Ninja's
/// `FAILED:` block naming the target and the command that failed, which is
/// more than GNU Make's `*** [Makefile:2: all] Error 1` says.
// [spec:ronin:req:compat.terminal-status/test]
#[cfg(feature = "make")]
#[test]
fn a_silenced_failure_is_shown_on_a_terminal() {
    let directory = make_directory("failing", "all:\n\t@false\n.PHONY: all\n");
    let transcript = run_under_terminal(make_command(&directory, &[]), 80);
    assert_eq!(transcript.status.code(), Some(2));
    let said = screen(&transcript);
    assert!(
        said.starts_with("\r[0/1] \x1b[K\r[1/1] \x1b[K\n\x1b[31mFAILED: [code=1] \x1b[0mall \n"),
        "{said:?}"
    );
    assert!(said.contains("false"), "{said:?}");
    // The token stays above the block, as Ninja's status line stays above a
    // failure, and the command line is the third line.
    let kept = scrollback(&transcript.screen);
    assert_eq!(kept[0], "[1/1] ", "{kept:?}");
    assert_eq!(
        kept[1], "\u{1b}[31mFAILED: [code=1] \u{1b}[0mall ",
        "{kept:?}"
    );
    assert!(kept[2].contains("false"), "{kept:?}");
    assert_eq!(
        kept[3], "ronin: build stopped: subcommand failed.",
        "{kept:?}"
    );
}

/// `-n` shows every command line whole, and `-s` shows nothing but output:
/// neither is a run Ninja overprints.
// [spec:ronin:req:compat.terminal-status/test]
#[cfg(feature = "make")]
#[test]
fn dry_run_and_silent_leave_the_terminal_alone() {
    let makefile = "all:\n\t@echo spoken\n.PHONY: all\n";
    let directory = make_directory("dry-run", makefile);
    let dry_run = screen(&run_under_terminal(make_command(&directory, &["-n"]), 80));
    assert!(dry_run.starts_with("[1/1] "), "{dry_run:?}");
    assert!(dry_run.contains("echo spoken"), "{dry_run:?}");
    assert!(!dry_run.contains('\r'), "{dry_run:?}");

    let directory = make_directory("silent", makefile);
    let silent = screen(&run_under_terminal(make_command(&directory, &["-s"]), 80));
    assert_eq!(silent, "spoken\n");

    let directory = make_directory("trace", makefile);
    let traced = screen(&run_under_terminal(
        make_command(&directory, &["--trace"]),
        80,
    ));
    assert!(traced.contains("echo spoken"), "{traced:?}");
    assert!(!traced.contains('\r'), "{traced:?}");
}
