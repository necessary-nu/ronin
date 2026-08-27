#![cfg(all(unix, feature = "make"))]

#[path = "support/scratch.rs"]
mod scratch_directory;

use scratch_directory::Scratch;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// A scratch directory of this case's own, which goes away with the case.
fn test_directory(label: &str) -> Scratch {
    Scratch::named(&format!("ronin-make-regression-{label}-"))
}

fn invoked_as(directory: &Path) -> PathBuf {
    let link = directory.join("make");
    if !link.exists() {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
    }
    link
}

fn make_command(directory: &Path) -> Command {
    let mut command = Command::new(invoked_as(directory));
    command
        .current_dir(directory)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL");
    command
}

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

/// Automake bootstraps dependency fragments by piping a filtered generated
/// Makefile through `make -f - am--depfiles`.
#[test]
fn make_stdin_bootstraps_automake_depfiles() {
    let directory = test_directory("stdin");
    let mut child = make_command(&directory)
        .args(["-f", "-", "am--depfiles"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"DEPDIR = .deps\n\
          am__depfiles_remade = ./$(DEPDIR)/alias.Po ./$(DEPDIR)/cd.Po\n\
          $(am__depfiles_remade):\n\
          \t@mkdir -p $(@D)\n\
          \t@: >>$@\n\
          am--depfiles: $(am__depfiles_remade)\n",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(directory.join(".deps/alias.Po").exists());
    assert!(directory.join(".deps/cd.Po").exists());
}

/// A current wrapper must stop graph composition before a recursive recipe.
#[test]
fn make_skips_current_recursive_wrapper() {
    let directory = test_directory("current-recursive");
    fs::write(
        directory.join("Makefile"),
        "all: out\n\
         out: self\n\
         \t@cp self out\n\
         self: source\n\
         \t@$(MAKE) -f Makefile all\n",
    )
    .unwrap();
    write_at(&directory, "source", "source\n", 100);
    write_at(&directory, "self", "self\n", 200);

    let output = make_command(&directory).arg("all").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "self\n");
}

/// Automake's suffix rules use `$*` to give each object a distinct dependency
/// file. The stem must survive when that Makefile is a compiled child unit.
#[test]
fn make_populates_suffix_rule_stem() {
    let directory = test_directory("suffix-rule-stem");
    let child = directory.join("child");
    fs::create_dir_all(child.join(".deps")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\t@$(MAKE) -C child all\n",
    )
    .unwrap();
    fs::write(
        child.join("Makefile"),
        br"all: one.o two.o
.SUFFIXES: .c .o
.c.o:
	@printf '%s\n' '$@' > .deps/$*.Tpo
	@mv -f .deps/$*.Tpo .deps/$*.Po
	@cp $< $@
",
    )
    .unwrap();
    fs::write(child.join("one.c"), "one\n").unwrap();
    fs::write(child.join("two.c"), "two\n").unwrap();

    let output = make_command(&directory)
        .args(["-j2", "all"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(child.join(".deps/one.Po")).unwrap(),
        "one.o\n"
    );
    assert_eq!(
        fs::read_to_string(child.join(".deps/two.Po")).unwrap(),
        "two.o\n"
    );
    assert!(!child.join(".deps/.Po").exists());
}

/// Command-line variables must survive a recursive Make hidden inside a shell
/// loop. Such a command cannot be composed as a semantic subninja, so the real
/// child process learns the override from the canonical MAKEFLAGS exported to
/// every recipe.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn shell_loop_submake_inherits_overrides() {
    let directory = test_directory("shell-loop-overrides");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\
         \t@printf 'FLAGS=%s\\nMFLAGS=%s\\n' \"$$MAKEFLAGS\" \"$$MFLAGS\"\n\
         \t@for dir in sub; do (cd $$dir && $(MAKE) print); done\n\
         .PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("sub/Makefile"),
        "VALUE = file-default\n\
         print: ; @printf 'VALUE=%s\\n' '$(VALUE)'\n\
         .PHONY: print\n",
    )
    .unwrap();

    let output = make_command(&directory)
        .args(["-j2", "-k", "all", "VALUE="])
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert!(reported.contains("FLAGS=k -j2"), "{reported}");
    assert!(reported.contains("MFLAGS=-k -j2"), "{reported}");
    assert!(reported.contains("VALUE=\n"), "{reported}");
    assert!(!reported.contains("file-default"), "{reported}");
}

/// Zstd selects a recursive build directory with a deferred `$(shell ...)`.
/// Kati deliberately leaves that computation as shell command substitution in
/// a recipe, so Ronin must settle it before parsing the child invocation.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn submake_expands_shell_computed_assignment() {
    let directory = test_directory("submake-shell-assignment");
    fs::write(
        directory.join("Makefile"),
        "HASH_DIR = conf_$(shell printf hash | sed -n 1p)\n\
         all:\n\
         \t+$(MAKE) --no-print-directory child BUILD_DIR=obj/$(HASH_DIR)\n\
         child: ; @printf '%s\\n' '$(BUILD_DIR)' > result\n\
         .PHONY: all child\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("result")).unwrap(),
        "obj/conf_hash\n"
    );
}

/// Deferred command substitution must observe files made by the recursive
/// wrapper's prerequisites, not run during the provisional graph compilation.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn submake_shell_waits_for_prerequisite() {
    let directory = test_directory("submake-shell-boundary");
    fs::write(
        directory.join("Makefile"),
        "all: stamp\n\
         \t+$(MAKE) --no-print-directory child VALUE=$(shell cat stamp)\n\
         stamp: ; @printf ready > $@\n\
         child: ; @printf '%s\\n' '$(VALUE)' > result\n\
         .PHONY: all child\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("result")).unwrap(),
        "ready\n"
    );
}

/// Zstd converts C and assembler names in two suffix-substitution passes. A
/// nonmatching second pass must retain `debug.o`, not append another `.o`.
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn substitution_reference_keeps_nonmatches() {
    let directory = test_directory("substitution-reference");
    fs::write(
        directory.join("Makefile"),
        "SRC := debug.c start.S notes.txt\n\
         OBJ0 := $(SRC:.c=.o)\n\
         OBJ := $(OBJ0:.S=.o)\n\
         all: ; @printf '%s\\n' '$(OBJ)' > result\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("result")).unwrap(),
        "debug.o start.o notes.txt\n"
    );
}

/// Findutils links one generated manual fragment to another. A later recursive
/// group updates the referent, making the followed link exactly as new as its
/// prerequisite. GNU Make reads the equal filesystem timestamps and leaves the
/// link alone; a wall-clock record of when the link was made would say the
/// opposite, and Make mode holds no such record to be misled by.
// [spec:ronin:req:make.state-outside-the-tree+3/test]
#[test]
fn symlink_freshness_reads_the_filesystem_alone() {
    let directory = test_directory("symlink-freshness");
    fs::write(
        directory.join("Makefile"),
        "all: link\n\
         link: source\n\
         \t@ln -s source link\n",
    )
    .unwrap();
    write_at(&directory, "source", "old\n", 100);

    let first = make_command(&directory).arg("all").output().unwrap();
    let reported = String::from_utf8_lossy(&first.stdout).into_owned()
        + &String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "{reported}");
    assert_eq!(
        fs::read_link(directory.join("link")).unwrap(),
        Path::new("source")
    );

    // Newer than the wall clock the first build ran at. The existing link
    // follows this file, so both filesystem mtimes are still equal.
    write_at(&directory, "source", "updated", 2_000_000_000);
    let output = make_command(&directory).arg("all").output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_link(directory.join("link")).unwrap(),
        Path::new("source")
    );
    assert_eq!(
        fs::read_to_string(directory.join("source")).unwrap(),
        "updated"
    );
}

/// Every autoconf tree carries `config.h: stamp-h1` with a recipe that does
/// nothing when the header is already there. Its prerequisite is permanently
/// newer, so the recipe runs on every invocation and the header never moves —
/// and a build that reads "the recipe ran" as "the header changed" recompiles
/// the entire tree, every time, forever. In pcre2 that put the `pcre2test`
/// link into the same graph as `make install`, whose libtool relink removes
/// `.libs/libpcre2-posix.so` for a few milliseconds, and the link found it
/// missing.
///
/// The invocations after the first are the whole point: one is the state, three
/// is the claim that the state is a fixed point.
// [spec:ronin:req:make.remade-target-re-observed/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn an_unmoved_stamp_rebuilds_nothing() {
    let directory = test_directory("stamp-no-cascade");
    fs::write(
        directory.join("Makefile"),
        "all: app\n\
         app: config.h\n\
         \t@echo run >> app.runs\n\
         \t@touch app\n\
         config.h: stamp-h1\n\
         \t@test -f $@ || echo made > $@\n",
    )
    .unwrap();
    write_at(&directory, "config.h", "header\n", 100);
    write_at(&directory, "app", "built\n", 150);
    write_at(&directory, "stamp-h1", "", 200);

    for invocation in 1..=3 {
        let output = make_command(&directory).output().unwrap();
        let reported = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "invocation {invocation}: {reported}"
        );
        assert!(
            !directory.join("app.runs").exists(),
            "invocation {invocation} remade app though config.h never moved: {reported}"
        );
    }

    // The other half of the claim: re-observing is not the same as never
    // rebuilding, so a header that does move still reaches what reads it.
    write_at(&directory, "config.h", "header\n", 300);
    let output = make_command(&directory).output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("app.runs")).unwrap(),
        "run\n",
        "a moved header must remake what reads it: {reported}"
    );
}

/// Zstd's object rules consist of nothing but GNU Make's built-in compile
/// variables. Undefined, the C rule lost both its compiler and its output
/// argument and ran `MMD`; the assembler rule became the source path alone and
/// the shell tried to execute it.
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn builtin_compile_variables_drive_recipes() {
    let directory = test_directory("builtin-compile");
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).unwrap();
    // Stands in for the catalogue's default `cc` and records the command line
    // it was reached with as the object it was asked to produce.
    let shim = bin.join("cc");
    fs::write(
        &shim,
        "#!/bin/sh\nout=\nprev=\nfor a in \"$@\"; do\n\
         \t[ \"$prev\" = -o ] && out=$a\n\tprev=$a\ndone\n\
         printf '%s\\n' \"$*\" > \"$out\"\n",
    )
    .unwrap();
    fs::set_permissions(&shim, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    fs::write(
        directory.join("Makefile"),
        "CFLAGS = -O3\n\
         ASFLAGS = -Wa,--noexecstack\n\
         DEPFLAGS = -MMD -MP\n\
         all: debug.o huf.o\n\
         debug.o: debug.c\n\
         \t$(COMPILE.c) $(DEPFLAGS) $(OUTPUT_OPTION) $<\n\
         huf.o: huf.S\n\
         \t$(COMPILE.S) $(OUTPUT_OPTION) $<\n",
    )
    .unwrap();
    fs::write(directory.join("debug.c"), "int debug;\n").unwrap();
    fs::write(directory.join("huf.S"), "/* nothing */\n").unwrap();

    let path = std::env::var("PATH").unwrap_or_default();
    let output = make_command(&directory)
        .env("PATH", format!("{}:{path}", bin.display()))
        .arg("all")
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("debug.o")).unwrap(),
        "-O3 -c -MMD -MP -o debug.o debug.c\n"
    );
    assert_eq!(
        fs::read_to_string(directory.join("huf.o")).unwrap(),
        "-Wa,--noexecstack -c -o huf.o huf.S\n"
    );
}

