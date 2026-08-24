//! Ronin invoked as the shell.
//!
//! The executable answers to `sh` the way it answers to `make`: by the name it
//! was invoked under and by nothing else. What it must answer *as* is the
//! shell a build resolved — Debian's `/bin/sh` is dash, and these are the
//! behaviours a recipe leans on, checked against it where the host's own shell
//! is the same one.
//!
//! Nothing here sends a signal to anything but the shell's own process. The
//! whole point of a shell is that it owns process groups, and a test that
//! reaches outside its own is a test that can reach the session.
#![cfg(unix)]

use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Link the executable under the name that selects the shell.
///
/// A symlink is how a multi-call binary is installed, so it is how this is
/// tested.
fn shell_named(directory: &Path, name: &str) -> PathBuf {
    let link = directory.join(name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
    link
}

/// Run the shell with `argv[0]` spelled as `spelling`, which is what a
/// substitution does and what a diagnostic reports.
fn shell(spelling: &str, arguments: &[&str]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "sh");
    Command::new(&link)
        .arg0(spelling)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The host's `/bin/sh`, when it is the shell this one is a port of.
///
/// Everywhere else the comparison would be against a different shell and its
/// disagreements would say nothing.
fn dash_on_this_host() -> Option<PathBuf> {
    let resolved = std::fs::canonicalize("/bin/sh").ok()?;
    (resolved.file_name()? == "dash").then_some(resolved)
}

// [spec:ronin:req:product.shell-identity/test]
#[test]
fn the_invoked_name_selects_the_shell() {
    let output = shell("sh", &["-c", "echo hello"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "hello\n");
}

/// The name is the whole answer here too: reached by any other name, the same
/// arguments are a build's and the shell is not in the way.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn another_name_is_not_the_shell() {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "ronin");
    let output = Command::new(&link)
        .args(["-c", "echo hello"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a build read `-c echo hello` as a shell would: {}",
        stdout(&output)
    );
    assert_ne!(stdout(&output), "hello\n");
}

/// `argv[0]` is reported as written, which is what lets a substituted shell
/// produce the bytes the shell it replaced would have produced.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn argv0_is_reported_as_written() {
    let output = shell("/bin/sh", &["-c", "echo $0"]);
    assert_eq!(stdout(&output), "/bin/sh\n");

    let output = shell("/bin/sh", &["-c", "nosuchcommandanywhere"]);
    assert_eq!(output.status.code(), Some(127));
    assert_eq!(
        stderr(&output),
        "/bin/sh: 1: nosuchcommandanywhere: not found\n"
    );

    // Spelled another way, the shell says that other way — an imitation of one
    // fixed shell would not.
    let output = shell("/usr/bin/sh", &["-c", "echo $0"]);
    assert_eq!(stdout(&output), "/usr/bin/sh\n");
}

// [spec:ronin:req:product.shell-identity/test]
#[test]
fn a_pipeline_reports_its_last_command() {
    let output = shell("sh", &["-c", "printf 'a\\nb\\n' | grep b"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "b\n");

    let output = shell("sh", &["-c", "true | false"]);
    assert_eq!(output.status.code(), Some(1));
}

/// The two flag shapes a build ever hands a shell, and the difference between
/// them is the whole of `.POSIX:`.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn a_builds_flags_mean_dashs_flags() {
    let output = shell("sh", &["-c", "false; echo reached"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "reached\n");

    let output = shell("sh", &["-ec", "false; echo reached"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");

    let output = shell("sh", &["-c", "exit 7"]);
    assert_eq!(output.status.code(), Some(7));
}

/// The response-file shape: past a size a command line will not carry, a
/// recipe reaches the shell as a file rather than as an argument.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn a_script_file_runs_as_one() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("recipe.rsp");
    let mut file = std::fs::File::create(&script).unwrap();
    file.write_all(b"echo from-a-file\nexit 3\n").unwrap();
    drop(file);

    let link = shell_named(directory.path(), "sh");
    let output = Command::new(&link)
        .arg0("/bin/sh")
        .arg(&script)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stdout(&output), "from-a-file\n");
}

/// A recipe that kills itself is killed, rather than reported dead by a
/// launcher that outlived it. Nothing outside this shell's own process is
/// signalled.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn an_interrupted_shell_leaves_by_signal() {
    use std::os::unix::process::ExitStatusExt as _;

    let output = shell("sh", &["-c", "kill -INT $$"]);
    assert_eq!(output.status.signal(), Some(libc_sigint()));
    assert_eq!(output.status.code(), None);
}

/// SIGINT's number, spelled without reaching for a crate this test does not
/// otherwise need.
const fn libc_sigint() -> i32 {
    2
}

/// SIGPIPE's default disposition is inherited, not Rust's `SIG_IGN`. A recipe
/// whose reader leaves early is the case: it has to die on the signal rather
/// than run to completion writing into a closed pipe.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn the_inherited_process_is_presented() {
    let output = shell(
        "sh",
        &[
            "-c",
            "sh -c 'trap \"\" PIPE; yes 2>/dev/null | head -1 >/dev/null'; \
             printf %s ${-}; echo",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    // The disposition itself, read where a child can see it: a shell that
    // inherited an ignored SIGPIPE keeps it ignored, and one that inherited
    // the default keeps the default. Rust's runtime would have made both
    // ignored.
    let output = shell("sh", &["-c", "grep ^SigIgn: /proc/self/status"]);
    if output.status.success() {
        let ignored = stdout(&output);
        let mask = ignored
            .split_whitespace()
            .nth(1)
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .expect("a signal mask");
        assert_eq!(mask & (1 << 12), 0, "SIGPIPE is ignored: {ignored}");
    }
}

/// An argument need not be valid UTF-8, and a shell passes such bytes through
/// untouched rather than dying on a conversion.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn a_non_text_argument_survives() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "sh");
    let output = Command::new(&link)
        .arg0("/bin/sh")
        .arg("-c")
        .arg("printf %s \"$1\" | od -An -tx1")
        .arg("sh")
        .arg(OsString::from_vec(vec![0xff, 0xfe]))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output).split_whitespace().collect::<Vec<_>>(),
        ["ff", "fe"]
    );
}

