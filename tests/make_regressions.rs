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
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
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
        br#"all: one.o two.o
.SUFFIXES: .c .o
.c.o:
	@printf '%s\n' '$@' > .deps/$*.Tpo
	@mv -f .deps/$*.Tpo .deps/$*.Po
	@cp $< $@
"#,
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