/// Feed a Makefile through standard input and report what the run said and did.
///
/// A run that refuses its arguments before reading standard input is one of the
/// outcomes tested here, and it closes the pipe while this write is still in
/// flight. The resulting `EPIPE` is that refusal arriving on the writing side,
/// not a failure of the harness: nothing was owed a reader. Whether the refusal
/// happened is decided by what the run said and what it left on disk, which is
/// where every assertion about it belongs — so a broken pipe delivers nothing
/// and says so, and any other write error is still a fault.
fn piped_make(directory: &Path, arguments: &[&str], source: &str) -> (bool, String) {
    let mut child = make_command(directory)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    match stdin.write_all(source.as_bytes()) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
        Err(error) => panic!("{arguments:?}: writing to standard input: {error}"),
    }
    // A run that does read standard input reads to the end of it, so the write
    // handle has to be closed before waiting rather than when the call returns.
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), reported)
}

/// There is one standard input, and a read that may start over. GNU Make
/// refuses an invocation naming it twice, however the two were spelled, and
/// refuses before it reads a byte of it.
#[test]
fn make_refuses_standard_input_twice() {
    let directory = test_directory("stdin-twice");
    fs::write(directory.join("bye.mk"), "def: ; @printf bye > out\n").unwrap();

    // Standard input announces itself if it is ever read, so "before it reads a
    // byte" is asserted by what the run says rather than by which side of the
    // pipe won the race. A run that reads this refuses for the wrong reason and
    // fails the assertion below; one that never reads it never sees it, whether
    // the write was delivered into the pipe's buffer or took EPIPE.
    let source = "$(error standard input was read before the invocation was refused)\n\
                  all: ; @printf hello > out\n";

    for spelling in [
        vec!["-fbye.mk", "-f-", "-f-"],
        vec!["-fbye.mk", "-f", "-", "-f", "-"],
        vec!["-fbye.mk", "-f-", "--file=-"],
        vec!["-fbye.mk", "--file", "-", "--makefile", "-"],
        vec!["-fbye.mk", "--file=-", "--makefile=-"],
    ] {
        let (succeeded, reported) = piped_make(&directory, &spelling, source);
        assert!(!succeeded, "{spelling:?} was accepted: {reported}");
        assert!(
            reported.contains("Makefile from standard input specified twice"),
            "{spelling:?} refused for another reason: {reported}"
        );
        assert!(!directory.join("out").exists(), "{spelling:?} built anyway");
    }
}

/// One `-f-` among several files is read in its turn, and the default goal
/// still comes from the first file named rather than from standard input.
#[test]
fn make_orders_standard_input_among_files() {
    let directory = test_directory("stdin-among-files");
    fs::write(directory.join("bye.mk"), "def: ; @printf bye > out\n").unwrap();

    let (succeeded, reported) = piped_make(
        &directory,
        &["-f", "bye.mk", "-f-"],
        "all: ; @printf hello > out\n",
    );
    assert!(succeeded, "{reported}");
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "bye");

    let (succeeded, reported) = piped_make(
        &directory,
        &["-f-", "-f", "bye.mk"],
        "all: ; @printf hello > out\n",
    );
    assert!(succeeded, "{reported}");
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "hello");

    // Named as a goal, standard input's rule is reached from either position.
    let (succeeded, reported) = piped_make(
        &directory,
        &["-f", "bye.mk", "-f-", "all"],
        "all: ; @printf hello > out\n",
    );
    assert!(succeeded, "{reported}");
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "hello");
}

/// GNU Make expands a recipe, finds it came to no command line at all, runs
/// nothing and reports the target up to date. What the run says about it is
/// narration; what it does not say is a command it never ran.
///
/// GNU Make 4.4.1 on `EMPTY =` / `all: ; $(EMPTY)` prints
/// `make: 'all' is up to date.` and nothing else.
#[test]
fn empty_expansion_reports_no_command() {
    let directory = test_directory("empty-expansion");
    fs::write(directory.join("Makefile"), "EMPTY =\nall: ; $(EMPTY)\n").unwrap();

    let output = make_command(&directory).output().unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{reported}");
    assert!(
        !reported.contains("/bin/sh"),
        "an empty expansion reached a shell: {reported}"
    );
    assert!(
        reported.contains("no work to do"),
        "a build that ran nothing did not say so: {reported}"
    );
}

/// A dry run stands in for the update a prerequisite's command would have
/// made, because it does not run the command that would make it. A recipe
/// holding no command line makes no such update in either run, so the
/// dependent stays as up to date as it already was.
///
/// GNU Make's own suite reaches this shape as options/dash-n tests 2 and 3,
/// where 4.4.1 answers `'a' is up to date.` with and without `-n`.
#[test]
fn dry_run_stops_at_empty_expansion() {
    let directory = test_directory("empty-expansion-dry-run");
    fs::write(
        directory.join("Makefile"),
        "EMPTY =\na: b ; echo made > $@\nb: c ; $(EMPTY)\n",
    )
    .unwrap();
    write_at(&directory, "b", "", 1_000);
    write_at(&directory, "a", "", 2_000);
    write_at(&directory, "c", "", 3_000);

    for arguments in [&[][..], &["-n"][..]] {
        let output = make_command(&directory).args(arguments).output().unwrap();
        let reported = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "{arguments:?}: {reported}");
        assert!(
            !reported.contains("echo made"),
            "{arguments:?}: remade a target GNU Make leaves alone: {reported}"
        );
    }
}

/// One stream, in the order the run wrote it.
///
/// Make mode gathers its narration and writes it at the end, while a diagnostic
/// goes to standard error as it happens — so concatenating the two captures
/// says nothing about which came first. A question about order has to be asked
/// of one descriptor, which means merging them before the run rather than after
/// it.
fn merged_make(directory: &Path, arguments: &[&str]) -> (bool, String) {
    let mut command = format!("exec {}", invoked_as(directory).display());
    for argument in arguments {
        command.push(' ');
        command.push_str(argument);
    }
    command.push_str(" 2>&1");
    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(directory)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// GNU Make does not report an `include` it could not open where it fails to
/// open it. `eval_makefile` records the errno on the goaldep and says nothing;
/// `show_goal_error` prints the complaint from inside the update that brings
/// the makefiles up to date, one line ahead of the refusal it belongs to. So a
/// Makefile that remakes itself narrates that work first, and the located line
/// comes after it rather than before.
///
/// GNU Make 4.4.1 on this case prints `REMAKE`, then
/// `Makefile:1: nope.mk: No such file or directory`, then
/// `make: *** No rule to make target 'nope.mk'.  Stop.`, and exits 2.
///
/// The build-intent gate compares files and status rather than output, which is
/// why this ordering is asserted here instead.
#[test]
fn include_complaint_waits_for_the_refusal() {
    let directory = test_directory("unopenable-include-order");
    let source = "include nope.mk\nMakefile: Makefile.src ; @echo REMAKE; cp Makefile.src Makefile\nall: ; @echo all\n";
    fs::write(directory.join("Makefile.src"), source).unwrap();
    write_at(&directory, "Makefile", source, 1_000);

    let (succeeded, reported) = merged_make(&directory, &[]);
    assert!(!succeeded, "{reported}");

    let remade = reported.find("REMAKE").expect(&reported);
    let complaint = reported
        .find("nope.mk: No such file or directory")
        .expect(&reported);
    let refusal = reported
        .find("No rule to make target 'nope.mk'")
        .expect(&reported);
    assert!(
        remade < complaint,
        "the complaint came out at the read rather than at the refusal: {reported}"
    );
    assert!(
        complaint < refusal,
        "the complaint came out after what it explains: {reported}"
    );
}

/// The complaint belongs to the `include` line that asked for the file, so a
/// Makefile the command line named has none — and GNU Make reports that one
/// from the read instead, before any remaking rather than after it.
///
/// GNU Make 4.4.1 on `make -fbye.mk -fR - all`, with `bye.mk` backdated and
/// remaking itself, prints `make: R: No such file or directory`, then the
/// remaking, then `make: *** No rule to make target 'R'.  Stop.`.
#[test]
fn named_makefile_complains_at_the_read() {
    let directory = test_directory("unopenable-named-makefile");
    let source = "all: ; @echo all\nbye.mk: bye.mk.src ; @echo REMAKE; touch bye.mk\n";
    fs::write(directory.join("bye.mk.src"), "").unwrap();
    write_at(&directory, "bye.mk", source, 1_000);

    let (succeeded, reported) = merged_make(&directory, &["-fbye.mk", "-fR", "-", "all"]);
    assert!(!succeeded, "{reported}");

    let complaint = reported
        .find("R: No such file or directory")
        .expect(&reported);
    let remade = reported.find("REMAKE").expect(&reported);
    assert!(
        complaint < remade,
        "a Makefile the command line named was complained of at the refusal: {reported}"
    );
}

/// `-q` is a status, and the three it can give are three different answers:
/// zero says the goals are already up to date, one says something would have to
/// run, two says the question could not be answered at all.
///
/// A forgiven Makefile the command line named turns one into two. GNU Make
/// restores `-q` for it while the makefiles are being rebuilt
/// (`file->cmd_target`, remake.c:169), so it is asked about rather than made;
/// the answer is "not up to date", which for a makefile is a failed update;
/// `dontcare` forgives it there and `no_diag` remembers that nothing was said;
/// and the goals then reach the same file, find it `updated` with a failing
/// status, and complain — which is fatal, so two outranks the question's one.
///
/// GNU Make 4.4.1 answers 2 to `make -q one.mk` here and leaves `one.mk`
/// uncreated. It answers 1 to the same case with `include` in place of
/// `-include`, because a required makefile sets no `no_diag` and so is never
/// complained about.
#[test]
fn question_refuses_a_forgiven_makefile_goal() {
    let directory = test_directory("question-forgiven-goal");
    let source = |include: &str| {
        format!(
            "{include} one.mk\nall: ; @echo all ran\none.mk: ; @echo GEN-one; echo X=1 > one.mk\n"
        )
    };

    fs::write(directory.join("Makefile"), source("-include")).unwrap();
    let forgiven = make_command(&directory)
        .args(["-q", "one.mk"])
        .output()
        .unwrap();
    assert_eq!(
        forgiven.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&forgiven.stderr)
    );
    assert!(
        !directory.join("one.mk").exists(),
        "the question built the file it was asked about"
    );

    fs::write(directory.join("Makefile"), source("include")).unwrap();
    let required = make_command(&directory)
        .args(["-q", "one.mk"])
        .output()
        .unwrap();
    assert_eq!(
        required.status.code(),
        Some(1),
        "a required makefile was refused over rather than asked about: {}",
        String::from_utf8_lossy(&required.stderr)
    );
    assert!(!directory.join("one.mk").exists());
}

