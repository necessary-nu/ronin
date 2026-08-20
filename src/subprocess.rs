//! Shell subprocess execution and completion-set bookkeeping.

use crate::error::{ProcessError, ShellOperation};
use crate::graph::EdgeId;
use crate::signal::Signal;
use crate::util::{BString, ByteSlice};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

type ProcessResult<T> = Result<T, ProcessError>;

fn signal_process(pid: u32, process_group: bool, signal: Signal) -> ProcessResult<()> {
    crate::signal::forward(pid, process_group, signal)
        .map(|_| ())
        .map_err(|source| ProcessError::SignalDelivery {
            pid,
            process_group,
            signal,
            source,
        })
}

/// What one process the build starts is.
///
/// Ninja has only one answer: a command line, and a shell to read it. GNU Make
/// has two, decided per recipe line by `construct_command_argv_internal` — a
/// line holding shell syntax is the shell's errand, and one that holds none is
/// exec'd directly, which is why a program that is not there is reported by
/// Make itself rather than in a shell's words.
#[derive(Clone)]
pub(crate) enum Launch {
    /// A command line for a shell, exactly as Ninja hands one over.
    Shell(BString),
    /// An argument list to run with no shell in between.
    Direct(Box<DirectLaunch>),
}

/// A command to exec with nothing between the build and it.
///
/// The directory and the environment travel with it because they have nowhere
/// else to go: a shell command carries its own `cd` and `env`, and there is no
/// shell here to read them.
#[derive(Clone)]
pub(crate) struct DirectLaunch {
    /// The program and its arguments, already unquoted.
    pub(crate) argv: Vec<BString>,
    /// Where to run it, or empty to stay where the build is.
    pub(crate) directory: PathBuf,
    /// What this command changes about the environment the build would pass
    /// on: `Some` sets, `None` removes.
    pub(crate) environment: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
    /// What a failure to start it is reported as, colon and space included.
    ///
    /// GNU Make prints `make: ./prog: No such file or directory` and the
    /// recipe fails with 127 — for a program that is not there and for one
    /// that may not be run alike, since what it reports is that it could not
    /// start the command rather than what the command said.
    pub(crate) diagnostic_prefix: String,
}

impl Launch {
    /// How this launch is named in a diagnostic and in the failure block.
    pub(crate) fn rendered(&self) -> BString {
        match self {
            Self::Shell(command) => command.clone(),
            Self::Direct(direct) => {
                let mut rendered = Vec::new();
                for word in &direct.argv {
                    if !rendered.is_empty() {
                        rendered.push(b' ');
                    }
                    rendered.extend_from_slice(word.as_bytes());
                }
                BString::from(rendered)
            }
        }
    }
}

/// How a spawn attempt came out.
#[cfg(unix)]
enum Started {
    Running(std::process::Child, Option<std::io::PipeReader>),
    /// Nothing was started and nothing will be: what the launch would have
    /// reported is here instead, to be delivered as the command's own answer.
    NeverStarted(ProcessOutput),
}

/// POSIX's status for a command that could not be run at all, which is what
/// GNU Make's child reports when the exec itself failed.
const COMMAND_NOT_FOUND: i32 = 127;

/// `ENOEXEC`, which POSIX fixes at 8 and which every Unix this builds for
/// agrees on. `io::ErrorKind` has no name for it.
#[cfg(unix)]
const EXEC_FORMAT_ERROR: i32 = 8;

pub(crate) struct ProcessOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) struct ProcessCompletion {
    pub(crate) edge: EdgeId,
    pub(crate) result: ProcessResult<Option<ProcessOutput>>,
}

enum ProcessEvent<External> {
    #[cfg(not(unix))]
    Started {
        edge: EdgeId,
        pid: u32,
        process_group: bool,
    },
    #[cfg(not(unix))]
    Finished(ProcessCompletion),
    External(External),
}

pub(crate) enum SupervisorWake<External> {
    Process(ProcessCompletion),
    External(External),
}

pub(crate) struct ExternalEventSender<External> {
    sender: Sender<ProcessEvent<External>>,
    #[cfg(unix)]
    poller: Arc<polling::Poller>,
}

impl<External> ExternalEventSender<External> {
    pub(crate) fn send(&self, event: External) {
        if self.sender.send(ProcessEvent::External(event)).is_ok() {
            #[cfg(unix)]
            let _ = self.poller.notify();
        }
    }
}

#[cfg(unix)]
struct RunningChild {
    child: std::process::Child,
    command: BString,
    process_group: bool,
    output: Option<std::io::PipeReader>,
    output_bytes: Vec<u8>,
    registered: bool,
}

#[cfg(unix)]
const SIGNAL_EVENT_KEY: usize = 0;

/// The first interval waited before re-asking whether a child can be reaped.
///
/// Sized to the gap between a child closing its descriptors and the kernel
/// making it waitable, which is one scheduler hop rather than a millisecond.
#[cfg(unix)]
const MIN_REAP_INTERVAL: std::time::Duration = std::time::Duration::from_micros(50);

/// The ceiling that interval grows to, for a child that closed its output and
/// then carried on working.
#[cfg(unix)]
const MAX_REAP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

pub(crate) struct ProcessSupervisor<External = ()> {
    sender: Sender<ProcessEvent<External>>,
    receiver: Receiver<ProcessEvent<External>>,
    running: usize,
    #[cfg(unix)]
    poller: Arc<polling::Poller>,
    #[cfg(unix)]
    signal_wake: Option<std::os::unix::net::UnixStream>,
    #[cfg(unix)]
    events: polling::Events,
    #[cfg(unix)]
    event_edges: Vec<EdgeId>,
    #[cfg(unix)]
    ready: VecDeque<ProcessCompletion>,
    #[cfg(unix)]
    reap_candidates: Vec<EdgeId>,
    /// How long to wait before asking again whether a child can be reaped.
    ///
    /// A child whose output pipe has reported end of file has already begun
    /// exiting: it closed its descriptors on the way out, and becomes reapable
    /// a scheduler hop later. Waiting a fixed ten milliseconds to ask cost that
    /// on every single job — 1.4 seconds across 128 of them, against 180 ms for
    /// the same work — because the parent lost that race every time and then
    /// slept through it. Start far below the gap and grow, so the usual child is
    /// collected almost immediately while one that closes its output and keeps
    /// running is still only asked about occasionally.
    #[cfg(unix)]
    reap_backoff: std::time::Duration,
    /// Scratch for draining child output, kept rather than declared per read.
    #[cfg(unix)]
    read_buffer: Vec<u8>,
    /// Live children, keyed by edge.
    ///
    /// Rapid-hashed rather than `SipHash`ed: the key is a four-byte identifier
    /// looked up six or so times per job — on spawn, on each readiness, on
    /// drain, and on reap — and the default hasher's collision resistance buys
    /// nothing against keys this process mints itself.
    #[cfg(unix)]
    children: crate::htab::RapidHashMap<EdgeId, RunningChild>,
    #[cfg(not(unix))]
    children: HashMap<EdgeId, (u32, bool)>,
    interrupted: Option<Signal>,
    working_directory: PathBuf,
    shell: ShellMode,
    /// Variables imposed on every child, empty unless this build serves a
    /// jobserver its children are meant to draw on.
    ///
    /// Imposing nothing is not the same as imposing what is already there:
    /// with no variables set, `Command` hands the child this process's own
    /// environment block untouched, and the copy it would otherwise build per
    /// spawn never happens.
    environment: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

impl<External> ProcessSupervisor<External> {
    #[cfg(test)]
    pub(crate) fn new() -> ProcessResult<Self> {
        Self::in_directory(Path::new(""), ShellMode::default(), &[])
    }

