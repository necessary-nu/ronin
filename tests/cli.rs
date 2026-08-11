use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn test_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ronin-{label}-{}-{}",
        std::process::id(),
        NEXT_TEST.fetch_add(1, Ordering::Relaxed)
    ))
}

// [spec:ronin:req:product.ronin-identity/test]
// [spec:ronin:req:product.no-samuflags/test]
// [spec:ronin:req:compat.version-reporting/test]
// [spec:ronin:req:compat.cli-and-tools/test]
// [spec:ronin:sem:samu.main-fn+1/test]
// [spec:ronin:sem:samu.parseenvargs-fn+1/test]
#[test]
fn binary_is_ronin_and_ignores_samuflags() {
    let binary = env!("CARGO_BIN_EXE_ronin");
    assert!(
        PathBuf::from(binary)
            .file_stem()
            .is_some_and(|name| name == "ronin"),
        "unexpected binary path: {binary}"
    );

    let output = Command::new(binary)
        .arg("--version")
        .env("SAMUFLAGS", "-d invalid-if-parsed")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        format!("{}\n", ronin::NINJA_COMPAT_VERSION).into_bytes()
    );
    assert!(output.stderr.is_empty());

    let error = Command::new(binary)
        .arg("--definitely-invalid")
        .output()
        .unwrap();
    assert!(!error.status.success());
    assert!(String::from_utf8_lossy(&error.stderr).starts_with("ronin: "));
}

// [spec:ronin:req:compat.ninja-owned-names/test]
#[test]
fn default_manifest_and_state_files_keep_ninja_names() {
    let directory = test_directory("ninja-names");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule emit\n  command = printf ronin > $out\nbuild output: emit\ndefault output\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("output")).unwrap(),
        "ronin"
    );
    assert!(directory.join(".ninja_log").exists());
    assert!(directory.join(".ninja_deps").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
// [spec:ronin:req:compat.cli-and-tools/test]
#[test]
fn missing_manifest_names_selected_source() {
    let directory = test_directory("missing-manifest");
    fs::create_dir_all(&directory).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .args(["-f", "absent.custom"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"ronin: error: loading 'absent.custom': No such file or directory\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
// [spec:ronin:req:compat.process-integration/test]
#[test]
fn stale_jobserver_uses_local_scheduler() {
    let directory = test_directory("stale-jobserver");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule emit\n  command = printf built > $out\nbuild output: emit\ndefault output\n",
    )
    .unwrap();
    let makeflags = format!(
        " -j2 --jobserver-auth=fifo:{}/missing-jobserver",
        directory.display()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .env("MAKEFLAGS", makeflags)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output
            .stdout
            .starts_with(b"ronin: Jobserver mode detected:  -j2 "),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        output.stderr,
        b"ronin: error: Could not initialize jobserver: Error opening fifo for reading: No such file or directory\n"
    );
    assert_eq!(
        fs::read_to_string(directory.join("output")).unwrap(),
        "built"
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.cli-and-tools/test]
#[test]
fn ninja_compatible_options_tools_streams_and_statuses_are_connected() {
    let directory = test_directory("cli-tools");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        concat!(
            "rule cc\n",
            "  command = printf compiled > $out\n",
            "  description = compile $out\n",
            "build object: cc source\n",
            "build all: phony object\n",
            "default all\n"
        ),
    )
    .unwrap();
    fs::write(directory.join("source"), "input").unwrap();
    let binary = env!("CARGO_BIN_EXE_ronin");
    let invoke = |arguments: &[&str]| {
        Command::new(binary)
            .current_dir(&directory)
            .args(arguments)
            .output()
            .unwrap()
    };

    let tools = invoke(&["-t", "list"]);
    assert!(tools.status.success());
    assert!(tools.stderr.is_empty());
    let tools = String::from_utf8(tools.stdout).unwrap();
    assert!(tools.starts_with("ronin subtools:\n"));
    for tool in [
        "browse",
        "clean",
        "commands",
        "inputs",
        "multi-inputs",
        "deps",
        "missingdeps",
        "graph",
        "query",
        "targets",
        "compdb",
        "compdb-targets",
        "recompact",
        "restat",
        "rules",
        "cleandead",
    ] {
        assert!(
            tools
                .lines()
                .any(|line| line.split_whitespace().next() == Some(tool)),
            "missing tool {tool}"
        );
    }

    let help = invoke(&["-t", "commands", "-h"]);
    assert_eq!(help.status.code(), Some(1));
    assert!(help.stderr.is_empty());
    assert!(
        String::from_utf8_lossy(&help.stdout)
            .starts_with("usage: ronin -t commands [options] [targets]\n")
    );

    let unknown = invoke(&["-t", "nope"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(unknown.stdout.is_empty());
    assert_eq!(
        unknown.stderr,
        b"ronin: fatal: unknown tool 'nope', did you mean 'deps'?\n"
    );

    let dry_run = invoke(&["-n", "-j2", "--status", "[$finished/$total] $description"]);
    assert!(dry_run.status.success());
    assert!(dry_run.stderr.is_empty());
    assert_eq!(dry_run.stdout, b"[1/1] compile object\n");

    let inputs = invoke(&["-t", "inputs", "all"]);
    assert!(inputs.status.success());
    assert_eq!(inputs.stdout, b"object\nsource\n");
    assert!(inputs.stderr.is_empty());

    let compdb = invoke(&["-t", "compdb-targets", "all"]);
    assert!(compdb.status.success());
    let compdb = String::from_utf8(compdb.stdout).unwrap();
    assert!(compdb.contains("\"output\": \"object\""));
    assert!(compdb.contains("\"file\": \"source\""));

    fs::write(directory.join("object"), "compiled").unwrap();
    let clean = invoke(&["-t", "clean"]);
    assert!(clean.status.success());
    assert_eq!(clean.stdout, b"Cleaning... 1 files.\n");
    assert!(clean.stderr.is_empty());
    assert!(!directory.join("object").exists());

    let quiet = invoke(&["-n", "--quiet"]);
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty());
    assert!(quiet.stderr.is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
// [spec:ronin:req:compat.byte-inputs/test]
#[test]
fn accepts_a_non_utf8_manifest_argument() {
    use std::os::unix::ffi::OsStringExt;

    let directory = test_directory("byte-argument");
    fs::create_dir_all(&directory).unwrap();
    let mut manifest_name = b"build-".to_vec();
    manifest_name.push(0xff);
    manifest_name.extend_from_slice(b".ninja");
    let manifest = directory.join(std::ffi::OsString::from_vec(manifest_name));
    fs::write(
        &manifest,
        "rule emit\n  command = printf exact > $out\nbuild output: emit\ndefault output\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .arg("-f")
        .arg(&manifest)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("output")).unwrap(),
        "exact"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn streams_failure_context_and_buffered_output_before_the_final_diagnostic() {
    let directory = test_directory("failure-output");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule fail\n  command = printf child; false\n  description = failing action\nbuild output: fail\ndefault output\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    let status = stdout.find("[1/1] failing action\n").unwrap();
    let failure = stdout.find("FAILED: [code=1] output \n").unwrap();
    let command = stdout.find("printf child; false\n").unwrap();
    let child = stdout.rfind("child").unwrap();
    assert!(status < failure && failure < command && command < child);
    // Ninja reports the stop on stdout, after the build's own output, and
    // leaves stderr for diagnostics.
    assert!(stdout.ends_with("ronin: build stopped: subcommand failed.\n"));
    assert!(child < stdout.find("build stopped").unwrap());
    assert!(result.stderr.is_empty());
    assert_eq!(result.status.code(), Some(1));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn writes_explanations_to_stderr_and_status_to_stdout() {
    let directory = test_directory("explain-streams");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule emit\n  command = touch $out\nbuild output: emit\ndefault output\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .args(["-d", "explain"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "[1/1] touch output\n"
    );
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.starts_with("ronin explain: output output"));
    assert!(!stderr.contains("[1/1]"));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
// [spec:ronin:req:compat.process-integration/test]
// [spec:ronin:req:product.build-outcome/test]
#[test]
// [spec:ronin:req:runtime.process-supervisor-scalability/test]
fn forwards_interrupts_and_removes_partial_outputs() {
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;

    let directory = test_directory("interrupt-forwarding");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule slow\n  command = touch $out; touch started; sleep 30\nbuild output: slow\ndefault output\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if directory.join("started").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(directory.join("started").exists());
    let child_id = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let status = child.wait().unwrap();
    // Ninja exits with 130 rather than dying by the signal it caught. C samurai
    // re-raised, and Ronin followed it here until the exit-status surface was
    // measured against Ninja; the contract is Ninja's.
    assert_eq!(status.signal(), None);
    assert_eq!(status.code(), Some(ronin::INTERRUPTED_EXIT_CODE));
    assert!(!directory.join("output").exists());
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.manifest-semantics/test]
#[test]
fn a_too_new_required_version_is_refused_in_ninjas_words() {
    let directory = test_directory("required-version-too-new");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "ninja_required_version = 1.99\nrule t\n  command = touch $out\nbuild o: t\n",
    )
    .unwrap();
    let error = ronin::Runner::new(&directory)
        .unwrap()
        .run(&["ronin".into(), "-n".into()])
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "fatal: ninja version ({}) incompatible with build file \
             ninja_required_version version (1.99).",
            ronin::NINJA_COMPAT_VERSION
        )
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.manifest-semantics/test]
#[test]
fn an_older_required_major_is_accepted_with_ninjas_warning() {
    let directory = test_directory("required-version-older-major");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "ninja_required_version = 0.9\nrule t\n  command = touch $out\nbuild o: t\n",
    )
    .unwrap();
    let result = ronin::Runner::new(&directory)
        .unwrap()
        .run_os(&["ronin".into(), "-n".into()])
        .unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert_eq!(
        stderr,
        format!(
            "ronin: warning: ninja executable version ({}) greater than build file \
             ninja_required_version (0.9); versions may be incompatible.\n",
            ronin::NINJA_COMPAT_VERSION
        )
    );
    assert_eq!(result.exit_code, 0);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.cli-and-tools/test]
#[test]
fn dupbuild_is_deprecated_and_duplicates_stay_fatal() {
    let directory = test_directory("warn-dupbuild");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule t\n  command = touch $out\nbuild o: t\nbuild o: t\n",
    )
    .unwrap();
    for flag in ["dupbuild=warn", "dupbuild=err"] {
        let mut stderr = Vec::new();
        let error = ronin::Runner::new(&directory)
            .unwrap()
            .run_os_with_sinks(
                &["ronin".into(), "-w".into(), flag.into(), "-n".into()],
                &mut Vec::new(),
                &mut stderr,
            )
            .unwrap_err();
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "ronin: warning: deprecated warning 'dupbuild'\n"
        );
        assert!(
            error.to_string().contains("multiple rules generate"),
            "{flag}: {error}"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.cli-and-tools/test]
#[test]
fn a_self_referencing_phony_warns_by_default_and_errors_on_request() {
    let directory = test_directory("warn-phonycycle");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("build.ninja"), "build a: phony a\n").unwrap();

    let mut stderr = Vec::new();
    ronin::Runner::new(&directory)
        .unwrap()
        .run_os_with_sinks(
            &["ronin".into(), "-n".into(), "a".into()],
            &mut Vec::new(),
            &mut stderr,
        )
        .unwrap();
    assert_eq!(
        String::from_utf8(stderr).unwrap(),
        "ronin: warning: phony target 'a' names itself as an input; \
         ignoring [-w phonycycle=warn]\n"
    );

    let error = ronin::Runner::new(&directory)
        .unwrap()
        .run(&[
            "ronin".into(),
            "-w".into(),
            "phonycycle=err".into(),
            "-n".into(),
            "a".into(),
        ])
        .unwrap_err();
    assert!(error.to_string().contains("dependency cycle"), "{error}");
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.command-execution/test]
#[test]
fn a_command_needing_no_shell_behaves_the_same_with_and_without_one() {
    let directory = test_directory("launcher-equivalence");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule cc\n  command = touch $out\nbuild out: cc\n",
    )
    .unwrap();
    let mut outputs = Vec::new();
    for arguments in [
        vec!["ronin".to_string(), "out".to_string()],
        vec!["ronin".into(), "--compat".into(), "out".into()],
    ] {
        fs::remove_file(directory.join("out")).ok();
        fs::remove_file(directory.join(".ninja_log")).ok();
        outputs.push(
            ronin::Runner::new(&directory)
                .unwrap()
                .run(&arguments)
                .unwrap(),
        );
        assert!(directory.join("out").exists());
    }
    assert_eq!(outputs[0], outputs[1]);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.command-execution/test]
#[test]
fn a_missing_program_still_gets_the_shells_diagnostic() {
    // The direct path cannot produce `sh: 1: …: not found`, so it must hand
    // the command back to the shell rather than invent a message of its own.
    let directory = test_directory("launcher-not-found");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule x\n  command = ronin-no-such-program-exists\nbuild out: x\n",
    )
    .unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = ronin::Runner::new(&directory)
        .unwrap()
        .run_os_with_sinks(&["ronin".into(), "out".into()], &mut stdout, &mut stderr)
        .unwrap();
    let seen = String::from_utf8_lossy(&stdout).into_owned();
    assert!(
        seen.contains("ronin-no-such-program-exists: not found"),
        "stdout was {seen:?}"
    );
    // The shell's own status for a command it could not find, carried out.
    assert_eq!(result.exit_code, 127);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.cli-and-tools/test]
// [spec:ronin:req:product.build-outcome/test]
#[test]
fn a_failing_command_carries_its_own_status_out_of_the_process() {
    // The number a CI reads: it distinguishes a compile error from an OOM kill,
    // and Ronin reported 1 for both until this was measured against Ninja.
    let directory = test_directory("exit-status-propagation");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule f\n  command = exit 7\nrule g\n  command = exit 5\n\
         build a: f\nbuild b: g\ndefault a b\n",
    )
    .unwrap();

    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ronin"))
            .args(arguments)
            .current_dir(&directory)
            .output()
            .unwrap()
    };

    let stopped = run(&["-j", "1", "a"]);
    assert_eq!(stopped.status.code(), Some(7));
    assert!(
        String::from_utf8_lossy(&stopped.stdout)
            .ends_with("ronin: build stopped: subcommand failed.\n")
    );

    // Under keep-going the last failure wins, and the reason changes because
    // the allowance was never used up.
    let kept_going = run(&["-k", "0", "-j", "1", "a", "b"]);
    assert_eq!(kept_going.status.code(), Some(5));
    assert!(
        String::from_utf8_lossy(&kept_going.stdout)
            .ends_with("ronin: build stopped: cannot make progress due to previous errors.\n")
    );

    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.cli-and-tools/test]
#[test]
fn entering_a_directory_is_announced_unless_output_is_being_parsed() {
    let base = test_directory("entering-directory");
    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(
        work.join("build.ninja"),
        "rule cp\n  command = cp $in $out\nbuild a: cp source\ndefault a\n",
    )
    .unwrap();
    fs::write(work.join("source"), "source\n").unwrap();

    let run = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
            .args(arguments)
            .current_dir(&base)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            output.status.code(),
        )
    };

    // The line leads stdout, quoted the way Emacs and the compiler-error
    // parsers that copied GNU Make expect, so relative paths resolve.
    let (stdout, _, code) = run(&["-C", "work"]);
    assert!(stdout.starts_with("ronin: Entering directory `work'\n"));
    assert_eq!(code, Some(0));
    fs::remove_file(work.join("a")).unwrap();

    // Tool output is routinely piped into a file, and --quiet asked for none.
    let (tool_stdout, _, _) = run(&["-C", "work", "-t", "targets"]);
    assert!(!tool_stdout.contains("Entering directory"));
    let (quiet_stdout, _, _) = run(&["-C", "work", "--quiet"]);
    assert!(!quiet_stdout.contains("Entering directory"));
    fs::remove_file(work.join("a")).unwrap();

    // Announced before the move is attempted, so a directory that cannot be
    // entered is still named.
    let (missing_stdout, missing_stderr, missing_code) = run(&["-C", "nope"]);
    assert_eq!(missing_stdout, "ronin: Entering directory `nope'\n");
    assert_eq!(
        missing_stderr,
        "ronin: fatal: chdir to 'nope' - No such file or directory\n"
    );
    assert_eq!(missing_code, Some(1));

    fs::remove_dir_all(base).unwrap();
}

