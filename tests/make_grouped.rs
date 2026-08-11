#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

struct Scratch {
    directory: tempfile::TempDir,
    make: PathBuf,
}

impl Scratch {
    fn new(makefile: &str) -> Self {
        let directory = tempfile::tempdir().expect("create grouped-action scratch directory");
        fs::write(directory.path().join("Makefile"), makefile).expect("write Makefile");
        let make = directory.path().join("make");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &make)
            .expect("create make-named Ronin link");
        Self { directory, make }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(&self.make)
            .args(arguments)
            .current_dir(self.directory.path())
            .env("LC_ALL", "C")
            .env_remove("MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .output()
            .expect("run Ronin in Make mode")
    }
}

fn write_at(path: &Path, seconds: u64) {
    fs::write(path, []).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap_or_else(|error| panic!("set mtime for {}: {error}", path.display()));
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "Make-mode build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn grouped_double_uses_published_input_mtime() {
    let makefile = r#".PHONY: force

a b &:: p
	@printf 'group ?=%s\n' "$?" >> actions
	@touch a b

p: force
	@touch -d @VALUE p
"#;

    for (published, expected) in [(100, None), (500, Some("group ?=p\n"))] {
        let scratch = Scratch::new(&makefile.replace("VALUE", &published.to_string()));
        write_at(&scratch.path("a"), 300);
        write_at(&scratch.path("b"), 300);
        write_at(&scratch.path("p"), 200);

        assert_success(&scratch.run(&["--no-print-directory", "a"]));
        let observed = fs::read_to_string(scratch.path("actions")).ok();
        assert_eq!(observed.as_deref(), expected);
    }
}

#[test]
fn order_only_change_keeps_leaf_snapshot() {
    let scratch = Scratch::new(
        r".PHONY: prep

a b &:: p | prep
	@printf 'group\n' >> actions
	@touch a b

prep:
	@touch -d @500 p
",
    );
    write_at(&scratch.path("a"), 300);
    write_at(&scratch.path("b"), 300);
    write_at(&scratch.path("p"), 200);

    assert_success(&scratch.run(&["--no-print-directory", "a"]));
    assert!(!scratch.path("actions").exists());
    assert_eq!(
        fs::metadata(scratch.path("p"))
            .expect("p remains present")
            .modified()
            .expect("read p mtime"),
        SystemTime::UNIX_EPOCH + Duration::from_secs(500)
    );
}

#[test]
fn completion_join_prunes_unchanged_member() {
    let scratch = Scratch::new(
        r".PHONY: force

consumer: a
	@printf 'consumer\n' >> actions
	@touch consumer

a b &:: p
	@printf 'group\n' >> actions

p: force
	@touch -d @500 p
",
    );

    // The first build records the stable command hashes. The grouped recipe
    // deliberately leaves its members absent, so the consumer is reached too.
    assert_success(&scratch.run(&["--no-print-directory", "consumer"]));
    write_at(&scratch.path("a"), 300);
    write_at(&scratch.path("b"), 300);
    write_at(&scratch.path("p"), 200);
    write_at(&scratch.path("consumer"), 1_000);
    fs::remove_file(scratch.path("actions")).expect("clear first-build action log");

    assert_success(&scratch.run(&["--no-print-directory", "consumer"]));
    assert_eq!(
        fs::read_to_string(scratch.path("actions")).expect("group action log"),
        "group\n"
    );
}

#[test]
fn private_action_persists_without_stamp() {
    let scratch = Scratch::new(
        r#"a b &:: p
	@printf 'group @=%s ?=%s\n' '$@' "$?" >> actions
	@touch a b
"#,
    );
    write_at(&scratch.path("p"), 100);

    assert_success(&scratch.run(&["--no-print-directory", "a"]));
    assert_success(&scratch.run(&["--no-print-directory", "a"]));
    assert_eq!(
        fs::read_to_string(scratch.path("actions")).expect("group action log"),
        "group @=a ?=p\n"
    );
    assert!(!scratch.path(".ronin_grouped_double").exists());
    assert!(!scratch.path(".ronin_grouped_join").exists());
}

#[test]
fn question_mode_ignores_clean_candidate() {
    let scratch = Scratch::new(
        r"a b &:: p
	@touch a b
",
    );
    write_at(&scratch.path("a"), 300);
    write_at(&scratch.path("b"), 300);
    write_at(&scratch.path("p"), 200);
    assert_eq!(
        scratch
            .run(&["--no-print-directory", "-q", "a"])
            .status
            .code(),
        Some(0)
    );

    write_at(&scratch.path("p"), 500);
    assert_eq!(
        scratch
            .run(&["--no-print-directory", "-q", "a"])
            .status
            .code(),
        Some(1)
    );

    let candidate = Scratch::new(
        r".PHONY: prep
a b &:: p | prep
	@touch a b
",
    );
    write_at(&candidate.path("a"), 300);
    write_at(&candidate.path("b"), 300);
    write_at(&candidate.path("p"), 200);
    assert_eq!(
        candidate
            .run(&["--no-print-directory", "-q", "a"])
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn recursive_group_activates_same_member_child() {
    let scratch = Scratch::new(
        r"a b &:: p
	@$(MAKE) --no-print-directory -f Child.mk a
",
    );
    fs::write(
        scratch.path("Child.mk"),
        r"a: q
	@printf 'child\n' >> actions
	@touch a b
",
    )
    .expect("write recursive child Makefile");
    write_at(&scratch.path("a"), 300);
    write_at(&scratch.path("b"), 300);
    write_at(&scratch.path("p"), 200);
    write_at(&scratch.path("q"), 400);

    assert_success(&scratch.run(&["--no-print-directory", "a"]));
    assert!(!scratch.path("actions").exists());

    write_at(&scratch.path("p"), 500);
    assert_success(&scratch.run(&["--no-print-directory", "a"]));
    assert_eq!(
        fs::read_to_string(scratch.path("actions")).expect("recursive action log"),
        "child\n"
    );

    assert_success(&scratch.run(&["--no-print-directory", "a"]));
    assert_eq!(
        fs::read_to_string(scratch.path("actions")).expect("stable recursive action log"),
        "child\n"
    );
}

#[test]
fn mixed_double_colon_keeps_member_actions() {
    let scratch = Scratch::new(
        r"all: a b

a b :: p
	@printf 'ordinary-%s\n' '$@' >> actions
	@touch '$@'

a b &:: q
	@printf 'grouped-%s\n' '$@' >> actions
	@touch a

p q:
	@touch '$@'
",
    );

    assert_success(&scratch.run(&["--no-builtin-rules", "--no-print-directory", "all"]));
    assert_eq!(
        fs::read_to_string(scratch.path("actions")).expect("mixed action log"),
        "ordinary-a\ngrouped-a\nordinary-b\n"
    );
}

#[test]
fn grouped_second_expands_each_member() {
    let scratch = Scratch::new(
        r".SECONDEXPANSION:
all: 15.x 1.x
15.x: 5.z 6.z 5.z | 7.z 7.z 8.z
1.x: 1.z 2.z 2.z | 3.z 4.z
15.x 1.x &: 9.z $$(info @=$$@,<=$$<,^=$$^,+=$$+,|=$$|) ;
%.z: ;
",
    );

    let output = scratch.run(&["--no-builtin-rules", "--no-print-directory", "all"]);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("@=15.x,<=5.z,^=5.z 6.z,+=5.z 6.z 5.z,|=7.z 8.z"));
    assert!(stdout.contains("@=1.x,<=1.z,^=1.z 2.z,+=1.z 2.z 2.z,|=3.z 4.z"));
}
