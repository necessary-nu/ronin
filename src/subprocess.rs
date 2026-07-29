//! Shell subprocess execution and completion-set bookkeeping.

use crate::error::ProcessError;
use crate::graph::EdgeId;
use crate::util::{BString, ByteSlice};
use std::collections::HashMap;
use std::io;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

type ProcessResult<T> = Result<T, ProcessError>;

#[cfg(unix)]
mod signals {
    use std::os::raw::c_int;
    use std::sync::atomic::{AtomicI32, Ordering};

    static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
    const SIGNALS: [c_int; 4] = [1, 2, 3, 15];
    const SIG_DFL: usize = 0;
    const SIG_ERR: usize = usize::MAX;

    unsafe extern "C" {
        fn signal(signal: c_int, handler: usize) -> usize;
        fn raise(signal: c_int) -> c_int;
        fn kill(pid: c_int, signal: c_int) -> c_int;
    }

    extern "C" fn record_interrupt(signal: c_int) {
        INTERRUPTED.store(signal, Ordering::Relaxed);
    }

    pub(super) fn install() -> std::io::Result<()> {
        INTERRUPTED.store(0, Ordering::Relaxed);
        for signal_number in SIGNALS {
            if unsafe { signal(signal_number, record_interrupt as usize) } == SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub(super) fn interrupted() -> Option<i32> {
        match INTERRUPTED.load(Ordering::Relaxed) {
            0 => None,
            signal => Some(signal),
        }
    }

    pub(super) fn send(pid: u32, process_group: bool, signal_number: i32) {
        let pid = i32::try_from(pid).unwrap_or(i32::MAX);
        let target = if process_group { -pid } else { pid };
        unsafe {
            kill(target, signal_number);
        }
    }

    pub(super) fn reraise(signal_number: i32) -> ! {
        unsafe {
            signal(signal_number, SIG_DFL);
            raise(signal_number);
        }
        std::process::exit(128 + signal_number);
    }
}

/// Installs Ronin's process-interruption handlers.
///
/// Call this once in an embedding executable before invoking [`crate::run_os`].
pub fn install_signal_handlers() -> io::Result<()> {
    #[cfg(unix)]
    {
        signals::install()
    }
    #[cfg(not(unix))]
    {
        Ok(())
    }
}

/// Returns the operating-system signal most recently observed by Ronin.
pub fn interrupted_signal() -> Option<i32> {
    #[cfg(unix)]
    {
        signals::interrupted()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Restores the default handler and re-raises an observed signal.
///
/// This function does not return.
pub fn reraise_signal(signal: i32) -> ! {
    #[cfg(unix)]
    {
        signals::reraise(signal)
    }
    #[cfg(not(unix))]
    {
        std::process::exit(128 + signal);
    }
}

fn signal_process(pid: u32, process_group: bool, signal: i32) {
    #[cfg(unix)]
    signals::send(pid, process_group, signal);
    #[cfg(not(unix))]
    {
        let _ = (pid, process_group, signal);
    }
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

enum ProcessEvent {
    Started {
        edge: EdgeId,
        pid: u32,
        process_group: bool,
    },
    Finished(ProcessCompletion),
}

pub(crate) struct ProcessSupervisor {
    sender: Sender<ProcessEvent>,
    receiver: Receiver<ProcessEvent>,
    running: usize,
    children: HashMap<EdgeId, (u32, bool)>,
    interrupted: Option<i32>,
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            running: 0,
            children: HashMap::new(),
            interrupted: None,
        }
    }
}

// [spec:samurai:req:compat.process-integration]
impl ProcessSupervisor {
    pub(crate) fn spawn<'scope, 'environment: 'scope>(
        &mut self,
        scope: &'scope std::thread::Scope<'scope, 'environment>,
        edge: EdgeId,
        command: BString,
        use_console: bool,
        dryrun: bool,
    ) {
        let sender = self.sender.clone();
        self.running += 1;
        scope.spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_shell(&command, use_console, dryrun, |pid, process_group| {
                    let _ = sender.send(ProcessEvent::Started {
                        edge,
                        pid,
                        process_group,
                    });
                })
                .map_err(ProcessError::from)
            }))
            .unwrap_or_else(|_| Err("subcommand thread panicked".into()));
            let _ = sender.send(ProcessEvent::Finished(ProcessCompletion { edge, result }));
        });
    }

    pub(crate) fn wait(
        &mut self,
        timeout: Option<std::time::Duration>,
    ) -> ProcessResult<Option<ProcessCompletion>> {
        loop {
            let event = if let Some(timeout) = timeout {
                match self.receiver.recv_timeout(timeout) {
                    Ok(event) => Some(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("subcommand completion channel disconnected".into());
                    }
                }
            } else {
                Some(
                    self.receiver
                        .recv()
                        .map_err(|_| "subcommand completion channel disconnected".to_owned())?,
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
                        signal_process(pid, process_group, signal);
                    }
                }
                Some(ProcessEvent::Finished(completion)) => {
                    self.children.remove(&completion.edge);
                    self.running -= 1;
                    return Ok(Some(completion));
                }
                None => return Ok(None),
            }
        }
    }

    pub(crate) fn running_len(&self) -> usize {
        self.running
    }

    pub(crate) fn interrupt(&mut self, signal: i32) {
        if self.interrupted.replace(signal) == Some(signal) {
            return;
        }
        for (pid, process_group) in self.children.values().copied() {
            signal_process(pid, process_group, signal);
        }
    }
}

