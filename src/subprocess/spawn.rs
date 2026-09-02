//! Making the `clone` call somewhere other than the thread that schedules.
//!
//! Everything here exists because of one property of the system call Rust's
//! `Command::spawn` reaches. See [`SpawnPool`].

use super::ShellMode;
use std::path::PathBuf;

#[cfg(unix)]
use super::{
    COMMAND_NOT_FOUND, DirectLaunch, EXEC_FORMAT_ERROR, ProcessError, ProcessEvent, ProcessOutput,
    ProcessResult, SYSTEM_SHELL, ShellOperation, Started, direct_command, retry_with_shell,
    shell_command,
};
#[cfg(unix)]
use crate::graph::EdgeId;
#[cfg(unix)]
use crate::util::{BString, ByteSlice};
#[cfg(unix)]
use std::collections::VecDeque;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::mpsc::Sender;

/// The most threads a build will keep for launching commands.
///
/// Not a thread per child and not a thread per job: a spawner is busy only for
/// the length of one launch, so a handful of them saturate any job budget a
/// build tool is asked for. Six sustains twenty thousand launches a second
/// against the eleven thousand the busiest measured row asks for, and keeps
/// this runtime inside the eight-thread ceiling
/// `evented_supervisor_scales_without_a_thread_per_child` holds it to.
#[cfg(unix)]
pub(super) const MAX_SPAWNERS: usize = 6;

/// Everything a launch needs that every launch in a build shares.
///
/// Behind an [`Arc`] because launching does not happen on the thread that
/// schedules — see [`SpawnPool`] — so the ground a command stands on has to be
/// readable from any of them.
pub(super) struct SpawnContext {
    pub(super) working_directory: PathBuf,
    pub(super) shell: ShellMode,
    /// Variables imposed on every child, empty unless this build serves a
    /// jobserver its children are meant to draw on.
    ///
    /// Imposing nothing is not the same as imposing what is already there:
    /// with no variables set, `Command` hands the child this process's own
    /// environment block untouched, and the copy it would otherwise build per
    /// spawn never happens.
    pub(super) environment: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

#[cfg(unix)]
impl SpawnContext {
    /// Start one process, however many attempts that takes.
    ///
    /// Two of the retries are GNU Make's and one is Ninja's: a shell-free
    /// command that turns out to be a script with no `#!` line is exec'd again
    /// under an interpreter, a shell-free command that cannot be started at
    /// all is answered for here rather than handed to anyone else, and a
    /// direct launch Ronin chose for itself falls back to the shell.
    pub(super) fn start(
        &self,
        edge: EdgeId,
        command: &BString,
        direct: Option<&DirectLaunch>,
        use_console: bool,
    ) -> ProcessResult<Started> {
        use std::os::unix::process::CommandExt;

        // Held for the whole attempt, on whichever thread is making it: a
        // child inherits the process's directory, and a Make front end may be
        // moving that process between reads.
        #[cfg(feature = "make")]
        let _directory = crate::make::stable_process_directory_guard();
        let mut mode = self.shell.clone();
        // Set once a direct launch turns out to be a script with no `#!` line,
        // which is the one thing GNU Make's own exec does not simply report.
        let mut interpreter: Option<std::ffi::OsString> = None;
        loop {
            let mut shell = direct.map_or_else(
                || shell_command(command, &self.working_directory, &mode, &self.environment),
                |direct| {
                    direct_command(
                        direct,
                        interpreter.as_deref(),
                        &self.working_directory,
                        &self.environment,
                    )
                },
            );
            if !use_console {
                shell.process_group(0);
            }
            let output = configure_output(&mut shell, edge, command, use_console)?;
            match shell.spawn() {
                Ok(child) => return Ok(Started::Running(child, output)),
                // A file that is executable and is not a program: GNU Make
                // reads `ENOEXEC` as "try it as a shell script" and execs it
                // again under `$SHELL`, or `/bin/sh` where the environment
                // names none. That is how a recipe naming a script with no
                // `#!` line runs at all, and it is the only errno the exec
                // does anything about rather than report.
                Err(ref source)
                    if direct.is_some()
                        && interpreter.is_none()
                        && source.raw_os_error() == Some(EXEC_FORMAT_ERROR) =>
                {
                    interpreter =
                        Some(self.script_interpreter(
                            direct.expect("the direct launch was matched above"),
                        ));
                }
                // The one launch that answers for this itself. GNU Make execs
                // a shell-free command line in the forked child and reports
                // what the exec said against the command's own name, so the
                // recipe fails with 127 and the sentence is Make's rather than
                // a shell's — and there is no shell here to produce one.
                Err(source) if direct.is_some() => {
                    return Ok(Started::NeverStarted(failed_to_start(
                        direct.expect("the direct launch was matched above"),
                        &source,
                    )));
                }
                // A direct spawn that cannot find the program has not run
                // anything, so hand the command to the shell and let it
                // produce the diagnostic and exit status it always would.
                // Emulating that text here would pin us to one shell's wording.
                Err(source) if retry_with_shell(&mode, &source) => {
                    mode = ShellMode::Compat;
                }
                Err(source) => {
                    return Err(ProcessError::Shell {
                        edge,
                        command: command.clone(),
                        operation: ShellOperation::Spawn,
                        source,
                    });
                }
            }
        }
    }