/// Where the host's `/bin/sh` is the shell this one is a port of, they are
/// compared directly. Everywhere else this reports what it skipped and why.
// [spec:ronin:req:product.shell-identity/test]
#[test]
#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "these strings are shell scripts; `${x:-d}` is dash's parameter expansion, not a placeholder"
)]
fn the_host_shell_answers_the_same() {
    let Some(reference) = dash_on_this_host() else {
        eprintln!(
            "/bin/sh is not dash on this host; the comparison would be against another shell"
        );
        return;
    };

    let scripts = [
        "echo one two three",
        "printf '%s\\n' a b c | tr a-z A-Z",
        "for i in 1 2 3; do printf %s $i; done; echo",
        "x=1; echo ${x:-d} ${y:-d} ${y:+set}",
        "echo $((7 % 3)) $((1 << 4))",
        "case abc in a*) echo yes;; *) echo no;; esac",
        "IFS=:; set -- a:b:c; echo $*",
        "f() { echo in-a-function; }; f",
        "cd /tmp && pwd",
        "FOO=bar env | grep '^FOO='",
        "trap 'echo leaving' EXIT; echo body",
        "echo \"a'b\" 'c\"d'",
        "cat <<EOF\nheredoc $((1+1))\nEOF",
        "( exit 4 ); echo $?",
        "set -- ; echo \"[$*]\" $#",
        "nosuchcommandanywhere",
        "exit 42",
        "false && echo no; echo after",
        ": && printf '%s' ok",
    ];

    for script in scripts {
        let ours = shell("/bin/sh", &["-c", script]);
        let theirs = Command::new(&reference)
            .arg0("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(
            (stdout(&ours), stderr(&ours), ours.status.code()),
            (stdout(&theirs), stderr(&theirs), theirs.status.code()),
            "the two shells disagree about `{script}`"
        );
    }
}