    pub(crate) fn in_directory(
        working_directory: &Path,
        shell: ShellMode,
        environment: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
    ) -> ProcessResult<Self> {
        let (sender, receiver) = mpsc::channel();
        #[cfg(unix)]
        let poller =
            polling::Poller::new()
                .map(Arc::new)
                .map_err(|source| ProcessError::Supervisor {
                    operation: crate::error::SupervisorOperation::CreatePoller,
                    source,
                })?;
        #[cfg(unix)]
        let signal_wake =
            crate::signal::wake_reader().map_err(|source| ProcessError::Supervisor {
                operation: crate::error::SupervisorOperation::RegisterSignalWake,
                source,
            })?;
        #[cfg(unix)]
        if let Some(wake) = signal_wake.as_ref() {
            // SAFETY: `signal_wake` is owned by the supervisor and explicitly
            // removed from `poller` before either field is dropped.
            unsafe { poller.add(wake, polling::Event::readable(SIGNAL_EVENT_KEY)) }.map_err(
                |source| ProcessError::Supervisor {
                    operation: crate::error::SupervisorOperation::RegisterSignalWake,
                    source,
                },
            )?;
        }
        Ok(Self {
            sender,
            receiver,
            running: 0,
            #[cfg(unix)]
            poller,
            #[cfg(unix)]
            signal_wake,
            #[cfg(unix)]
            events: polling::Events::new(),
            #[cfg(unix)]
            event_edges: Vec::new(),
            #[cfg(unix)]
            ready: VecDeque::new(),
            #[cfg(unix)]
            reap_candidates: Vec::new(),
            #[cfg(unix)]
            reap_backoff: MIN_REAP_INTERVAL,
            #[cfg(unix)]
            read_buffer: vec![0; 16 * 1024],
            children: HashMap::default(),
            interrupted: None,
            shell,
            working_directory: directory_to_impose(working_directory),
            environment: environment.to_vec(),
        })
    }

    fn completion(&mut self, completion: ProcessCompletion) -> SupervisorWake<External> {
        debug_assert!(self.running > 0);
        self.running -= 1;
        SupervisorWake::Process(completion)
    }

    #[cfg(unix)]
    fn try_channel(&self) -> ProcessResult<Option<SupervisorWake<External>>> {
        match self.receiver.try_recv() {
            Ok(ProcessEvent::External(event)) => Ok(Some(SupervisorWake::External(event))),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(ProcessError::CompletionChannelDisconnected)
            }
        }
    }

    #[cfg(not(unix))]
    fn try_channel(&mut self) -> ProcessResult<Option<SupervisorWake<External>>> {
        match self.receiver.try_recv() {
            Ok(ProcessEvent::External(event)) => Ok(Some(SupervisorWake::External(event))),
            Ok(ProcessEvent::Finished(completion)) => {
                self.children.remove(&completion.edge);
                Ok(Some(self.completion(completion)))
            }
            Ok(ProcessEvent::Started {
                edge,
                pid,
                process_group,
            }) => {
                self.children.insert(edge, (pid, process_group));
                if let Some(signal) = self.interrupted {
                    signal_process(pid, process_group, signal)?;
                }
                Ok(None)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(ProcessError::CompletionChannelDisconnected)
            }
        }
    }
}

