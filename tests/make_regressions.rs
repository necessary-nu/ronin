#![cfg(all(unix, feature = "make"))]

use std::fs;
use std::io::Write as _;
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
// [spec:ronin:req:make.recursive-invocation+1/test]
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
// [spec:ronin:req:make.recursive-invocation+1/test]
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
// [spec:ronin:req:make.recursive-invocation+1/test]
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