/// One filed difference: the script, what dash does with it, and what this
/// shell does. Each side is the output and the status together.
type FiledDifference = (&'static str, (&'static str, i32), (&'static str, i32));

/// The differences between this shell and dash that are known and filed.
///
/// They are asserted rather than omitted. A case merely left out of the list
/// above would go quiet the day it changed, in either direction: an excuse
/// that can never fail is how a real regression gets waved through. Each entry
/// below states what each shell does, so a fix upstream fails this test and
/// says which filing to close.
// [spec:ronin:req:product.shell-identity/test]
#[test]
fn the_differences_from_dash_are_filed() {
    let Some(reference) = dash_on_this_host() else {
        eprintln!("/bin/sh is not dash on this host; there is nothing to compare against");
        return;
    };

    let filed: [FiledDifference; 4] = [
        // A write that fails: dash names the line and repeats the builtin, and
        // leaves with 1. Filed against nsh: the prefix and the status.
        (
            "echo a > /dev/full",
            ("/bin/sh: 1: echo: echo: I/O error\n", 1),
            ("/bin/sh: echo: I/O error\n", 2),
        ),
        // `set -u` on an unset parameter: dash prefixes the diagnostic and
        // leaves with 2. Filed against nsh: the prefix and the status.
        (
            "set -u; echo \"${UNSETVAR}\"",
            ("/bin/sh: 1: UNSETVAR: parameter not set\n", 2),
            ("UNSETVAR: parameter not set\n", 1),
        ),
        // Division by zero: the wording only. Filed against nsh.
        (
            "echo $((1/0))",
            (
                "/bin/sh: 1: arithmetic expression: division by zero: \"1/0\"\n",
                2,
            ),
            (
                "/bin/sh: 1: arithmetic expression: division error: \"1/0\"\n",
                2,
            ),
        ),
        // A command file that cannot be opened. This one is sanctioned
        // upstream as `missing_command_file_status`: POSIX gives the failure
        // 127 and dash routes it through its generic shell-error 2. Ronin
        // writes the response file it hands over, so nothing a build does
        // reaches it.
        (
            "",
            (
                "/bin/sh: 0: cannot open /nonexistent/script.sh: No such file\n",
                2,
            ),
            (
                "/bin/sh: 0: cannot open /nonexistent/script.sh: No such file\n",
                127,
            ),
        ),
    ];

    for (script, (dash_error, dash_status), (ours_error, our_status)) in filed {
        let arguments: Vec<&str> = if script.is_empty() {
            vec!["/nonexistent/script.sh"]
        } else {
            vec!["-c", script]
        };
        let ours = shell("/bin/sh", &arguments);
        let theirs = Command::new(&reference)
            .arg0("/bin/sh")
            .args(&arguments)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(
            (stderr(&theirs), theirs.status.code()),
            (dash_error.to_owned(), Some(dash_status)),
            "dash changed its answer to `{script}`"
        );
        assert_eq!(
            (stderr(&ours), ours.status.code()),
            (ours_error.to_owned(), Some(our_status)),
            "the filed difference on `{script}` is not the one this shell shows"
        );
    }
}

/// The path of the running program, as the shell that runs a command sees it.
///
/// `readlink /proc/$$/exe` is the one thing a command can ask that says which
/// shell read it, and the substitution is otherwise invisible on purpose: the
/// spelling in the graph, the diagnostics and the status are all the ones the
/// machine's shell would have produced.
fn which_shell(destination: &str) -> String {
    // The trailing `:` is load-bearing: a shell asked for one simple command
    // execs it in place rather than forking, and then `/proc/$$/exe` is the
    // program that replaced the shell rather than the shell.
    format!("readlink /proc/$$/exe > {destination}; :")
}

fn this_executable() -> PathBuf {
    std::fs::canonicalize(env!("CARGO_BIN_EXE_ronin")).unwrap()
}

/// A shell the machine has that is not the default's spelling, so a build can
/// name one and be given it.
fn a_named_shell() -> Option<(PathBuf, PathBuf)> {
    let named = PathBuf::from("/bin/dash");
    // Named one way and reported another: `/proc/$$/exe` is the file, and the
    // spelling a build wrote is a path to it.
    let resolved = std::fs::canonicalize(&named).ok()?;
    named.is_file().then_some((named, resolved))
}

fn ninja_build(directory: &Path, manifest: &str, arguments: &[&str]) -> Output {
    std::fs::write(directory.join("build.ninja"), manifest).unwrap();
    Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// Ninja mode: a command that needs a shell is read by this executable, and a
/// `--shell` that names one is given the one it named.
// [spec:ronin:req:product.builtin-shell/test]
#[test]
fn a_ninja_command_reads_here() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = format!(
        "rule ask\n  command = {}\nbuild out: ask\n",
        which_shell("$out")
    )
    .replace('$', "$$")
    .replace("$$out", "$out");
    let read = |arguments: &[&str]| {
        let output = ninja_build(directory.path(), &manifest, arguments);
        assert!(output.status.success(), "{}", stderr(&output));
        let answer = std::fs::read_to_string(directory.path().join("out")).unwrap();
        let _ = std::fs::remove_file(directory.path().join("out"));
        PathBuf::from(answer.trim())
    };

    assert_eq!(read(&[]), this_executable());
    // The whole command goes to a shell under `--compat`, and it is still this
    // one.
    assert_eq!(read(&["--compat"]), this_executable());
    // Asking for the default by name asks for the program, so it is the same
    // answer: the substitution is about which shell, not about how it was
    // requested.
    assert_eq!(read(&["--shell", "/bin/sh"]), this_executable());
    if let Some((named, resolved)) = a_named_shell() {
        // A build that names a shell is given it.
        assert_eq!(read(&["--shell", named.to_str().unwrap()]), resolved);
    }
}

/// Make mode: a recipe line that needs a shell is read by this executable, and
/// a Makefile that sets `SHELL` is given the shell it set.
// [spec:ronin:req:product.builtin-shell/test]
#[cfg(feature = "make")]
#[test]
fn a_recipe_reads_through_this_shell() {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "make");
    let read = |makefile: &str| {
        std::fs::write(directory.path().join("Makefile"), makefile).unwrap();
        let output = Command::new(&link)
            .current_dir(directory.path())
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        let answer = std::fs::read_to_string(directory.path().join("out")).unwrap();
        let _ = std::fs::remove_file(directory.path().join("out"));
        PathBuf::from(answer.trim())
    };

    // `$$$$` is a Makefile's way of writing the shell's `$$`, and the `>` is
    // what keeps the line off the shell-free fast path.
    assert_eq!(
        read("out:\n\t@readlink /proc/$$$$/exe > out; :\n"),
        this_executable()
    );
    // What the Makefile computed while it was being read, rather than while it
    // was being built, goes the same way.
    assert_eq!(
        read("WHO := $(shell readlink /proc/$$$$/exe; :)\nout:\n\t@echo $(WHO) > out\n"),
        this_executable()
    );
    if let Some((named, resolved)) = a_named_shell() {
        assert_eq!(
            read(&format!(
                "SHELL := {}\nout:\n\t@readlink /proc/$$$$/exe > out; :\n",
                named.display()
            )),
            resolved
        );
        assert_eq!(
            read(&format!(
                "SHELL := {}\nWHO := $(shell readlink /proc/$$$$/exe; :)\nout:\n\t@echo $(WHO) > out\n",
                named.display()
            )),
            resolved
        );
    }
}