// [spec:ronin:req:compat.process-integration]
impl<External: Send + 'static> ProcessSupervisor<External> {
    pub(crate) fn spawn(
        &mut self,
        edge: EdgeId,
        launch: Launch,
        use_console: bool,
        dryrun: bool,
    ) -> ProcessResult<()> {
        #[cfg(unix)]
        {
            self.spawn_evented(edge, launch, use_console, dryrun)
        }
        #[cfg(not(unix))]
        {
            let Launch::Shell(command) = launch else {
                unreachable!("a direct launch is decided per recipe line, and only on Unix")
            };
            let sender = self.sender.clone();
            let working_directory = self.working_directory.clone();
            let shell = self.shell.clone();
            let environment = self.environment.clone();
            self.running += 1;
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_shell(
                        &command,
                        &working_directory,
                        &shell,
                        &environment,
                        use_console,
                        dryrun,
                        |pid, process_group| {
                            let _ = sender.send(ProcessEvent::Started {
                                edge,
                                pid,
                                process_group,
                            });
                        },
                    )
                    .map_err(|failure| ProcessError::Shell {
                        edge,
                        command: command.clone(),
                        operation: failure.operation,
                        source: failure.source,
                    })
                }))
                .unwrap_or(Err(ProcessError::ThreadPanicked { edge }));
                let _ = sender.send(ProcessEvent::Finished(ProcessCompletion { edge, result }));
            });
            Ok(())
        }
    }

    /// Start one process, however many attempts that takes.
    ///
    /// Two of the retries are GNU Make's and one is Ninja's: a shell-free
    /// command that turns out to be a script with no `#!` line is exec'd again
    /// under an interpreter, a shell-free command that cannot be started at
    /// all is answered for here rather than handed to anyone else, and a
    /// direct launch Ronin chose for itself falls back to the shell.
    #[cfg(unix)]
    fn start(
        &self,
        edge: EdgeId,
        command: &BString,
        direct: Option<&DirectLaunch>,
        use_console: bool,
    ) -> ProcessResult<Started> {
        use std::os::unix::process::CommandExt;

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
    #[cfg(unix)]
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

    pub(crate) fn wait(
        &mut self,
        timeout: Option<std::time::Duration>,
    ) -> ProcessResult<Option<SupervisorWake<External>>> {
        #[cfg(unix)]
        {
            self.wait_evented(timeout)
        }
        #[cfg(not(unix))]
        {
            loop {
                let event = if let Some(timeout) = timeout {
                    match self.receiver.recv_timeout(timeout) {
                        Ok(event) => Some(event),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(ProcessError::CompletionChannelDisconnected);
                        }
                    }
                } else {
                    Some(
                        self.receiver
                            .recv()
                            .map_err(|_| ProcessError::CompletionChannelDisconnected)?,
                    )
                };
                match event {
                    Some(ProcessEvent::Started {
                        edge,
                        pid,
                        process_group,
                    }) => {
                        self.children.insert(edge, (pid, process_group));
                        if let Some(signal) = self.interrupted {
                            signal_process(pid, process_group, signal)?;
                        }
                    }
                    Some(ProcessEvent::Finished(completion)) => {
                        self.children.remove(&completion.edge);
                        return Ok(Some(self.completion(completion)));
                    }
                    Some(ProcessEvent::External(event)) => {
                        return Ok(Some(SupervisorWake::External(event)));
                    }
                    None => return Ok(None),
                }
            }
        }
    }

    pub(crate) fn external_sender(&self) -> ExternalEventSender<External> {
        ExternalEventSender {
            sender: self.sender.clone(),
            #[cfg(unix)]
            poller: self.poller.clone(),
        }
    }

    pub(crate) const fn running_len(&self) -> usize {
        self.running
    }

    pub(crate) fn interrupt(&mut self, signal: Signal) -> ProcessResult<()> {
        if self.interrupted.replace(signal) == Some(signal) {
            return Ok(());
        }
        let mut first_error = None;
        #[cfg(unix)]
        for child in self.children.values() {
            let result = signal_process(child.child.id(), child.process_group, signal);
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        #[cfg(not(unix))]
        for (pid, process_group) in self.children.values().copied() {
            let result = signal_process(pid, process_group, signal);
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

// [spec:ronin:req:runtime.process-supervisor-scalability]
#[cfg(unix)]
impl<External: Send + 'static> ProcessSupervisor<External> {
    fn spawn_evented(
        &mut self,
        edge: EdgeId,
        launch: Launch,
        use_console: bool,
        dryrun: bool,
    ) -> ProcessResult<()> {
        use polling::Event;

        if dryrun {
            self.running += 1;
            self.ready.push_back(ProcessCompletion {
                edge,
                result: Ok(None),
            });
            return Ok(());
        }

        #[cfg(feature = "make")]
        let _directory = crate::make::stable_process_directory_guard();
        let command = launch.rendered();
        let direct = match launch {
            Launch::Shell(_) => None,
            Launch::Direct(direct) => Some(direct),
        };
        let (child, output) = match self.start(edge, &command, direct.as_deref(), use_console)? {
            Started::Running(child, output) => (child, output),
            Started::NeverStarted(reported) => {
                self.running += 1;
                self.ready.push_back(ProcessCompletion {
                    edge,
                    result: Ok(Some(reported)),
                });
                return Ok(());
            }
        };
        let process_group = !use_console;
        let mut child = RunningChild {
            child,
            command,
            process_group,
            output,
            output_bytes: Vec::new(),
            registered: false,
        };
        if let Some(signal) = self.interrupted
            && let Err(error) = signal_process(child.child.id(), process_group, signal)
        {
            terminate_and_reap(&mut child);
            return Err(error);
        }
        let previous = self.children.insert(edge, child);
        debug_assert!(previous.is_none(), "an edge cannot run twice concurrently");

        if use_console {
            self.reap_candidates.push(edge);
            self.reap_backoff = MIN_REAP_INTERVAL;
        } else {
            let registration = {
                let output = self.children[&edge]
                    .output
                    .as_ref()
                    .expect("captured children own an output pipe");
                // SAFETY: `self.children` owns the stream until `delete` is
                // called by completion, failure cleanup, or `Drop`.
                unsafe { self.poller.add(output, Event::readable(edge.event_key())) }
            };
            if let Err(source) = registration {
                let mut child = self
                    .children
                    .remove(&edge)
                    .expect("the child was inserted before registration");
                terminate_and_reap(&mut child);
                return Err(ProcessError::Shell {
                    edge,
                    command: child.command,
                    operation: ShellOperation::RegisterOutput,
                    source,
                });
            }
            self.children
                .get_mut(&edge)
                .expect("the registered child remains present")
                .registered = true;
        }
        self.running += 1;
        Ok(())
    }

    fn wait_evented(
        &mut self,
        timeout: Option<std::time::Duration>,
    ) -> ProcessResult<Option<SupervisorWake<External>>> {
        let deadline = timeout.and_then(|timeout| std::time::Instant::now().checked_add(timeout));
        loop {
            if let Some(completion) = self.ready.pop_front() {
                return Ok(Some(self.completion(completion)));
            }
            if let Some(event) = self.try_channel()? {
                return Ok(Some(event));
            }
            self.reap_without_output();
            if let Some(completion) = self.ready.pop_front() {
                return Ok(Some(self.completion(completion)));
            }

            let wait = deadline.map_or_else(
                || (!self.reap_candidates.is_empty()).then_some(self.reap_backoff),
                |deadline| Some(deadline.saturating_duration_since(std::time::Instant::now())),
            );
            self.events.clear();
            self.poller.wait(&mut self.events, wait).map_err(|source| {
                ProcessError::Supervisor {
                    operation: crate::error::SupervisorOperation::WaitForEvent,
                    source,
                }
            })?;

            if !self.reap_candidates.is_empty() {
                self.reap_backoff = (self.reap_backoff * 2).min(MAX_REAP_INTERVAL);
            }

            let signal_ready = self
                .events
                .iter()
                .any(|event| event.key == SIGNAL_EVENT_KEY);
            if signal_ready {
                self.drain_signal_wake()?;
            }
            self.event_edges.clear();
            self.event_edges.extend(
                self.events
                    .iter()
                    .filter_map(|event| EdgeId::from_event_key(event.key)),
            );
            while let Some(edge) = self.event_edges.pop() {
                if self.children.contains_key(&edge) {
                    self.drain_output(edge);
                }
            }
            self.reap_without_output();
            // Only now, with what the poller reported already consumed. Each
            // descriptor is armed for one event and rearmed by the drain, so an
            // event abandoned here is never delivered again: the child that
            // closed its output would never be reaped, and a wait with nothing
            // else to wake it never returns. Reporting the token one iteration
            // later costs a token latency; reporting it sooner cost a build.
            if let Some(event) = self.try_channel()? {
                return Ok(Some(event));
            }
            if signal_ready {
                return Ok(None);
            }
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
                && self.ready.is_empty()
            {
                return Ok(None);
            }
        }
    }

    fn drain_output(&mut self, edge: EdgeId) {
        use std::io::Read;

        enum Drain {
            Rearm,
            Eof,
            Failed(io::Error),
        }

        // Borrowed out and put back so the read buffer, which belongs to the
        // supervisor rather than to any one child, can be reused across jobs.
        // It used to be a sixteen-kilobyte array declared here, which the
        // compiler must zero on entry — two megabytes of pointless writes over
        // a thousand-job build, and the largest single entry in this process's
        // own profile before it was moved.
        let mut buffer = std::mem::take(&mut self.read_buffer);
        let drain = {
            let child = self
                .children
                .get_mut(&edge)
                .expect("poll events refer to live children");
            let output = child
                .output
                .as_mut()
                .expect("only captured output is registered");
            loop {
                match output.read(&mut buffer) {
                    Ok(0) => break Drain::Eof,
                    Ok(count) => child.output_bytes.extend_from_slice(&buffer[..count]),
                    Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
                        break Drain::Rearm;
                    }
                    Err(source) => break Drain::Failed(source),
                }
            }
        };
        self.read_buffer = buffer;

        match drain {
            Drain::Rearm => {
                let result = {
                    let child = &self.children[&edge];
                    self.poller.modify(
                        child.output.as_ref().expect("captured child"),
                        polling::Event::readable(edge.event_key()),
                    )
                };
                if let Err(source) = result {
                    self.fail_child(edge, ShellOperation::RegisterOutput, source);
                }
            }
            Drain::Eof => {
                let result = {
                    let child = &self.children[&edge];
                    self.poller
                        .delete(child.output.as_ref().expect("captured child"))
                };
                if let Err(source) = result {
                    self.fail_child(edge, ShellOperation::RegisterOutput, source);
                    return;
                }
                self.children
                    .get_mut(&edge)
                    .expect("the child remains live")
                    .registered = false;
                if !self.try_finish_child(edge) {
                    self.reap_candidates.push(edge);
                    self.reap_backoff = MIN_REAP_INTERVAL;
                }
            }
            Drain::Failed(source) => {
                self.fail_child(edge, ShellOperation::ReadOutput, source);
            }
        }
    }

    fn drain_signal_wake(&mut self) -> ProcessResult<()> {
        use std::io::Read;

        let wake = self
            .signal_wake
            .as_mut()
            .expect("signal events require an installed wake stream");
        let mut buffer = [0; 64];
        loop {
            match wake.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::WouldBlock => break,
                Err(source) => {
                    return Err(ProcessError::Supervisor {
                        operation: crate::error::SupervisorOperation::ReadSignalWake,
                        source,
                    });
                }
            }
        }
        self.poller
            .modify(wake, polling::Event::readable(SIGNAL_EVENT_KEY))
            .map_err(|source| ProcessError::Supervisor {
                operation: crate::error::SupervisorOperation::RegisterSignalWake,
                source,
            })
    }

    fn reap_without_output(&mut self) {
        let mut index = 0;
        while index < self.reap_candidates.len() {
            let edge = self.reap_candidates[index];
            if self.try_finish_child(edge) {
                self.reap_candidates.swap_remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn try_finish_child(&mut self, edge: EdgeId) -> bool {
        let result = self
            .children
            .get_mut(&edge)
            .expect("reap candidates refer to live children")
            .child
            .try_wait();
        match result {
            Ok(Some(status)) => {
                let child = self
                    .children
                    .remove(&edge)
                    .expect("the completed child remains present");
                self.ready.push_back(ProcessCompletion {
                    edge,
                    result: Ok(Some(ProcessOutput {
                        status,
                        stdout: child.output_bytes,
                        stderr: Vec::new(),
                    })),
                });
                true
            }
            Ok(None) => false,
            Err(source) => {
                self.fail_child(edge, ShellOperation::Wait, source);
                true
            }
        }
    }

    fn fail_child(&mut self, edge: EdgeId, operation: ShellOperation, source: io::Error) {
        let mut child = self
            .children
            .remove(&edge)
            .expect("failing child remains present");
        if child.registered
            && let Some(output) = child.output.as_ref()
        {
            let _ = self.poller.delete(output);
        }
        terminate_and_reap(&mut child);
        self.ready.push_back(ProcessCompletion {
            edge,
            result: Err(ProcessError::Shell {
                edge,
                command: child.command,
                operation,
                source,
            }),
        });
    }
}

#[cfg(unix)]
impl<External> Drop for ProcessSupervisor<External> {
    fn drop(&mut self) {
        if let Some(wake) = self.signal_wake.as_ref() {
            let _ = self.poller.delete(wake);
        }
        for child in self.children.values_mut() {
            if child.registered
                && let Some(output) = child.output.as_ref()
            {
                let _ = self.poller.delete(output);
            }
            terminate_and_reap(child);
        }
    }
}

#[cfg(unix)]
fn terminate_and_reap(child: &mut RunningChild) {
    if child.process_group {
        let _ = crate::signal::kill_process_group(child.child.id());
    }
    let _ = child.child.kill();
    let _ = child.child.wait();
}

/// Whether a child's death reads as the user interrupting the build.
///
/// Ninja counts exactly `SIGINT`, `SIGTERM` and `SIGHUP`, and deliberately not
/// `SIGQUIT` — which Ronin does handle when the signal arrives at the build tool
/// itself, but which in a child is an ordinary failure. A build stops on this
/// without reporting the command as failed, because the command did not fail —
/// the whole build was being brought down around it.
// [spec:ronin:req:compat.process-integration]
pub(crate) fn status_interrupted(status: std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        matches!(
            status.signal(),
            Some(libc_signal::SIGINT | libc_signal::SIGTERM | libc_signal::SIGHUP)
        )
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

#[cfg(unix)]
mod libc_signal {
    pub(super) const SIGHUP: i32 = rustix::process::Signal::HUP.as_raw();
    pub(super) const SIGINT: i32 = rustix::process::Signal::INT.as_raw();
    pub(super) const SIGTERM: i32 = rustix::process::Signal::TERM.as_raw();
}

/// The signal that killed a command, if one did.
///
/// Only ever answered for the process the build itself waited on, so it says a
/// recipe died this way exactly when no shell stood between the two to turn the
/// death into an exit status of its own. That is why Make mode runs its recipe
/// shell in place of the shell that launched it: a shell reports a signalled
/// child as `128 + signal` and exits normally, which a recipe that plainly ran
/// `exit 143` is indistinguishable from.
pub(crate) fn killed_by_signal(status: std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

/// What Make mode reports for a command a signal killed.
///
/// `128 + signal`, the number a shell exits with when the command it ran died
/// that way — so the status a recipe carries out is the one it would have
/// carried out had a shell reported it, and running the recipe shell directly
/// to see the signal does not change the number anyone reads.
pub(crate) fn signalled_exit_code(status: std::process::ExitStatus) -> Option<i32> {
    killed_by_signal(status).map(|signal| 128_i32.wrapping_add(signal))
}

/// Ninja's interpretation of a finished child's wait status.
///
/// A child that exited reports its own code, transparently — this is the number
/// a build tool's own exit status carries, so `exit 3` has to stay 3 all the way
/// out. A child killed by an interrupting signal reports 130. Anything else
/// takes Ninja's remaining branch, `raw wait status + 128`, which is not the
/// same as `128 + signal`: the raw status still has the core-dump bit set, so a
/// dumping `SIGQUIT` reports 259 rather than 131. That is Ninja's arithmetic and
/// it is visible in the `FAILED: [code=…]` line, so it is reproduced rather than
/// corrected.
// [spec:ronin:req:compat.process-integration]
pub(crate) fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if status_interrupted(status) {
            return INTERRUPTED_EXIT_CODE;
        }
        status.into_raw().wrapping_add(128)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

/// Ninja's `ExitInterrupted`, the status a build reports when it was cut short.
///
/// Ninja exits with this rather than dying by the signal it caught, so the
/// executable reports it the same way however far the build had got.
pub const INTERRUPTED_EXIT_CODE: i32 = 130;

// [spec:ronin:def:os.osspawn-fn]
// [spec:ronin:sem:os.osspawn-fn]
// [spec:ronin:def:os-posix.osspawn-fn]
// [spec:ronin:sem:os-posix.osspawn-fn]
#[cfg(not(unix))]
struct ShellFailure {
    operation: ShellOperation,
    source: io::Error,
}

/// `/dev/null` for a child's standard input, opened once for the process.
///
/// `Stdio::null()` opens it afresh for every spawn, which puts a path
/// resolution on the dispatch loop between one job finishing and the next
/// starting. Duplicating a descriptor already held is the same result for a
/// fraction of the work. Falling back to `Stdio::null()` costs only the open
/// this exists to avoid, so a failure here is not worth reporting.
#[cfg(unix)]
fn null_stdin() -> Stdio {
    use std::os::fd::OwnedFd;

    static NULL: std::sync::OnceLock<Option<OwnedFd>> = std::sync::OnceLock::new();
    NULL.get_or_init(|| std::fs::File::open("/dev/null").ok().map(OwnedFd::from))
        .as_ref()
        .and_then(|null| null.try_clone().ok())
        .map_or_else(Stdio::null, Stdio::from)
}

/// The directory to impose on children, empty when they would inherit it.
///
/// The library lets several runners share a process with different roots, so a
/// child's directory has to be set per spawn rather than by moving the process
/// — but the binary's root is the process's own directory, which is every
/// invocation from a shell, and there imposing it is asking for what is
/// already true. Skipping it is worth more than the redundant work: a `cwd`
/// makes Rust's `Command` reach for `posix_spawn_file_actions_addchdir_np`,
/// which is a weak symbol, and where the linker has not supplied it the whole
/// spawn falls back from `posix_spawn` to `fork` and `exec` — page tables
/// copied per job instead of shared. Measured at 128 jobs, that fallback is
/// `clone` at 127 microseconds a call against `clone3` at 74.
fn directory_to_impose(working_directory: &Path) -> PathBuf {
    if working_directory.as_os_str().is_empty() {
        return PathBuf::new();
    }
    match std::env::current_dir() {
        Ok(process) if process == working_directory => PathBuf::new(),
        _ => working_directory.to_owned(),
    }
}

/// The shell Ninja hands commands to on this platform.
const SYSTEM_SHELL: &str = "/bin/sh";

/// The executable that answers to `sh`, once a process entry point has said
/// that its own does.
#[cfg(unix)]
static BUILTIN_SHELL: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Say that this executable carries the shell, so a build may spawn it as one.
///
/// The declaration is what entitles the substitution, and it has to be a
/// declaration rather than a discovery: `current_exe` names whatever program
/// is running, and only a program that dispatches on its invoked name will
/// answer as a shell when spawned as one. Ronin's own entry point does; a test
/// binary linking this library, or a program embedding it, does not — and
/// neither declares, so both get the machine's shell exactly as they did
/// before there was another.
///
/// `current_exe` reads `/proc/self/exe` on Linux. Where there is no `/proc`,
/// or the answer names a file that is gone, there is nothing to spawn and the
/// declaration stands as "no builtin shell" rather than failing an invocation
/// that has not started yet.
// [spec:ronin:req:product.builtin-shell]
#[cfg(unix)]
pub fn declare_builtin_shell() {
    let _ = BUILTIN_SHELL.set(std::env::current_exe().ok().filter(|own| own.is_file()));
}

/// Say that this executable carries the shell, which no build here can use.
///
/// Windows has no shell in the position a builtin one would stand in — Ninja
/// hands the whole command line to `CreateProcess` — so the declaration is
/// accepted and means nothing.
#[cfg(not(unix))]
pub fn declare_builtin_shell() {}

/// The executable to spawn in place of the default shell, if any.
///
/// `None` until a process entry point declares one, and `None` for good on a
/// process that never does.
// [spec:ronin:req:product.builtin-shell]
#[cfg(unix)]
pub(crate) fn builtin_shell() -> Option<&'static Path> {
    BUILTIN_SHELL.get()?.as_deref()
}

/// The process that runs the shell spelled `named`.
///
/// Where `named` is the default shell, that is this executable under the name
/// the build asked for: `arg0` carries the spelling through, because dash
/// prefixes its diagnostics with `argv[0]` exactly as written, so a
/// substituted shell says `/bin/sh: 1: cc: not found` where the shell it
/// replaced would have. A shell the build named — a Makefile's `SHELL`, a
/// `--shell`, the `$SHELL` an unmarked script is run under — is spawned as
/// named, so choosing a shell still chooses one.
// [spec:ronin:req:product.builtin-shell]
#[cfg(unix)]
fn shell_process(named: &std::ffi::OsStr) -> Command {
    use std::os::unix::process::CommandExt;

    if named == std::ffi::OsStr::new(SYSTEM_SHELL)
        && let Some(own) = builtin_shell()
    {
        let mut shell = Command::new(own);
        shell.arg0(named);
        return shell;
    }
    Command::new(named)
}

/// How a command string is turned into a process.
///
/// Ninja hands every command to `/bin/sh -c` on Unix, which makes the shell
/// the interpreter for the `command` binding — its quoting rules, its
/// operators, its `VAR=value` prefixes. That is the language definition, not
/// an implementation detail, so nothing here may change what a command *means*
/// — only whether a shell process is spawned to arrive at that meaning.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum ShellMode {
    /// Spawn a shell only for commands that need one.
    #[default]
    Auto,
    /// Always spawn the system shell, exactly as Ninja does.
    Compat,
    /// Always spawn this shell instead of the system one.
    Program(PathBuf),
}

/// Bytes that mean something to `sh` beyond being part of a word.
///
/// Deliberately over-broad. `#` only opens a comment at the start of a word
/// and `~` only expands there, but a command containing either anywhere falls
/// back to the shell rather than inviting an argument about where exactly.
/// Being wrong here does not produce a crash, it produces a build that ran a
/// different command than it was told to, so the set errs toward the shell.
const SHELL_SIGNIFICANT: &[u8] = b"|&;<>()$`\\\"'*?[]~#{}!\n\t\r";

/// Whether `sh` would do anything to this command beyond splitting it.
///
/// A command free of significant bytes is one `sh` would split on spaces and
/// then execute, resolving the first word through `PATH` — which is precisely
/// what spawning it directly does. A leading `VAR=value` is rejected even
/// though `=` is not otherwise significant, because to the shell it is an
/// assignment and to `execvp` it is the name of a program.
pub(crate) fn needs_shell(command: &[u8]) -> bool {
    if command.iter().any(|byte| SHELL_SIGNIFICANT.contains(byte)) {
        return true;
    }
    let mut words = command
        .split(|byte| *byte == b' ')
        .filter(|word| !word.is_empty());
    let Some(first) = words.next() else {
        // Nothing to run: let the shell produce its own answer for that.
        return true;
    };
    // `exec` is the shell's, not a program on the path: it says the command
    // should replace the shell rather than run under it, and spawning it
    // directly would look for a file by that name and not find one.
    first == b"exec" || first.contains(&b'=')
}

/// Whether a failed direct spawn should be retried through the shell.
fn retry_with_shell(mode: &ShellMode, source: &std::io::Error) -> bool {
    matches!(mode, ShellMode::Auto)
        && matches!(
            source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        )
}

/// Split a command that [`needs_shell`] rejected into its argument vector.
fn direct_argv(command: &[u8]) -> Vec<&[u8]> {
    command
        .split(|byte| *byte == b' ')
        .filter(|word| !word.is_empty())
        .collect()
}

/// Build the process that runs `direct` with nothing between it and the build.
///
/// The supervisor's own directory and environment are the ground the command
/// stands on, exactly as they are for a shell command; what the launch carries
/// is the difference a `cd` and an `env` would otherwise have expressed.
#[cfg(unix)]
fn direct_command(
    direct: &DirectLaunch,
    interpreter: Option<&std::ffi::OsStr>,
    working_directory: &Path,
    environment: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
) -> Command {
    let word = |bytes: &BString| {
        bytes
            .to_os_str()
            .expect("byte strings are valid on Unix")
            .to_owned()
    };
    let mut command = interpreter.map_or_else(
        // A launch names its own program, so a front end that put the default
        // shell there — which is what a recipe line needing a shell is — gets
        // the build's own, under the name it wrote.
        || shell_process(&word(&direct.argv[0])),
        |interpreter| {
            // The interpreter is a shell, so it is one this executable can be:
            // an executable file with no `#!` line is run by `$SHELL`, and
            // where that is the default shell the build's own reads it.
            let mut command = shell_process(interpreter);
            command.arg(word(&direct.argv[0]));
            command
        },
    );
    for argument in &direct.argv[1..] {
        command.arg(word(argument));
    }
    command.stdin(null_stdin());
    if !working_directory.as_os_str().is_empty() {
        command.current_dir(working_directory);
    }
    if !direct.directory.as_os_str().is_empty() {
        command.current_dir(&direct.directory);
    }
    for (name, value) in environment.iter().chain(&direct.environment) {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command
}

/// What a command the build could not even start reports.
///
/// GNU Make's forked child says `strerror (errno)` against the name it was
/// asked to run and exits 127, for a program that is not there and for one it
/// may not run alike. The text goes back as the command's own stderr because
/// that is where GNU Make's copy of it comes from — the child had already
/// replaced its descriptors before it tried.
#[cfg(unix)]
fn failed_to_start(direct: &DirectLaunch, source: &io::Error) -> ProcessOutput {
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

/// Build the process that runs `command` under `mode`.
///
/// `Auto` skips the shell only where the shell provably has nothing to do;
/// every other command, and every command at all under `Compat`, is handed to
/// a shell exactly as Ninja hands it over.
fn shell_command(
    command: &BString,
    working_directory: &Path,
    mode: &ShellMode,
    environment: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
) -> Command {
    let mut shell = match mode {
        #[cfg(unix)]
        ShellMode::Program(program) => {
            let mut shell = shell_process(program.as_os_str());
            shell
                .arg("-c")
                .arg(command.to_os_str().expect("byte strings are valid on Unix"));
            shell
        }
        #[cfg(not(unix))]
        ShellMode::Program(program) => {
            let mut shell = Command::new(program);
            shell
                .arg("-c")
                .arg(command.to_os_str().expect("byte strings are valid on Unix"));
            shell
        }
        #[cfg(unix)]
        ShellMode::Auto if !needs_shell(command.as_bytes()) => {
            let argv = direct_argv(command.as_bytes());
            let mut direct =
                Command::new(argv[0].to_os_str().expect("byte strings are valid on Unix"));
            for argument in &argv[1..] {
                direct.arg(
                    argument
                        .to_os_str()
                        .expect("byte strings are valid on Unix"),
                );
            }
            direct
        }
        #[cfg(unix)]
        ShellMode::Auto | ShellMode::Compat => {
            let mut shell = shell_process(std::ffi::OsStr::new(SYSTEM_SHELL));
            shell
                .arg("-c")
                .arg(command.to_os_str().expect("byte strings are valid on Unix"));
            shell
        }
        // Windows has no shell in this position at all: Ninja hands the whole
        // command line to `CreateProcess` and lets Windows find the program in
        // it. `Compat` therefore asks for the same thing `Auto` does — there is
        // nothing to fall back to — and the POSIX word splitting used above
        // would be actively wrong here, since it would break a quoted program
        // path at its spaces.
        #[cfg(not(unix))]
        ShellMode::Auto | ShellMode::Compat => {
            use std::os::windows::process::CommandExt;
            let (program, arguments) = windows_program_and_arguments(command.as_bytes());
            let mut direct = Command::new(
                std::str::from_utf8(program).expect("Windows command lines are UTF-8"),
            );
            if !arguments.is_empty() {
                direct.raw_arg(
                    std::str::from_utf8(arguments).expect("Windows command lines are UTF-8"),
                );
            }
            direct
        }
    };
    #[cfg(unix)]
    shell.stdin(null_stdin());
    #[cfg(not(unix))]
    shell.stdin(Stdio::null());
    if !working_directory.as_os_str().is_empty() {
        shell.current_dir(working_directory);
    }
    for (name, value) in environment {
        match value {
            Some(value) => shell.env(name, value),
            None => shell.env_remove(name),
        };
    }
    shell
}

/// Splits a Windows command line into the program and everything after it.
///
/// Windows does this itself when `CreateProcess` is given a command line and no
/// program, which is what Ninja relies on. Rust's `Command` insists on a
/// separate program, so the same split is done here rather than reusing the
/// POSIX word splitting, which would break `"C:\\Program Files\\x\\cl.exe" /c`
/// at the space inside the quoted path. A quoted program ends at its closing
/// quote; an unquoted one ends at the first space.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "the splitting rule is pure, so it is compiled and tested everywhere rather than only where it runs"
    )
)]
fn windows_program_and_arguments(command: &[u8]) -> (&[u8], &[u8]) {
    let command = command.trim_ascii_start();
    if let Some(rest) = command.strip_prefix(b"\"") {
        // An unterminated quote makes Windows take the rest of the line as
        // the program.
        return rest
            .iter()
            .position(|byte| *byte == b'\"')
            .map_or((rest, &[][..]), |end| {
                (&rest[..end], rest[end + 1..].trim_ascii_start())
            });
    }
    command
        .iter()
        .position(|byte| *byte == b' ')
        .map_or((command, &[][..]), |end| {
            (&command[..end], command[end..].trim_ascii_start())
        })
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
fn configure_output(
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

#[cfg(not(unix))]
fn run_shell(
    command: &BString,
    working_directory: &Path,
    shell: &ShellMode,
    environment: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
    use_console: bool,
    dryrun: bool,
    started: impl FnOnce(u32, bool),
) -> Result<Option<ProcessOutput>, ShellFailure> {
    if dryrun {
        return Ok(None);
    }
    let mut child = shell_command(command, working_directory, shell, environment);
    if use_console {
        let mut child = child.spawn().map_err(|source| ShellFailure {
            operation: ShellOperation::Spawn,
            source,
        })?;
        started(child.id(), false);
        let status = child.wait().map_err(|source| ShellFailure {
            operation: ShellOperation::Wait,
            source,
        })?;
        Ok(Some(ProcessOutput {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }))
    } else {
        child.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = child.spawn().map_err(|source| ShellFailure {
            operation: ShellOperation::Spawn,
            source,
        })?;
        started(child.id(), false);
        let result = child.wait_with_output().map_err(|source| ShellFailure {
            operation: ShellOperation::Wait,
            source,
        })?;
        Ok(Some(ProcessOutput {
            status: result.status,
            stdout: result.stdout,
            stderr: result.stderr,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // [spec:ronin:req:product.command-execution/test]
    #[test]
    fn a_windows_command_line_keeps_a_quoted_program_whole() {
        // The reason this cannot reuse the POSIX splitting: a quoted program
        // path would be cut at the space inside it.
        assert_eq!(
            windows_program_and_arguments(br#""C:\Program Files\x\cl.exe" /c a.c"#),
            (&br"C:\Program Files\x\cl.exe"[..], &b"/c a.c"[..])
        );
        assert_eq!(
            windows_program_and_arguments(b"cl.exe /c a.c"),
            (&b"cl.exe"[..], &b"/c a.c"[..])
        );
        assert_eq!(
            windows_program_and_arguments(b"cl.exe"),
            (&b"cl.exe"[..], &b""[..])
        );
        // Windows takes an unterminated quote as running to the end.
        assert_eq!(
            windows_program_and_arguments(br#""C:\x\cl.exe /c"#),
            (&br"C:\x\cl.exe /c"[..], &b""[..])
        );
        assert_eq!(
            windows_program_and_arguments(b"  cl.exe  /c"),
            (&b"cl.exe"[..], &b"/c"[..])
        );
    }

    #[cfg(unix)]
    // [spec:ronin:req:compat.process-integration/test]
    // [spec:ronin:req:product.build-outcome/test]
    #[test]
    fn a_finished_child_is_read_the_way_ninja_reads_it() {
        use std::os::unix::process::ExitStatusExt as _;
        let status = std::process::ExitStatus::from_raw;

        // A child that exited reports its own code, which is the number a
        // caller's CI reads to tell a compile error from an OOM kill.
        assert_eq!(exit_status_code(status(7 << 8)), 7);
        assert_eq!(exit_status_code(status(0)), 0);

        // The signals that mean the build is being brought down.
        for signal in [
            libc_signal::SIGINT,
            libc_signal::SIGTERM,
            libc_signal::SIGHUP,
        ] {
            assert!(status_interrupted(status(signal)));
            assert_eq!(exit_status_code(status(signal)), INTERRUPTED_EXIT_CODE);
        }

        // SIGQUIT is not one of them: Ronin handles it when it arrives here,
        // but in a child it is an ordinary failure.
        let quit = 3;
        assert!(!status_interrupted(status(quit)));
        assert_eq!(exit_status_code(status(quit)), 131);

        // Ninja adds 128 to the raw wait status rather than to the signal, so a
        // dumping SIGQUIT reports 259 — reproduced because it is visible in the
        // `FAILED: [code=…]` line.
        assert_eq!(exit_status_code(status(quit | 0x80)), 259);
        assert_eq!(exit_status_code(status(9)), 137);
    }

    #[test]
    fn a_child_is_only_moved_to_a_directory_it_is_not_already_in() {
        let process = std::env::current_dir().unwrap();

        // The binary's root is the process's own directory, so nothing is
        // imposed and `Command` keeps the `posix_spawn` path.
        assert_eq!(directory_to_impose(&process), PathBuf::new());
        assert_eq!(directory_to_impose(Path::new("")), PathBuf::new());

        // A runner rooted elsewhere still moves its children, which is what
        // lets several of them share one process.
        let elsewhere = process.join("a-directory-the-process-is-not-in");
        assert_eq!(directory_to_impose(&elsewhere), elsewhere);
    }

    #[cfg(target_os = "linux")]
    fn thread_count() -> usize {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("Threads:"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn evented_supervisor_scales_without_a_thread_per_child() {
        const CHILDREN: usize = 64;

        let before = thread_count();
        let mut supervisor = ProcessSupervisor::<()>::new().unwrap();
        for index in 0..CHILDREN {
            supervisor
                .spawn(
                    EdgeId::from_event_key(index + 1).expect("test edge key is nonzero"),
                    Launch::Shell(BString::from("sleep 0.05; printf x")),
                    false,
                    false,
                )
                .unwrap();
        }
        let during = thread_count();
        assert!(
            during <= before + 8,
            "evented supervision added {} threads",
            during - before
        );

        let mut completed = 0;
        while supervisor.running_len() != 0 {
            let Some(SupervisorWake::Process(completion)) = supervisor.wait(None).unwrap() else {
                continue;
            };
            assert_eq!(completion.result.unwrap().unwrap().stdout, b"x");
            completed += 1;
        }
        assert_eq!(completed, CHILDREN);
    }

    #[cfg(target_os = "linux")]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn dropping_the_supervisor_terminates_process_groups_and_reaps_children() {
        let edge = EdgeId::from_event_key(11 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::<()>::new().unwrap();
        supervisor
            .spawn(
                edge,
                Launch::Shell(BString::from("sleep 30 & wait")),
                false,
                false,
            )
            .unwrap();
        let pid = supervisor.children[&edge].child.id();
        let children_path = format!("/proc/{pid}/task/{pid}/children");
        let descendant = (0..100)
            .find_map(|_| {
                let child = std::fs::read_to_string(&children_path)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .and_then(|pid| pid.parse::<u32>().ok());
                if child.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                child
            })
            .expect("the shell started its child");
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
        assert!(std::path::Path::new(&format!("/proc/{descendant}")).exists());

        drop(supervisor);

        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        let descendant_stopped = (0..100).any(|_| {
            let status = std::fs::read_to_string(format!("/proc/{descendant}/status"));
            let stopped = status.as_ref().map_or(true, |status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("State:"))
                    .is_some_and(|state| state.trim_start().starts_with('Z'))
            });
            if !stopped {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            stopped
        });
        assert!(descendant_stopped, "the process-group child remained live");
    }

    #[cfg(unix)]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn external_events_wake_the_blocked_supervisor() {
        let mut supervisor = ProcessSupervisor::<usize>::new().unwrap();
        let sender = supervisor.external_sender();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let sender_barrier = barrier.clone();
        let thread = std::thread::spawn(move || {
            sender_barrier.wait();
            std::thread::sleep(std::time::Duration::from_millis(10));
            sender.send(17);
        });

        barrier.wait();
        let wake = supervisor
            .wait(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        assert!(matches!(wake, Some(SupervisorWake::External(17))));
        thread.join().unwrap();
    }

    /// An external event arriving while the poller is reporting a child's
    /// output closing must not cost that report.
    ///
    /// Each descriptor is armed for one event and rearmed by the drain, so an
    /// event abandoned in favour of the external one is never delivered again:
    /// the child is never reaped and a supervisor with nothing else to wake it
    /// waits forever. That deadlock was reachable from any build under an
    /// inherited jobserver, where token arrivals and completions interleave
    /// constantly.
    #[cfg(unix)]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn a_completion_survives_an_external_event_in_the_same_wait() {
        let edge = EdgeId::from_event_key(13 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::<usize>::new().unwrap();
        let sender = supervisor.external_sender();
        let thread = std::thread::spawn(move || {
            // Long enough to land while the poller is blocked, short enough to
            // be there before the child's output closes.
            std::thread::sleep(std::time::Duration::from_millis(50));
            sender.send(23);
        });
        supervisor
            .spawn(
                edge,
                Launch::Shell(BString::from("sleep 0.1; printf done")),
                false,
                false,
            )
            .unwrap();

        let mut external = None;
        let mut completion = None;
        for _ in 0..3 {
            match supervisor
                .wait(Some(std::time::Duration::from_secs(2)))
                .unwrap()
            {
                Some(SupervisorWake::Process(reported)) => {
                    completion = Some(reported);
                    break;
                }
                Some(SupervisorWake::External(value)) => external = Some(value),
                None => {}
            }
        }
        thread.join().unwrap();
        assert_eq!(external, Some(23));
        let completion = completion.expect("the child that finished was reported");
        assert_eq!(completion.edge, edge);
        assert_eq!(completion.result.unwrap().unwrap().stdout, b"done");
    }

    #[cfg(unix)]
    #[test]
    // [spec:ronin:req:compat.process-integration/test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn ronin_process_supervisor_reports_keyed_signal_completion() {
        let edge = EdgeId::from_event_key(7 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::new().unwrap();
        supervisor
            .spawn(
                edge,
                Launch::Shell(BString::from("kill -INT $$")),
                false,
                false,
            )
            .unwrap();
        let completion = supervisor
            .wait(None)
            .unwrap()
            .map(|wake| match wake {
                SupervisorWake::Process(completion) => completion,
                SupervisorWake::External(()) => unreachable!("no external event sender"),
            })
            .expect("the subprocess produces a completion");
        assert_eq!(completion.edge, edge);
        let output = completion.result.unwrap().unwrap();
        assert!(status_interrupted(output.status));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }

    #[cfg(unix)]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn ronin_process_supervisor_preserves_stdout_stderr_order() {
        let edge = EdgeId::from_event_key(9 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::new().unwrap();
        supervisor
            .spawn(
                edge,
                Launch::Shell(BString::from("printf out; printf err >&2; printf end")),
                false,
                false,
            )
            .unwrap();
        let completion = supervisor
            .wait(None)
            .unwrap()
            .map(|wake| match wake {
                SupervisorWake::Process(completion) => completion,
                SupervisorWake::External(()) => unreachable!("no external event sender"),
            })
            .expect("the subprocess produces a completion");
        let output = completion.result.unwrap().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"outerrend");
        assert!(output.stderr.is_empty());
    }
}

#[cfg(test)]
mod launcher_tests {
    use super::{direct_argv, needs_shell};

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn a_plain_command_does_not_need_a_shell() {
        assert!(!needs_shell(b"touch jobs/0"));
        assert!(!needs_shell(b"/usr/bin/c++ -DFOO=1 -O3 -c a.cc -o a.o"));
        assert!(!needs_shell(b"cp  a   b"));
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn anything_the_shell_would_interpret_keeps_the_shell() {
        for command in [
            &b"cd x && y"[..],
            b"a; b",
            b"a | b",
            b"a > out",
            b"a < in",
            b"echo \"hi\"",
            b"echo 'hi'",
            b"ls *.c",
            b"ls a?.c",
            b"echo $HOME",
            b"echo `date`",
            b"echo ~",
            b"a # comment",
            b"a\tb",
            b"a\nb",
            b"(a)",
            b"a \\ b",
        ] {
            assert!(
                needs_shell(command),
                "{:?} must keep the shell",
                String::from_utf8_lossy(command)
            );
        }
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn a_leading_assignment_is_the_shells_business() {
        // `execvp` would look for a program literally named `FOO=1`.
        assert!(needs_shell(b"FOO=1 cmd"));
        assert!(needs_shell(b"A=b C=d cmd"));
        // Not an assignment: the word is an argument, not the program.
        assert!(!needs_shell(b"cmd FOO=1"));
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn an_empty_command_is_left_to_the_shell() {
        assert!(needs_shell(b""));
        assert!(needs_shell(b"   "));
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn a_leading_exec_needs_the_shell() {
        // `exec` is the shell's builtin for "run this in my place", and
        // spawning it directly would look for a program by that name.
        assert!(needs_shell(b"exec /bin/sh recipe.rsp"));
        // Not the builtin: the word is an argument or part of a longer name.
        assert!(!needs_shell(b"/usr/bin/exec-thing a"));
        assert!(!needs_shell(b"cmd exec"));
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn splitting_collapses_runs_of_spaces_as_the_shell_would() {
        assert_eq!(direct_argv(b"cp  a   b"), vec![&b"cp"[..], b"a", b"b"]);
        assert_eq!(direct_argv(b" touch x "), vec![&b"touch"[..], b"x"]);
    }
}