/// The same question under `-k`, which is the only thing that lets GNU Make's
/// two failing statuses for a makefile be told apart.
///
/// `complain()` chooses `error` over `fatal` on `keep_going_flag`
/// (remake.c:422), so the run does not die inside the complaint — it returns
/// whatever `update_file_1` was going to return, which for a makefile the `-q`
/// pass merely ASKED about is `us_question`, and `main.c` turns that into
/// `MAKE_TROUBLE` rather than `MAKE_FAILURE`. Without `-k` the complaint is fatal
/// and 2 wins whatever the status was, which is the cell above.
///
/// The negative alongside it is what keeps the distinction off the ordinary
/// path: a goal with no rule was never asked about in a makefile pass, so it
/// takes `update_file_1`'s ordinary route, returns `us_failed`, and answers 2
/// under every combination of the two switches.
///
/// GNU Make 4.4.1 answers 1 to `make -q -k one.mk` here, reports the refusal,
/// and still leaves `one.mk` uncreated.
#[test]
fn keep_going_keeps_the_questions_status() {
    let directory = test_directory("question-keep-going");
    fs::write(
        directory.join("Makefile"),
        "-include one.mk\nall: ; @echo all ran\none.mk: ; @echo GEN-one; echo X=1 > one.mk\n",
    )
    .unwrap();
    let questioned = make_command(&directory)
        .args(["-q", "-k", "one.mk"])
        .output()
        .unwrap();
    assert_eq!(
        questioned.status.code(),
        Some(1),
        "a question was answered as a failure: {}",
        String::from_utf8_lossy(&questioned.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&questioned.stderr).is_empty(),
        "the refusal that made the question unanswerable was not reported"
    );
    assert!(
        !directory.join("one.mk").exists(),
        "the question built the file it was asked about"
    );

    fs::write(directory.join("Makefile"), "all: missing ; @echo all\n").unwrap();
    for arguments in [
        ["-q", "-k", "all"],
        ["-q", "-q", "all"],
        ["-k", "-k", "all"],
    ] {
        let ordinary = make_command(&directory).args(arguments).output().unwrap();
        assert_eq!(
            ordinary.status.code(),
            Some(2),
            "an ordinary goal with no rule stopped answering 2 under {arguments:?}"
        );
    }
}

/// `-k` makes `complain()` report rather than die (remake.c:422), so the
/// makefile update walks on: every required makefile nothing can make gets its
/// complaint and its refusal, rather than the run ending at the first.
///
/// GNU Make follows those with a second pass — `main.c`'s `us_failed` arm adds
/// `Failed to remake makefile 'X'.` for each — and Ronin does not: every name in
/// that list has already been reported one line above it, so the pass reports no
/// failure of its own. `[spec:ronin:req:make.narration+1]`.
///
/// GNU Make 4.4.1 on `make -k` here prints, in order: nope1's complaint, its
/// refusal, nope2's complaint, its refusal, then both summaries, and exits 2
/// with `all` never run.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn keep_going_refuses_every_makefile() {
    let directory = test_directory("keep-going-refusals");
    fs::write(
        directory.join("Makefile"),
        "include nope1.mk\ninclude nope2.mk\nall: ; @echo all\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &["-k"]);
    assert!(!succeeded, "{reported}");
    assert!(!reported.contains("all\n"), "the goal ran: {reported}");

    let positions = [
        "Makefile:1: nope1.mk: No such file or directory",
        "No rule to make target 'nope1.mk'",
        "Makefile:2: nope2.mk: No such file or directory",
        "No rule to make target 'nope2.mk'",
    ]
    .map(|line| {
        reported
            .find(line)
            .unwrap_or_else(|| panic!("{line}: {reported}"))
    });
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "each refusal did not come beside the complaint that explains it: {reported}"
    );
    assert!(
        !reported.contains("Failed to remake makefile"),
        "the run repeated names it had already reported: {reported}"
    );
}

/// Without `-k` the same case stops at the first: `complain()` is `fatal`, so
/// the update never reaches the second makefile.
#[test]
fn one_refusal_without_keep_going() {
    let directory = test_directory("single-refusal");
    fs::write(
        directory.join("Makefile"),
        "include nope1.mk\ninclude nope2.mk\nall: ; @echo all\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &[]);
    assert!(!succeeded, "{reported}");
    assert!(
        reported.contains("No rule to make target 'nope1.mk'"),
        "{reported}"
    );
    assert!(
        !reported.contains("nope2.mk"),
        "a makefile behind the refusal was considered: {reported}"
    );
}

/// `show_goal_error` has two callers, and this is the second: `child_error`
/// (job.c:581) prints the complaint a required `include` has been holding since
/// the open failed, one line ahead of the line that names the failure. A rule
/// that wins starts the read over instead, and the complaint is never made.
///
/// GNU Make 4.4.1 on this case prints `GENFAIL` on stdout and, on stderr,
/// `Makefile:1: gen.mk: No such file or directory` then
/// `make: *** [Makefile:2: gen.mk] Error 1`, and exits 2.
///
/// The complaint is a diagnostic, so it goes to stderr — where GNU Make puts it
/// and where every other Ronin diagnostic goes — rather than onto stdout beside
/// the `build stopped` line it explains. `[spec:ronin:req:make.narration+1]`.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn lost_remake_reports_its_unread_include() {
    let directory = test_directory("lost-remake-complaint");
    fs::write(
        directory.join("Makefile"),
        "include gen.mk\ngen.mk: ; @echo GENFAIL; exit 1\nall: ; @echo all\n",
    )
    .unwrap();

    let output = make_command(&directory).output().unwrap();
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    let complained = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{said}{complained}");

    assert!(said.contains("GENFAIL"), "{said}");
    assert!(said.contains("build stopped"), "{said}");
    assert!(
        complained.contains("gen.mk: No such file or directory"),
        "the complaint was not made on the diagnostic stream: {said}{complained}"
    );
    assert!(
        !said.contains("No such file or directory"),
        "the complaint was narrated on stdout: {said}"
    );
}

/// The same complaint, once per makefile, when `-k` lets the update reach more
/// than one of them.
///
/// `complain()` chooses `error` over `fatal` on `keep_going_flag`
/// (remake.c:422). Without `-k` the first complaint ends the run inside the
/// update and the makefiles after it are never considered; with `-k` the walk
/// carries on and every required makefile whose own recipe ran and lost gets its
/// own complaint. This is the recipe-lost half of the accounting
/// `keep_going_refuses_every_makefile` covers for makefiles nothing can make.
///
/// GNU Make 4.4.1 on `make -k` here prints `GEN1`, one.mk's complaint, its
/// failure, `GEN2`, two.mk's complaint, its failure, then a summary line for
/// each; on `make` it stops after one.mk's. Both exit 2 and neither writes
/// either fragment. The summaries are GNU runtime narration Ronin does not
/// repeat — every name in them has already been reported.
/// `[spec:ronin:req:make.narration+1]`.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn keep_going_complains_per_lost_makefile() {
    let directory = test_directory("keep-going-lost-makefiles");
    fs::write(
        directory.join("Makefile"),
        "include one.mk\ninclude two.mk\nall: ; @echo all ran\n\
         one.mk: ; @echo GEN1; exit 1\ntwo.mk: ; @echo GEN2; exit 1\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &["-k"]);
    assert!(!succeeded, "{reported}");
    let positions = [
        "Makefile:1: one.mk: No such file or directory",
        "Makefile:2: two.mk: No such file or directory",
    ]
    .map(|line| {
        reported
            .find(line)
            .unwrap_or_else(|| panic!("{line}: {reported}"))
    });
    assert!(
        positions[0] < positions[1],
        "the complaints were not made in the order the read reached them: {reported}"
    );
    assert!(
        reported.contains("GEN2"),
        "the update stopped early: {reported}"
    );
    assert!(
        !reported.contains("Failed to remake makefile"),
        "the run repeated names it had already reported: {reported}"
    );
    assert!(!reported.contains("all ran"), "the goal ran: {reported}");
    assert!(
        !directory.join("one.mk").exists() && !directory.join("two.mk").exists(),
        "a losing recipe left its makefile behind"
    );

    // Without the switch the complaint is fatal, so only the first is made.
    let (succeeded, reported) = merged_make(&directory, &[]);
    assert!(!succeeded, "{reported}");
    assert!(
        reported.contains("Makefile:1: one.mk: No such file or directory"),
        "{reported}"
    );
    assert!(
        !reported.contains("two.mk"),
        "a makefile behind the fatal complaint was considered: {reported}"
    );
}

/// A name `MAKEFILES` gave is remade like any other makefile the read reached.
///
/// GNU Make reads them with `eval_makefile (name, RM_NO_DEFAULT_GOAL|RM_INCLUDED
/// |RM_DONTCARE)` (read.c:204) and the goaldep that comes back joins `read_files`
/// like any other, so the update remakes it if a rule says how and restarts the
/// read on what the recipe wrote. `RM_DONTCARE` forgives the failure; it does
/// not excuse the attempt.
///
/// GNU Make 4.4.1 on `make MAKEFILES=gen.mk` here prints `GEN` then `all X=1`,
/// leaves `gen.mk` on disk and exits 0. A name with no rule behind it, and one
/// that will not open, are passed over without a word and exit 0 too — the
/// forgiveness covers the diagnostic as well as the failure.
#[test]
fn makefiles_variable_entries_are_remade() {
    let directory = test_directory("makefiles-variable-remade");
    fs::write(
        directory.join("Makefile"),
        "all: ; @echo all X=$(X)\ngen.mk: ; @echo GEN; echo X=1 > gen.mk\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &["MAKEFILES=gen.mk"]);
    assert!(succeeded, "{reported}");
    assert!(
        reported.contains("GEN"),
        "the fragment was not made: {reported}"
    );
    assert!(
        reported.contains("all X=1"),
        "the read did not start over on what the recipe wrote: {reported}"
    );
    assert_eq!(
        fs::read_to_string(directory.join("gen.mk")).unwrap(),
        "X=1\n"
    );

    // A name nothing knows how to make is forgiven in silence, as `-include` is.
    let (succeeded, reported) = merged_make(&directory, &["MAKEFILES=nope.mk"]);
    assert!(succeeded, "{reported}");
    assert!(
        !reported.contains("nope.mk"),
        "a forgiven name was reported: {reported}"
    );
}

/// The complaint belongs to the goal, not to the recipe that lost. GNU Make's
/// `show_goal_error` reads `goal_dep` — the goaldep `update_goal_chain` is
/// working on — so a required `include` whose rule never ran because one of its
/// own prerequisites failed is still what gets named.
///
/// GNU Make 4.4.1 prints `DEPFAIL`, then `Makefile:1: gen.mk: No such file or
/// directory`, then `make: *** [Makefile:4: dep] Error 1` — the complaint names
/// `gen.mk` and the failure names `dep`.
#[test]
fn a_lost_prerequisite_reports_the_goal() {
    let directory = test_directory("lost-remake-prerequisite");
    fs::write(
        directory.join("Makefile"),
        "include gen.mk\ngen.mk: dep\n\t@echo MAKEGEN; echo X=1 > gen.mk\ndep: ; @echo DEPFAIL; exit 1\nall: ; @echo all\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &[]);
    assert!(!succeeded, "{reported}");
    assert!(reported.contains("DEPFAIL"), "{reported}");
    assert!(
        reported.contains("gen.mk: No such file or directory"),
        "the complaint named the failing recipe rather than the goal: {reported}"
    );
    assert!(
        !reported.contains("MAKEGEN"),
        "the makefile's own recipe ran after its prerequisite lost: {reported}"
    );
}

/// `-include` never complains however it fails. GNU Make's guard is
/// `(goal_dep->flags & (RM_INCLUDED|RM_DONTCARE)) != RM_INCLUDED`, so the
/// forgiveness the read granted covers the diagnostic as well as the failure —
/// what ends this run is the refusal over a file nothing can make, with no
/// located line before it.
#[test]
fn a_forgiven_remake_makes_no_complaint() {
    let directory = test_directory("forgiven-remake-complaint");
    fs::write(
        directory.join("Makefile"),
        "-include gen.mk\ngen.mk: ; @echo GENFAIL; exit 1\nall: gen.mk\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &[]);
    assert!(!succeeded, "{reported}");
    assert!(reported.contains("GENFAIL"), "{reported}");
    assert!(
        !reported.contains("No such file or directory"),
        "an optional include complained about a read it forgave: {reported}"
    );
}

/// Whether this host lets an unreadable file be unreadable.
///
/// Running as root defeats mode 000, and a test that turns into its own
/// opposite is worse than one that does not run. The question is asked by trying
/// rather than by comparing user ids, because that is the property the case
/// needs.
fn permissions_are_enforced(directory: &Path) -> bool {
    let probe = directory.join(".permission-probe");
    fs::write(&probe, "x").unwrap();
    let mut permissions = fs::metadata(&probe).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o000);
    fs::set_permissions(&probe, permissions).unwrap();
    let enforced = fs::read(&probe).is_err();
    let mut permissions = fs::metadata(&probe).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o600);
    fs::set_permissions(&probe, permissions).unwrap();
    fs::remove_file(&probe).unwrap();
    enforced
}

/// A required `include` that would not open is a makefile the read did not get,
/// not a failure of the read — and GNU Make records it by giving the file the
/// timestamp of one that is not there (`eval_makefile` writes `deps->file->
/// last_mtime = NONEXISTENT_MTIME` beside the errno, read.c:409).
///
/// Which is what makes its own rule run. `secret.mk` exists and cannot be read;
/// a recipe with no prerequisites would be up to date the moment its target
/// exists, so only the nonexistent timestamp puts it out of date. GNU Make 4.4.1
/// on this case runs the rule, restarts the read on the repaired file, and
/// builds `all` with the variable that file defines.
///
/// The build-intent corpus cannot hold this one: mode 000 is not a thing a
/// committed file can be, and a run as root would record the opposite case
/// without saying so.
#[test]
fn unreadable_include_repaired_by_its_rule() {
    let directory = test_directory("unreadable-include-repaired");
    if !permissions_are_enforced(&directory) {
        return;
    }
    fs::write(
        directory.join("Makefile"),
        "include secret.mk\nsecret.mk: ; @echo GEN; chmod 644 secret.mk; echo X=1 >> secret.mk\nall: ; @echo ALL-$(X)\n",
    )
    .unwrap();
    let secret = directory.join("secret.mk");
    fs::write(&secret, "").unwrap();
    let mut permissions = fs::metadata(&secret).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o000);
    fs::set_permissions(&secret, permissions).unwrap();

    let (succeeded, reported) = merged_make(&directory, &["all"]);
    assert!(succeeded, "{reported}");
    assert!(
        reported.contains("GEN"),
        "the rule for an unreadable include never ran: {reported}"
    );
    assert!(
        reported.contains("ALL-1"),
        "the read did not start over on the repaired file: {reported}"
    );
}