// [spec:ronin:req:compat.graph-semantics/test]
#[test]
fn a_dependency_cycle_is_named_by_the_path_around_it() {
    let directory = test_directory("cycle-path");
    fs::create_dir_all(&directory).unwrap();
    let run = |manifest: &str, arguments: &[&str]| {
        fs::write(directory.join("build.ninja"), manifest).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
            .args(arguments)
            .current_dir(&directory)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        String::from_utf8_lossy(&output.stderr).into_owned()
    };

    // The flag exists for the self-referencing phony CMake used to emit, and
    // names itself so the reader knows which flag turned this into an error.
    assert_eq!(
        run(
            "build a: phony a\nbuild b: phony a\ndefault b\n",
            &["-w", "phonycycle=err"]
        ),
        "ronin: error: dependency cycle: a -> a [-w phonycycle=err]\n"
    );

    // A longer cycle is an ordinary one, and the flag is not mentioned.
    assert_eq!(
        run(
            "rule cp\n  command = cp $in $out\n\
             build a: cp b\nbuild b: cp c\nbuild c: cp a\ndefault a\n",
            &[]
        ),
        "ronin: error: dependency cycle: a -> b -> c -> a\n"
    );

    // Asking for `b` still reports the cycle from `a`, the node that closes
    // it, rather than from the other output of the edge that starts it.
    assert_eq!(
        run(
            "rule cat\n  command = cat $in > $out\n\
             build a b: cat c\nbuild c: cat a\ndefault b\n",
            &[]
        ),
        "ronin: error: dependency cycle: a -> c -> a\n"
    );

    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.manifest-semantics/test]
#[test]
fn a_manifest_diagnostic_points_at_the_token_it_is_about() {
    let directory = test_directory("diagnostic-anchor");
    fs::create_dir_all(&directory).unwrap();
    let run = |manifest: &str| {
        fs::write(directory.join("build.ninja"), manifest).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
            .current_dir(&directory)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stderr).into_owned()
    };

    // Scanning has already reached `build` on line 4 by the time the binding
    // is rejected; the reader has to go to line 3.
    let rule_variable = run("rule cc\n  command = gcc\n  nonsense = x\nbuild a: cc\n");
    assert!(
        rule_variable.contains("build.ninja:3:"),
        "reported {rule_variable:?}"
    );
    // Ninja marks each chunk of a value as it reads it, so a complaint about
    // the binding lands on what ended the value rather than on the value.
    assert!(
        rule_variable.ends_with("  nonsense = x\n              ^ near here\n"),
        "reported {rule_variable:?}"
    );

    // A name is marked where it starts, so the caret sits under the word and
    // not past the end of it.
    let unknown_rule = run("build a: nosuchrule\n");
    assert!(
        unknown_rule.ends_with("build a: nosuchrule\n         ^ near here\n"),
        "reported {unknown_rule:?}"
    );

    // Column zero carries no context in Ninja, so neither does this — and an
    // error with no context ends with a blank line, because Ninja terminates
    // the message and the printer terminates the line.
    let indented = run("  command = x\n");
    assert_eq!(
        indented,
        "ronin: error: build.ninja:1: unexpected indent\n\n"
    );

    fs::remove_dir_all(directory).unwrap();
}

/// The peak number of work units alive at once, from a log of start and end
/// stamps, and how many units the log recorded.
#[cfg(unix)]
fn peak_concurrency(log: &str) -> (usize, usize) {
    let mut events = log
        .lines()
        .filter_map(|line| {
            let (kind, stamp) = line.split_once(' ')?;
            let start = kind == "S";
            // Ends sort before starts at an identical stamp, so a unit that
            // finishes exactly as another begins is not counted as an overlap.
            Some((stamp.parse::<f64>().ok()?, i32::from(start), start))
        })
        .collect::<Vec<_>>();
    events.sort_by(|left, right| left.partial_cmp(right).expect("stamps are finite"));
    let (mut live, mut peak, mut started) = (0, 0, 0);
    for (_, _, start) in events {
        if start {
            live += 1;
            started += 1;
            peak = peak.max(live);
        } else {
            live -= 1;
        }
    }
    (peak, started)
}

