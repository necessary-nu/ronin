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
    assert!(String::from_utf8_lossy(&help.stdout)
        .starts_with("usage: ronin -t commands [options] [targets]\n"));

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
    assert!(String::from_utf8_lossy(&stopped.stdout)
        .ends_with("ronin: build stopped: subcommand failed.\n"));

    // Under keep-going the last failure wins, and the reason changes because
    // the allowance was never used up.
    let kept_going = run(&["-k", "0", "-j", "1", "a", "b"]);
    assert_eq!(kept_going.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&kept_going.stdout)
        .ends_with("ronin: build stopped: cannot make progress due to previous errors.\n"));

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
// [spec:ronin:req:make.jobserver/test]
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