    /// What runs a file that is executable and is not a program.
    ///
    /// GNU Make's `exec_command` reads `getenv ("SHELL")` for this and falls
    /// back to its own default — the environment's shell rather than the one
    /// the makefile set, because what is being answered is "how does this host
    /// run a script", not "what does this recipe want".
    fn script_interpreter(&self, direct: &DirectLaunch) -> std::ffi::OsString {
        let named = |environment: &[(std::ffi::OsString, Option<std::ffi::OsString>)]| {
            environment
                .iter()
                .rev()
                .find(|(name, _)| name == "SHELL")
                .map(|(_, value)| value.clone())
        };
        named(&direct.environment)
            .or_else(|| named(&self.environment))
            .unwrap_or_else(|| std::env::var_os("SHELL"))
            .unwrap_or_else(|| std::ffi::OsString::from(SYSTEM_SHELL))
    }
}

/// One command waiting to be launched, and everything launching it takes.
#[cfg(unix)]
pub(super) struct SpawnRequest {
    pub(super) edge: EdgeId,
    pub(super) command: BString,
    pub(super) direct: Option<Box<DirectLaunch>>,
    pub(super) use_console: bool,
}

#[cfg(unix)]
#[derive(Default)]
pub(super) struct SpawnQueueState {
    pub(super) waiting: VecDeque<SpawnRequest>,
    /// Spawners parked on the condition variable, so the pool can tell a burst
    /// nobody is free for from one an idle thread will take.
    pub(super) idle: usize,
    pub(super) shutdown: bool,
}

#[cfg(unix)]
#[derive(Default)]
pub(super) struct SpawnQueue {
    pub(super) state: std::sync::Mutex<SpawnQueueState>,
    pub(super) ready: std::sync::Condvar,
}

/// The threads that make the `clone` call, and the reason they exist.
///
/// Rust's `Command::spawn` reaches `posix_spawn`, which is `clone3` with
/// `CLONE_VM | CLONE_VFORK`: the calling thread is suspended until the child
/// it made reaches `execve`. That is not a cost, it is a WAIT — measured on
/// this machine at 266 microseconds a launch, of which only 68 are CPU and the
/// remaining 198 are the thread off-CPU waiting for its own child to be given
/// a processor. Ninja's `fork` costs 120 microseconds and every one of them is
/// CPU, which is why Ninja's builder thread never stops working and this one
/// did.
///
/// Charged to the one thread that also chooses edges, drains output and reaps,
/// that wait is the whole job budget: at `-j8` over 128 `touch` edges it held
/// the scheduler off-CPU for 25 milliseconds of a 51-millisecond build and
/// kept 1.9 jobs in flight out of the eight asked for. And because it is a
/// wake-up rather than work, its price is the host's run queue — which is why
/// that row's ratio against Ninja climbed with load while both binaries stood
/// still.
///
/// `CLONE_VFORK` suspends the calling THREAD, not the process, so the fix is
/// to make the call somewhere else. The scheduler hands a launch over and goes
/// back to scheduling; a spawner thread wears the suspension; the started
/// child comes back through the same channel and wake-up the jobserver's
/// tokens already use.
#[cfg(unix)]
#[derive(Default)]
pub(super) struct SpawnPool {
    pub(super) queue: Arc<SpawnQueue>,
    pub(super) workers: Vec<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Drop for SpawnPool {
    fn drop(&mut self) {
        {
            let mut state = self
                .queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.shutdown = true;
        }
        self.queue.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Take launches off the queue and make them until the pool is shut down.
///
/// A panic in here is answered the way the non-Unix runner answers one, as the
/// edge's own failure: the scheduler is waiting for exactly one reply per
/// submitted launch and a thread that died without sending it would hang the
/// build.
#[cfg(unix)]
pub(super) fn spawner_loop<External: Send + 'static>(
    queue: &Arc<SpawnQueue>,
    context: &Arc<SpawnContext>,
    sender: &Sender<ProcessEvent<External>>,
    poller: &Arc<polling::Poller>,
) {
    loop {
        let request = {
            let mut state = queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(request) = state.waiting.pop_front() {
                    break request;
                }
                state.idle += 1;
                state = queue
                    .ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.idle -= 1;
            }
        };
        let SpawnRequest {
            edge,
            command,
            direct,
            use_console,
        } = request;
        let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            context.start(edge, &command, direct.as_deref(), use_console)
        }))
        .unwrap_or(Err(ProcessError::ThreadPanicked { edge }));
        let delivered = sender.send(ProcessEvent::Spawned {
            edge,
            command,
            use_console,
            started,
        });
        if delivered.is_ok() {
            let _ = poller.notify();
        }
    }
}

