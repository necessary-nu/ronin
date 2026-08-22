//! What an interrupt does to a build, in both front ends.
//!
//! Its own suite because the group outgrew `cli.rs`, and because it is one
//! subject rather than a handful of leftovers: every case here starts a build,
//! waits for it to reach a recipe, sends the process one signal, and reads back
//! a status and the files on disk. They share the readiness gate below, they
//! are the cases that must run under `scripts/sandboxed` — a signal sent from a
//! suite has a path out of the suite — and they are the ones a change to
//! `Builder`'s stop path or to the Make front end's status mapping moves.
//!
//! The two guards at the end send no signal at all. They are here because they
//! state the boundary from the other side: a recipe that reports 130 for its
//! own reasons, or dies of a signal nobody sent the build, is an ordinary
//! failure and must not be read as an interrupt.
#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[path = "support/scratch.rs"]
mod scratch_directory;

use scratch_directory::Scratch;

/// A scratch directory of this test's own, which goes away with the test.
fn test_directory(label: &str) -> Scratch {
    Scratch::named(&format!("ronin-{label}-"))
}

/// A Makefile of this case's own, reached through a `make`-named symlink: the
/// invoked name is what selects the Make front end.
#[cfg(feature = "make")]
fn make_case(label: &str, makefile: &str) -> Scratch {
    let directory = test_directory(label);
    fs::write(directory.join("Makefile"), makefile).unwrap();
    directory
}

#[cfg(feature = "make")]
fn invoked_as(directory: &std::path::Path, name: &str) -> PathBuf {
    let link = directory.join(name);
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
    link
}

#[cfg(feature = "make")]
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

