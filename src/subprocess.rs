//! Shell subprocess execution and completion-set bookkeeping.

use crate::error::{ProcessError, ShellOperation};
use crate::graph::EdgeId;
use crate::signal::Signal;
use crate::util::{BString, ByteSlice};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

/// Making the `clone` call off the thread that schedules. See
/// [`spawn::SpawnPool`], whose whole reason for existing is documented there.
mod spawn;
use spawn::SpawnContext;
#[cfg(unix)]
use spawn::{MAX_SPAWNERS, SpawnPool, SpawnRequest, spawner_loop};

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
    /// A line that cannot be started at all, and what to say about it.
    ///
    /// Everything about the command is settled except something only a process
    /// needs — a value for the environment it would have run in, which the
    /// front end could not read. A run that never starts this line never needs
    /// it, which is why the refusal travels with the launch instead of having
    /// stopped the read: it is charged where a process would have been.
    ///
    /// The text is the front end's own diagnostic, already rendered.
    Refused(String),
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
    /// Whether the build stands in for this line instead of starting it, which
    /// is GNU Make's empty command.
    ///
    /// `start_job_command` (job.c) takes a Bourne-compatible shell asked to run
    /// exactly `:` to the next command line rather than forking — "People use
    /// this for timestamp rules, so avoid forking a useless shell" — after the
    /// line has been echoed and counted as started, and before it builds any
    /// environment for it. The front end decides it, because it is a fact about
    /// the argument list GNU Make would have assembled; the engine acts on it
    /// without knowing what a Makefile is, the way it acts on
    /// [`crate::build::LateStep::runs_while_pretending`].
    pub(crate) starts_no_process: bool,
}

impl Launch {
    /// Why this launch cannot be made, for a run that is about to make it.
    ///
    /// `None` for every launch that can be started, and for a refused one the
    /// build is standing in for rather than running — GNU Make reads the value
    /// this is about where it builds the environment for a job it is starting,
    /// so a job it does not start never asks.
    pub(crate) fn refusal(&self, stood_in_for: bool) -> Option<String> {
        match self {
            Self::Refused(diagnostic) if !stood_in_for => Some(diagnostic.clone()),
            _ => None,
        }
    }

    /// Whether the build stands in for this line rather than starting it. See
    /// [`DirectLaunch::starts_no_process`].
    pub(crate) fn is_the_empty_command(&self) -> bool {
        matches!(self, Self::Direct(direct) if direct.starts_no_process)
    }

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
                    rendered.extend_from_slice(word);
                }
                BString::from(rendered)
            }
            // Nothing was started, so nothing has a command line to be named
            // by. The refusal itself is what gets reported.
            Self::Refused(_) => BString::default(),
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
    /// A launch a spawner thread has finished making, on its way back to the
    /// thread that schedules. See [`SpawnPool`].
    #[cfg(unix)]
    Spawned {
        edge: EdgeId,
        command: BString,
        use_console: bool,
        started: ProcessResult<Started>,
    },
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

