//! Make-compiled graphs use Ninja's persistence without a frontend exception.
//!
//! Each test runs the real executable under the name `make`, which selects the
//! Make compiler, then inspects the same `.ninja_log` and `.ninja_deps` that a
//! manifest graph would use in the build directory.

#![cfg(all(unix, feature = "make"))]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A Makefile whose compiler-discovered header can only survive through the
/// Ninja dependency log.
const MAKEFILE: &str = "\
all: app\n\
app: main.o\n\
\tcc -o app main.o\n\
main.o: main.c\n\
\tcc -MMD -MF main.o.d -c main.c -o main.o\n\
main.o: .KATI_DEPFILE := main.o.d\n\
.PHONY: all\n";

struct Fixture {
    root: tempfile::TempDir,
    program: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("make");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &program).unwrap();
        Self { root, program }
    }

    fn tree(&self, name: &str) -> PathBuf {
        let tree = self.root.path().join(name);
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("Makefile"), MAKEFILE).unwrap();
        fs::write(
            tree.join("main.c"),
            "#include \"hdr.h\"\nint main(void){return V;}\n",
        )
        .unwrap();
        fs::write(tree.join("hdr.h"), "#define V 0\n").unwrap();
        tree
    }

    fn build_in(&self, directory: &Path, arguments: &[&str]) -> String {
        let output = Command::new(&self.program)
            .current_dir(directory)
            .args(arguments)
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

fn listing(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn named(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

// [spec:ronin:req:make.state-outside-the-tree+1/test]
// [spec:ronin:req:make.compiler-boundary/test]
#[test]
fn make_build_uses_ninja_state() {
    let fixture = Fixture::new();
    let tree = fixture.tree("project");
    let sources = named(&["Makefile", "main.c", "hdr.h"]);
    assert_eq!(listing(&tree), sources);

    fixture.build_in(&tree, &[]);

    let mut built = sources;
    built.extend(named(&[".ninja_deps", ".ninja_log", "app", "main.o"]));
    assert_eq!(listing(&tree), built);
}

// [spec:ronin:req:make.state-outside-the-tree+1/test]
#[test]
fn state_preserves_deps_and_hashes() {
    let fixture = Fixture::new();
    let tree = fixture.tree("project");
    assert!(fixture.build_in(&tree, &[]).contains("main.o"));
    assert!(fixture.build_in(&tree, &[]).contains("no work to do"));

    fs::write(tree.join("hdr.h"), "#define V 0\n/* edited */\n").unwrap();
    let after_header = fixture.build_in(&tree, &[]);
    assert!(after_header.contains("main.o"), "{after_header}");
    assert!(fixture.build_in(&tree, &[]).contains("no work to do"));

    fs::write(
        tree.join("Makefile"),
        MAKEFILE.replace("cc -o app main.o", "cc -o app main.o && true"),
    )
    .unwrap();
    let after_command = fixture.build_in(&tree, &[]);
    assert!(after_command.contains("app"), "{after_command}");
    assert!(!after_command.contains("main.o"), "{after_command}");
    assert!(fixture.build_in(&tree, &[]).contains("no work to do"));
}

// [spec:ronin:req:make.state-outside-the-tree+1/test]
#[test]
fn state_follows_working_directory() {
    let fixture = Fixture::new();
    let elsewhere = fixture.tree("elsewhere");
    let work = fixture.root.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("main.c"), "int main(void){return 0;}\n").unwrap();

    fixture.build_in(&work, &["-f", "../elsewhere/Makefile"]);

    assert_eq!(
        listing(&work),
        named(&[".ninja_deps", ".ninja_log", "main.c", "app", "main.o"])
    );
    assert_eq!(listing(&elsewhere), named(&["Makefile", "main.c", "hdr.h"]));
    assert!(
        fixture
            .build_in(&work, &["-f", "../elsewhere/Makefile"])
            .contains("no work to do")
    );
}

// [spec:ronin:req:make.state-outside-the-tree+1/test]
// [spec:ronin:req:make.compiler-boundary/test]
#[test]
fn equivalent_frontends_execute_identically() {
    let fixture = Fixture::new();
    let make_tree = fixture.root.path().join("make-graph");
    let ninja_tree = fixture.root.path().join("ninja-graph");
    fs::create_dir_all(&make_tree).unwrap();
    fs::create_dir_all(&ninja_tree).unwrap();
    fs::write(
        make_tree.join("Makefile"),
        "out: in\n\tprintf rebuilt > out\n",
    )
    .unwrap();
    fs::write(
        ninja_tree.join("build.ninja"),
        "rule rebuild\n  command = printf rebuilt > out\nbuild out: rebuild in\ndefault out\n",
    )
    .unwrap();
    for tree in [&make_tree, &ninja_tree] {
        fs::write(tree.join("in"), "source\n").unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    for tree in [&make_tree, &ninja_tree] {
        fs::write(tree.join("out"), "preexisting\n").unwrap();
    }

    fixture.build_in(&make_tree, &[]);
    let run_ninja = || {
        let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
            .current_dir(&ninja_tree)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    run_ninja();

    assert_eq!(
        fs::read(make_tree.join("out")).unwrap(),
        fs::read(ninja_tree.join("out")).unwrap()
    );
    assert!(make_tree.join(".ninja_log").exists());
    assert!(ninja_tree.join(".ninja_log").exists());
    assert!(fixture.build_in(&make_tree, &[]).contains("no work to do"));
    assert!(run_ninja().contains("no work to do"));
}

const GOALS: [&str; 8] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight",
];

// [spec:ronin:req:make.state-outside-the-tree+1/test]
#[test]
fn concurrent_builds_share_ninja_log() {
    let fixture = Fixture::new();
    let tree = fixture.root.path().join("contended");
    fs::create_dir_all(&tree).unwrap();
    fs::write(
        tree.join("Makefile"),
        format!(
            "all: {goals}\n{goals}:\n\tprintf %s $@ > $@\n.PHONY: all\n",
            goals = GOALS.join(" ")
        ),
    )
    .unwrap();
    fixture.build_in(&tree, &[GOALS[0]]);

    let racing = GOALS
        .iter()
        .map(|goal| {
            Command::new(&fixture.program)
                .current_dir(&tree)
                .arg(goal)
                .env_remove("MAKEFLAGS")
                .env_remove("MFLAGS")
                .env_remove("CARGO_MAKEFLAGS")
                .env_remove("MAKELEVEL")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    for mut build in racing {
        assert!(build.wait().unwrap().success());
    }

    assert!(tree.join(".ninja_log").exists());
    let settled = fixture.build_in(&tree, &GOALS);
    assert!(settled.contains("no work to do"), "{settled}");
}
