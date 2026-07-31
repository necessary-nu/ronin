//! Shell subprocess execution and completion-set bookkeeping.

use crate::error::{ProcessError, ShellOperation};
use crate::graph::EdgeId;
use crate::signal::Signal;
use crate::util::{BString, ByteSlice};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(unix)]
use std::sync::Arc;

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
    output: Option<std::os::unix::net::UnixStream>,
    output_bytes: Vec<u8>,
    registered: bool,
}

#[cfg(unix)]
const SIGNAL_EVENT_KEY: usize = 0;

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
    #[cfg(unix)]
    children: HashMap<EdgeId, RunningChild>,
    #[cfg(not(unix))]
    children: HashMap<EdgeId, (u32, bool)>,
    interrupted: Option<Signal>,
    working_directory: PathBuf,
}

impl<External> ProcessSupervisor<External> {
    #[cfg(test)]
    pub(crate) fn new() -> ProcessResult<Self> {
        Self::in_directory(Path::new(""))
    }

    pub(crate) fn in_directory(working_directory: &Path) -> ProcessResult<Self> {
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
            children: HashMap::new(),
            interrupted: None,
            working_directory: working_directory.to_owned(),
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

// [spec:samurai:req:compat.process-integration]
impl<External: Send + 'static> ProcessSupervisor<External> {
    pub(crate) fn spawn(
        &mut self,
        edge: EdgeId,
        command: BString,
        use_console: bool,
        dryrun: bool,
    ) -> ProcessResult<()> {
        #[cfg(unix)]
        {
            self.spawn_evented(edge, command, use_console, dryrun)
        }
        #[cfg(not(unix))]
        {
            let sender = self.sender.clone();
            let working_directory = self.working_directory.clone();
            self.running += 1;
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_shell(
                        &command,
                        &working_directory,
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

// [spec:samurai:req:runtime.process-supervisor-scalability]
#[cfg(unix)]
impl<External: Send + 'static> ProcessSupervisor<External> {
    fn spawn_evented(
        &mut self,
        edge: EdgeId,
        command: BString,
        use_console: bool,
        dryrun: bool,
    ) -> ProcessResult<()> {
        use polling::Event;
        use std::os::unix::process::CommandExt;

        if dryrun {
            self.running += 1;
            self.ready.push_back(ProcessCompletion {
                edge,
                result: Ok(None),
            });
            return Ok(());
        }

        let mut shell = shell_command(&command, &self.working_directory);
        if !use_console {
            shell.process_group(0);
        }
        let output = configure_output(&mut shell, edge, &command, use_console)?;
        let child = shell.spawn().map_err(|source| ProcessError::Shell {
            edge,
            command: command.clone(),
            operation: ShellOperation::Spawn,
            source,
        })?;
        let process_group = !use_console;
        let mut child = RunningChild {
            child,
            command,
            process_group,
            output,
            output_bytes: Vec::new(),
            registered: false,
        };
        if let Some(signal) = self.interrupted {
            if let Err(error) = signal_process(child.child.id(), process_group, signal) {
                terminate_and_reap(&mut child);
                return Err(error);
            }
        }
        let previous = self.children.insert(edge, child);
        debug_assert!(previous.is_none(), "an edge cannot run twice concurrently");

        if use_console {
            self.reap_candidates.push(edge);
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
                || {
                    (!self.reap_candidates.is_empty())
                        .then_some(std::time::Duration::from_millis(10))
                },
                |deadline| Some(deadline.saturating_duration_since(std::time::Instant::now())),
            );
            self.events.clear();
            self.poller.wait(&mut self.events, wait).map_err(|source| {
                ProcessError::Supervisor {
                    operation: crate::error::SupervisorOperation::WaitForEvent,
                    source,
                }
            })?;
            if let Some(event) = self.try_channel()? {
                return Ok(Some(event));
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

        let drain = {
            let child = self
                .children
                .get_mut(&edge)
                .expect("poll events refer to live children");
            let output = child
                .output
                .as_mut()
                .expect("only captured output is registered");
            let mut buffer = [0; 16 * 1024];
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
        if child.registered {
            if let Some(output) = child.output.as_ref() {
                let _ = self.poller.delete(output);
            }
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
            if child.registered {
                if let Some(output) = child.output.as_ref() {
                    let _ = self.poller.delete(output);
                }
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

pub(crate) fn status_interrupted(status: std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .signal()
            .and_then(|raw| usize::try_from(raw).ok())
            .and_then(Signal::from_raw)
            .is_some()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

// [spec:samurai:def:os.osspawn-fn]
// [spec:samurai:sem:os.osspawn-fn]
// [spec:samurai:def:os-posix.osspawn-fn]
// [spec:samurai:sem:os-posix.osspawn-fn]
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

fn shell_command(command: &BString, working_directory: &Path) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell
        .arg("-c")
        .arg(command.to_os_str().expect("byte strings are valid on Unix"));
    #[cfg(unix)]
    shell.stdin(null_stdin());
    #[cfg(not(unix))]
    shell.stdin(Stdio::null());
    if !working_directory.as_os_str().is_empty() {
        shell.current_dir(working_directory);
    }
    shell
}

#[cfg(unix)]
fn configure_output(
    shell: &mut Command,
    edge: EdgeId,
    command: &BString,
    use_console: bool,
) -> ProcessResult<Option<std::os::unix::net::UnixStream>> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    if use_console {
        return Ok(None);
    }
    let (reader, writer) = UnixStream::pair().map_err(|source| ProcessError::Shell {
        edge,
        command: command.clone(),
        operation: ShellOperation::CreateOutputPipe,
        source,
    })?;
    reader
        .set_nonblocking(true)
        .map_err(|source| ProcessError::Shell {
            edge,
            command: command.clone(),
            operation: ShellOperation::ConfigureOutputPipe,
            source,
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
    use_console: bool,
    dryrun: bool,
    started: impl FnOnce(u32, bool),
) -> Result<Option<ProcessOutput>, ShellFailure> {
    if dryrun {
        return Ok(None);
    }
    let mut child = shell_command(command, working_directory);
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
    // [spec:samurai:req:runtime.process-supervisor-scalability/test]
    fn evented_supervisor_scales_without_a_thread_per_child() {
        const CHILDREN: usize = 64;

        let before = thread_count();
        let mut supervisor = ProcessSupervisor::<()>::new().unwrap();
        for index in 0..CHILDREN {
            supervisor
                .spawn(
                    EdgeId::from_event_key(index + 1).expect("test edge key is nonzero"),
                    BString::from("sleep 0.05; printf x"),
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
    // [spec:samurai:req:runtime.process-supervisor-scalability/test]
    fn dropping_the_supervisor_terminates_process_groups_and_reaps_children() {
        let edge = EdgeId::from_event_key(11 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::<()>::new().unwrap();
        supervisor
            .spawn(edge, BString::from("sleep 30 & wait"), false, false)
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
    // [spec:samurai:req:runtime.process-supervisor-scalability/test]
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

    #[cfg(unix)]
    #[test]
    // [spec:samurai:req:compat.process-integration/test]
    // [spec:samurai:req:runtime.process-supervisor-scalability/test]
    fn ronin_process_supervisor_reports_keyed_signal_completion() {
        let edge = EdgeId::from_event_key(7 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::new().unwrap();
        supervisor
            .spawn(edge, BString::from("kill -INT $$"), false, false)
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
    // [spec:samurai:req:runtime.process-supervisor-scalability/test]
    fn ronin_process_supervisor_preserves_stdout_stderr_order() {
        let edge = EdgeId::from_event_key(9 + 1).expect("test edge key is nonzero");
        let mut supervisor = ProcessSupervisor::new().unwrap();
        supervisor
            .spawn(
                edge,
                BString::from("printf out; printf err >&2; printf end"),
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