/// Wait for the marker a recipe writes when it starts, on wall-clock rather
/// than on a turn count.
///
/// The case below used to poll two hundred times with a 10 ms sleep between —
/// a two-second budget for spawning a freshly linked binary, reading a
/// manifest, planning, launching a shell, and having that shell reach the
/// second word of a recipe. Generous on an idle host, and not a bound at all
/// under `cargo test --workspace`, where twenty-odd test binaries run at once
/// beside a corpus harness doing a thousand subprocess-heavy cases. When the
/// budget ran out nothing was ever signalled, so the recipe's own `sleep` ran
/// to the end and the suite paid twenty-three seconds for the failure.
///
/// A deadline says what the case means: wait for as long as it takes, within
/// reason, and if the marker never comes, stop paying for the recipe and say
/// what was being waited for. The two ways of not arriving are told apart
/// because they mean different things — a build that ended before writing the
/// marker is a product defect, and a build still going without having written
/// it is a hang or a host slower than any deadline.
fn wait_for_the_recipes_marker(child: &mut std::process::Child, marker: &std::path::Path) {
    use std::time::{Duration, Instant};

    // Far above what a loaded host needs, and far below the wrapper's own
    // wall-clock ceiling, so an expiry here is a finding rather than a second
    // flapper.
    let budget = Duration::from_mins(1);
    let waited_since = Instant::now();
    while !marker.exists() {
        if let Some(status) = child.try_wait().expect("waiting on the build") {
            panic!(
                "the build ended with {status} before writing {}",
                marker.display()
            );
        }
        if waited_since.elapsed() > budget {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the build did not write {} within {budget:?}",
                marker.display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Spelled `forwards_interrupts_and_removes_partial_outputs` until this suite
/// was split off, where the file-size gate's move brought the name under the
/// function-name gate for the first time. Three plan notes quote the old
/// spelling; the case and its assertions are unchanged.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
#[test]
// [spec:ronin:req:runtime.process-supervisor-scalability/test]
fn forwards_an_interrupt_and_withdraws_output() {
    use std::os::unix::process::ExitStatusExt;

    let directory = test_directory("interrupt-forwarding");
    fs::write(
        directory.join("build.ninja"),
        "rule slow\n  command = touch $out; touch started; sleep 30\nbuild output: slow\ndefault output\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    let child_id = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let status = child.wait().unwrap();
    // Ninja exits with 130 rather than dying by the signal it caught. C samurai
    // re-raised, and Ronin followed it here until the exit-status surface was
    // measured against Ninja; the contract is Ninja's.
    assert_eq!(status.signal(), None);
    assert_eq!(status.code(), Some(ronin::INTERRUPTED_EXIT_CODE));
    assert!(!directory.join("output").exists());
}

/// A recipe that declines the interrupt is the case that tells the two halves
/// of the contract apart, and it is the deterministic form of a recipe whose
/// shell took the signal between two of the command lines it was given: either
/// way the command reaches the end of its own script after the build was cut
/// short. Upstream Ninja waits for such a recipe and then removes what it
/// wrote anyway, and never reports the edge as finished — measured. Recording
/// it instead is what left a half-written output standing behind a build that
/// exits 130.
///
/// The `trap` is what makes the window a certainty rather than a rare race:
/// the recipe cannot be killed by the signal it was sent, so what stops it is
/// the build or nothing.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
#[test]
fn an_interrupt_stops_an_outliving_recipe() {
    use std::os::unix::process::ExitStatusExt;

    let directory = test_directory("interrupt-declined");
    fs::write(
        directory.join("build.ninja"),
        "rule slow\n  \
         command = trap '' INT; touch $out; touch started; sleep 30; touch survived\n\
         build output: slow\ndefault output\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    let child_id = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let finished = child.wait_with_output().unwrap();

    assert_eq!(finished.status.signal(), None);
    assert_eq!(
        finished.status.code(),
        Some(ronin::INTERRUPTED_EXIT_CODE),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    // Nothing of the command line past the signal ran, so the build stopped the
    // recipe rather than being held by it.
    assert!(!directory.join("survived").exists());
    assert!(!directory.join("output").exists());
    // And the edge never became a finished one. The `[1/1]` line is what a
    // recorded edge says about itself, and saying it is what kept the output.
    let narration = String::from_utf8_lossy(&finished.stdout);
    assert!(!narration.contains("[1/1]"), "{narration:?}");
    // `[spec:ronin:req:compat.process-integration+2]` says such an edge is
    // neither reported as finished NOR RECORDED, and those are two different
    // places: the line above is the report, and the build log is the record. A
    // recorded edge would be treated as up to date by the next build, which is
    // the half a caller feels rather than reads. Measured against upstream
    // Ninja, which writes no log at all for this run; Ronin opens one and
    // leaves no entry in it, and both are the clause being honoured.
    let log = directory.join(".ninja_log");
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.contains("output"),
        "the interrupted edge was recorded in the build log: {recorded:?}"
    );
}

/// The other side of the recording assertion above: a build left alone DOES
/// record its edge, so that the assertion is a statement about the interrupt
/// rather than about a log Ronin never writes.
// [spec:ronin:req:compat.process-integration+2/test]
#[test]
fn a_finished_edge_is_recorded() {
    let directory = test_directory("interrupt-log-control");
    fs::write(
        directory.join("build.ninja"),
        "rule quick\n  command = touch $out\nbuild output: quick\ndefault output\n",
    )
    .unwrap();
    let finished = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .output()
        .unwrap();

    assert!(finished.status.success());
    let recorded = fs::read_to_string(directory.join(".ninja_log")).unwrap();
    assert!(recorded.contains("output"), "{recorded:?}");
}

/// GNU Make deletes the target a recipe was making when a signal cuts it short
/// and spares one the Makefile called `.PRECIOUS`. Measured against 4.4.1,
/// which prints `make: *** Deleting file 'gone'` and leaves `kept` where it is;
/// Ronin says nothing about it — Make mode narrates in the manifest front end's
/// shape — and does the same thing to the same two files.
///
/// Both recipes decline the signal, which is what makes this a statement about
/// the build rather than about the shell: with nothing to kill them, a build
/// that waits for them records two successful edges and leaves `gone` standing.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
// [spec:ronin:req:make.semantics+1/test]
#[cfg(feature = "make")]
#[test]
fn make_interrupt_spares_precious_targets() {
    use std::os::unix::process::ExitStatusExt;

    let directory = make_case(
        "make-interrupt-precious",
        ".PRECIOUS: kept\n\
         all: gone kept\n\
         gone:\n\
         \t@trap '' INT; touch $@; touch started; sleep 30\n\
         kept:\n\
         \t@trap '' INT; touch $@; sleep 30\n",
    );
    let mut child = make_command(&invoked_as(&directory, "make"), &directory)
        .arg("-j2")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    // Both recipes have to have written their target before the signal, or the
    // `.PRECIOUS` half asserts that a file nothing ever made was spared. Its
    // own target is the marker: the recipe writes it and then sleeps.
    wait_for_the_recipes_marker(&mut child, &directory.join("kept"));
    let child_id = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let finished = child.wait_with_output().unwrap();

    // Measured against GNU Make 4.4.1, which exits 130 by dying of the signal,
    // and against upstream Ninja, which exits 130 without re-raising it. Ronin
    // takes Ninja's spelling for both front ends.
    assert_eq!(finished.status.signal(), None);
    assert_eq!(
        finished.status.code(),
        Some(ronin::INTERRUPTED_EXIT_CODE),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    assert!(!directory.join("gone").exists());
    assert!(directory.join("kept").exists());
}

/// A recipe is one process per command line here, as it is in GNU Make, so
/// between one line ending and the next starting there is a moment when
/// signalling the recipe's process group reaches nothing. A build that has
/// been interrupted launches no line into that gap.
///
/// This one pins the contract rather than catching the defect it was written
/// beside: before the fix the line WAS launched and then signalled where it
/// stood, which killed it before it could leave a mark, so the files look the
/// same either way. What changed is that there is now nothing to kill.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn make_interrupt_launches_no_further_line() {
    use std::os::unix::process::ExitStatusExt;

    let directory = make_case(
        "make-interrupt-next-line",
        "out:\n\
         \t@trap '' INT; touch $@; touch started; sleep 30\n\
         \t@touch second-line-ran\n",
    );
    let mut child = make_command(&invoked_as(&directory, "make"), &directory)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    let child_id = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let finished = child.wait_with_output().unwrap();

    assert_eq!(finished.status.signal(), None);
    assert_eq!(
        finished.status.code(),
        Some(ronin::INTERRUPTED_EXIT_CODE),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    assert!(!directory.join("second-line-ran").exists());
    assert!(!directory.join("out").exists());
}

/// `SIGTERM` is where the two references part, and Ronin follows Ninja.
///
/// GNU Make 4.4.1 kills its children, deletes the target it was making, and
/// then dies of the signal it caught, so a shell reads 143. Upstream Ninja
/// exits 130 for every signal it treats as an interrupt, and
/// `[spec:ronin:req:product.build-outcome]` takes that as the contract in so
/// many words — an interrupt leaves with Ninja's 130 rather than re-raising the
/// signal, so the status says the build was cut short rather than how far it
/// had got. Measured: 143 from GNU, 130 from upstream Ninja, 130 from Ronin's
/// own manifest front end, and this case holds Make mode to the same 130 rather
/// than to GNU's number.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn make_termination_leaves_ninjas_status() {
    use std::os::unix::process::ExitStatusExt;

    let directory = make_case(
        "make-terminate-status",
        "out:\n\t@touch $@; touch started; sleep 30\n",
    );
    let mut child = make_command(&invoked_as(&directory, "make"), &directory)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    let child_id = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(child_id, rustix::process::Signal::TERM).unwrap();
    let finished = child.wait_with_output().unwrap();

    assert_eq!(finished.status.signal(), None);
    assert_eq!(
        finished.status.code(),
        Some(ronin::INTERRUPTED_EXIT_CODE),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    assert!(!directory.join("out").exists());
}

/// The number cannot say what stopped the build, so the reason is what is read.
///
/// A recipe that exits 130 of its own accord is a failed recipe and nothing
/// more, and Make mode reports it with the 2 every other failed recipe gets —
/// measured against GNU Make 4.4.1, which says the same, with and without `-k`.
/// This is the case that would catch an interrupt status inferred from the
/// build's exit code rather than from why the build stopped, and it needs no
/// signal to say so.
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn a_recipes_own_130_stays_two() {
    let directory = make_case("make-recipe-exits-130", "out:\n\t@exit 130\n");
    let make = invoked_as(&directory, "make");
    for arguments in [&[][..], &["-k"][..]] {
        let finished = make_command(&make, &directory)
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(finished.status.code(), Some(2), "{arguments:?}");
    }
}

/// A recipe killed by a signal nobody sent the build is a failure, not an
/// interrupt.
///
/// GNU Make reaps such a child and reports it like any other failure —
/// `make: *** [Makefile:2: out] Interrupt` — and leaves with 2; only a signal
/// delivered to Make itself ends the build. Ronin's Make mode says the same,
/// which is what `BuildOptions::recipe_signal_fails` is set for, and this case
/// pins the boundary from the other side: the recipe's death carries the same
/// signal number as the interrupt above and must not be read as one.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn a_signalled_recipe_abandons_with_two() {
    let directory = make_case(
        "make-recipe-signals-itself",
        // `$$$$` is one `$$` after the makefile is read and the shell's own pid
        // after that; a single `$$` here would reach the shell as a bare `$`,
        // and the recipe would fail for the wrong reason.
        "out:\n\t@touch $@; kill -INT $$$$\n",
    );
    let finished = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();

    assert_eq!(
        finished.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    // The target the dead recipe was making goes, as it does for an interrupt
    // and for every other recipe that did not finish.
    assert!(!directory.join("out").exists());
}

/// A read the user stops does not wait for the `$(shell)` that was running.
///
/// The read phase is the compiler, not the build, and it runs processes: a
/// `$(shell)` call is a command line like any other. Before this case Ronin ran
/// the shell function to its end, finished compiling, started the build and only
/// then noticed the flag the handler had set, so Ctrl-C took as long as the
/// makefile's slowest shell function — measured at 10,007 ms against GNU Make
/// 4.4.1's 8 ms for the same makefile, both numbers being the child's `sleep` to
/// the millisecond.
///
/// What GNU does with the child is ABANDON it, and that is what is reproduced
/// here rather than a kill. `fatal_error_signal` (commands.c) waits for the
/// children Make is running as jobs and knows nothing about the one a `$(shell)`
/// left behind, so it re-raises and is gone while that child runs on. Measured
/// through `scripts/sandboxed`, SIGINT to the tool alone: GNU left in 1-3 ms
/// over three rounds and its `/bin/sh` and `sleep` were still there afterwards,
/// reparented to the namespace's init and never reaped. Ronin now leaves in the
/// same 2-3 ms and leaves the same child standing.
///
/// The `trap` makes the window a certainty rather than a race, exactly as it
/// does for the recipe case above: the child cannot be killed by the signal, so
/// what stops the tool waiting for it is the fix or nothing. `survived` is the
/// teeth — the child writes it thirty seconds later, so a tool that waited would
/// have it and a tool that abandoned cannot.
///
/// Deliberately NOT `wait_with_output`: the abandoned child inherits the pipe's
/// write end, so a test that read the tool's stdout to EOF would block on the
/// orphan for the whole thirty seconds and report the wait it was written to
/// catch as a pass.
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn a_read_interrupt_abandons_its_command() {
    use std::os::unix::process::ExitStatusExt;

    let directory = make_case(
        "make-read-phase-interrupt",
        "X := $(shell trap '' INT; touch started; sleep 30; touch survived)\n\
         out:\n\t@touch $@\n",
    );
    let mut child = make_command(&invoked_as(&directory, "make"), &directory)
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    let child_id = rustix::process::Pid::from_child(&child);
    let signalled = std::time::Instant::now();
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let status = child.wait().unwrap();
    let waited = signalled.elapsed();

    assert_eq!(status.signal(), None);
    assert_eq!(status.code(), Some(ronin::INTERRUPTED_EXIT_CODE));
    // The child is still sleeping, so the read did not wait for it. Read before
    // any assertion that could leave the check unrun.
    assert!(
        !directory.join("survived").exists(),
        "the read waited for the shell function it was interrupted during"
    );
    // And the same thing said as a bound rather than as a file, far enough
    // under the child's thirty seconds to be a finding rather than a flapper.
    assert!(
        waited < std::time::Duration::from_secs(10),
        "the tool took {waited:?} to leave after the signal"
    );
    // The compile never finished, so nothing was built.
    assert!(!directory.join("out").exists());
}

/// An interrupted read starts no further shell function.
///
/// The other half of the same contract, and the one that needs no signal from
/// this suite at all: the makefile's own first `$(shell)` interrupts the tool,
/// which makes the moment the interrupt arrives a fact of the makefile rather
/// than a race against a sleep. What must not happen afterwards is the second
/// `$(shell)` running, and that is a file rather than a timing.
///
/// The first line is written the way it is so that the interrupt lands where
/// nothing is being waited for, which is the only place the evaluator's own
/// check is what stops the read. `exec >/dev/null` closes the pipe the tool is
/// reading, so the tool sees the end of the output and stops reading BEFORE the
/// signal is sent; it is then inside `wait` for the child, which the sleep holds
/// open long enough for the flag to be set. So the read of the first command
/// completes normally, and the interrupt is there to be found between one
/// statement and the next.
///
/// Measured against GNU Make 4.4.1 for this exact makefile: exit 130,
/// `second-shell-ran` absent, `out` absent, and nothing written to either
/// stream. Before the fix Ronin left the same 130 — the parent node had settled
/// that — with `second-shell-ran` PRESENT, because the read carried on to the
/// end and the interrupt was only noticed once the build started; the build
/// then narrated `ronin: build stopped: interrupted by user.`, which GNU does
/// not say here because GNU never reached a build.
// [spec:ronin:req:compat.process-integration+2/test]
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn an_interrupted_read_launches_nothing_further() {
    use std::os::unix::process::ExitStatusExt;

    let directory = make_case(
        "make-read-phase-next-shell",
        // `$$PPID` reaches the shell as `$PPID`, and the shell a `$(shell)`
        // starts is the tool's own child, so this is the tool being signalled.
        "X := $(shell echo one; exec >/dev/null; kill -INT $$PPID; sleep 0.2)\n\
         Y := $(shell touch second-shell-ran; echo two)\n\
         out:\n\t@touch $@\n",
    );
    let finished = make_command(&invoked_as(&directory, "make"), &directory)
        .output()
        .unwrap();

    assert_eq!(finished.status.signal(), None);
    assert_eq!(
        finished.status.code(),
        Some(ronin::INTERRUPTED_EXIT_CODE),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    assert!(
        !directory.join("second-shell-ran").exists(),
        "the read ran a shell function after it had been interrupted"
    );
    assert!(!directory.join("out").exists());
    // GNU says nothing here, having re-raised before it could. Ronin's build
    // narration belongs to a build, and this run never started one.
    assert_eq!(String::from_utf8_lossy(&finished.stdout), "");
    assert_eq!(String::from_utf8_lossy(&finished.stderr), "");
}

/// A read the user stops does not wait for a shell function that closed its
/// output early either.
///
/// The other half of the wait, and the half a fixed read leaves standing on its
/// own. A `$(shell)` is read to the end of its output and then WAITED for, and
/// a command that is told `exec >/dev/null` reaches the end of its output while
/// it is still running — so the read finishes at once and everything left is the
/// wait for the child. Measured before the fix: 20,013 ms of a twenty-second
/// child, against GNU Make 4.4.1's 2 ms for the same makefile, with `survived`
/// present rather than absent.
///
/// The shell has to be a real one for the case to be the one it says it is. A
/// `SHELL` spelled `/bin/sh` is stood in for by Ronin's own builtin shell,
/// which holds the pipe until the command ends and so reaches the interrupt
/// through the READ rather than through the wait; naming the same shell through
/// a symlink of this test's own takes the stand-in out and puts the case back on
/// the path it was written for. `[spec:ronin:req:product.builtin-shell]` is what
/// makes the two spellings different, and it is a Makefile-visible choice rather
/// than a trick.
// [spec:ronin:req:product.build-outcome/test]
#[cfg(feature = "make")]
#[test]
fn an_interrupt_ends_the_second_wait() {
    use std::os::unix::process::ExitStatusExt;

    let directory = test_directory("make-read-phase-wait");
    let shell = directory.join("realsh");
    std::os::unix::fs::symlink("/bin/sh", &shell).unwrap();
    fs::write(
        directory.join("Makefile"),
        format!(
            "SHELL := {}\n\
             X := $(shell exec >/dev/null 2>&1; touch started; trap '' INT; sleep 30; touch survived)\n\
             out:\n\t@touch $@\n",
            shell.display()
        ),
    )
    .unwrap();
    let mut child = make_command(&invoked_as(&directory, "make"), &directory)
        .spawn()
        .unwrap();
    wait_for_the_recipes_marker(&mut child, &directory.join("started"));
    let child_id = rustix::process::Pid::from_child(&child);
    let signalled = std::time::Instant::now();
    rustix::process::kill_process(child_id, rustix::process::Signal::INT).unwrap();
    let status = child.wait().unwrap();
    let waited = signalled.elapsed();

    assert_eq!(status.signal(), None);
    assert_eq!(status.code(), Some(ronin::INTERRUPTED_EXIT_CODE));
    assert!(
        !directory.join("survived").exists(),
        "the read waited for a shell function that had closed its output"
    );
    assert!(
        waited < std::time::Duration::from_secs(10),
        "the tool took {waited:?} to leave after the signal"
    );
    assert!(!directory.join("out").exists());
}