pub(crate) fn status_interrupted(status: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        matches!(status.signal(), Some(1 | 2 | 3 | 15))
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
fn run_shell(
    command: &BString,
    use_console: bool,
    dryrun: bool,
    started: impl FnOnce(u32, bool),
) -> io::Result<Option<ProcessOutput>> {
    if dryrun {
        return Ok(None);
    }
    let mut child = Command::new("/bin/sh");
    child
        .arg("-c")
        .arg(command.to_os_str().expect("byte strings are valid on Unix"))
        .stdin(Stdio::null());
    #[cfg(unix)]
    if !use_console {
        use std::os::unix::process::CommandExt;
        child.process_group(0);
    }
    if use_console {
        let mut child = child.spawn()?;
        started(child.id(), false);
        let status = child.wait()?;
        Ok(Some(ProcessOutput {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }))
    } else {
        #[cfg(unix)]
        {
            use std::io::Read;
            use std::os::fd::OwnedFd;
            use std::os::unix::net::UnixStream;

            let (mut output_reader, output_writer) = UnixStream::pair()?;
            let stdout: OwnedFd = output_writer.try_clone()?.into();
            let stderr: OwnedFd = output_writer.into();
            child
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr));
            let mut process = child.spawn()?;
            drop(child);
            started(process.id(), true);
            let mut output = Vec::new();
            output_reader.read_to_end(&mut output)?;
            let status = process.wait()?;
            Ok(Some(ProcessOutput {
                status,
                stdout: output,
                stderr: Vec::new(),
            }))
        }
        #[cfg(not(unix))]
        {
            child.stdout(Stdio::piped()).stderr(Stdio::piped());
            let child = child.spawn()?;
            started(child.id(), false);
            let result = child.wait_with_output()?;
            Ok(Some(ProcessOutput {
                status: result.status,
                stdout: result.stdout,
                stderr: result.stderr,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    // [spec:samurai:req:compat.process-integration/test]
    fn ronin_process_supervisor_reports_keyed_signal_completion() {
        let edge = EdgeId::from_index(7);
        let mut supervisor = ProcessSupervisor::default();
        std::thread::scope(|scope| {
            supervisor.spawn(scope, edge, BString::from("kill -INT $$"), false, false);
            let completion = supervisor
                .wait(None)
                .unwrap()
                .expect("the subprocess produces a completion");
            assert_eq!(completion.edge, edge);
            let output = completion.result.unwrap().unwrap();
            assert!(status_interrupted(&output.status));
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        });
    }

    #[cfg(unix)]
    #[test]
    fn ronin_process_supervisor_preserves_stdout_stderr_order() {
        let edge = EdgeId::from_index(9);
        let mut supervisor = ProcessSupervisor::default();
        std::thread::scope(|scope| {
            supervisor.spawn(
                scope,
                edge,
                BString::from("printf out; printf err >&2; printf end"),
                false,
                false,
            );
            let completion = supervisor
                .wait(None)
                .unwrap()
                .expect("the subprocess produces a completion");
            let output = completion.result.unwrap().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"outerrend");
            assert!(output.stderr.is_empty());
        });
    }
}