#[cfg(unix)]
#[test]
fn a_recursive_make_tree_shares_one_job_budget() {
    use std::fmt::Write as _;

    const LEVELS: [&str; 3] = ["a", "b", "c"];
    const UNITS: usize = 6;
    /// The same tree with the shared budget removed from what each level is
    /// told, so every level runs `-j` of its own. Present as a control: it is
    /// what the tree does when nothing serves it, and it is what makes the
    /// measurement below evidence rather than a tautology.
    const UNSHARED: &str = "unshared.ninja";

    if Command::new("make").arg("--version").output().is_err() {
        return;
    }
    let directory = test_directory("recursive-make-budget");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let mut shared = String::from("rule submake\n  command = make -f $mk all\n");
    let mut unshared =
        String::from("rule submake\n  command = env MAKEFLAGS=$budget make -f $mk all\n");
    for level in LEVELS {
        let units = (0..UNITS)
            .map(|unit| format!("{level}{unit}"))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            directory.join(format!("{level}.mk")),
            format!(
                "all: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
                stamp.display()
            ),
        )
        .unwrap();
        write!(shared, "build {level}: submake\n  mk = {level}.mk\n").unwrap();
        write!(
            unshared,
            "build {level}: submake\n  mk = {level}.mk\n  budget = -j{UNITS}\n"
        )
        .unwrap();
    }
    for (name, manifest) in [("build.ninja", &mut shared), (UNSHARED, &mut unshared)] {
        writeln!(manifest, "default {}", LEVELS.join(" ")).unwrap();
        fs::write(directory.join(name), &*manifest).unwrap();
    }

    // Every level takes its parallelism from MAKEFLAGS, so a level told `-j`
    // and nothing else runs that many units on its own. The measurement is of
    // overlap on the clock, not of what any level believes about its budget.
    let measure = |jobs: usize, manifest: &str| {
        let _ = fs::remove_file(&log);
        for level in LEVELS {
            let _ = fs::remove_file(directory.join(level));
        }
        let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
            .current_dir(&directory)
            .args([format!("-j{jobs}"), "-f".into(), manifest.into()])
            .env("LOG", &log)
            // Somewhere only this run writes, so what is left behind at the
            // end is this run's and not a sibling test's.
            .env("TMPDIR", &served)
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKELEVEL")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let (peak, units) = peak_concurrency(&fs::read_to_string(&log).unwrap());
        assert_eq!(units, LEVELS.len() * UNITS);
        peak
    };

    for jobs in [1, 2, 4] {
        let peak = measure(jobs, "build.ninja");
        assert!(
            peak <= jobs,
            "-j{jobs} let {peak} units of a recursive Make tree run at once"
        );
    }
    let unshared_peak = measure(2, UNSHARED);
    assert!(
        unshared_peak > UNITS,
        "the control did not oversubscribe, so the shared measurement proves nothing"
    );

    // The fifo belongs to the run that created it and outlives none of them.
    assert_eq!(fs::read_dir(&served).unwrap().count(), 0);
    fs::remove_dir_all(directory).unwrap();
}

/// A Makefile tree with a recipe that reports what Make told it about itself.
#[cfg(all(unix, feature = "make"))]
fn makefile_tree(label: &str) -> PathBuf {
    let directory = test_directory(label);
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(directory.join("in.txt"), "source\n").unwrap();
    fs::write(
        directory.join("Makefile"),
        "WHERE = makefile\n\
         all: out.txt\n\
         out.txt: in.txt\n\
         \tcp in.txt out.txt\n\
         \t@echo \"version=$(MAKE_VERSION) level=$(MAKELEVEL) exported=$$MAKELEVEL where=$(WHERE)\"\n\
         .PHONY: all\n",
    )
    .unwrap();
    directory
}

/// Every feature `.FEATURES` claims, exercised.
///
/// The list is short on purpose — see `EVALUATOR_FEATURES` — and the risk in a
/// short list is the opposite of the risk in a long one: it is cheap to add a
/// name here and never find out whether it was true. So each claimed feature
/// gets a construct that only works if the feature does, and a Makefile that
/// branches on `.FEATURES` is entitled to exactly this much.
///
/// `jobserver`, `jobserver-fifo`, and `output-sync` are build-side claims. Make
/// mode can map an inherited budget onto its Ninja scheduler, and Ninja's
/// reporter publishes each command edge's captured output as one unit.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_finds_a_prerequisite_through_vpath() {
    let directory = test_directory("make-vpath");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(directory.join("sub").join("hello.bar"), "source\n").unwrap();
    fs::write(
        directory.join("Makefile"),
        // The directive and the variable, and a directory that has nothing in
        // it, so the search has to keep looking rather than stop at the first.
        "vpath %.bar nowhere sub\n\
         all: hello.tsk\n\
         hello.tsk: hello.bar\n\
         \t@echo found $< \n\
         .PHONY: all\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The path the search found, which is what `$<` hands the recipe.
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("found sub/hello.bar"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A leading dot is Make's own spelling only for the names it reserves; `.1`
/// falls out of matching `%bye.x` against `bye.x`.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_builds_a_target_whose_name_begins_with_a_dot() {
    let directory = test_directory("make-dot-target");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: .1\n\
         \t@echo built $@ from $^\n\
         .1:\n\
         \t@echo made .1\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The recipe ran, and the parent saw both prerequisites by the names it
    // wrote — a target that is merely tolerated rather than built would show up
    // as a missing `made .1` with the rest still passing.
    assert!(stdout.contains("made .1"), "{stdout}");
    assert!(stdout.contains("built all from .1"), "{stdout}");
    fs::remove_dir_all(directory).unwrap();
}

/// `.RECIPEPREFIX` decides what introduces a recipe line, from where it is
/// written until it is cleared, and a tab is an ordinary character in between.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_reads_a_recipe_introduced_by_the_declared_prefix() {
    let directory = test_directory("make-recipeprefix");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        ".RECIPEPREFIX = >\n\
         all: one two\n\
         one:\n\
         > @echo made $@\n\
         .RECIPEPREFIX =\n\
         two:\n\
         \t@echo made $@\n\
         .PHONY: all one two\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("made one"), "{stdout}");
    assert!(stdout.contains("made two"), "{stdout}");
    fs::remove_dir_all(directory).unwrap();
}

/// `undefine` removes a variable rather than emptying it, and reaches no
/// further than the makefile unless it says `override`.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_undefines_a_variable() {
    let directory = test_directory("make-undefine");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "a = one\n\
         b := two\n\
         n = a\n\
         undefine $(n)\n\
         undefine b\n\
         undefine c\n\
         override undefine d\n\
         $(info [$(flavor a)][$(flavor b)][$(flavor c)][$(flavor d)][$(c)])\n\
         all: ; @echo done\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("c=kept")
        .arg("d=gone")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[undefined][undefined][recursive][undefined][kept]"),
        "{stdout}"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// `!=` runs its right-hand side and keeps what the command printed, folding
/// every newline into a space and dropping one at the end.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_assigns_what_a_shell_command_printed() {
    let directory = test_directory("make-shell-assignment");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "one!=printf 'a\\nb\\n'\n\
         two != printf 'c\\n\\n\\n'\n\
         all: ; @echo \"<$(one)> <$(two)>\"\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("<a b> <c  >"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A variable name is one word, so `x y = 1` is not an assignment at all and
/// the line is read as a rule, which has no separator.
// [spec:ronin:req:make.semantics+1/test]
// [spec:ronin:req:make.narration/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_refuses_an_assignment_whose_name_is_two_words() {
    let directory = test_directory("make-spaced-name");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("Makefile"), "x y = 1\nall: ; @echo built\n").unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ronin: Makefile:1: missing separator."),
        "{stderr}"
    );
    assert!(!stderr.contains("***"), "{stderr}");
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_treats_wait_as_a_barrier_and_not_as_a_prerequisite() {
    let directory = test_directory("make-wait");
    fs::create_dir_all(&directory).unwrap();
    // pre1 is the slow one and comes first, so unbarriered these invert.
    fs::write(
        directory.join("Makefile"),
        "all: pre1 .WAIT pre2\n\
         \t@echo all from $^\n\
         pre1: ; @sleep 0.3; echo pre1\n\
         pre2: ; @echo pre2\n\
         .WAIT:\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-j10")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let recipes = stdout
        .lines()
        .filter(|line| ["pre1", "pre2"].contains(line))
        .collect::<Vec<_>>();
    assert_eq!(recipes, ["pre1", "pre2"], "{stdout}");
    assert!(stdout.contains("all from pre1 pre2"), "{stdout}");
    fs::remove_dir_all(directory).unwrap();
}

/// The first expansion leaves `$$@` as `$@`; the second happens when the rule
/// is used and `$@` has a value.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_expands_prerequisites_again_under_second_expansion() {
    let directory = test_directory("make-second-expansion");
    fs::create_dir_all(&directory).unwrap();
    // `early` is above the declaration and must not get it.
    fs::write(
        directory.join("Makefile"),
        "early: $$(PRE)\n\
         .SECONDEXPANSION:\n\
         PRE = dep\n\
         all: $$(PRE) $$@.extra\n\
         \t@echo got $^\n\
         dep: ; @echo made dep\n\
         all.extra: ; @echo made $@\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("all")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("got dep all.extra"), "{stdout}");

    let early = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("early")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&early.stderr).contains("$(PRE)"),
        "{}",
        String::from_utf8_lossy(&early.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A pattern rule is chosen by whether its prerequisites are there, so under
/// `.SECONDEXPANSION` the expansion is part of the search: `%.o` must build
/// `named.o` where `named.c` exists and decline where it does not.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_expands_a_pattern_rules_prerequisites_again() {
    let directory = test_directory("make-second-expansion-pattern");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        ".SECONDEXPANSION:\n\
         all: named.o\n\
         %.o: $$*.c $$@.flags ; @echo built $@ from $^\n",
    )
    .unwrap();
    fs::write(directory.join("named.c"), "").unwrap();
    fs::write(directory.join("named.o.flags"), "").unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("named.o")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("built named.o from named.c named.o.flags"),
        "{stdout}"
    );

    let missing = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("absent.o")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    fs::remove_dir_all(directory).unwrap();
}