/// A Makefile the command line named that would not open is complained of at
/// the read and refused over after the remaking, which is the same two-part
/// shape as the absent case and reaches it by the same deferral.
///
/// `afile` is an ordinary file, so opening `afile/one.mk` fails with ENOTDIR
/// rather than ENOENT — the errno GNU Make does not distinguish and Ronin used
/// to raise on. GNU Make 4.4.1 on `make -f side.mk -f afile/one.mk all` prints
/// `make: afile/one.mk: Not a directory`, then `REMAKE-SIDE`, then
/// `make: *** No rule to make target 'afile/one.mk'.  Stop.`
///
/// Asserted here rather than in the corpus because it is an ordering: the
/// complaint has to come out before the work and the refusal after it.
#[test]
fn unopenable_makefile_refused_after_remaking() {
    let directory = test_directory("unopenable-named-makefile-refused");
    fs::write(directory.join("afile"), "notadir\n").unwrap();
    fs::write(directory.join("side.src"), "").unwrap();
    write_at(
        &directory,
        "side.mk",
        "all: ; @echo all\nside.mk: side.src ; @echo REMAKE-SIDE; touch side.mk\n",
        1_000,
    );

    let (succeeded, reported) =
        merged_make(&directory, &["-f", "side.mk", "-f", "afile/one.mk", "all"]);
    assert!(!succeeded, "{reported}");

    let complaint = reported
        .find("afile/one.mk: Not a directory")
        .expect(&reported);
    let remade = reported.find("REMAKE-SIDE").expect(&reported);
    let refusal = reported
        .find("No rule to make target 'afile/one.mk'")
        .expect(&reported);
    assert!(
        complaint < remade,
        "the read's own complaint came out after the remaking: {reported}"
    );
    assert!(
        remade < refusal,
        "the refusal came out before the Makefiles ahead of it were remade: {reported}"
    );
}

/// GNU Make prints an `$(info ...)` while it expands the recipe, so the text
/// is never a command. What the corpus cannot see is that the text still
/// reaches stdout, and that nothing was announced as having run.
#[test]
fn printing_alone_starts_no_command() {
    let directory = test_directory("recipe-output-function");
    fs::write(
        directory.join("Makefile"),
        "all: ; $(info EXPANDED)$(warning WARNED)\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &["all"]);
    assert!(succeeded, "{reported}");
    assert!(reported.contains("EXPANDED"), "{reported}");
    assert!(reported.contains("Makefile:1: WARNED"), "{reported}");
    assert!(
        !reported.contains("printf") && !reported.contains("[1/"),
        "the text was printed rather than run: {reported}"
    );
}

/// `$(error)` in recipe position is raised out of the expansion, which is what
/// makes it fire under `-n`: the recipe is expanded either way, and there is no
/// command whose absence could swallow it.
#[test]
fn a_dry_run_raises_the_error() {
    let directory = test_directory("recipe-error-function");
    fs::write(
        directory.join("Makefile"),
        "all: ; $(error BOOM)@echo built > out\n",
    )
    .unwrap();

    let (succeeded, reported) = merged_make(&directory, &["-n", "all"]);
    assert!(!succeeded, "{reported}");
    assert!(reported.contains("Makefile:1: BOOM"), "{reported}");
    assert!(!directory.join("out").exists(), "{reported}");
}

/// A `$(shell)` command with no shell syntax in it is exec'd directly, so the
/// program that is not there is reported against its own name.
///
/// GNU Make's `construct_command_argv_internal` (reference/gnumake/src/job.c)
/// hands a line to `$(SHELL)` only when something in it needs a shell. For one
/// that does not, Make goes looking for the program itself and says so:
/// `make: ./nosuchprog: No such file or directory`. A single `>` in the line
/// makes it the shell's errand instead, and then the shell is the one that
/// reports — `/bin/sh: 1: ./nosuchprog: not found` — which is what Ronin used
/// to say for both.
#[test]
fn shell_function_names_the_missing_program() {
    let directory = test_directory("shell-direct");
    write_at(
        &directory,
        "Makefile",
        "DIRECT := $(shell ./nosuchprog arg)\n\
         all: ; @printf '%s\\n' 'direct [$(DIRECT)]'\n",
        1,
    );
    let direct = make_command(&directory).output().unwrap();
    let said = String::from_utf8_lossy(&direct.stderr).into_owned();
    assert!(
        said.contains("./nosuchprog: No such file or directory"),
        "the program's own name and the errno, not a shell's wording: {said}"
    );
    assert!(
        !said.contains("/bin/sh"),
        "no shell was involved, so none may be quoted: {said}"
    );
    assert!(
        String::from_utf8_lossy(&direct.stdout).contains("direct []"),
        "the command produced nothing, as it does for GNU Make"
    );

    // The same command with one redirection in it: now a shell is required,
    // and the shell is what reports.
    write_at(
        &directory,
        "Makefile",
        "SHELLED := $(shell ./nosuchprog > out)\n\
         all: ; @printf '%s\\n' 'shelled [$(SHELLED)]'\n",
        1,
    );
    let shelled = make_command(&directory).output().unwrap();
    let said = String::from_utf8_lossy(&shelled.stderr).into_owned();
    assert!(
        said.contains("/bin/sh"),
        "a redirection is the shell's errand and its diagnostic: {said}"
    );
}

/// A recipe of several lines is several processes, and what all of them wrote
/// reaches the caller in the order they wrote it — one report for one edge,
/// which is what the corpus cannot see because it compares files.
#[test]
fn recipe_lines_report_output_in_order() {
    let directory = test_directory("per-line-output");
    fs::write(
        directory.join("Makefile"),
        "all:\n\
         \t@echo first\n\
         \t@echo second >&2\n\
         \t@echo third\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    let order = ["first", "second", "third"].map(|line| said.find(line));
    assert!(
        order.iter().all(Option::is_some) && order.windows(2).all(|pair| pair[0] < pair[1]),
        "{said}"
    );
}

/// A line the makefile said to ignore fails without the build noticing, and
/// the lines after it still run. The status is the only evidence a corpus case
/// could not carry, since the files are written either way.
#[test]
fn an_ignored_line_does_not_fail() {
    let directory = test_directory("ignored-line");
    fs::write(
        directory.join("Makefile"),
        "all:\n\
         \t-false\n\
         \t-./nosuchprogram\n\
         \t@echo reached > reached\n\
         \t-false\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    assert!(
        output.status.success(),
        "status {:?}\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("reached")).unwrap(),
        "reached\n"
    );
}

/// GNU Make execs a command line with no shell syntax in it itself, so it is
/// Make that reports a program it could not start — against the command's own
/// name, and with the status POSIX gives a command that never ran.
#[test]
fn make_reports_a_missing_program() {
    let directory = test_directory("missing-program");
    fs::write(directory.join("Makefile"), "all:\n\t./nosuchprogram arg\n").unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("ronin: ./nosuchprogram: No such file or directory"),
        "{said}"
    );
    // What a shell would have said instead, had one been in the way. The
    // failure block still names the edge's command, wrapper included, because
    // that is the one name the edge has; what must not appear is a shell's
    // account of a program it went looking for.
    assert!(!said.contains("not found"), "{said}");
    assert!(said.contains("code=127"), "{said}");
}

/// A recipe line GNU Make classifies recursive whose invocation cannot be
/// lifted out is residual work, and reaches the executor as written. That is
/// what the same line does when it is a recipe's only recursion, so having a
/// liftable sibling cannot be what makes it intolerable: refusing the whole
/// recipe made a mixed one stricter than either of the recipes it is made of.
///
/// vim's top-level Makefile is the tree that showed it — one liftable
/// `cd src && $(MAKE) $@` beside two guards holding `$(MAKE)` calls that are
/// false for every goal but `test` and `clean` — and it built nothing at all.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn an_unliftable_line_keeps_its_siblings() {
    let directory = test_directory("make-recursion-guard");
    for child in ["a", "b"] {
        fs::create_dir_all(directory.join(child)).unwrap();
        fs::write(
            directory.join(child).join("Makefile"),
            format!("child: ; echo {child} > built\n"),
        )
        .unwrap();
    }

    // The second invocation is real but sits behind a runtime test, so it is
    // not one static child compilation. The first is, and is composed; the
    // second runs as the shell command it is, and its test is true.
    fs::write(
        directory.join("Makefile"),
        "all:\n\t$(MAKE) -C a\n\ttest -d b && $(MAKE) -C b\n",
    )
    .unwrap();
    let mixed = make_command(&directory).output().unwrap();
    assert!(
        mixed.status.success(),
        "{}",
        String::from_utf8_lossy(&mixed.stderr)
    );
    for child in ["a", "b"] {
        assert!(
            directory.join(child).join("built").exists(),
            "{child} was not built"
        );
    }

    // vim's own shape: the guard is false, so what the line holds is never
    // reached and nothing beside the composed child happens.
    for child in ["a", "b"] {
        fs::remove_file(directory.join(child).join("built")).unwrap();
    }
    fs::write(
        directory.join("Makefile"),
        "all:\n\t$(MAKE) -C a\n\t@if false; then (cd b && $(MAKE)); fi\n",
    )
    .unwrap();
    let guarded = make_command(&directory).output().unwrap();
    assert!(
        guarded.status.success(),
        "{}",
        String::from_utf8_lossy(&guarded.stderr)
    );
    assert!(directory.join("a").join("built").exists());
    assert!(
        !directory.join("b").join("built").exists(),
        "a guard that is false built something"
    );

    // MAKE named as an argument is not a Make being started, and 4.4.1 runs
    // the line under `-n` without recursing into anything either.
    fs::write(
        directory.join("Makefile"),
        "all:\n\t$(MAKE) -C a\n\ttest -d b && echo mentioned $(MAKE) > mentioned\n",
    )
    .unwrap();
    let mentioned = make_command(&directory).output().unwrap();
    assert!(
        mentioned.status.success(),
        "naming MAKE in an argument was read as recursion: {}",
        String::from_utf8_lossy(&mentioned.stderr)
    );
    assert!(directory.join("mentioned").exists());
    assert!(directory.join("a").join("built").exists());
}

