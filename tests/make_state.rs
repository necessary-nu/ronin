//! Where a Make-mode build keeps what it remembers.
//!
//! The claim is about a directory listing, so these tests read directories.
//! Each one runs the real executable under the name `make`, which is what
//! selects the Make front end, and points `RONIN_STATE_HOME` at a scratch
//! directory so that the state a test writes is a test's to inspect and to
//! throw away rather than something left in whoever ran it.

#![cfg(all(unix, feature = "make"))]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A Makefile that compiles a source file including a header the Makefile
/// itself never names.
///
/// The header is the interesting part: nothing in the graph says `main.o`
/// depends on it, so only the dependency log the compiler's `-MF` output fed
/// can know, and only if that log survived to the next build.
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
    /// A scratch root holding a state home, and a `make`-named link to Ronin.
    ///
    /// The name is how a multi-call binary is installed and how the front end
    /// is chosen, so it is how this is tested.
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("make");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &program).unwrap();
        Self { root, program }
    }

    fn state_home(&self) -> PathBuf {
        self.root.path().join("state")
    }

    /// A tree holding [`MAKEFILE`], a source, and the header it includes.
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
            .env("RONIN_STATE_HOME", self.state_home())
            .env_remove("MAKEFLAGS")
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

    /// The entries the state home holds, by name.
    fn entries(&self) -> BTreeSet<String> {
        fs::read_dir(self.state_home().join("make"))
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
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

// [spec:ronin:req:make.state-outside-the-tree/test]
#[test]
fn a_built_tree_holds_only_what_the_makefile_put_there() {
    let fixture = Fixture::new();
    let tree = fixture.tree("project");
    let sources = named(&["Makefile", "main.c", "hdr.h"]);
    assert_eq!(listing(&tree), sources);

    fixture.build_in(&tree, &[]);

    // The recipes' own outputs and nothing else: no .ninja_log, no
    // .ninja_deps, and no manifest either. `main.o.d` is gone because the
    // build consumed the depfile, which is what Ninja does with one.
    let mut built = sources;
    built.extend(named(&["app", "main.o"]));
    assert_eq!(listing(&tree), built);
    // It went somewhere, though: discarding it would have been the other
    // reading of compatibility and a much worse one.
    let entry = fixture.state_home().join("make").join(
        fixture
            .entries()
            .into_iter()
            .next()
            .expect("the build recorded itself somewhere"),
    );
    assert!(entry.join(".ninja_log").exists());
    assert!(entry.join(".ninja_deps").exists());
}

// [spec:ronin:req:make.state-outside-the-tree/test]
#[test]
fn relocated_state_still_answers_the_two_questions_make_cannot() {
    let fixture = Fixture::new();
    let tree = fixture.tree("project");
    assert!(fixture.build_in(&tree, &[]).contains("main.o"));

    // Nothing changed, so there is nothing to do: the build log survived the
    // end of the first invocation and was found again at the start of this
    // one.
    assert!(fixture.build_in(&tree, &[]).contains("no work to do"));

    // A header the Makefile never mentions. Only the dependency log the last
    // build wrote knows `main.o` reads it, so a rebuild here is that log
    // having survived, and nothing else could have caused one.
    fs::write(tree.join("hdr.h"), "#define V 0\n/* edited */\n").unwrap();
    let after_header = fixture.build_in(&tree, &[]);
    assert!(after_header.contains("main.o"), "{after_header}");
    assert!(fixture.build_in(&tree, &[]).contains("no work to do"));

    // A changed recipe with untouched inputs. Only the command hash the build
    // log recorded can notice this, and GNU Make never does.
    fs::write(
        tree.join("Makefile"),
        MAKEFILE.replace("cc -o app main.o", "cc -o app main.o && true"),
    )
    .unwrap();
    let after_command = fixture.build_in(&tree, &[]);
    assert!(
        after_command.contains("cc -o app main.o"),
        "{after_command}"
    );
    // The compile recipe, not the word `main.o` — the link recipe names that
    // file too, and now that Make mode echoes recipes rather than describing
    // edges, the only way to say "main.o was not rebuilt" is to name the
    // recipe that would have rebuilt it.
    assert!(!after_command.contains("-c main.c"), "{after_command}");
    assert!(fixture.build_in(&tree, &[]).contains("no work to do"));
}

// [spec:ronin:req:make.state-outside-the-tree/test]
#[test]
fn two_checkouts_of_one_project_do_not_share_an_entry() {
    let fixture = Fixture::new();
    let first = fixture.tree("checkout-one");
    let second = fixture.tree("checkout-two");

    fixture.build_in(&first, &[]);
    fixture.build_in(&second, &[]);

    assert_eq!(fixture.entries().len(), 2);
    // Each entry names the tree it belongs to, which is how a person finds the
    // one to remove — `make clean` cannot, because a Makefile's clean rule has
    // never heard of it.
    let claimed = fixture
        .entries()
        .iter()
        .map(|entry| {
            let marker = fixture.state_home().join("make").join(entry).join("tree");
            fs::read_to_string(marker)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        claimed,
        named(&[first.to_str().unwrap(), second.to_str().unwrap()])
    );

    // Removing an entry is the whole of clearing it: the next build is the
    // first build again.
    fs::remove_dir_all(fixture.state_home().join("make")).unwrap();
    fs::remove_file(first.join("app")).unwrap();
    assert!(fixture.build_in(&first, &[]).contains("main.o"));
}

// [spec:ronin:req:make.state-outside-the-tree/test]
#[test]
fn a_makefile_read_from_elsewhere_leaves_neither_directory_marked() {
    let fixture = Fixture::new();
    // -f does not move the process, so the tree a build runs in is the working
    // directory and the relative paths in the graph are relative to it. That
    // is what the state has to be keyed on, and the directory the Makefile
    // happens to sit in is not it.
    let elsewhere = fixture.tree("elsewhere");
    let work = fixture.root.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("main.c"), "int main(void){return 0;}\n").unwrap();

    fixture.build_in(&work, &["-f", "../elsewhere/Makefile"]);

    assert_eq!(listing(&work), named(&["main.c", "app", "main.o"]));
    assert_eq!(listing(&elsewhere), named(&["Makefile", "main.c", "hdr.h"]));
    let claimed = fixture
        .entries()
        .into_iter()
        .map(|entry| {
            fs::read_to_string(fixture.state_home().join("make").join(entry).join("tree")).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert!(
        claimed[0].starts_with(work.to_str().unwrap()),
        "{claimed:?}"
    );
    assert!(fixture
        .build_in(&work, &["-f", "../elsewhere/Makefile"])
        .contains("no work to do"));
}

/// The goals the concurrency test builds, one per racing invocation.
const GOALS: [&str; 8] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight",
];

// [spec:ronin:req:make.state-outside-the-tree/test]
#[test]
fn concurrent_builds_of_one_tree_share_one_log_without_losing_a_record() {
    let fixture = Fixture::new();
    let tree = fixture.root.path().join("contended");
    fs::create_dir_all(&tree).unwrap();
    // One target per racing build, so what they contend over is the shared log
    // rather than one output file — two commands writing one path is a race
    // GNU Make and Ninja lose too, and it is not what this is about.
    fs::write(
        tree.join("Makefile"),
        format!(
            "all: {goals}\n{goals}:\n\tprintf %s $@ > $@\n.PHONY: all\n",
            goals = GOALS.join(" ")
        ),
    )
    .unwrap();
    // The first build alone, so that the racing ones all find a log with a
    // header and append to it rather than each creating one over the others.
    fixture.build_in(&tree, &[GOALS[0]]);

    // Nothing locks it. The Ninja path has always relied on appending whole
    // records to a file opened for append, and on skipping what it cannot
    // parse when it reads one back; relocating the file keeps that code rather
    // than adding a guarantee to Make mode that Ninja mode would not have.
    let racing = GOALS
        .iter()
        .map(|goal| {
            Command::new(&fixture.program)
                .current_dir(&tree)
                .arg(goal)
                .env("RONIN_STATE_HOME", fixture.state_home())
                .env_remove("MAKEFLAGS")
                .env_remove("MAKELEVEL")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    for mut build in racing {
        assert!(build.wait().unwrap().success());
    }

    // One tree, one entry, however many builds were in it at once.
    assert_eq!(fixture.entries().len(), 1);
    // Every record survived the interleaving: a build asking for all of them
    // finds all of them recorded, which it could not if one append had landed
    // inside another.
    let settled = fixture.build_in(&tree, &GOALS);
    assert!(settled.contains("no work to do"), "{settled}");
}