/// The same question, written the way a recipe writes it: a Makefile spells
/// the shell's `$` as `$$`.
#[cfg(feature = "make")]
fn which_shell_recipe(destination: &str) -> String {
    which_shell(destination).replace('$', "$$")
}

/// Make mode: a recipe that reaches ONE shell as a whole assembled script is
/// read by this executable too.
///
/// Three shapes cannot be handed over as the command lines they are made of,
/// and each of them arrives as the script instead: a line too long to be an
/// argument, whose response file is named per edge and so can only be one; a
/// script a depfile extraction rewrote, which is no longer those lines; and the
/// segments a recipe is cut into around a `$(MAKE)` composed into the graph.
/// The command line that runs such a script names `/bin/sh` in its own text,
/// which is the machine's however the line reaches a shell, so each shape needs
/// the launch to name the program instead.
// [spec:ronin:req:product.builtin-shell/test]
#[cfg(feature = "make")]
#[test]
fn an_assembled_script_reads_here() {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "make");
    let make = |makefile: &str| {
        std::fs::write(directory.path().join("Makefile"), makefile).unwrap();
        Command::new(&link)
            .current_dir(directory.path())
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let built = |makefile: &str| {
        let output = make(makefile);
        assert!(output.status.success(), "{}", stderr(&output));
    };
    let who = |name: &str| {
        let path = directory.path().join(name);
        let answer = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        PathBuf::from(answer.trim())
    };

    // Generated rather than checked in: what matters is the length, and a file
    // holding it would be a hundred kilobytes of `x`.
    let too_long = "x".repeat(120 * 1000);
    built(&format!(
        "out:\n\t@{}; : {too_long}\n",
        which_shell_recipe("out")
    ));
    assert_eq!(who("out"), this_executable());

    // A composed `$(MAKE)` cuts the recipe into the lines written ahead of the
    // invocation and the lines written after it. Both are scripts of their own,
    // and the child's own recipe is a whole recipe again.
    std::fs::write(
        directory.path().join("sub.mk"),
        format!("sub:\n\t@{}\n", which_shell_recipe("child")),
    )
    .unwrap();
    built(&format!(
        "recurse:\n\t@{}\n\t@$(MAKE) -f sub.mk sub\n\t@{}\n",
        which_shell_recipe("before"),
        which_shell_recipe("after")
    ));
    assert_eq!(who("before"), this_executable());
    assert_eq!(who("child"), this_executable());
    assert_eq!(who("after"), this_executable());

    // The recipe's own answer about a failure is still the recipe's: kati sets
    // it only where every line of the assembled script said so.
    let ignored = make(&format!(
        "out:\n\t-@: {too_long}\n\t-@false\n\t-@{}\n",
        which_shell_recipe("out")
    ));
    assert!(ignored.status.success(), "{}", stderr(&ignored));
    assert_eq!(who("out"), this_executable());
    let refused = make(&format!("out:\n\t@: {too_long}\n\t@false\n"));
    assert_eq!(refused.status.code(), Some(2), "{}", stderr(&refused));

    // The substitution boundary: a recipe naming a shell of its own is given
    // it, whole script and all.
    if let Some((named, resolved)) = a_named_shell() {
        built(&format!(
            "SHELL := {}\nout:\n\t@{}; : {too_long}\n",
            named.display(),
            which_shell_recipe("out")
        ));
        assert_eq!(who("out"), resolved);
    }
}