/// A recipe line that is one invocation inside a subshell is one invocation.
/// The parentheses keep a directory change from reaching the rest of the
/// script, and a line holding only the sequence has no rest of the script for
/// it to reach, so the child is the same child either way.
///
/// Proved by a dry run rather than by the build: a composed child's work is
/// printed and not done, where a nested Make would have been started to find
/// out what it was.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn a_subshell_holds_one_invocation() {
    let directory = test_directory("make-recursion-subshell");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\t(cd sub && $(MAKE) child)\n",
    )
    .unwrap();
    fs::write(
        directory.join("sub").join("Makefile"),
        "child: ; echo child > child\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("-n").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.contains("echo child > child"),
        "the subshell's child graph was not in the dry run: {printed}"
    );
    assert!(
        !directory.join("sub").join("child").exists(),
        "the dry run wrote the child's target"
    );
}

/// A brace group is the subshell written the other way and holds the same one
/// invocation. What it costs to see is the reading: `(` is always the operator
/// and `{` is a reserved word, so a group is one only as a word of its own
/// where a command may begin — `echo a{b}` is one word and must stay one.
///
/// Proved the same way as the subshell: a dry run prints the composed child's
/// work, where a nested Make would have been started to find out what it was.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn a_brace_group_holds_one_invocation() {
    let directory = test_directory("make-recursion-brace");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\t{ cd sub && $(MAKE) child; }\n",
    )
    .unwrap();
    fs::write(
        directory.join("sub").join("Makefile"),
        "child: ; echo child > child\n",
    )
    .unwrap();

    let output = make_command(&directory).arg("-n").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.contains("echo child > child"),
        "the brace group's child graph was not in the dry run: {printed}"
    );
    assert!(
        !directory.join("sub").join("child").exists(),
        "the dry run wrote the child's target"
    );

    // A brace that is part of a word is not a group, so the line stays a line
    // and the reference on it is data.
    fs::write(
        directory.join("Makefile"),
        "all:\n\techo a{b} $(MAKE) > mentioned\n",
    )
    .unwrap();
    let mentioned = make_command(&directory).output().unwrap();
    assert!(
        mentioned.status.success(),
        "a brace inside a word was read as a group: {}",
        String::from_utf8_lossy(&mentioned.stderr)
    );
    let written = fs::read_to_string(directory.join("mentioned")).unwrap();
    assert!(written.starts_with("a{b} "), "{written}");
}

/// The environment is the third way a write reaches a name GNU Make works out
/// for itself, and it is the one no corpus case can carry: the port harness
/// runs every case with the same environment.
///
/// `.VARIABLES` is rebuilt from the variable table at every lookup, so what
/// the environment says about it is stored and never read back; `.SHELLSTATUS`
/// does not exist until a `$(shell)` has run, so an environment value for it
/// is the name's first definition and stands until one does. Neither stops the
/// read, which is what this gates — Ronin used to abandon with `cannot assign
/// to readonly variable` and build nothing at all.
#[test]
fn environment_reaches_a_worked_out_name() {
    let directory = test_directory("environment-write-to-a-worked-out-name");
    fs::write(
        directory.join("Makefile"),
        "all:\n\
         \t@printf 'status=[%s] wrote=[%s] foo=[%s]\\n' \
         '$(.SHELLSTATUS)' '$(filter from-env,$(.VARIABLES))' '$(FOO)' > out\n",
    )
    .unwrap();

    let output = make_command(&directory)
        .env(".VARIABLES", "from-env")
        .env(".SHELLSTATUS", "from-env")
        .env("FOO", "from-env")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "an environment write to a worked-out name stopped the read: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = fs::read_to_string(directory.join("out")).unwrap();
    assert_eq!(
        written, "status=[from-env] wrote=[] foo=[from-env]\n",
        "{written}"
    );
}

/// GNU Make's `decode_switches` never dies where it notices a word it cannot
/// read: it raises a flag, consumes the word, and only the command line acts on
/// the flag afterwards — `if (bad && origin == o_command) print_usage (bad)`.
/// Ronin's second failure channel used to raise instead, which came out as
/// Ninja's status rather than Make's for a command line, and ended a build a
/// makefile would have finished.
#[test]
fn a_bad_switch_ends_only_argv() {
    let directory = test_directory("unreadable-switch");
    fs::write(directory.join("Makefile"), "all: ; @echo built > out\n").unwrap();

    // The command line abandons with Make's status, not Ninja's.
    for argument in ["-W", "-I", "-f", "--eval", "--jobserver-style", "-jabc"] {
        let refused = make_command(&directory).arg(argument).output().unwrap();
        assert_eq!(
            refused.status.code(),
            Some(2),
            "{argument}: {}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(
            !directory.join("out").exists(),
            "{argument} built something"
        );
    }

    // A makefile's own write loses the switch and the build goes on, and the
    // two GNU Make complains about at every origin are complained about here.
    fs::write(
        directory.join("Makefile"),
        "MAKEFLAGS += -W\n\
         MAKEFLAGS += -jabc\n\
         MAKEFLAGS += --include-dir=\n\
         all: ; @echo built > out\n",
    )
    .unwrap();
    let built = make_command(&directory).output().unwrap();
    assert!(
        built.status.success(),
        "status {:?}\n{}",
        built.status.code(),
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("out")).unwrap(),
        "built\n"
    );
    let said = String::from_utf8_lossy(&built.stderr);
    assert!(
        said.contains("the '-j' option requires a positive integer argument"),
        "{said}"
    );
    assert!(
        said.contains("the '-I' option requires a non-empty string argument"),
        "{said}"
    );
    assert!(
        !said.contains("missing -W value"),
        "a switch GNU Make's getopt loses silently is lost silently: {said}"
    );
}

/// What `-q` exits with once a `+` line has run inside it.
///
/// The corpus records whether a case succeeded, not with which number, so the
/// three statuses `-q` can leave and the difference between them live here.
/// GNU Make's `reap_children` (job.c:954) reads a line it ran while
/// questioning by its status: zero carries the recipe on, exactly one is
/// `MAKE_TROUBLE` — the question's own "something to do", reported silently —
/// and anything else is `MAKE_FAILURE`, a build that ran and lost.
///
/// GNU Make 4.4.1 answers 0, 1, 1 and 2 to these four in order, and writes the
/// `+` line's file in every one of them.
#[test]
fn question_status_reads_the_plus_line() {
    let directory = test_directory("question-plus-status");
    let cases = [
        ("+@echo plus > plus.txt\n", 0),
        (
            "+@echo plus > plus.txt\n\t@echo ordinary > ordinary.txt\n",
            1,
        ),
        ("+@echo plus > plus.txt; exit 1\n", 1),
        ("+@echo plus > plus.txt; exit 2\n", 2),
    ];
    for (recipe, status) in cases {
        let _ = fs::remove_file(directory.join("plus.txt"));
        let _ = fs::remove_file(directory.join("ordinary.txt"));
        fs::write(directory.join("Makefile"), format!("all:\n\t{recipe}")).unwrap();
        let answered = make_command(&directory).arg("-q").output().unwrap();
        assert_eq!(
            answered.status.code(),
            Some(status),
            "`{recipe}` answered {:?}: {}",
            answered.status.code(),
            String::from_utf8_lossy(&answered.stderr)
        );
        assert_eq!(
            fs::read_to_string(directory.join("plus.txt")).unwrap(),
            "plus\n",
            "the question did not run `{recipe}`"
        );
        assert!(
            !directory.join("ordinary.txt").exists(),
            "the question ran the line it was supposed to answer on"
        );
    }
}

/// `-k` is the only thing that lets a `+` line past an answer already given.
///
/// `update_goal_chain` sets `stop` on `question_flag && !keep_going_flag`
/// (remake.c:206), so without the switch the walk ends at the first goal that
/// answers and the marked line behind it never runs. GNU Make 4.4.1 answers 1
/// to both and writes `plus.txt` only under `-k`.
#[test]
fn keep_going_passes_an_answered_goal() {
    let directory = test_directory("question-plus-keep-going");
    fs::write(
        directory.join("Makefile"),
        "all: ordinary marked\nordinary: ; @echo ordinary > ordinary.txt\nmarked: ; +@echo plus > plus.txt\n",
    )
    .unwrap();

    let stopped = make_command(&directory).arg("-q").output().unwrap();
    assert_eq!(stopped.status.code(), Some(1));
    assert!(
        !directory.join("plus.txt").exists(),
        "the walk carried on past the goal that had already answered"
    );

    let carried = make_command(&directory)
        .args(["-q", "-k"])
        .output()
        .unwrap();
    assert_eq!(carried.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(directory.join("plus.txt")).unwrap(),
        "plus\n",
        "-k did not carry the question past the goal that answered"
    );
    assert!(!directory.join("ordinary.txt").exists());
}