/// The prerequisite patterns after a static pattern rule's second colon get the
/// same treatment, with `%` standing for the stem in what the expansion left.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_expands_a_static_pattern_rules_prerequisites_again() {
    let directory = test_directory("make-second-expansion-statpat");
    fs::create_dir_all(&directory).unwrap();
    // `$<` and `$^` are worth what the rule above recorded, and `%` is the stem.
    fs::write(
        directory.join("Makefile"),
        ".SECONDEXPANSION:\n\
         one.o: first.c\n\
         one.o: %.o: $$(addsuffix .$$*,$$^) %.h ; @echo built $@ from $^\n",
    )
    .unwrap();
    for name in ["first.c", "first.c.one", "one.h"] {
        fs::write(directory.join(name), "").unwrap();
    }

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("one.o")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("built one.o from first.c first.c.one one.h"),
        "{stdout}"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A `|` ends the word it falls in, so the order-only list can arrive from the
/// expansion rather than from what was written.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_reads_an_order_only_marker_out_of_an_expansion() {
    let directory = test_directory("make-second-expansion-order-only");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        ".SECONDEXPANSION:\n\
         PRE = dep|after\n\
         all: $$(PRE) ; @echo all from [$^] and [$|]\n\
         dep after: ; @echo made $@\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("all")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("all from [dep] and [after]"), "{stdout}");
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_exports_variables_to_the_recipe_environment() {
    let directory = test_directory("make-export");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "export NAMED\n\
         NAMED = named\n\
         export INLINE = inline\n\
         QUIET = quiet\n\
         unexport INHERITED\n\
         all: ; @echo \"[$$NAMED][$$INLINE][$$QUIET][$$INHERITED]\"\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .env("INHERITED", "from-the-caller")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Exported by both spellings, silent for the one nobody exported, and
    // `unexport` takes back what the caller's environment supplied.
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[named][inline][][]"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// `.EXPORT_ALL_VARIABLES` covers what the Makefile defined and nothing else —
/// GNU Make leaves the built-in defaults unset in a recipe.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_exports_every_variable_the_makefile_defined() {
    let directory = test_directory("make-export-all");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        ".EXPORT_ALL_VARIABLES:\n\
         MINE = mine\n\
         all: ; @echo \"[$$MINE][$$CC]\"\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[mine][]"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// `.IGNORE` is `-i` asked for by the Makefile, and with prerequisites it is
/// that for those targets alone.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_ignores_recipe_failures_the_makefile_named() {
    let directory = test_directory("make-ignore");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        ".IGNORE: forgiven\n\
         all: forgiven; @echo reached-all\n\
         forgiven: ; @false\n",
    )
    .unwrap();
    let forgiving = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-k")
        .output()
        .unwrap();
    assert!(
        forgiving.status.success(),
        "{}",
        String::from_utf8_lossy(&forgiving.stderr)
    );
    assert!(
        String::from_utf8_lossy(&forgiving.stdout).contains("reached-all"),
        "{}",
        String::from_utf8_lossy(&forgiving.stdout)
    );

    // A target it did not name still fails, so the declaration is doing the
    // work rather than failure having stopped mattering.
    fs::write(
        directory.join("Makefile"),
        ".IGNORE: forgiven\n\
         all: other; @echo reached-all\n\
         other: ; @false\n",
    )
    .unwrap();
    let strict = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(!strict.status.success());
    fs::remove_dir_all(directory).unwrap();
}