/// How long a command that has been sent the interrupt is given to take it
/// before it is killed.
///
/// Not a deadline on anything a caller reads. Whether the command dies of the
/// signal within this or of the kill after it, the edge is not reported and
/// what it wrote is withdrawn either way, so no answer depends on the number.
/// What it buys is the command's own handler — a compiler unlinking the object
/// file it was half-way through — and it is short enough that a build tool
/// asked to stop does.
#[cfg(unix)]
const INTERRUPT_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

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
    context: Arc<SpawnContext>,
    /// Threads that make the `clone` call, so the scheduler does not wear the
    /// `vfork` suspension. See [`SpawnPool`].
    #[cfg(unix)]
    spawners: SpawnPool,
    /// Launches handed to [`Self::spawners`] and not yet reported back.
    ///
    /// Counted rather than inferred, because the two things that must not
    /// happen before every one of them has landed are stopping the build and
    /// dropping the supervisor: a child whose parent never learned it exists
    /// is a child nothing will kill.
    #[cfg(unix)]
    pending_spawns: usize,
    /// External events already taken off the channel and not yet handed back.
    ///
    /// The channel carries started launches as well as these, and a launch left
    /// sitting in it is a child whose output nothing is polling — so every look
    /// at the channel drains it completely, and what the caller has not asked
    /// for yet waits here instead of there.
    #[cfg(unix)]
    pending_external: VecDeque<External>,
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
            context: Arc::new(SpawnContext {
                shell,
                working_directory: directory_to_impose(working_directory),
                environment: environment.to_vec(),
            }),
            #[cfg(unix)]
            spawners: SpawnPool::default(),
            #[cfg(unix)]
            pending_spawns: 0,
            #[cfg(unix)]
            pending_external: VecDeque::new(),
        })
    }

    fn completion(&mut self, completion: ProcessCompletion) -> SupervisorWake<External> {
        debug_assert!(self.running > 0);
        self.running -= 1;
        SupervisorWake::Process(completion)
    }

    /// Take everything the channel is holding, and answer with the next
    /// external event if one of it was for the caller.
    ///
    /// Draining rather than peeking is load-bearing. A started launch arriving
    /// here is a child that exists and whose output descriptor is not in the
    /// poller yet; leaving it in the channel to return an external event
    /// sooner would leave the build with nothing to wake it for that child, and
    /// the poller's notification for it already spent.
    #[cfg(unix)]
    fn try_channel(&mut self) -> ProcessResult<Option<SupervisorWake<External>>> {
        loop {
            match self.receiver.try_recv() {
                Ok(ProcessEvent::External(event)) => self.pending_external.push_back(event),
                Ok(ProcessEvent::Spawned {
                    edge,
                    command,
                    use_console,
                    started,
                }) => self.receive_spawned(edge, command, use_console, started),
                Err(mpsc::TryRecvError::Empty) => {
                    return Ok(self
                        .pending_external
                        .pop_front()
                        .map(SupervisorWake::External));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(ProcessError::CompletionChannelDisconnected);
                }
            }
        }
    }

    /// Wait for every launch already handed to a spawner to be reported back.
    ///
    /// What the build does next is stop children or drop the supervisor, and
    /// both walk the live-child table. A launch still in flight is not in it,
    /// so a build that did not wait here would leave the command it started
    /// last running behind it.
    #[cfg(unix)]
    fn settle_pending_spawns(&mut self) {
        while self.pending_spawns > 0 {
            match self.receiver.recv() {
                Ok(ProcessEvent::External(event)) => self.pending_external.push_back(event),
                Ok(ProcessEvent::Spawned {
                    edge,
                    command,
                    use_console,
                    started,
                }) => self.receive_spawned(edge, command, use_console, started),
                Err(mpsc::RecvError) => break,
            }
        }
    }

    /// Take a launch a spawner has made into the live-child table.
    ///
    /// Everything a started child needs from the scheduling thread happens
    /// here — the poller registration, the interrupt a build already under one
    /// owes it, the completion a launch that never started answers with — so
    /// that the thread which made it did nothing but make it.
    #[cfg(unix)]
    fn receive_spawned(
        &mut self,
        edge: EdgeId,
        command: BString,
        use_console: bool,
        started: ProcessResult<Started>,
    ) {
        use polling::Event;

        debug_assert!(
            self.pending_spawns > 0,
            "a reply answers a submitted launch"
        );
        self.pending_spawns = self.pending_spawns.saturating_sub(1);
        let (child, output) = match started {
            Ok(Started::Running(child, output)) => (child, output),
            Ok(Started::NeverStarted(reported)) => {
                self.ready.push_back(ProcessCompletion {
                    edge,
                    result: Ok(Some(reported)),
                });
                return;
            }
            // Reported as the command's own answer rather than returned to the
            // caller, because the caller has already been told the launch was
            // accepted. The build reads it out of the completion and settles
            // the edge with it, which is what it did with the returned error.
            Err(error) => {
                self.ready.push_back(ProcessCompletion {
                    edge,
                    result: Err(error),
                });
                return;
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
            self.ready.push_back(ProcessCompletion {
                edge,
                result: Err(error),
            });
            return;
        }
        let previous = self.children.insert(edge, child);
        debug_assert!(previous.is_none(), "an edge cannot run twice concurrently");

        if use_console {
            self.reap_candidates.push(edge);
            self.reap_backoff = MIN_REAP_INTERVAL;
            return;
        }
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
            self.ready.push_back(ProcessCompletion {
                edge,
                result: Err(ProcessError::Shell {
                    edge,
                    command: child.command,
                    operation: ShellOperation::RegisterOutput,
                    source,
                }),
            });
            return;
        }
        self.children
            .get_mut(&edge)
            .expect("the registered child remains present")
            .registered = true;
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

// [spec:ronin:req:compat.process-integration+2]
impl<External: Send + 'static> ProcessSupervisor<External> {
    /// Ask for a command to be run. Asking cannot fail.
    ///
    /// A launch that could not be made is the COMMAND's failure, not the
    /// request's: it comes back as that edge's completion carrying the error,
    /// the way a command that ran and said no comes back carrying its status.
    /// That is what lets the launch happen on another thread — the caller is
    /// told the request was taken, and told separately how it went.
    pub(crate) fn spawn(&mut self, edge: EdgeId, launch: Launch, use_console: bool, dryrun: bool) {
        #[cfg(unix)]
        {
            self.spawn_evented(edge, launch, use_console, dryrun);
        }
        #[cfg(not(unix))]
        {
            let Launch::Shell(command) = launch else {
                unreachable!("a direct launch is decided per recipe line, and only on Unix")
            };
            let sender = self.sender.clone();
            let working_directory = self.context.working_directory.clone();
            let shell = self.context.shell.clone();
            let environment = self.context.environment.clone();
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
        }
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

    /// Stop every command still running, and reap it.
    ///
    /// Ninja does this in `SubprocessSet::Clear`, reached from
    /// `Builder::Cleanup` the moment the build loop is handed the interrupt
    /// back: each running group is signalled, and then every one of them is
    /// waited for. That wait has no bound, so a command which declines the
    /// signal — or whose shell took it between two of the command lines it was
    /// given and carried on to the next — holds the build until it finishes of
    /// its own accord. The same signal has already been delivered here by
    /// [`Self::interrupt`], the same chance to take it is given, and what is
    /// still standing after it is killed: a build tool must not be held by the
    /// thing it has just stopped.
    ///
    /// Reaping happens before any output is withdrawn, in that order and for
    /// that reason: a command still running is a command that can still write
    /// the file being taken back.
    pub(crate) fn stop(&mut self) {
        #[cfg(unix)]
        {
            // A command being launched right now is one this is about to be
            // asked to have stopped, so it has to exist before the walk.
            self.settle_pending_spawns();
            let grace = std::time::Instant::now() + INTERRUPT_GRACE;
            let mut backoff = MIN_REAP_INTERVAL;
            while !self.children.is_empty() && std::time::Instant::now() < grace {
                let taken = self
                    .children
                    .iter_mut()
                    .filter_map(|(edge, child)| {
                        matches!(child.child.try_wait(), Ok(Some(_))).then_some(*edge)
                    })
                    .collect::<Vec<_>>();
                for edge in taken {
                    self.discard(edge);
                }
                if self.children.is_empty() {
                    break;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(MAX_REAP_INTERVAL);
            }
            let declined = self.children.keys().copied().collect::<Vec<_>>();
            for edge in declined {
                self.discard(edge);
            }
        }
    }
}

// [spec:ronin:req:runtime.process-supervisor-scalability]
#[cfg(unix)]
impl<External: Send + 'static> ProcessSupervisor<External> {
    /// Forget one child: unregister whatever it was writing to, make sure it
    /// is dead, and reap it. What it had left to say is dropped with it —
    /// nothing reports an edge the build has already stopped.
    fn discard(&mut self, edge: EdgeId) {
        let Some(mut child) = self.children.remove(&edge) else {
            return;
        };
        if child.registered
            && let Some(output) = child.output.as_ref()
        {
            let _ = self.poller.delete(output);
        }
        terminate_and_reap(&mut child);
        self.running = self.running.saturating_sub(1);
    }

    fn spawn_evented(&mut self, edge: EdgeId, launch: Launch, use_console: bool, dryrun: bool) {
        if dryrun {
            self.running += 1;
            self.ready.push_back(ProcessCompletion {
                edge,
                result: Ok(None),
            });
            return;
        }

        let command = launch.rendered();
        let direct = match launch {
            Launch::Shell(_) => None,
            Launch::Direct(direct) => Some(direct),
            // Refused before the supervisor is reached: the build charges the
            // refusal to the run and never asks for a process.
            Launch::Refused(_) => unreachable!("a refused launch never reaches a spawn"),
        };
        // The launch counts against the job budget from the moment it is
        // asked for, whichever thread ends up making it: a build that only
        // counted started children would ask for another eight while the first
        // eight were still being made.
        self.running += 1;
        self.pending_spawns += 1;
        // Made here when there is nothing to overlap it with. That is every
        // `-j1` build, whose one thread has nothing else to do while its one
        // command starts, and every console command, which the build loop only
        // ever starts with the job budget empty. Handing either to a spawner
        // would buy nothing and cost a wake-up.
        if use_console || self.running == 1 {
            let started = self
                .context
                .start(edge, &command, direct.as_deref(), use_console);
            self.receive_spawned(edge, command, use_console, started);
            return;
        }
        if let Some(request) = self.submit_spawn(SpawnRequest {
            edge,
            command,
            direct,
            use_console,
        }) {
            // No spawner took it and none could be made — a host out of
            // threads. The launch is still owed an answer, so it is made here
            // rather than left in a queue nothing will read.
            let started = self.context.start(
                request.edge,
                &request.command,
                request.direct.as_deref(),
                use_console,
            );
            self.receive_spawned(request.edge, request.command, use_console, started);
        }
    }

    /// Hand a launch to a spawner, making one more if none is free to take it.
    ///
    /// Grown on demand rather than sized up front: a build whose commands are
    /// long enough to keep the budget full on its own never needs a second
    /// thread, and one whose commands are `touch` gets as many as the burst
    /// asks for, up to [`MAX_SPAWNERS`]. Answers with the launch again when
    /// there is no thread to make it on, which is the caller's to make itself.
    fn submit_spawn(&mut self, request: SpawnRequest) -> Option<SpawnRequest> {
        let queue = self.spawners.queue.clone();
        let grow = {
            let mut state = queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.waiting.push_back(request);
            state.waiting.len() > state.idle && self.spawners.workers.len() < MAX_SPAWNERS
        };
        if grow {
            let spawner = {
                let queue = queue.clone();
                let context = self.context.clone();
                let sender = self.sender.clone();
                let poller = self.poller.clone();
                std::thread::Builder::new()
                    .name(String::from("ronin-spawn"))
                    .spawn(move || spawner_loop(&queue, &context, &sender, &poller))
            };
            match spawner {
                Ok(worker) => self.spawners.workers.push(worker),
                Err(_) if self.spawners.workers.is_empty() => {
                    let mut state = queue
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    return state.waiting.pop_back();
                }
                Err(_) => {}
            }
        }
        queue.ready.notify_one();
        None
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
        // Before anything is torn down: a child a spawner made and has not
        // reported yet is a child this walk would not see, and nothing else
        // would ever kill it.
        self.settle_pending_spawns();
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
// [spec:ronin:req:compat.process-integration+2]
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
// [spec:ronin:req:compat.process-integration+2]
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
    // [spec:ronin:req:compat.process-integration+2/test]
    // [spec:ronin:req:product.build-outcome+1/test]
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
            supervisor.spawn(
                EdgeId::from_event_key(index + 1).expect("test edge key is nonzero"),
                Launch::Shell(BString::from("sleep 0.05; printf x")),
                false,
                false,
            );
        }
        let during = thread_count();
        // The supervisor's own threads, which is the number the requirement is
        // about: a constant that does not read the child count.
        assert!(
            supervisor.spawners.workers.len() <= MAX_SPAWNERS,
            "supervision kept {} spawners for {CHILDREN} children",
            supervisor.spawners.workers.len()
        );
        // And the process did not grow a thread per child. Deliberately loose:
        // the count is process-wide and the tests beside this one hold
        // supervisors of their own, each entitled to its own handful of
        // spawners. Still nowhere near the sixty-four a thread per child costs.
        assert!(
            during < before + CHILDREN / 2,
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

    /// A launch a spawner is still making is a child the supervisor owns.
    ///
    /// The `clone` call happens on another thread now, so between the request
    /// and the reply there is a window in which the child exists and the
    /// live-child table does not know it. Everything that walks that table —
    /// stopping the build, dropping the supervisor — has to wait for the
    /// window to close first, or it walks past a running command and leaves it
    /// behind. Every launch here but the first is made on a spawner.
    #[cfg(target_os = "linux")]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn a_launch_still_being_made_is_reaped() {
        const CHILDREN: usize = 8;

        let mut pids = Vec::new();
        {
            let mut supervisor = ProcessSupervisor::<()>::new().unwrap();
            for index in 0..CHILDREN {
                supervisor.spawn(
                    EdgeId::from_event_key(index + 1).expect("test edge key is nonzero"),
                    Launch::Shell(BString::from("sleep 30")),
                    false,
                    false,
                );
            }
            // Dropped with the requests barely older than the loop that made
            // them, which is the window the settling exists for.
            supervisor.settle_pending_spawns();
            pids.extend(supervisor.children.values().map(|child| child.child.id()));
            assert_eq!(pids.len(), CHILDREN, "every launch reached the child table");
        }
        for pid in pids {
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "a command left behind by the drop is still running"
            );
        }
    }

    /// A launch that could not be made is the command's answer, not the
    /// request's refusal — the contract that lets the `clone` happen off the
    /// scheduling thread. The first child holds the budget open so the failing
    /// one is made on a spawner rather than in place.
    #[cfg(target_os = "linux")]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn an_unmakeable_launch_becomes_a_completion() {
        let holder = EdgeId::from_event_key(1).expect("test edge key is nonzero");
        let refused = EdgeId::from_event_key(2).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::<()>::in_directory(
            Path::new(""),
            ShellMode::Program(PathBuf::from("/nonexistent/shell")),
            &[],
        )
        .unwrap();
        supervisor.spawn(
            holder,
            Launch::Direct(Box::new(direct_true())),
            false,
            false,
        );
        supervisor.spawn(refused, Launch::Shell(BString::from("true")), false, false);

        let mut reported = None;
        while supervisor.running_len() != 0 {
            let Some(SupervisorWake::Process(completion)) = supervisor.wait(None).unwrap() else {
                continue;
            };
            if completion.edge == refused {
                reported = Some(completion.result);
            }
        }
        let Err(error) = reported.expect("the refused edge was reported") else {
            panic!("a shell that is not there cannot have run");
        };
        assert!(
            matches!(
                error,
                ProcessError::Shell {
                    operation: ShellOperation::Spawn,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[cfg(target_os = "linux")]
    fn direct_true() -> DirectLaunch {
        DirectLaunch {
            argv: vec![BString::from("/bin/sleep"), BString::from("0.2")],
            directory: PathBuf::new(),
            environment: Vec::new(),
            diagnostic_prefix: String::new(),
            starts_no_process: false,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn dropping_the_supervisor_terminates_process_groups_and_reaps_children() {
        let edge = EdgeId::from_event_key(11 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::<()>::new().unwrap();
        supervisor.spawn(
            edge,
            Launch::Shell(BString::from("sleep 30 & wait")),
            false,
            false,
        );
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
        supervisor.spawn(
            edge,
            Launch::Shell(BString::from("sleep 0.1; printf done")),
            false,
            false,
        );

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
    // [spec:ronin:req:compat.process-integration+2/test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn ronin_process_supervisor_reports_keyed_signal_completion() {
        let edge = EdgeId::from_event_key(7 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::new().unwrap();
        supervisor.spawn(
            edge,
            Launch::Shell(BString::from("kill -INT $$")),
            false,
            false,
        );
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
        supervisor.spawn(
            edge,
            Launch::Shell(BString::from("printf out; printf err >&2; printf end")),
            false,
            false,
        );
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

    // [spec:ronin:req:compat.process-integration+2/test]
    #[test]
    fn a_plain_command_does_not_need_a_shell() {
        assert!(!needs_shell(b"touch jobs/0"));
        assert!(!needs_shell(b"/usr/bin/c++ -DFOO=1 -O3 -c a.cc -o a.o"));
        assert!(!needs_shell(b"cp  a   b"));
    }

    // [spec:ronin:req:compat.process-integration+2/test]
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

    // [spec:ronin:req:compat.process-integration+2/test]
    #[test]
    fn a_leading_assignment_is_the_shells_business() {
        // `execvp` would look for a program literally named `FOO=1`.
        assert!(needs_shell(b"FOO=1 cmd"));
        assert!(needs_shell(b"A=b C=d cmd"));
        // Not an assignment: the word is an argument, not the program.
        assert!(!needs_shell(b"cmd FOO=1"));
    }

    // [spec:ronin:req:compat.process-integration+2/test]
    #[test]
    fn an_empty_command_is_left_to_the_shell() {
        assert!(needs_shell(b""));
        assert!(needs_shell(b"   "));
    }

    // [spec:ronin:req:compat.process-integration+2/test]
    #[test]
    fn a_leading_exec_needs_the_shell() {
        // `exec` is the shell's builtin for "run this in my place", and
        // spawning it directly would look for a program by that name.
        assert!(needs_shell(b"exec /bin/sh recipe.rsp"));
        // Not the builtin: the word is an argument or part of a longer name.
        assert!(!needs_shell(b"/usr/bin/exec-thing a"));
        assert!(!needs_shell(b"cmd exec"));
    }

    // [spec:ronin:req:compat.process-integration+2/test]
    #[test]
    fn splitting_collapses_runs_of_spaces_as_the_shell_would() {
        assert_eq!(direct_argv(b"cp  a   b"), vec![&b"cp"[..], b"a", b"b"]);
        assert_eq!(direct_argv(b" touch x "), vec![&b"touch"[..], b"x"]);
    }
}