/// The complaint about text after an `ifeq` is owed to the branch the directive
/// is in, not to the file.
///
/// GNU Make's `EXTRATEXT` sits below `conditional_line`'s ignoring loop, so it
/// is only ever reached for a condition the read is actually looking at. The
/// effect is the same either way — `ifeq (a,a) junk` compares `a` against `a`
/// and builds — so nothing in the port corpus can hold this half; what moves is
/// whether Ronin says anything at all, and the only place it can be said is
/// here.
/// Where a diagnostic raised inside an expansion points.
///
/// GNU Make raises them at `*expanding_var` (expand.c), which is the location
/// of the VARIABLE being expanded rather than of the text inside it -- so a
/// `define`, the one binding whose text spans lines, names the `define` line
/// and not the body line the call is written on. Everything written on one line
/// gives the two the same answer, which is why this only shows up here.
///
/// The three controls matter as much as the cells: `$(error)` and `$(warning)`
/// are raised at `reading_file` instead, so they name where the expansion was
/// asked for; and a variable the command line defined has no location of its
/// own to install, so the reference's line stands.
///
/// The build-intent gate cannot see any of this -- both tools refuse and write
/// nothing, and the only observable is the line number.
#[test]
fn a_define_body_names_the_define() {
    let directory = test_directory("define-diagnostic-location");
    // (label, argv, makefile, the located prefix it must print)
    let cases: [(&str, &[&str], &str, &str); 9] = [
        (
            "a define reached by a plain reference",
            &[],
            "define D\n$(subst a)\nendef\n\nall: ; @echo $(D) > out\n",
            "Makefile:1: insufficient number of arguments (1) to function 'subst'.",
        ),
        (
            "a define reached by $(call), which GNU Make expands as $(D)",
            &[],
            "define D\n$(subst $(1))\nendef\n\nall: ; @echo $(call D,x) > out\n",
            "Makefile:1: insufficient number of arguments (1) to function 'subst'.",
        ),
        (
            "the define's own line, not the file's first",
            &[],
            "# pad\n# pad\ndefine D\n$(subst a)\nendef\nall: ; @echo $(D) > out\n",
            "Makefile:3: insufficient number of arguments (1) to function 'subst'.",
        ),
        (
            "the innermost define, which is the one being expanded",
            &[],
            "define OUTER\n$(INNER)\nendef\n\ndefine INNER\n$(subst a)\nendef\n\nall: ; @echo $(OUTER) > out\n",
            "Makefile:5: insufficient number of arguments (1) to function 'subst'.",
        ),
        (
            "a complaint raised from inside a function rather than about one",
            &[],
            "define D\n$(word 0,a b c)\nendef\n\nall: ; @echo $(D) > out\n",
            "Makefile:1: first argument to 'word' function must be greater than 0.",
        ),
        (
            "a one-line binding, where the binding and the call share a line",
            &[],
            "X = $(subst a)\nall: ; @echo $(X) > out\n",
            "Makefile:1: insufficient number of arguments (1) to function 'subst'.",
        ),
        (
            "a call written in the recipe, which is not a binding at all",
            &[],
            "# pad\nall: ; @echo $(subst a) > out\n",
            "Makefile:2: insufficient number of arguments (1) to function 'subst'.",
        ),
        (
            "$(error) names where the expansion was asked for, not the define",
            &[],
            "define D\n$(error boom)\nendef\n\nall: ; @echo $(D) > out\n",
            "Makefile:5: boom",
        ),
        (
            "a command-line binding has no location to install",
            &["X='$(subst a)'"],
            "# pad\n# pad\nall: ; @echo $(X) > out\n",
            "Makefile:3: insufficient number of arguments (1) to function 'subst'.",
        ),
    ];

    // Every case is measured before any of them is judged, so one run names
    // every cell that moved rather than only the first.
    let mut wrong = String::new();
    for (label, arguments, source, located) in cases {
        fs::write(directory.join("Makefile"), source).unwrap();
        let (succeeded, said) = merged_make(&directory, arguments);
        if succeeded || !said.contains(located) {
            let _ = writeln!(
                wrong,
                "  {label}\n    wanted {located:?}\n    got    {:?}",
                said.trim_end()
            );
        }
    }
    assert!(wrong.is_empty(), "located in the wrong place:\n{wrong}");
}

#[test]
fn an_ignored_conditional_draws_no_complaint() {
    let directory = test_directory("ignored-branch-quiet");
    fs::write(
        directory.join("Makefile"),
        "ifeq (x,y)\nifeq (a,a) junk\nendif\nendif\nall: ; @echo built > out\n",
    )
    .unwrap();
    let ignored = make_command(&directory).output().unwrap();
    let said = String::from_utf8_lossy(&ignored.stdout).into_owned()
        + &String::from_utf8_lossy(&ignored.stderr);
    assert_eq!(ignored.status.code(), Some(0), "{said}");
    assert!(
        !said.contains("extraneous text"),
        "a condition in a branch that was never taken was complained about: {said}"
    );

    // The same line where the read does look at it, so the quiet above is the
    // branch's doing rather than the complaint having been dropped.
    fs::write(
        directory.join("Makefile"),
        "ifeq (x,x)\nifeq (a,a) junk\nendif\nendif\nall: ; @echo built > out\n",
    )
    .unwrap();
    let read = make_command(&directory).output().unwrap();
    let said =
        String::from_utf8_lossy(&read.stdout).into_owned() + &String::from_utf8_lossy(&read.stderr);
    assert_eq!(read.status.code(), Some(0), "{said}");
    assert!(
        said.contains("extraneous text after 'ifeq' directive"),
        "a condition the read reached was not complained about: {said}"
    );
}

/// A recipe whose `$(MAKE)` composes is cut into segments, and the lines ahead
/// of the invocation are staged as an edge of their own. That edge needs a name
/// for the compilation to ask for it by, and the name is a handle rather than a
/// file: `.ronin_recipe_stage/N`, which nothing writes and nothing reads.
///
/// The build could not tell a handle from a file and did to it what it does to
/// every output — created the directory it appears to sit in. So a tree whose
/// recipe recursed was left with an empty `.ronin_recipe_stage/` in its build
/// root, a working name of Ronin's own in a directory a Makefile author owns.
/// GNU Make 4.4.1 leaves nothing there in any of these modes.
///
/// `-n` is the row that makes it more than untidiness: a mode whose promise is
/// that nothing reaches the disk was creating a directory.
#[test]
fn a_stage_proxy_makes_no_directory() {
    for switches in [&[][..], &["-n"][..], &["-t"][..], &["-q"][..], &["-B"][..]] {
        let directory = test_directory("recipe-stage-proxy");
        fs::write(
            directory.join("Makefile"),
            "pre.out:\n\t@echo before > pre.out\n\t@$(MAKE) -f sub.mk sub\n\t@echo after >> pre.out\n",
        )
        .unwrap();
        fs::write(directory.join("sub.mk"), "sub:\n\t@echo sub > sub.out\n").unwrap();

        let mut arguments = switches.to_vec();
        arguments.push("pre.out");
        let output = make_command(&directory).args(&arguments).output().unwrap();

        assert!(
            !directory.join(".ronin_recipe_stage").exists(),
            "a staged segment left its own name in the tree under {switches:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The build the row above is the tidiness of: a composed recursion still runs
/// all three lines and the child, so the name that stopped being a file did not
/// take the work with it.
#[test]
fn a_staged_segment_runs_the_recipe() {
    let directory = test_directory("recipe-stage-effects");
    fs::write(
        directory.join("Makefile"),
        "pre.out:\n\t@echo before > pre.out\n\t@$(MAKE) -f sub.mk sub\n\t@echo after >> pre.out\n",
    )
    .unwrap();
    fs::write(directory.join("sub.mk"), "sub:\n\t@echo sub > sub.out\n").unwrap();

    let output = make_command(&directory).arg("pre.out").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("pre.out")).unwrap(),
        "before\nafter\n"
    );
    assert_eq!(
        fs::read_to_string(directory.join("sub.out")).unwrap(),
        "sub\n"
    );
}

/// GNU Make turns `-n`, `-t` and `-q` off across the makefile update and back
/// on for the goals (`update_goal_chain`, main.c), because a Makefile it only
/// pretended to remake is one whose contents the read would then have to guess.
///
/// A Makefile whose own rule holds a composed `$(MAKE)` has that recipe cut
/// into segments, and the segments run at a compilation boundary rather than
/// inside the update — so they used to be built under the invocation's own
/// switches and were pretended where GNU Make executes. `gen.mk` was never
/// written, the child never ran, and the read carried on over the text that
/// was missing.
///
/// The corpus records the same three cases on their files; this one is here for
/// the exit status, which the corpus records only as success or failure and
/// which is the whole of what `-q` answers.
#[test]
fn a_composing_remake_is_not_pretended() {
    // (switches, exit status, whether `all` is touched)
    for (switches, status, touched) in [("-n", 0, false), ("-t", 0, true), ("-q", 1, false)] {
        let directory = test_directory("composing-remake-pretended");
        fs::create_dir(directory.join("sub")).unwrap();
        fs::write(
            directory.join("Makefile"),
            "all: ; @printf '%s\\n' '$(GENERATED)' > out\n\ninclude gen.mk\n\ngen.mk:\n\t@printf 'GENERATED := from-generated\\n' > $@\n\t@$(MAKE) -C sub marker\n",
        )
        .unwrap();
        fs::write(
            directory.join("sub").join("Makefile"),
            "marker: ; @printf 'child\\n' > marker\n",
        )
        .unwrap();

        let output = make_command(&directory)
            .args([switches, "all"])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(status), "{switches}: {said}");
        assert_eq!(
            fs::read_to_string(directory.join("gen.mk")).ok().as_deref(),
            Some("GENERATED := from-generated\n"),
            "{switches}: the makefile was pretended rather than remade: {said}"
        );
        assert!(
            directory.join("sub").join("marker").exists(),
            "{switches}: the child of the remaking recipe never ran: {said}"
        );
        assert!(
            !directory.join("out").exists(),
            "{switches}: the goal's own recipe ran: {said}"
        );
        assert_eq!(
            directory.join("all").exists(),
            touched,
            "{switches}: the goal's touch went the wrong way: {said}"
        );
    }
}

/// The other side of the split, which must not move with it: a segment of an
/// ORDINARY goal's recipe keeps the switches the command line gave. GNU Make
/// has no staging phase there, so `-n` describes the recipe and runs none of
/// it, and `-t` touches the goal rather than making it.
#[test]
fn a_goals_segment_keeps_the_switches() {
    // (switches, whether the child's target is made)
    for (switches, child) in [("-n", false), ("-t", true), ("-q", false)] {
        let directory = test_directory("goal-staged-segment");
        fs::create_dir(directory.join("sub")).unwrap();
        fs::write(
            directory.join("Makefile"),
            "goal:\n\t@printf 'before\\n' > pre.out\n\t@$(MAKE) -C sub marker\n\t@printf 'after\\n' >> pre.out\n",
        )
        .unwrap();
        fs::write(
            directory.join("sub").join("Makefile"),
            "marker: ; @printf 'child\\n' > marker\n",
        )
        .unwrap();

        let output = make_command(&directory)
            .args([switches, "goal"])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            !directory.join("pre.out").exists(),
            "{switches}: a goal's staged segment ran: {said}"
        );
        assert_eq!(
            directory.join("sub").join("marker").exists(),
            child,
            "{switches}: the child went the wrong way: {said}"
        );
    }
}