/// What a command the build could not even start reports.
///
/// GNU Make's forked child says `strerror (errno)` against the name it was
/// asked to run and exits 127, for a program that is not there and for one it
/// may not run alike. The text goes back as the command's own stderr because
/// that is where GNU Make's copy of it comes from — the child had already
/// replaced its descriptors before it tried.
#[cfg(unix)]
pub(super) fn failed_to_start(direct: &DirectLaunch, source: &io::Error) -> ProcessOutput {
    use std::os::unix::process::ExitStatusExt;

    // What the C library would have said, which is what GNU Make prints.
    let reason = source.raw_os_error().map_or_else(
        || source.to_string(),
        |code| io::Error::from_raw_os_error(code).to_string(),
    );
    let reason = reason
        .split(" (os error")
        .next()
        .unwrap_or_default()
        .to_owned();
    let program = direct.argv[0].as_bytes().as_bstr().to_string();
    ProcessOutput {
        status: std::process::ExitStatus::from_raw(COMMAND_NOT_FOUND << 8),
        stdout: Vec::new(),
        stderr: format!("{}{program}: {reason}\n", direct.diagnostic_prefix).into_bytes(),
    }
}

/// Give the child a pipe for its output, and keep the read end.
///
/// A pipe rather than a socket pair, which is what this used to create. The
/// output only ever travels one way, so the second direction was never used —
/// and a socket pair is not a cheap thing to leave unused. Two `AF_UNIX`
/// sockets carry far more kernel state than a pipe's single buffer, and a
/// build pays for a fresh one per job: measured here, creating and closing a
/// non-blocking socket pair costs about 4.9 microseconds against 2.3 for a
/// pipe and its duplicate. C samurai has always used a pipe.
///
/// Only the read end is made non-blocking, because the drain loop reads until
/// it would block. The child's end must stay blocking, or a command writing
/// faster than this process reads would see its output fail with `EAGAIN`
/// rather than wait — which is why the flag cannot simply be passed to
/// `pipe2`, since that would set it on both ends.
#[cfg(unix)]
pub(super) fn configure_output(
    shell: &mut Command,
    edge: EdgeId,
    command: &BString,
    use_console: bool,
) -> ProcessResult<Option<std::io::PipeReader>> {
    use std::os::fd::OwnedFd;

    if use_console {
        return Ok(None);
    }
    let (reader, writer) = std::io::pipe().map_err(|source| ProcessError::Shell {
        edge,
        command: command.clone(),
        operation: ShellOperation::CreateOutputPipe,
        source,
    })?;
    rustix::fs::fcntl_setfl(&reader, rustix::fs::OFlags::NONBLOCK).map_err(|source| {
        ProcessError::Shell {
            edge,
            command: command.clone(),
            operation: ShellOperation::ConfigureOutputPipe,
            source: source.into(),
        }
    })?;
    let stdout: OwnedFd = writer
        .try_clone()
        .map_err(|source| ProcessError::Shell {
            edge,
            command: command.clone(),
            operation: ShellOperation::DuplicateOutputPipe,
            source,
        })?
        .into();
    let stderr: OwnedFd = writer.into();
    shell
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    Ok(Some(reader))
}

