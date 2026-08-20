#![cfg(all(unix, feature = "make"))]

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

fn test_directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ronin-make-regression-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
}

/// Findutils links one generated manual fragment to another. A later recursive
/// group updates the referent, making the followed link exactly as new as its
/// prerequisite while Ronin's build log still remembers the link's old mtime.
/// GNU Make uses the equal filesystem timestamps and leaves the link alone.
// [spec:ronin:req:make.state-outside-the-tree+2/test]
#[test]
fn symlink_freshness_ignores_old_build_log() {
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

    // Newer than the first build's wall-clock log record. The existing link
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
        fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
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
    fs::remove_dir_all(directory).unwrap();
}