/// A recipe held together by a line too long to be a shell argument reaches one
/// shell as a whole script, so a `+`-marked line inside it does not run under
/// `-t` or `-q` — where GNU Make, launching each line on its own, runs the
/// marked ones (the whole set under `-t`, and those before the first unmarked
/// line under `-q`). Splitting them out needs a response file per step: the one
/// `<output>.rsp` is named per edge and written from the whole script, so a
/// second oversized line would want the same name and content. That is a change
/// to the build engine's response-file lifetime, out of proportion to a shape
/// no measured makefile reaches — no recipe line in the corpus, the vendored
/// kati corpus, GNU Make's own suite, or vim/zsh comes within two orders of
/// magnitude of the 100 kB threshold. So the divergence is a recorded decision,
/// owned by `an-oversized-recipes-marked-lines-cannot-be-split-out` and written
/// up in docs/make-oracle-divergences.md. This gates it: launching the marked
/// lines out would make the `-t`/`-q` assertions fail, which is the decision
/// being reopened. No corpus case, because the Makefile is 120 kB — generated
/// here the way `an_assembled_script_reads_here` generates its own.
#[cfg(feature = "make")]
#[test]
fn oversized_marks_are_not_split_out() {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "make");
    // Generated rather than checked in: what matters is the length.
    let too_long = "x".repeat(120 * 1000);
    let makefile = format!(
        "goal:\n\t+@echo marked > marked.out\n\t@: {too_long}\n\t+@echo after > after.out\n"
    );
    std::fs::write(directory.path().join("Makefile"), &makefile).unwrap();
    let run = |args: &[&str]| {
        Command::new(&link)
            .current_dir(directory.path())
            .args(args)
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let wrote = |name: &str| directory.path().join(name).exists();
    let clean = |name: &str| {
        let _ = std::fs::remove_file(directory.path().join(name));
    };

    // `-t`: GNU touches `goal` and runs both marked lines. Ronin touches the
    // target and runs neither — the recipe is one launch, with one answer to
    // "does this run anyway", and it is no.
    let touched = run(&["-t", "goal"]);
    assert!(touched.status.success(), "{}", stderr(&touched));
    assert!(wrote("goal"), "the target is touched");
    assert!(
        !wrote("marked.out") && !wrote("after.out"),
        "the marked lines stay inside the one shell under -t"
    );
    clean("goal");

    // `-q`: GNU runs the marked line before the first unmarked one and answers
    // 1. Ronin answers 1 having written nothing, and touches nothing.
    let questioned = run(&["-q", "goal"]);
    assert_eq!(questioned.status.code(), Some(1), "{}", stderr(&questioned));
    assert!(
        !wrote("marked.out") && !wrote("goal"),
        "the marked line stays inside the one shell under -q"
    );

    // A real build runs the whole recipe as one shell: every line happens, the
    // same as GNU. The divergence is only in the pretending modes.
    let built = run(&["goal"]);
    assert!(built.status.success(), "{}", stderr(&built));
    assert!(
        wrote("marked.out") && wrote("after.out"),
        "the whole recipe runs when the build is real"
    );
}