#[cfg(all(test, unix))]
mod tests {
    use super::{DirectLaunch, ShellMode, SpawnContext, configure_output, failed_to_start};
    use crate::graph::EdgeId;
    use crate::util::BString;
    use std::path::PathBuf;
    use std::process::Command;

    fn direct(program: &str) -> DirectLaunch {
        DirectLaunch {
            argv: vec![BString::from(program)],
            directory: PathBuf::new(),
            environment: Vec::new(),
            diagnostic_prefix: String::from("make: "),
            starts_no_process: false,
        }
    }

    fn context(environment: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>) -> SpawnContext {
        SpawnContext {
            working_directory: PathBuf::new(),
            shell: ShellMode::default(),
            environment,
        }
    }

    fn named(name: &str, value: &str) -> (std::ffi::OsString, Option<std::ffi::OsString>) {
        (
            std::ffi::OsString::from(name),
            Some(std::ffi::OsString::from(value)),
        )
    }

    /// GNU Make's own sentence for a command it could not exec, and its status.
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    #[test]
    fn a_failed_exec_reports_the_c_library_reason() {
        let reported = failed_to_start(
            &direct("./prog"),
            &std::io::Error::from_raw_os_error(libc_enoent()),
        );
        assert_eq!(
            String::from_utf8_lossy(&reported.stderr),
            "make: ./prog: No such file or directory\n"
        );
        assert_eq!(reported.status.code(), Some(127));
        assert!(reported.stdout.is_empty());
    }

    const fn libc_enoent() -> i32 {
        2
    }

    /// The launch's own `SHELL` outranks the build's, and the build's outranks
    /// the machine's. What is being answered is how this host runs a script.
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    #[test]
    fn a_script_interpreter_is_the_nearest_shell() {
        let mut launch = direct("./script");
        launch.environment.push(named("SHELL", "/bin/launch-sh"));
        let context = context(vec![named("SHELL", "/bin/build-sh")]);
        assert_eq!(context.script_interpreter(&launch), "/bin/launch-sh");

        let bare = direct("./script");
        assert_eq!(context.script_interpreter(&bare), "/bin/build-sh");
    }

    /// A console command writes where the build writes, so there is no pipe to
    /// keep; every other command gets one whose read end will not block.
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    #[test]
    fn only_a_captured_command_gets_a_pipe() {
        let edge = EdgeId::from_event_key(1).expect("test edge key is nonzero");
        let rendered = BString::from("true");
        let mut console = Command::new("true");
        assert!(
            configure_output(&mut console, edge, &rendered, true)
                .unwrap()
                .is_none()
        );

        let mut captured = Command::new("true");
        let reader = configure_output(&mut captured, edge, &rendered, false)
            .unwrap()
            .expect("a captured command keeps its read end");
        let flags = rustix::fs::fcntl_getfl(&reader).unwrap();
        assert!(flags.contains(rustix::fs::OFlags::NONBLOCK));
    }
}