/// Every Make dry-run spelling is Ninja's dry run over the compiled graph, so
/// the whole recipe is printed and no line of it runs — the `+` prefix
/// included.
///
/// GNU Make 4.4.1 runs a `+` line under `-n` because running is the only way
/// it can learn what the line would do. Nothing here needs that: the recipe is
/// already compiled, and a run told to touch nothing touches nothing.
// [spec:ronin:req:make.interface-compatibility/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn dry_run_spellings_run_nothing() {
    let directory = test_directory("make-dry-run");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\
         \techo before > before\n\
         \t+echo plus > plus\n\
         \techo mentioned $(MAKE) > mentioned\n\
         \techo after > after\n",
    )
    .unwrap();

    let program = invoked_as(&directory, "make");
    for spelling in ["-n", "--just-print", "--dry-run", "--recon"] {
        let output = make_command(&program, &directory)
            .arg(spelling)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{spelling}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(
            printed.contains("echo plus > plus"),
            "{spelling} did not print the recipe it declined to run: {printed}"
        );
        for skipped in ["before", "plus", "mentioned", "after"] {
            assert!(
                !directory.join(skipped).exists(),
                "{spelling} ran {skipped}, which it was told not to"
            );
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

/// A dry run over a recursive Makefile prints the child's work too, because
/// the child is part of the graph rather than a process this would have to
/// start to find out.
// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn dry_run_shows_the_composed_child() {
    let directory = test_directory("make-dry-run-recursive");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\t+$(MAKE) -C sub\n\techo parent > parent\n",
    )
    .unwrap();
    fs::write(
        directory.join("sub").join("Makefile"),
        "child: ; echo child > child\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-n")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.contains("echo child > child"),
        "the child graph was not in the dry run: {printed}"
    );
    assert!(
        printed.contains("echo parent > parent"),
        "the parent's own work was not in the dry run: {printed}"
    );
    for skipped in ["parent", "sub/child"] {
        assert!(
            !directory.join(skipped).exists(),
            "the dry run wrote {skipped}"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

/// Splitting a recipe into child graphs is all or nothing, and the line GNU
/// Make classifies recursive from its unexpanded text is how the compiler sees
/// the recursion that no static invocation can be lifted out of. A recipe
/// holding one of those is left whole rather than half-composed with a nested
/// Make hidden in what remains.
// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn recursive_recipes_are_never_half_composed() {
    let directory = test_directory("make-recursion-guard");
    for child in ["a", "b"] {
        fs::create_dir_all(directory.join(child)).unwrap();
        fs::write(
            directory.join(child).join("Makefile"),
            format!("child: ; echo {child} > built\n"),
        )
        .unwrap();
    }
    let program = invoked_as(&directory, "make");

    // The second invocation is real but sits behind a runtime test, so it is
    // not one static child compilation. Composing only the first would leave
    // it to start a nested Make beside the graph the first became.
    fs::write(
        directory.join("Makefile"),
        "all:\n\t$(MAKE) -C a\n\ttest -d b && $(MAKE) -C b\n",
    )
    .unwrap();
    let mixed = make_command(&program, &directory).output().unwrap();
    assert!(
        !mixed.status.success(),
        "a recipe was half-composed and the rest run as a nested Make"
    );
    let refusal = String::from_utf8_lossy(&mixed.stderr);
    assert!(
        refusal.contains("subninja"),
        "the refusal did not name the compilation it could not do: {refusal}"
    );
    for child in ["a", "b"] {
        assert!(
            !directory.join(child).join("built").exists(),
            "{child} was built despite the refusal"
        );
    }

    // MAKE named as an argument is not a Make being started, and 4.4.1 runs
    // the line under `-n` without recursing into anything either.
    fs::write(
        directory.join("Makefile"),
        "all:\n\t$(MAKE) -C a\n\ttest -d b && echo mentioned $(MAKE) > mentioned\n",
    )
    .unwrap();
    let mentioned = make_command(&program, &directory).output().unwrap();
    assert!(
        mentioned.status.success(),
        "naming MAKE in an argument was read as recursion: {}",
        String::from_utf8_lossy(&mentioned.stderr)
    );
    assert!(directory.join("mentioned").exists());
    assert!(directory.join("a").join("built").exists());
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_searches_the_include_directories() {
    let directory = test_directory("make-include-dir");
    fs::create_dir_all(directory.join("first")).unwrap();
    fs::create_dir_all(directory.join("second")).unwrap();
    fs::write(directory.join("second").join("extra.mk"), "V = second\n").unwrap();
    fs::write(directory.join("local.mk"), "W = local\n").unwrap();
    fs::write(directory.join("second").join("local.mk"), "W = searched\n").unwrap();
    fs::write(
        directory.join("Makefile"),
        "include extra.mk\n\
         include local.mk\n\
         all: ; @echo \"[$(V)][$(W)]\"\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .args(["-I", "first", "-Isecond"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // `first` has neither, and the working directory outranks the search.
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[second][local]"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// `-I -` is a restart of the search path, not a directory called `-`.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_forgets_the_include_directories_before_a_bare_dash() {
    let directory = test_directory("make-include-dir-reset");
    fs::create_dir_all(directory.join("first")).unwrap();
    fs::create_dir_all(directory.join("second")).unwrap();
    fs::write(directory.join("first").join("extra.mk"), "V = first\n").unwrap();
    fs::write(directory.join("second").join("extra.mk"), "V = second\n").unwrap();
    fs::write(
        directory.join("Makefile"),
        "include extra.mk\n\
         all: ; @echo \"[$(V)]\"\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .args(["-I", "first", "-I", "-", "-Isecond"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[second]"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A `.x.y:` rule is a suffix rule only while both suffixes are on the list.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_reads_the_declared_suffix_list() {
    let directory = test_directory("make-suffixes");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("hello.bar"), "source\n").unwrap();
    fs::write(directory.join("hello.baz"), "source\n").unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: hello.tsk\n\
         .SUFFIXES:\n\
         .SUFFIXES: .bar .tsk\n\
         .bar.tsk: ; @echo tsk from $<\n\
         .baz.tsk: ; @echo tsk from $<\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // `.baz` never made it onto the list, so its rule is not a suffix rule.
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("tsk from hello.bar"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_answers_the_order_only_automatic_variable() {
    let directory = test_directory("make-order-only-var");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: a | oo1 oo2 oo1\n\
         \t@echo \"^=$^ |=$| |D=$(|D)\"\n\
         a oo1 oo2: ; @:\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Deduplicated, and no D form — GNU Make reads $(|D) as a variable nobody
    // defined.
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("^=a |=oo1 oo2 |D="),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A phony prerequisite still makes the rule run, but filtering it out of `$?`
/// must operate on the final prerequisite names rather than an opaque shell
/// placeholder. This is the shape used by the Linux header-generation rules.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_filters_phony_from_new_inputs() {
    use std::time::{Duration, SystemTime};

    let directory = test_directory("make-filter-newer-prerequisites");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        ".PHONY: FORCE\n\
         PHONY := FORCE\n\
         out: old new FORCE\n\
         \t@printf '[%s]\\n' '$(filter-out $(PHONY),$?)' > answer\n",
    )
    .unwrap();
    for (name, seconds) in [("old", 100), ("out", 200), ("new", 300)] {
        let path = directory.join(name);
        fs::write(&path, []).unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("out")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(directory.join("answer")).unwrap(), b"[new]\n");
    fs::remove_dir_all(directory).unwrap();
}

/// Make's step 7: the recipe for a target nothing else could make.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_falls_back_to_the_default_rule() {
    let directory = test_directory("make-default-rule");
    fs::create_dir_all(&directory).unwrap();
    // `declared` has a rule, so it is not the default rule's business even
    // though that rule makes nothing.
    fs::write(
        directory.join("Makefile"),
        "all: nowhere declared\n\
         declared:\n\
         .DEFAULT:\n\
         \t@echo default made $@\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("default made nowhere"), "{stdout}");
    assert!(!stdout.contains("default made declared"), "{stdout}");
    fs::remove_dir_all(directory).unwrap();
}

/// Step 6 of GNU Make's implicit rule search.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_chains_implicit_rules_through_an_intermediate_file() {
    let directory = test_directory("make-implicit-chain");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("hello.f"), "source\n").unwrap();
    fs::write(
        directory.join("Makefile"),
        // Only hello.f exists; hello.o and hello.x are never named.
        "all: hello.z\n\
         %.z: %.x\n\
         \t@echo z from $<\n\
         %.x: %.o\n\
         \t@echo x from $<\n\
         %.o: %.f\n\
         \t@echo o from $<\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-r")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Each link ran, and was handed what the link below it made.
    for expected in ["o from hello.f", "x from hello.o", "z from hello.x"] {
        assert!(
            stdout.contains(expected),
            "{expected} missing from {stdout}"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

/// `build_options` starts at `JobLimit::Auto`, meaning nothing was asked for;
/// `normalize_runtime_options` resolves that to one job. Reading only the first
/// half says Make mode defaults to parallel, which it does not.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_runs_one_recipe_at_a_time_unless_asked_otherwise() {
    let directory = test_directory("make-serial-default");
    fs::create_dir_all(&directory).unwrap();
    // Descending sleeps: serially these finish a, b, c; at once, c, b, a.
    fs::write(
        directory.join("Makefile"),
        "all: a b c\n\
         a: ; @sleep 0.3; echo a\n\
         b: ; @sleep 0.2; echo b\n\
         c: ; @sleep 0.1; echo c\n",
    )
    .unwrap();

    let recipes = |arguments: &[&str]| {
        let output = make_command(&invoked_as(&directory, "make"), &directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| ["a", "b", "c"].contains(line))
            .collect::<Vec<_>>()
            .join("")
    };

    assert_eq!(recipes(&[]), "abc");
    // Without this a build incapable of parallelism would also pass.
    assert_eq!(recipes(&["-j4"]), "cba");
    fs::remove_dir_all(directory).unwrap();
}

/// `.NOTPARALLEL` serialises this Makefile's own recipes and leaves what it
/// hands a sub-make alone. Every CMake-generated Makefile declares it at the
/// top and still expects the levels below to run wide.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_serialises_only_the_makefile_that_declared_notparallel() {
    const UNITS: usize = 6;
    const JOBS: usize = 4;

    let directory = test_directory("make-notparallel");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let units = (0..UNITS)
        .map(|unit| format!("u{unit}"))
        .collect::<Vec<_>>()
        .join(" ");
    let work = format!(
        "all: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
        stamp.display()
    );
    fs::write(directory.join("work.mk"), &work).unwrap();
    fs::write(
        directory.join("recurse.mk"),
        ".NOTPARALLEL:\nall:\n\t@$(MAKE) -f work.mk all\n.PHONY: all\n",
    )
    .unwrap();
    fs::write(directory.join("flat.mk"), format!(".NOTPARALLEL:\n{work}")).unwrap();

    let program = invoked_as(&directory, "make");
    let measure = |makefile: &str| {
        let _ = fs::remove_file(&log);
        let output = make_command(&program, &directory)
            .args([&format!("-j{JOBS}"), "-f", makefile])
            .env("LOG", &log)
            .env("TMPDIR", &served)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let (peak, ran) = peak_concurrency(&fs::read_to_string(&log).unwrap());
        assert_eq!(ran, UNITS);
        peak
    };

    assert_eq!(
        measure("flat.mk"),
        1,
        "the declaring Makefile ran in parallel"
    );
    // `.NOTPARALLEL` became a pool local to the declaring compilation unit; it
    // did not replace the root scheduler limit inherited by the composed child.
    assert_eq!(
        measure("recurse.mk"),
        JOBS,
        "the sub-make lost the budget its parent only declined for itself"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// Without `.ONESHELL` each recipe line is isolated; with it they share one
/// shell, so a `cd` carries and a failing line does not stop the rest.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_shares_one_shell_across_a_recipe_only_under_oneshell() {
    let directory = test_directory("make-oneshell");
    fs::create_dir_all(&directory).unwrap();
    let recipe = "all:\n\
                  \t@V=set\n\
                  \t@false\n\
                  \t@echo \"[$$V]\"\n";
    let run = |makefile: &str| {
        fs::write(directory.join("Makefile"), makefile).unwrap();
        let output = make_command(&invoked_as(&directory, "make"), &directory)
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };

    let (ok, reported) = run(recipe);
    assert!(!ok, "the false should have stopped the recipe: {reported}");

    let (ok, reported) = run(&format!(".ONESHELL:\n{recipe}"));
    assert!(ok, "{reported}");
    assert!(reported.contains("[set]"), "{reported}");
    fs::remove_dir_all(directory).unwrap();
}

/// `-R` withholds the tool defaults and implies `-r`, but leaves what Make
/// defines about itself.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_withholds_the_builtin_variables_under_dash_r() {
    let directory = test_directory("make-no-builtin-vars");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: ; @echo \"[$(CC)][$(AR)][$(MAKE_VERSION)][$(MAKEFLAGS)]\"\n",
    )
    .unwrap();
    let run = |arguments: &[&str]| {
        let output = make_command(&invoked_as(&directory, "make"), &directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    assert!(run(&[]).contains("[cc][ar]"), "{}", run(&[]));
    // GNU Make's own answers: the defaults gone, its own version still there,
    // and both letters handed to a child because -R is -r and more.
    for spelling in [["-R"].as_slice(), ["--no-builtin-variables"].as_slice()] {
        let reported = run(spelling);
        assert!(reported.contains("[][][4.4.1][rR]"), "{reported}");
    }
    fs::remove_dir_all(directory).unwrap();
}

/// `+=` on a target reads the target's own scope, not the one outside it.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_appends_to_a_target_variable_from_the_targets_own_scope() {
    let directory = test_directory("make-targetvar-append");
    fs::create_dir_all(&directory).unwrap();
    // The append is written above the assignment it reads, so getting this
    // right cannot be a matter of taking them in the order they appear.
    fs::write(
        directory.join("Makefile"),
        "A = start\n\
         Z = outer\n\
         all: A += $(Z)\n\
         all: Z = inner\n\
         all: ; @echo \"[$(A)]\"\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[start inner]"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// The second reads the first, so their order decides the answer.
// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_applies_target_specific_variables_in_a_settled_order() {
    let directory = test_directory("make-targetvar-order");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "BLAH := foo\n\
         COMMAND = echo $(BLAH)\n\
         all: ; @$(COMMAND)\n\
         all: BLAH := bar\n\
         all: COMMAND += snafu $(BLAH)\n",
    )
    .unwrap();

    // Repeated: the failure was a coin flip on the per-process hash seed.
    for _ in 0..8 {
        let output = make_command(&invoked_as(&directory, "make"), &directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("bar snafu bar"), "{stdout}");
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.semantics+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_claims_only_the_features_it_has() {
    let directory = test_directory("make-features");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        // target-specific: the assignment reaches only this target's recipe.
        // else-if: the second branch is the one taken.
        // shortest-stem: `%.o: %.c` beats `%: %.c` for `x.o`.
        // order-only: `dep` is built, and is not a reason to rebuild.
        "all: x.o | order\n\
         \t@echo who=$(WHO) branch=$(BRANCH)\n\
         \t@echo features=$(.FEATURES)\n\
         all: WHO = specific\n\
         WHO = global\n\
         ifeq (1,2)\n\
         BRANCH = first\n\
         else ifeq (1,1)\n\
         BRANCH = second\n\
         else\n\
         BRANCH = third\n\
         endif\n\
         x.c:;@touch x.c\n\
         %.o: %.c;@echo stem=long\n\
         %: %.c;@echo stem=short\n\
         order:;@echo order=built\n\
         .PHONY: all order\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reported = String::from_utf8_lossy(&output.stdout);

    assert!(reported.contains("stem=long"), "shortest-stem: {reported}");
    assert!(reported.contains("order=built"), "order-only: {reported}");
    assert!(
        reported.contains("who=specific branch=second"),
        "target-specific and else-if: {reported}"
    );

    // Nothing is claimed that the cases above do not cover.
    let claimed = reported
        .lines()
        .find_map(|line| line.strip_prefix("features="))
        .expect("the feature list");
    let mut claimed = claimed.split_whitespace().collect::<Vec<_>>();
    claimed.sort_unstable();
    assert_eq!(
        claimed,
        [
            "else-if",
            "jobserver",
            "jobserver-fifo",
            "order-only",
            "output-sync",
            "shortest-stem",
            "target-specific",
        ]
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.interface-compatibility/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_passes_linux_output_sync_guard() {
    let directory = make_case(
        "make-linux-output-sync-guard",
        "ifeq ($(filter output-sync,$(.FEATURES)),)\n\
         $(error GNU Make >= 4.0 is required. Your Make version is $(MAKE_VERSION))\n\
         endif\n\
         all:;@echo linux-guard-passed\n\
         .PHONY: all\n",
    );
    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("linux-guard-passed"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A Makefile write is visible immediately, controls this unit's scheduler,
/// and reaches a semantic child as canonical switches rather than extra goals.
// [spec:ronin:req:make.semantics+1/test]
// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn assigned_makeflags_control_build_and_children() {
    let directory = make_case(
        "makefile-assigned-makeflags",
        "MAKEFLAGS += -rR\n\
         MAKEFLAGS += -k\n\
         $(file >root.flags,MAKEFLAGS=$(MAKEFLAGS) MFLAGS=$(MFLAGS))\n\
         all: failing continued child\n\
         failing:;@false\n\
         continued:;@touch $@\n\
         child:;@$(MAKE) --no-print-directory -f child.mk child-output\n\
         .PHONY: all failing child\n",
    );
    fs::write(
        directory.join("child.mk"),
        "$(file >child.flags,GOALS=$(MAKECMDGOALS) MAKEFLAGS=$(MAKEFLAGS))\n\
         child-output:;@touch $@\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{said}");
    assert!(
        directory.join("continued").exists(),
        "-k was not applied: {said}"
    );
    assert!(
        directory.join("child-output").exists(),
        "child did not run: {said}"
    );
    assert_eq!(
        fs::read_to_string(directory.join("root.flags")).unwrap(),
        "MAKEFLAGS=krR MFLAGS=-krR\n"
    );
    assert_eq!(
        fs::read_to_string(directory.join("child.flags")).unwrap(),
        "GOALS=child-output MAKEFLAGS=krR --no-print-directory\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.narration/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_mode_synchronizes_each_target_output() {
    let directory = make_case(
        "make-output-sync",
        "all: left right\n\
         left:\n\
         \t@touch left.ready; while test ! -e right.ready; do sleep 0.01; done; printf 'left-1\\n'; sleep 0.05; printf 'left-2\\n'\n\
         right:\n\
         \t@touch right.ready; while test ! -e left.ready; do sleep 0.01; done; printf 'right-1\\n'; sleep 0.05; printf 'right-2\\n'\n\
         .PHONY: all left right\n",
    );
    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-j2")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let left_first = stdout.find("left-1").expect("left output begins");
    let left_last = stdout.find("left-2").expect("left output ends");
    let right_first = stdout.find("right-1").expect("right output begins");
    let right_last = stdout.find("right-2").expect("right output ends");
    assert!(
        left_last < right_first || right_last < left_first,
        "target output was interleaved:\n{stdout}"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// Run the binary under a name of our choosing, which is what selects the front
/// end. A symlink is how a multi-call binary is installed, so it is how this is
/// tested.
#[cfg(all(unix, feature = "make"))]
fn invoked_as(directory: &std::path::Path, name: &str) -> PathBuf {
    let link = directory.join(name);
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
    link
}

#[cfg(all(unix, feature = "make"))]
fn make_command(program: &std::path::Path, directory: &std::path::Path) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL");
    command
}

// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn the_invoked_name_selects_make_mode_and_builds_without_a_manifest() {
    let directory = makefile_tree("make-mode");
    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reported = String::from_utf8_lossy(&output.stdout);
    assert!(
        reported.contains(&format!("version={}", ronin::make::MAKE_VERSION)),
        "the version a Makefile can branch on is the one Make mode claims: {reported}"
    );
    assert!(reported.contains("level=0 exported=1"), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("out.txt")).unwrap(),
        "source\n"
    );
    // A Makefile becomes a graph, not a manifest: nothing on the way to the
    // build is written down.
    assert!(!directory.join("build.ninja").exists());

    // The other name, and the only other one.
    fs::remove_file(directory.join("out.txt")).unwrap();
    let output = make_command(&invoked_as(&directory, "gmake"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(directory.join("out.txt").exists());
    assert!(!directory.join("build.ninja").exists());
    fs::remove_dir_all(directory).unwrap();
}

/// Ninja mode is what every other name selects, including the one a build
/// directory is most likely to hold: the binary under its own name.
// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn ninja_mode_is_reachable_from_a_ninja_named_invocation() {
    let directory = test_directory("ninja-named-ninja-mode");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("in"), "manifest\n").unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule copy\n  command = cp $in $out\nbuild out: copy in\ndefault out\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "ninja"), &directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("out")).unwrap(),
        "manifest\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_options_reach_the_scheduler_and_the_evaluation() {
    let directory = makefile_tree("make-options");

    // A command-line assignment outranks the makefile's own, which is what
    // makes it a command-line variable rather than an environment variable:
    // the environment loses to the makefile and this does not.
    let assigned = make_command(&invoked_as(&directory, "make"), &directory)
        .args(["WHERE=command-line"])
        .env("WHERE", "environment")
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&assigned.stdout);
    assert!(reported.contains("where=command-line"), "{reported}");

    // -n reports without running, so the second build still has work to do.
    fs::remove_file(directory.join("out.txt")).unwrap();
    let dry = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-n")
        .output()
        .unwrap();
    assert!(dry.status.success());
    assert!(!directory.join("out.txt").exists());

    // -C enters each directory in turn, as Make does, rather than replacing
    // one with the next as Ninja does.
    fs::create_dir_all(directory.join("sub/deeper")).unwrap();
    fs::write(
        directory.join("sub/deeper/Makefile"),
        "all:\n\t@echo deep\n.PHONY: all\n",
    )
    .unwrap();
    let entered = make_command(&invoked_as(&directory, "make"), &directory)
        .args(["-C", "sub", "-C", "deeper"])
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&entered.stdout);
    assert!(entered.status.success(), "{reported}");
    assert!(reported.contains("deep"), "{reported}");

    // A goal names what to build, and a goal nothing declares is refused.
    let unknown = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("nothing-declares-this")
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    fs::remove_dir_all(directory).unwrap();
}

/// A Makefile that records each recipe that ran, so a build's effect can be
/// read back rather than inferred from what the scheduler printed.
#[cfg(all(unix, feature = "make"))]
fn make_case(label: &str, makefile: &str) -> PathBuf {
    let directory = test_directory(label);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("Makefile"), makefile).unwrap();
    directory
}

// [spec:ronin:req:make.interface-compatibility/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn always_make_is_interface_noop() {
    let directory = make_case(
        "make-always-make",
        "out.txt: in.txt\n\tcat in.txt >> out.txt\n",
    );
    fs::write(directory.join("in.txt"), "line\n").unwrap();
    let make = invoked_as(&directory, "make");
    let run = |arguments: &[&str]| {
        let output = make_command(&make, &directory).args(arguments).output();
        let output = output.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&[]);
    assert_eq!(
        fs::read_to_string(directory.join("out.txt")).unwrap(),
        "line\n"
    );

    // Runner-only flags are accepted but leave Ninja dirtiness unchanged.
    run(&[]);
    assert_eq!(
        fs::read_to_string(directory.join("out.txt")).unwrap(),
        "line\n"
    );
    run(&["-B"]);
    assert_eq!(
        fs::read_to_string(directory.join("out.txt")).unwrap(),
        "line\n"
    );
    run(&["--always-make"]);
    assert_eq!(
        fs::read_to_string(directory.join("out.txt")).unwrap(),
        "line\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.question-status/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn question_mode_answers_in_the_status_and_builds_nothing() {
    let directory = make_case("make-question", "out.txt: in.txt\n\tcp in.txt out.txt\n");
    fs::write(directory.join("in.txt"), "source\n").unwrap();
    let make = invoked_as(&directory, "make");
    let ask = |arguments: &[&str]| {
        let output = make_command(&make, &directory).args(arguments).output();
        let output = output.unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };

    // Something would have to run, and the question does not run it.
    let (code, said) = ask(&["-q"]);
    assert_eq!(code, Some(1), "{said}");
    assert!(said.is_empty(), "{said}");
    assert!(!directory.join("out.txt").exists());

    assert_eq!(ask(&[]).0, Some(0));
    // Now it is up to date, in both spellings.
    assert_eq!(ask(&["-q"]).0, Some(0));
    assert_eq!(ask(&["--question"]).0, Some(0));
    // An accepted runner no-op does not change Ninja's answer.
    assert_eq!(ask(&["-q", "-B"]).0, Some(0));

    // A question that cannot be answered is neither of the two answers.
    let (code, said) = ask(&["-q", "nothing-declares-this"]);
    assert_eq!(code, Some(2), "{said}");
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn ignore_errors_runs_the_rest_of_the_recipe() {
    let directory = make_case(
        "make-ignore-errors",
        "all:\n\tfalse\n\techo ran > ran.txt\n.PHONY: all\n",
    );
    let make = invoked_as(&directory, "make");
    let stopped = make_command(&make, &directory).output().unwrap();
    assert!(!stopped.status.success());
    assert!(!directory.join("ran.txt").exists());

    for spelling in ["-i", "--ignore-errors"] {
        let _ = fs::remove_file(directory.join("ran.txt"));
        let ignored = make_command(&make, &directory)
            .arg(spelling)
            .output()
            .unwrap();
        assert!(
            ignored.status.success(),
            "{spelling}: {}",
            String::from_utf8_lossy(&ignored.stderr)
        );
        assert_eq!(
            fs::read_to_string(directory.join("ran.txt")).unwrap(),
            "ran\n"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn environment_overrides_outrank_the_makefiles_own_assignment() {
    let directory = make_case(
        "make-environment-overrides",
        "WHERE = makefile\nall:\n\t@echo where=$(WHERE)\n.PHONY: all\n",
    );
    let make = invoked_as(&directory, "make");
    let where_is = |arguments: &[&str]| {
        let output = make_command(&make, &directory)
            .args(arguments)
            .env("WHERE", "environment")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    // Make's ordinary precedence: the makefile beats the environment.
    assert!(where_is(&[]).contains("where=makefile"));
    for spelling in ["-e", "--environment-overrides"] {
        let said = where_is(&[spelling]);
        assert!(said.contains("where=environment"), "{spelling}: {said}");
    }
    // A command-line assignment still outranks both, which is what -e does not
    // change.
    assert!(where_is(&["-e", "WHERE=command-line"]).contains("where=command-line"));
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn no_builtin_rules_withdraws_the_rules_nobody_wrote() {
    let directory = make_case("make-no-builtin-rules", "all: hello.o\n\t@echo linked\n");
    fs::write(directory.join("hello.c"), "int main(void){return 0;}\n").unwrap();
    let make = invoked_as(&directory, "make");
    let run = |arguments: &[&str]| {
        let _ = fs::remove_file(directory.join("hello.o"));
        make_command(&make, &directory)
            .args(arguments)
            .output()
            .unwrap()
    };

    // The built-in .c.o rule is the only thing that knows how to make hello.o.
    let built = run(&[]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(directory.join("hello.o").exists());

    for spelling in ["-r", "--no-builtin-rules"] {
        let refused = run(&[spelling]);
        assert!(!refused.status.success(), "{spelling}");
        let diagnostic = String::from_utf8_lossy(&refused.stderr);
        assert!(diagnostic.contains("hello.o"), "{spelling}: {diagnostic}");
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.narration/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_narration_flags_are_accepted_noops() {
    let directory = make_case("make-narration-noops", "all:\n\t@echo built\n.PHONY: all\n");
    let make = invoked_as(&directory, "make");
    for arguments in [
        ["-w"].as_slice(),
        ["--print-directory"].as_slice(),
        ["-Otarget"].as_slice(),
        ["--output-sync=line"].as_slice(),
        ["--debug=a"].as_slice(),
        ["--trace"].as_slice(),
    ] {
        let output = make_command(&make, &directory)
            .args(arguments)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{arguments:?}: {said}");
        assert!(said.contains("built"), "{arguments:?}: {said}");
        for make_only in [
            "Entering directory",
            "Leaving directory",
            "Reading makefiles",
            "Updating goal",
            "ronin[",
            "***",
            "Stop.",
        ] {
            assert!(!said.contains(make_only), "{arguments:?}: {said}");
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.make-identity/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn a_load_ceiling_is_read_in_every_spelling_and_a_bad_one_is_refused() {
    let directory = make_case("make-load-average", "all:\n\t@echo built\n.PHONY: all\n");
    let make = invoked_as(&directory, "make");
    let run = |arguments: &[&str]| {
        make_command(&make, &directory)
            .args(arguments)
            .output()
            .unwrap()
    };

    for spelling in [
        ["-l", "4"].as_slice(),
        ["-l4.5"].as_slice(),
        ["--load-average", "4.5"].as_slice(),
        ["--load-average=4.5"].as_slice(),
        // A bare -l lifts the ceiling rather than eating the goal after it.
        ["-l", "all"].as_slice(),
    ] {
        let output = run(spelling);
        assert!(
            output.status.success(),
            "{spelling:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("built"),
            "{spelling:?}"
        );
    }

    for spelling in ["-lnope", "--load-average=nope"] {
        let refused = run(&[spelling]);
        assert!(!refused.status.success(), "{spelling}");
        let diagnostic = String::from_utf8_lossy(&refused.stderr);
        assert!(
            diagnostic.contains("invalid -l parameter"),
            "{spelling}: {diagnostic}"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn recursive_make_compiles_as_subninja() {
    let directory = test_directory("make-subninja");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "MAKE := ./must-not-run\n\
         RECURSE = $(MAKE) -C sub -f Child.mk child FLAG='from parent'\n\
         export PARENT := seen-by-child-evaluator\n\
         export REMOVE := remove-before-child-recipe\n\
         all: ready\n\
         \t$(RECURSE)\n\
         ready:\n\
         \t@printf ready > ready-file\n\
         .PHONY: all ready\n",
    )
    .unwrap();
    fs::write(
        directory.join("sub/Child.mk"),
        "ifeq ($(PARENT),seen-by-child-evaluator)\n\
         EVALUATED := yes\n\
         else\n\
         EVALUATED := no\n\
         endif\n\
         LEVEL := $(MAKELEVEL)\n\
         export CHILD := child-recipe-environment\n\
         unexport REMOVE\n\
         child:\n\
         \t@test -f ../ready-file\n\
         \t@printf '%s' \"$(FLAG)|$(EVALUATED)|$$CHILD|$${REMOVE-unset}|$(LEVEL)|$$MAKELEVEL\" > result\n\
         .PHONY: child\n",
    )
    .unwrap();
    fs::write(
        directory.join("must-not-run"),
        "#!/bin/sh\ntouch nested-make-ran\nexit 99\n",
    )
    .unwrap();
    std::fs::set_permissions(
        directory.join("must-not-run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("sub/result")).unwrap(),
        "from parent|yes|child-recipe-environment|unset|1|2"
    );
    assert!(!directory.join("nested-make-ran").exists(), "{reported}");
    assert!(
        !directory.join("sub/nested-make-ran").exists(),
        "{reported}"
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn recursive_subtree_waits_for_parent_inputs() {
    let directory = test_directory("make-recursive-prerequisite-boundary");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all: prepare\n\
         \t+$(MAKE) -f first.mk first\n\
         \t+$(MAKE) -f second.mk second\n\
         prepare:\n\
         \t@sleep 0.1\n\
         \t@touch ready\n\
         .PHONY: all prepare\n",
    )
    .unwrap();
    fs::write(
        directory.join("first.mk"),
        "first: leaf\n\
         leaf:\n\
         \t@test -e ready\n\
         \t@touch first.leaf\n\
         .PHONY: first leaf\n",
    )
    .unwrap();
    fs::write(
        directory.join("second.mk"),
        "second: leaf\n\
         leaf:\n\
         \t@test -e first.leaf\n\
         \t@touch second.leaf\n\
         .PHONY: second leaf\n",
    )
    .unwrap();

    let program = invoked_as(&directory, "make");
    for attempt in 0..8 {
        for output in ["ready", "first.leaf", "second.leaf"] {
            let _ = fs::remove_file(directory.join(output));
        }
        let output = make_command(&program, &directory)
            .arg("-j16")
            .output()
            .unwrap();
        let reported = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "attempt {attempt}: {reported}");
        assert!(directory.join("second.leaf").is_file(), "{reported}");
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.semantics+1/test]
// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn reassigned_export_reaches_grandchild() {
    let directory = test_directory("make-inherited-export-reassignment");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "export ROOT := inherited-before-child\n\
         all: ; +$(MAKE) -f one.mk\n\
         .PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("one.mk"),
        "ROOT := .\n\
         all: ; +$(MAKE) -f two.mk\n\
         .PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("two.mk"),
        "include $(ROOT)/included.mk\n\
         all: ; @printf '%s\\n' 'ROOT=$(ROOT) VALUE=$(VALUE)' \"RAW=$$RAW SHELL=$$SHELL\" > result\n\
         .PHONY: all\n",
    )
    .unwrap();
    fs::write(directory.join("included.mk"), "VALUE := inherited\n").unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .env("RAW", "$(EXPANDED)")
        .env("EXPANDED", "must-stay-raw")
        .env("SHELL", "/caller/shell")
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("result")).unwrap(),
        "ROOT=. VALUE=inherited\nRAW=$(EXPANDED) SHELL=/caller/shell\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn mixed_recipe_composes_subninjas() {
    let directory = test_directory("make-mixed-subninjas");
    fs::create_dir_all(directory.join("first")).unwrap();
    fs::create_dir_all(directory.join("second")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "MAKE := ./must-not-run\n\
         all:\n\
         \t@printf ordinary > ordinary-result\n\
         \t$(MAKE) -C first all\n\
         \t$(MAKE) -C second all\n\
         \t@test -f first/result\n\
         \t@test -f second/result\n\
         \t@printf complete > result\n\
         .PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("first/Makefile"),
        "all:\n\t@printf first > result\n.PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("second/Makefile"),
        "all:\n\
         \t@test -f ../first/result\n\
         \t@printf second > result\n\
         .PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("must-not-run"),
        "#!/bin/sh\ntouch nested-make-ran\nexit 99\n",
    )
    .unwrap();
    std::fs::set_permissions(
        directory.join("must-not-run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert_eq!(
        fs::read_to_string(directory.join("first/result")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(directory.join("second/result")).unwrap(),
        "second"
    );
    assert_eq!(
        fs::read_to_string(directory.join("ordinary-result")).unwrap(),
        "ordinary"
    );
    assert_eq!(
        fs::read_to_string(directory.join("result")).unwrap(),
        "complete"
    );
    assert!(!directory.join("nested-make-ran").exists(), "{reported}");
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn recursive_targets_are_invocation_local() {
    let directory = make_case(
        "make-recursive-target-namespace",
        "all: one two\n\
         one: ; +$(MAKE) -f one.mk\n\
         two: ; +$(MAKE) -f two.mk\n\
         .PHONY: all one two\n",
    );
    fs::write(
        directory.join("one.mk"),
        "all: one.out\none.out: FORCE ; @touch $@\nFORCE:\n",
    )
    .unwrap();
    fs::write(
        directory.join("two.mk"),
        "all: two.out\ntwo.out: FORCE ; @touch $@\nFORCE:\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-j2")
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{reported}");
    assert!(directory.join("one.out").is_file(), "{reported}");
    assert!(directory.join("two.out").is_file(), "{reported}");
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn unsplittable_submake_never_executes() {
    let directory = test_directory("make-unsplittable-subninja");
    fs::create_dir_all(directory.join("first")).unwrap();
    fs::create_dir_all(directory.join("second")).unwrap();
    fs::create_dir_all(directory.join("third")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "MAKE := ./must-not-run\n\
         all:\n\
         \t$(MAKE) -C first all\n\
         \t$(MAKE) -C second all && $(MAKE) -C third all\n\
         \t@touch residual-ran\n\
         .PHONY: all\n",
    )
    .unwrap();
    for child in ["first", "second", "third"] {
        fs::write(
            directory.join(child).join("Makefile"),
            "all:\n\t@touch result\n.PHONY: all\n",
        )
        .unwrap();
    }
    fs::write(
        directory.join("must-not-run"),
        "#!/bin/sh\ntouch nested-make-ran\nexit 99\n",
    )
    .unwrap();
    std::fs::set_permissions(
        directory.join("must-not-run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let reported = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{reported}");
    assert!(
        reported.contains("recursive Make recipe cannot compile as subninja"),
        "{reported}"
    );
    assert!(!directory.join("nested-make-ran").exists(), "{reported}");
    assert!(!directory.join("residual-ran").exists(), "{reported}");
    for child in ["first", "second", "third"] {
        assert!(!directory.join(child).join("result").exists(), "{reported}");
    }
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_reference_as_data_stays_recipe() {
    let directory = test_directory("make-reference-data");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Makefile"),
        "MAKE := ./not-an-invocation\n\
         all:\n\
         \t@printf '%s' '$(MAKE)' > result\n\
         .PHONY: all\n",
    )
    .unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("result")).unwrap(),
        "./not-an-invocation"
    );
    fs::remove_dir_all(directory).unwrap();
}

/// A composed child uses the same Ninja narrator as every other edge; there is
/// no recursive Make reporter left to install directory banners around it.
// [spec:ronin:req:make.narration/test]
// [spec:ronin:req:make.recursive-invocation+1/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn recursive_make_uses_ninja_narration() {
    let directory = test_directory("make-recursive-narration");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\t@cd sub && $(MAKE) all\n.PHONY: all\n",
    )
    .unwrap();
    fs::write(
        directory.join("sub/Makefile"),
        "all:\n\t@echo bottom\n.PHONY: all\n",
    )
    .unwrap();
    let invocations: &[&[&str]] = &[&[], &["-w"]];
    for arguments in invocations {
        let output = make_command(&invoked_as(&directory, "make"), &directory)
            .args(*arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let said = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(said.contains("bottom"), "{arguments:?}: {said}");
        assert!(
            !said.contains("Entering directory"),
            "{arguments:?}: {said}"
        );
        assert!(!said.contains("Leaving directory"), "{arguments:?}: {said}");
        assert!(!said.contains("ronin["), "{arguments:?}: {said}");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(unix, feature = "make"))]
// [spec:ronin:req:make.jobserver+1/test]
// [spec:ronin:req:make.recursive-invocation+1/test]
#[test]
fn recursive_make_tree_uses_one_budget() {
    const LEVELS: [&str; 3] = ["a", "b", "c"];
    const UNITS: usize = 6;
    const BUDGETS: [usize; 3] = [1, 2, 4];

    let directory = test_directory("make-recursive-budget");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nset -- \"$TMPDIR\"/ronin-jobserver-*\n[ ! -e \"$1\" ] || printf 'JOBSERVER\\n' >> \"$LOG\"\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The levels nest and only the deepest one has work, which is the shape a
    // generated build has. Levels side by side would not measure the budget's
    // reach: every level owns one implicit slot, so a tree whose work sits one
    // hop down runs `-j` recipes whether or not the budget arrived at all.
    // Nothing tells any level how many jobs it may run.
    let tree = |prefix: &str, recurse: &str| {
        let (deepest, delegating) = LEVELS.split_last().expect("the tree has levels");
        for (index, level) in delegating.iter().enumerate() {
            let next = LEVELS[index + 1];
            fs::write(
                directory.join(format!("{prefix}{level}.mk")),
                format!("all:\n\t@{recurse} -f {prefix}{next}.mk all\n.PHONY: all\n"),
            )
            .unwrap();
        }
        let units = (0..UNITS)
            .map(|unit| format!("{deepest}{unit}"))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            directory.join(format!("{prefix}{deepest}.mk")),
            format!(
                "all: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
                stamp.display()
            ),
        )
        .unwrap();
        format!(
            "all:\n\t@{recurse} -f {prefix}{}.mk all\n.PHONY: all\n",
            LEVELS[0]
        )
    };
    fs::write(directory.join("Makefile"), tree("", "$(MAKE)")).unwrap();

    let program = invoked_as(&directory, "make");
    let measure = |jobs: usize| {
        let _ = fs::remove_file(&log);
        let output = make_command(&program, &directory)
            .arg(format!("-j{jobs}"))
            .env("LOG", &log)
            .env("TMPDIR", &served)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events = fs::read_to_string(&log).unwrap();
        assert!(
            !events.lines().any(|line| line == "JOBSERVER"),
            "Make mode created a recursive GNU Make jobserver"
        );
        let (peak, units) = peak_concurrency(&events);
        assert_eq!(units, UNITS);
        peak
    };

    // Exactly `-j`, not at most it. The ceiling alone is met by a tree that
    // runs one recipe at a time, which is what a budget reaching nobody looks
    // like; the whole point of sharing it is that it is also spent.
    for jobs in BUDGETS {
        let peak = measure(jobs);
        assert_eq!(
            peak, jobs,
            "-j{jobs} ran {peak} recipes of a recursive Makefile tree at once"
        );
    }
    assert_eq!(fs::read_dir(&served).unwrap().count(), 0);
    fs::remove_dir_all(directory).unwrap();
}

/// A child `-j` remains accepted interface data; it cannot create another
/// scheduler inside the graph the parent already owns.
#[cfg(all(unix, feature = "make"))]
// [spec:ronin:req:make.jobserver+1/test]
#[test]
fn child_jobs_keep_one_scheduler() {
    const LEVELS: [&str; 3] = ["a", "b", "c"];
    const UNITS: usize = 6;
    /// Different from the root limit, so two schedulers are distinguishable.
    const FORCED: usize = 4;

    let directory = test_directory("make-forced-budget");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The forcing level is the first, and the work is two hops below it, so
    // what is measured is the budget reaching the bottom rather than the
    // forcing level spending its own implicit slot.
    fs::write(
        directory.join("Makefile"),
        format!(
            "all:\n\t@$(MAKE) -j{FORCED} -f {}.mk all\n.PHONY: all\n",
            LEVELS[0]
        ),
    )
    .unwrap();
    let (deepest, delegating) = LEVELS.split_last().expect("the tree has levels");
    for (index, level) in delegating.iter().enumerate() {
        fs::write(
            directory.join(format!("{level}.mk")),
            format!(
                "all:\n\t@$(MAKE) -f {}.mk all\n.PHONY: all\n",
                LEVELS[index + 1]
            ),
        )
        .unwrap();
    }
    let units = (0..UNITS)
        .map(|unit| format!("{deepest}{unit}"))
        .collect::<Vec<_>>()
        .join(" ");
    fs::write(
        directory.join(format!("{deepest}.mk")),
        format!(
            "all: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
            stamp.display()
        ),
    )
    .unwrap();

    let program = invoked_as(&directory, "make");
    let output = make_command(&program, &directory)
        .arg("-j2")
        .env("LOG", &log)
        .env("TMPDIR", &served)
        .output()
        .unwrap();
    // Both streams: the forcing level is a recipe, and a recipe's diagnostics
    // reach the run through the captured output its parent replays.
    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{said}");
    let (peak, ran) = peak_concurrency(&fs::read_to_string(&log).unwrap());
    assert_eq!(ran, UNITS);
    // The child spelling did not replace the root graph's one scheduler.
    assert_eq!(
        peak, 2,
        "child -j{FORCED} split a root -j2 graph into another scheduler ({peak})"
    );
    assert!(!said.contains("resetting jobserver mode"), "{said}");

    assert_eq!(fs::read_dir(&served).unwrap().count(), 0);
    fs::remove_dir_all(directory).unwrap();
}

/// A Makefile-compiled graph fails in the same shape as a manifest graph.
// [spec:ronin:req:make.narration/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_recipe_failure_uses_ninja_narration() {
    let directory = test_directory("make-failure-line");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(
        directory.join("Makefile"),
        "all:\n\t@$(MAKE) --no-print-directory -C sub\n",
    )
    .unwrap();
    fs::write(directory.join("sub").join("Makefile"), "all:\n\t@false\n").unwrap();

    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{said}");
    assert!(said.contains("FAILED:"), "{said}");
    assert!(said.contains("build stopped: subcommand failed."), "{said}");
    for make_only in ["***", "Stop.", "ronin["] {
        assert!(!said.contains(make_only), "{said}");
    }
    fs::remove_dir_all(directory).unwrap();
}

/// An I/O failure reading a Makefile names the file and the line that asked
/// for it, in the system's own words.
///
/// Three shapes, each of which used to reach the user as a bare `io::Error`
/// and nothing else — `Permission denied (os error 13)`, with no path, no
/// line, and Rust's spelling of an errno that neither front end uses.
// [spec:ronin:req:make.narration/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_io_failures_name_their_source() {
    use std::os::unix::fs::PermissionsExt;

    let unreadable = |directory: &std::path::Path, name: &str, contents: &str| {
        let path = directory.join(name);
        fs::write(&path, contents).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    };
    let diagnostic_of = |directory: &std::path::Path| {
        let output = make_command(&invoked_as(directory, "make"), directory)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(2), "{said}");
        // Rust's rendering of an errno is nobody's wording: GNU Make quotes
        // `strerror` and so does the manifest front end.
        assert!(!said.contains("(os error"), "{said}");
        said
    };

    // An `include` of a file that will not open: the line of the directive.
    let directory = make_case("make-io-include", "include inc.mk\nall:;@echo hi\n");
    unreadable(&directory, "inc.mk", "FOO := foo\n");
    let said = diagnostic_of(&directory);
    assert!(
        said.contains("Makefile:1: inc.mk: Permission denied"),
        "{said}"
    );
    fs::remove_dir_all(&directory).unwrap();

    // `$(file >)` onto a file that will not open: the line of the expansion.
    let directory = make_case("make-io-file-write", "all:;@echo hi\n$(file >out.txt,x)\n");
    unreadable(&directory, "out.txt", "");
    let said = diagnostic_of(&directory);
    assert!(
        said.contains("Makefile:2: open: out.txt: Permission denied"),
        "{said}"
    );
    fs::remove_dir_all(&directory).unwrap();

    // The Makefile itself. No directive asked for it, so there is no line to
    // point at, and it is still named with the system's own reason.
    let directory = make_case("make-io-makefile", "all:;@echo hi\n");
    unreadable(&directory, "Makefile", "all:;@echo hi\n");
    let said = diagnostic_of(&directory);
    assert!(said.contains("Makefile: Permission denied"), "{said}");
    fs::remove_dir_all(&directory).unwrap();

    // `-include` is a Makefile saying it does not care whether the file is
    // there or readable, and GNU Make 4.4.1 reports neither. Verified side by
    // side rather than read off the manual, which covers only absence.
    let directory = make_case(
        "make-io-optional-include",
        "-include inc.mk\nall:;@echo hi\n",
    );
    unreadable(&directory, "inc.mk", "FOO := foo\n");
    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(!said.contains("inc.mk"), "{said}");
    fs::remove_dir_all(&directory).unwrap();
}

/// Compiler diagnostics keep their Makefile source without borrowing GNU
/// Make's fatal-error decorations.
// [spec:ronin:req:make.narration/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn make_evaluation_uses_ordinary_diagnostics() {
    let directory = make_case("make-evaluation-diagnostic", "$(error broken)\n");
    let output = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{diagnostic}");
    assert!(diagnostic.starts_with("ronin: Makefile:1:"), "{diagnostic}");
    assert!(diagnostic.contains("broken"), "{diagnostic}");
    for make_only in ["***", "Stop.", "ronin["] {
        assert!(!diagnostic.contains(make_only), "{diagnostic}");
    }
    fs::remove_dir_all(directory).unwrap();
}