/// A composed recipe whose lines ahead of a `$(MAKE)` include one too long to
/// be a shell argument stages that segment as its own edge, named after a proxy
/// the compiler invents — `.ronin_recipe_stage/N` — and falls back to handing
/// the whole segment to the shell through a response file, `…/N.rsp`. The
/// output-directory loop skips an invented output on purpose, so the response
/// file used to be written into a directory nothing had made and the build died
/// where GNU Make builds. The write now makes that directory first, and only
/// when a real build is about to write into it. GNU Make writes `big.out`, runs
/// the child, writes `trailing.out`; a dry run writes none of them.
#[cfg(feature = "make")]
#[test]
fn an_oversized_preceding_segment_writes_its_response_file() {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "make");
    std::fs::create_dir(directory.path().join("sub")).unwrap();
    std::fs::write(
        directory.path().join("sub").join("Makefile"),
        "child:\n\t@echo childran > ../child.out\n",
    )
    .unwrap();
    // Generated rather than checked in: a 120 kB Makefile is a quarter megabyte
    // echoed back, and what matters is only that the line crosses the threshold.
    let too_long = "A".repeat(120 * 1000);
    let makefile = format!(
        "goal:\n\t@echo {too_long} > big.out\n\
         \t@$(MAKE) --no-print-directory -C sub child\n\
         \t@echo trailing > trailing.out\n"
    );
    std::fs::write(directory.path().join("Makefile"), &makefile).unwrap();
    let run = |args: &[&str]| {
        Command::new(&link)
            .current_dir(directory.path())
            .args(args)
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let read = |name: &str| std::fs::read(directory.path().join(name)).ok();
    let clean = || {
        for name in ["big.out", "child.out", "trailing.out"] {
            let _ = std::fs::remove_file(directory.path().join(name));
        }
    };

    // A real build: the oversized preceding segment runs, its response file is
    // written and read, and every one of the recipe's effects lands — the same
    // as GNU Make. Before the fix this exited 2 with a `WriteFile` error and
    // wrote nothing at all.
    let built = run(&["goal"]);
    assert!(built.status.success(), "{}", stderr(&built));
    assert_eq!(
        read("big.out").map(|bytes| bytes.len()),
        Some(120 * 1000 + 1),
        "the oversized preceding segment writes big.out"
    );
    assert_eq!(read("child.out").as_deref(), Some(&b"childran\n"[..]));
    assert_eq!(read("trailing.out").as_deref(), Some(&b"trailing\n"[..]));
    clean();

    // A dry run reaches no disk: the response file is not written, so the
    // segment's proxy directory is not made either, and no output appears. GNU
    // Make prints the commands and writes nothing; before the fix even `-n`
    // died trying to write the response file.
    let pretended = run(&["-n", "goal"]);
    assert!(pretended.status.success(), "{}", stderr(&pretended));
    assert!(
        read("big.out").is_none() && read("child.out").is_none() && read("trailing.out").is_none(),
        "a dry run writes nothing"
    );
}

/// A `.KATI_DEPFILE` names the dependency file in a variable and leaves the
/// recipe's text untouched, so the recipe is launched one process per line like
/// any other — where a `--detect_depfiles` run, which rewrites the assembled
/// script, keeps the whole of it as one launch. So a line whose shell syntax is
/// left open dies where it stands under a `.KATI_DEPFILE` recipe, as GNU Make's
/// per-line launch makes it, rather than being closed by the line after it. The
/// depfile shape of `make-a-composed-recipe-is-still-one-shell`, which an
/// earlier probe had reported already right and which was not.
#[cfg(feature = "make")]
#[test]
fn a_depfile_recipe_launches_per_line() {
    let directory = tempfile::tempdir().unwrap();
    let link = shell_named(directory.path(), "make");
    let run = |makefile: &str| {
        std::fs::write(directory.path().join("Makefile"), makefile).unwrap();
        Command::new(&link)
            .current_dir(directory.path())
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = directory.path().join("out");
    let clean = || {
        let _ = std::fs::remove_file(&out);
    };

    // Line 1 opens a single quote that line 2 closes. Launched per line, line 1
    // is a shell syntax error and the recipe dies before `out` is written;
    // joined into one script the quote would close and `out` would be `x\ny`.
    let opened = run("out: .KATI_DEPFILE := out.d\nout:\n\t@printf %s 'x\n\ty' > out\n");
    assert_eq!(opened.status.code(), Some(2), "{}", stderr(&opened));
    assert!(!out.exists(), "the open-quote line dies where it stands");
    clean();

    // The whole recipe still runs when there is nothing to die on: both lines
    // launch, the depfile the first writes is read, and the target is built.
    let built = run(
        "out: .KATI_DEPFILE := out.d\nout:\n\t@printf 'out: dep.h\\n' > out.d\n\t@echo built > out\n",
    );
    assert!(built.status.success(), "{}", stderr(&built));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "built\n");
}