/// A Makefile made THROUGH a recursive prerequisite settles, and settles once.
///
/// GNU Make brings `helper` up to date, runs its child, writes `gen.mk`, starts
/// the read over once and builds the goal from the new text — one restart, the
/// same one it makes when a Makefile's own recipe remakes it.
///
/// Ronin cuts `helper`'s recipe into segments around its composed `$(MAKE)` and
/// finishes it across passes, and a recipe cut that way has to stop being called
/// begun once it is over. One that keeps saying it leaves `helper` dirty for the
/// whole invocation: `gen.mk` is remade on every pass, its stamp moves, the read
/// starts over, and the run ends at the hundred-try backstop with exit 2.
///
/// The corpus records the effects of the same shape on its files. Here for the
/// two things it cannot: `-q`'s answer is its exit STATUS, which the corpus
/// records only as success or failure, and the defect is a COUNT — the number of
/// times `gen.mk` is remade, which is what separates settling from looping.
#[test]
fn a_makefile_through_a_recursion_settles() {
    let directory = test_directory("makefile-through-a-recursive-prerequisite");
    fs::create_dir(directory.join("sub")).unwrap();
    let makefile = "all: ; @printf '%s\\n' '$(GENERATED)' > out\n\n\
                    include gen.mk\n\n\
                    gen.mk: helper\n\t@printf 'GENERATED := from-generated\\n' > $@\n\n\
                    helper:\n\t@printf 'h\\n' > helper\n\t@$(MAKE) -C sub marker\n";
    let child = "marker: ; @printf 'child\\n' > marker\n";
    fs::write(directory.join("Makefile"), makefile).unwrap();
    fs::write(directory.join("sub").join("Makefile"), child).unwrap();

    let output = make_command(&directory).arg("all").output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    let narrated = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(0), "{said}{narrated}");
    // The count is the whole point: a read that never settles remakes the
    // Makefile once per pass until the backstop stops it.
    assert_eq!(
        narrated.matches("> gen.mk").count(),
        1,
        "the Makefile was remade more than once: {narrated}"
    );
    assert_eq!(
        fs::read_to_string(directory.join("out")).unwrap(),
        "from-generated\n"
    );
    assert_eq!(
        fs::read_to_string(directory.join("sub").join("marker")).unwrap(),
        "child\n"
    );

    // `-q` over the same shape answers about the goal alone: GNU Make makes the
    // Makefile for real and then reports the goal out of date with 1. A run that
    // gave up at the backstop reports 2, which is a build that broke.
    let asked = test_directory("makefile-through-a-recursive-prerequisite-question");
    fs::create_dir(asked.join("sub")).unwrap();
    fs::write(asked.join("Makefile"), makefile).unwrap();
    fs::write(asked.join("sub").join("Makefile"), child).unwrap();
    let output = make_command(&asked).args(["-q", "all"]).output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert_eq!(
        fs::read_to_string(asked.join("gen.mk")).ok().as_deref(),
        Some("GENERATED := from-generated\n"),
        "the question pretended the makefile update: {said}"
    );
    assert!(!asked.join("out").exists(), "the goal's recipe ran: {said}");
}

/// A Makefile remade by a recipe whose `$(MAKE)` cannot compose starts a real
/// nested Make, and that child is not pretending either.
///
/// GNU Make recomputes `MAKEFLAGS` without `-n`, `-t` and `-q` for the length
/// of the makefile update and computes it again with them for the goals
/// (`define_makeflags`, main.c). The three carry `no_makefile` in its switch
/// table and nothing else does, so everything else — `-j`, `-k`, `-B`, the long
/// options, the `--` assignments — still reaches the child.
///
/// The corpus records this shape's effects on its files. Here for the exit
/// status, which the corpus keeps only as success or failure: a child handed
/// `-q` answers 1, the shell command carrying it exits 1, and the parent reads
/// the Makefile's own recipe as having FAILED — GNU's 1 is a question's answer
/// and that 2 is a build that broke.
#[test]
fn a_nested_remake_hides_the_switches() {
    // (switches, exit status)
    for (switches, status) in [("-n", 0), ("-t", 0), ("-q", 1)] {
        let directory = test_directory("nested-remake-switches");
        fs::create_dir(directory.join("sub")).unwrap();
        fs::write(
            directory.join("Makefile"),
            "all: ; @printf '%s\\n' '$(GENERATED)' > out\n\ninclude gen.mk\n\ngen.mk:\n\t@printf 'GENERATED := from-generated\\n' > $@; $(MAKE) --no-print-directory -C sub marker\n",
        )
        .unwrap();
        fs::write(
            directory.join("sub").join("Makefile"),
            "marker: ; @printf 'child\\n' > marker\n",
        )
        .unwrap();

        let output = make_command(&directory)
            .args([switches, "all"])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(status), "{switches}: {said}");
        assert_eq!(
            fs::read_to_string(directory.join("sub").join("marker"))
                .ok()
                .as_deref(),
            Some("child\n"),
            "{switches}: the child of the remaking recipe pretended: {said}"
        );
        assert!(
            !directory.join("out").exists(),
            "{switches}: the goal's own recipe ran: {said}"
        );
    }
}

/// The other side of the same split, which must not move with it: a `+` line in
/// a GOAL's recipe keeps the switches the command line gave, because GNU Make
/// has put them back by the time the goals are built.
#[test]
fn a_marked_goal_line_keeps_them() {
    // (switches, what the child was told)
    for (switches, told) in [("-t", "t"), ("-q", "q")] {
        let directory = test_directory("goal-marked-line-switches");
        fs::write(
            directory.join("Makefile"),
            "goal:\n\t+@printf '%s\\n' \"$$MAKEFLAGS\" > seen\n",
        )
        .unwrap();

        let output = make_command(&directory)
            .args([switches, "goal"])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(
            fs::read_to_string(directory.join("seen")).ok().as_deref(),
            Some(format!("{told}\n").as_str()),
            "{switches}: the goal's marked line was handed the update's flags: {said}"
        );
    }
}

/// A `SHELL` whose own value has to start a shell to expand is a recursion GNU
/// Make refuses: expanding `SHELL` reaches `$(shell)`, `$(shell)` has to know
/// what shell to start, asking that expands `SHELL`. GNU Make's guard is
/// `v->expanding` in `recursively_expand_for_file` (expand.c), set around the
/// whole expansion wherever it goes, so the second entry is caught however far
/// around the houses it came.
///
/// Here the guard was only on the path a `$(NAME)` in the text takes. Make's
/// own reads — `SHELL` before it starts a shell, `.SHELLFLAGS` beside it — went
/// through a second door with no mark on it, so the second entry was a descent
/// with no floor: `thread 'main' has overflowed its stack`, rc 134, a core
/// dump, and no diagnostic naming the makefile.
///
/// This case is the one that fails by crash without the fix, which is why it
/// asserts the status rather than only the words.
#[test]
fn a_recursive_shell_is_refused() {
    // Each row: the makefile, and the variable the refusal must name with the
    // line it must point at. GNU Make 4.4.1 answers every one of these
    // `Makefile:N: *** Recursive variable 'V' references itself (eventually).
    // Stop.` with rc 2.
    let rows: [(&str, &str, u32); 5] = [
        // The plain shape: expanding SHELL asks what shell to start.
        (
            "SHELL = $(shell echo /bin/sh)\nall:\n\techo one\n",
            "SHELL",
            1,
        ),
        // Around the houses. GNU Make names SHELL rather than the variable
        // standing between, because SHELL is the one re-entered, and points at
        // SHELL's own definition rather than at the reference.
        (
            "OTHER = $(shell echo /bin/sh)\nSHELL = $(OTHER)\nall:\n\techo one\n",
            "SHELL",
            2,
        ),
        // One target's own shell, which is read with that target's scope in
        // front of it and reaches the same guard.
        (
            "all: SHELL = $(shell echo /bin/sh)\nall:\n\techo one\n",
            "SHELL",
            1,
        ),
        // Reached while the makefile is read rather than while a recipe is:
        // the `$(shell)` on the second line asks what shell to start too.
        (
            "SHELL = $(shell echo /bin/sh)\nX := $(shell echo hi)\nall:\n\techo $(X)\n",
            "SHELL",
            1,
        ),
        // `.SHELLFLAGS` is the other half of what starting a shell needs, and
        // it is the same shape. GNU Make 4.4.1 does not survive this one --
        // it recurses until it segmentation faults, rc 139 -- so the row is
        // Ronin's own answer to a question the oracle cannot be asked.
        (
            ".SHELLFLAGS = $(shell echo -c)\nall:\n\techo one\n",
            ".SHELLFLAGS",
            1,
        ),
    ];

    for (makefile, named, line) in rows {
        let directory = test_directory("shell-needs-a-shell");
        fs::write(directory.join("Makefile"), makefile).unwrap();
        let refused = make_command(&directory).output().unwrap();
        let said = String::from_utf8_lossy(&refused.stdout).into_owned()
            + &String::from_utf8_lossy(&refused.stderr);
        assert_eq!(
            refused.status.code(),
            Some(2),
            "{makefile:?} did not refuse with a status: {said}"
        );
        assert!(
            said.contains(&format!(
                "Makefile:{line}: Recursive variable \"{named}\" references itself (eventually)."
            )),
            "{makefile:?} did not name {named} at line {line}: {said}"
        );
        assert!(
            !directory.join("one.txt").exists(),
            "{makefile:?} ran a recipe it was refused for"
        );
    }
}

/// The other side of the same read, and the reason it is done when a recipe
/// asks rather than before the walk starts.
///
/// GNU Make expands `$(SHELL)` in `construct_command_argv` (job.c), once per
/// recipe it is about to start, so a makefile with no recipe to run never asks
/// what the shell is -- and a `SHELL` that could not be expanded is one it runs
/// to completion. The boundary is the recipe as written rather than what it
/// expands to: `chop_commands` (commands.c) drops a blank line before anything
/// is expanded, while `all: ; $(EMPTY)` is a command line that survives to be
/// expanded and therefore does ask.
#[test]
fn a_shell_nothing_needs_is_unasked() {
    let unasked: [&str; 4] = [
        // An empty recipe: a target remade by doing nothing.
        "SHELL = $(shell echo /bin/sh)\nall: ;\n",
        // No recipe at all, and phony so the walk reaches it.
        "SHELL = $(shell echo /bin/sh)\n.PHONY: all\nall:\n",
        // A prerequisite with an empty recipe of its own.
        "SHELL = $(shell echo /bin/sh)\nall: dep\ndep: ;\n",
        // A recipe line of nothing but the prefix.
        "SHELL = $(shell echo /bin/sh)\nall:\n\t\n",
    ];
    for makefile in unasked {
        let directory = test_directory("shell-unasked");
        fs::write(directory.join("Makefile"), makefile).unwrap();
        let ran = make_command(&directory).output().unwrap();
        let said = String::from_utf8_lossy(&ran.stdout).into_owned()
            + &String::from_utf8_lossy(&ran.stderr);
        assert_eq!(
            ran.status.code(),
            Some(0),
            "{makefile:?} was refused for a shell nothing needed: {said}"
        );
        assert!(
            !said.contains("references itself"),
            "{makefile:?} was refused for a shell nothing needed: {said}"
        );
    }

    // The boundary. A recipe line that expands to nothing is still a command
    // line, so it does ask -- and GNU Make 4.4.1 refuses it.
    let directory = test_directory("shell-asked");
    fs::write(
        directory.join("Makefile"),
        "SHELL = $(shell echo /bin/sh)\nEMPTY =\nall: ; $(EMPTY)\n",
    )
    .unwrap();
    let refused = make_command(&directory).output().unwrap();
    let said = String::from_utf8_lossy(&refused.stdout).into_owned()
        + &String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{said}");
    assert!(said.contains("references itself"), "{said}");
}

/// What a reader is shown of a recipe holding `$?` is the names, not the name
/// the compiler carried them under.
///
/// `${KATI_NEW_INPUTS}` and its `_D` and `_F` neighbours are what kati writes
/// where the Makefile wrote `$?`, `$(?D)` and `$(?F)`: the list is not settled
/// until every prerequisite has been made, which is later than any expansion,
/// so the recipe carries a name and the build fills it in as the command
/// launches. A dry run launches nothing, and until this was gated it printed
/// the name — which is a working note, in the sense `.ronin_grouped_join/N`
/// was, rather than the command a run would execute.
///
/// Recorded from GNU Make 4.4.1, which prints
/// `echo "new=[p1 sub/p2] d=[. sub] f=[p1 p2]" > log.txt` for this makefile
/// under `-n`. The values are what is gated; the line around them is Ninja's.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn dry_run_narrates_new_input_names() {
    let directory = test_directory("dry-run-new-inputs");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: p1 sub/p2\n\t@echo \"new=[$?] d=[$(?D)] f=[$(?F)]\" > log.txt\n",
    )
    .unwrap();
    fs::write(directory.join("p1"), "").unwrap();
    fs::write(directory.join("sub").join("p2"), "").unwrap();

    let (succeeded, printed) = merged_make(&directory, &["-n"]);
    assert!(succeeded, "{printed}");
    assert!(
        printed.contains("new=[p1 sub/p2] d=[. sub] f=[p1 p2]"),
        "the dry run did not narrate the names GNU Make prints: {printed}"
    );
    assert!(
        !printed.contains("KATI_NEW_INPUTS"),
        "the dry run narrated the placeholder: {printed}"
    );
    assert!(
        !directory.join("log.txt").exists(),
        "the dry run wrote the recipe's target"
    );
}

/// The same names, in the line an ordinary build prints for the same recipe.
///
/// Not a second spelling of the case above but the other half of it: a build
/// with no `-n` narrates the recipe's own text where the dry run narrates the
/// command line, and kati writes the reference into the two differently — bare
/// in the text, escaped where the text is nested in a double-quoted `-c`
/// argument. A fix that filled in one spelling would leave the other showing
/// the placeholder to every reader of an ordinary build.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn a_build_narrates_new_input_names() {
    let directory = test_directory("build-new-inputs");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: p1 sub/p2\n\t@echo \"new=[$?] d=[$(?D)] f=[$(?F)]\" > log.txt\n",
    )
    .unwrap();
    fs::write(directory.join("p1"), "").unwrap();
    fs::write(directory.join("sub").join("p2"), "").unwrap();

    let (succeeded, printed) = merged_make(&directory, &[]);
    assert!(succeeded, "{printed}");
    assert!(
        printed.contains("new=[p1 sub/p2] d=[. sub] f=[p1 p2]"),
        "the build did not narrate the names its recipe ran with: {printed}"
    );
    assert!(
        !printed.contains("KATI_NEW_INPUTS"),
        "the build narrated the placeholder: {printed}"
    );
    assert_eq!(
        fs::read_to_string(directory.join("log.txt")).unwrap(),
        "new=[p1 sub/p2] d=[. sub] f=[p1 p2]\n",
        "the recipe ran with names other than the ones narrated"
    );
}

/// A prerequisite the directory search answered about carries a name of its
/// own until the build decides whether it is remade, and a dry run must show
/// the spelling the run settled on rather than that name.
///
/// `KATI_SETTLED_N` is the second family of reference the same substitution
/// fills in, and it reaches a reader through the same narration: a recipe read
/// early — this one is read early because `$?` made the edge deferred — holds
/// one for every prerequisite `VPATH` found.
///
/// Recorded from GNU Make 4.4.1, which prints `echo one >> out.o` and then
/// `echo "first=out.o all=out.o new=out.o" > log.txt` for this makefile under
/// `-n`: `keep` is newer than `src/out.o`, so the chain's target is remade and
/// every reference to it is the name as written.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn dry_run_narrates_a_settled_name() {
    let directory = test_directory("dry-run-settled-name");
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "VPATH = src\nall: out.o\n\t@echo \"first=$< all=$^ new=$?\" > log.txt\nout.o: keep\n\t@echo one >> $@\n",
    )
    .unwrap();
    write_at(&directory.join("src"), "out.o", "", 100);
    write_at(&directory, "keep", "", 200);

    let (succeeded, printed) = merged_make(&directory, &["-n"]);
    assert!(succeeded, "{printed}");
    assert!(
        printed.contains("first=out.o all=out.o new=out.o"),
        "the dry run did not narrate the settled name: {printed}"
    );
    assert!(
        !printed.contains("KATI_SETTLED_"),
        "the dry run narrated the settled-name placeholder: {printed}"
    );
}

/// An archive member reaches `$?` under the name the archive publishes it by,
/// and that name is the one a dry run shows.
///
/// The member's arm of the value is the one that answers off the front end's
/// published spelling rather than off the graph's node, so it is a third path
/// into the same substitution — and GNU Make's own SV 61436 shape, where the
/// recipe handed `$?` to `ar`, is where the placeholder reaching a shell was
/// first seen.
///
/// Recorded from GNU Make 4.4.1, which prints `echo "AR=ar Q=a.o b.o"` for
/// this makefile under `-n`.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn dry_run_narrates_archive_member_names() {
    let directory = test_directory("dry-run-archive-new-inputs");
    fs::write(
        directory.join("Makefile"),
        "mylib.a: mylib.a(a.o) mylib.a(b.o)\n\t@echo \"AR=$(AR) Q=$?\"\n(%): %\n\t$(AR) $(ARFLAGS) $@ $%\n%.o : %.c\n\t@echo Compile $<\n\t@touch $@\n",
    )
    .unwrap();
    fs::write(directory.join("a.c"), "int a;\n").unwrap();
    fs::write(directory.join("b.c"), "int b;\n").unwrap();

    let (succeeded, printed) = merged_make(&directory, &["-n"]);
    assert!(succeeded, "{printed}");
    assert!(
        printed.contains("AR=ar Q=a.o b.o"),
        "the dry run did not narrate the members' published names: {printed}"
    );
    assert!(
        !printed.contains("KATI_NEW_INPUTS"),
        "the dry run narrated the placeholder: {printed}"
    );
    assert!(
        !directory.join("mylib.a").exists(),
        "the dry run filed the archive"
    );
}

/// A composed child's recipe names its prerequisites the way the child's own
/// Makefile did, and the dry run that prints it says the same.
///
/// The value is spelt against the unit the recipe was read in, so this is the
/// cell that would catch a narration filled in from the parent's directory —
/// `sub/p1 sub/p2` where the recipe runs in `sub` and would find neither.
///
/// Recorded from GNU Make 4.4.1, which starts the child under `-n` and lets it
/// print `echo "new=[p1 p2]" > log.txt`.
// [spec:ronin:req:make.narration+1/test]
#[test]
fn a_child_narrates_its_own_spelling() {
    let directory = test_directory("dry-run-child-new-inputs");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(directory.join("Makefile"), "all:\n\t@$(MAKE) -C sub\n").unwrap();
    fs::write(
        directory.join("sub").join("Makefile"),
        "all: p1 p2\n\t@echo \"new=[$?]\" > log.txt\n",
    )
    .unwrap();
    fs::write(directory.join("sub").join("p1"), "").unwrap();
    fs::write(directory.join("sub").join("p2"), "").unwrap();

    let (succeeded, printed) = merged_make(&directory, &["-n"]);
    assert!(succeeded, "{printed}");
    assert!(
        printed.contains("new=[p1 p2]"),
        "the dry run did not narrate the child's own spelling: {printed}"
    );
    assert!(
        !printed.contains("KATI_NEW_INPUTS"),
        "the dry run narrated the placeholder: {printed}"
    );
}

/// `-q` answers about the goals without starting anything, so the exported
/// command-line value below is never read and its unterminated reference is
/// never a refusal.
///
/// Measured against reference/make-oracle/make-4.4.1/make on 2026-08-27:
/// `make -f M -q 'hello=$(world'` over this makefile exits 1 — "something to
/// do" — and says nothing. The status is what separates it from the refusal:
/// both 1 and 2 read as a failing run, and only 1 is an answer.
// [spec:ronin:req:make.exported-value-charged-to-the-job/test]
// [spec:ronin:req:make.question-status+1/test]
#[test]
fn a_question_reads_no_exported_value() {
    let directory = test_directory("question-no-environment");
    fs::write(directory.join("Makefile"), "all:; @echo ran > out\n").unwrap();

    let output = make_command(&directory)
        .args(["-q", "hello=$(world"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(!directory.join("out").exists());
}

/// The one line `-q` really starts is a line the makefile marked as running
/// anyway, and GNU Make reaches `target_environment` for it exactly as a build
/// would — so a value it cannot read refuses the question too.
///
/// Measured against 4.4.1 on 2026-08-27 over `all:; +@echo ran > out`: with a
/// readable value the line runs, `out` is written and the answer is 0; with
/// `hello=$(world` it is `make: *** unterminated variable reference.  Stop.`,
/// exit 2, and `out` is not written.
// [spec:ronin:req:make.exported-value-charged-to-the-job/test]
// [spec:ronin:req:make.question-status+1/test]
#[test]
fn a_question_refuses_the_line_it_runs() {
    let directory = test_directory("question-runs-a-plus-line");
    fs::write(directory.join("Makefile"), "all:; +@echo ran > out\n").unwrap();

    let refused = make_command(&directory)
        .args(["-q", "hello=$(world"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unterminated variable reference"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!directory.join("out").exists());

    let answered = make_command(&directory)
        .args(["-q", "hello=world"])
        .output()
        .unwrap();
    assert_eq!(answered.status.code(), Some(0), "{answered:?}");
    assert!(directory.join("out").exists());
}

/// And the same value read by a job that really starts refuses that job, with
/// the location GNU Make gives it — which for a value the command line supplied
/// is where the read had got to rather than a definition site, because there is
/// no file behind it.
///
/// Measured against 4.4.1 on 2026-08-27: `make: *** unterminated variable
/// reference.  Stop.`, exit 2, `out` not written.
// [spec:ronin:req:make.exported-value-charged-to-the-job/test]
#[test]
fn a_job_refuses_a_value_it_cannot_read() {
    let directory = test_directory("job-refuses-value");
    fs::write(directory.join("Makefile"), "all:; @echo ran > out\n").unwrap();

    let output = make_command(&directory)
        .args(["hello=$(world"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unterminated variable reference"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!directory.join("out").exists());
}

/// An exported value a MAKEFILE defined and that will not expand is reported
/// against the line that defined it, not against wherever the read had reached
/// when the environment was settled.
///
/// GNU Make's `recursively_expand_for_file` (variable.c) sets `expanding_var`
/// to the variable's own `fileinfo` before expanding, and
/// `variable_expand_string` dies at that location. Measured against 4.4.1 on
/// 2026-08-27 over this makefile: `M:2: *** unterminated variable reference.
/// Stop.` — the `export` line, with two lines of makefile behind it.
// [spec:ronin:req:make.exported-value-charged-to-the-job/test]
#[test]
fn an_unreadable_export_names_where_it_was_written() {
    let directory = test_directory("export-names-its-line");
    fs::write(
        directory.join("Makefile"),
        "# a comment\nexport FOO = $(bar\nall:; @echo ran > out\n",
    )
    .unwrap();

    let output = make_command(&directory).output().unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim_end(),
        "ronin: Makefile:2: unterminated variable reference."
    );
    assert!(!directory.join("out").exists());
}

/// A recipe line that comes to exactly `:` starts no process, so the makefile
/// below runs no shell at all and the exported value nothing else reads is
/// never read.
///
/// `start_job_command` (job.c): "Optimize an empty command. People use this for
/// timestamp rules, so avoid forking a useless shell." The line is still
/// counted as started, so the goal is remade rather than reported as having
/// nothing to do — which is the half a plain `LateBinding::Nothing` would get
/// wrong. Measured against 4.4.1 on 2026-08-27: silent, exit 0, and with
/// `hello=$(world` on the command line, still silent and still 0.
// [spec:ronin:req:make.exported-value-charged-to-the-job/test]
#[test]
fn an_empty_command_starts_no_process() {
    let directory = test_directory("empty-command");
    fs::write(directory.join("Makefile"), "all: ; @:\n").unwrap();

    for arguments in [&[][..], &["hello=$(world"][..]] {
        let output = make_command(&directory).args(arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{arguments:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}: {output:?}");
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("no work to do"),
            "{arguments:?}: the target was remade, not skipped: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
