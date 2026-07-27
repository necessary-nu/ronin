//! Shell subprocess execution and completion-set bookkeeping.

use std::collections::VecDeque;
use std::io;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessExit {
    Success,
    Failure,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkResult {
    NoWork,
    JobserverTokenAvailable,
    SubprocessFinished,
    Interrupted,
}

#[derive(Debug)]
pub struct Subprocess {
    command: String,
    use_console: bool,
    output: Vec<u8>,
    exit: Option<ProcessExit>,
}

impl Subprocess {
    pub fn done(&self) -> bool {
        self.exit.is_some()
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }

    pub fn finish(&self) -> Option<ProcessExit> {
        self.exit
    }
}

#[derive(Default)]
pub struct SubprocessSet {
    processes: Vec<Subprocess>,
    running: VecDeque<usize>,
    finished: VecDeque<usize>,
    interrupted: bool,
    jobserver_token_available: bool,
}

fn classify_exit(status: std::process::ExitStatus) -> ProcessExit {
    if status.success() {
        return ProcessExit::Success;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if matches!(status.signal(), Some(1 | 2 | 15)) {
            return ProcessExit::Interrupted;
        }
    }
    ProcessExit::Failure
}

fn run(command: &str, use_console: bool) -> io::Result<(ProcessExit, Vec<u8>)> {
    let mut child = Command::new("/bin/sh");
    child.arg("-c").arg(command).stdin(Stdio::null());
    if use_console {
        let status = child.status()?;
        Ok((classify_exit(status), Vec::new()))
    } else {
        let result = child.output()?;
        let mut output = result.stdout;
        output.extend_from_slice(&result.stderr);
        Ok((classify_exit(result.status), output))
    }
}

impl SubprocessSet {
    pub fn add(&mut self, command: impl Into<String>) -> usize {
        self.add_with_console(command, false)
    }

    pub fn add_with_console(&mut self, command: impl Into<String>, use_console: bool) -> usize {
        let handle = self.processes.len();
        self.processes.push(Subprocess {
            command: command.into(),
            use_console,
            output: Vec::new(),
            exit: None,
        });
        self.running.push_back(handle);
        handle
    }

    pub fn do_work(&mut self) -> io::Result<WorkResult> {
        if self.interrupted {
            return Ok(WorkResult::Interrupted);
        }
        if self.jobserver_token_available {
            return Ok(WorkResult::JobserverTokenAvailable);
        }
        let Some(handle) = self.running.pop_front() else {
            return Ok(WorkResult::NoWork);
        };
        let (exit, output) = {
            let process = &self.processes[handle];
            run(&process.command, process.use_console)?
        };
        let process = &mut self.processes[handle];
        process.exit = Some(exit);
        process.output = output;
        self.finished.push_back(handle);
        Ok(WorkResult::SubprocessFinished)
    }

    pub fn process(&self, handle: usize) -> &Subprocess {
        &self.processes[handle]
    }

    pub fn running_len(&self) -> usize {
        self.running.len()
    }

    pub fn finished_len(&self) -> usize {
        self.finished.len()
    }

    pub fn next_finished(&mut self) -> Option<usize> {
        self.finished.pop_front()
    }

    pub fn notify_interrupted(&mut self) {
        self.interrupted = true;
    }

    pub fn set_jobserver_token_available(&mut self, available: bool) {
        self.jobserver_token_available = available;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::IsTerminal;

    fn finish(set: &mut SubprocessSet, handle: usize) -> ProcessExit {
        while !set.process(handle).done() {
            set.do_work().unwrap();
        }
        set.process(handle).finish().unwrap()
    }

    #[test]
    fn ninja_subprocess_bad_command_stderr() {
        let mut set = SubprocessSet::default();
        let handle = set.add("cmd /c ninja_no_such_command");
        assert_ne!(finish(&mut set, handle), ProcessExit::Success);
        assert!(!set.process(handle).output().is_empty());
    }

    #[test]
    fn ninja_subprocess_no_such_command() {
        let mut set = SubprocessSet::default();
        let handle = set.add("ninja_no_such_command");
        assert_ne!(finish(&mut set, handle), ProcessExit::Success);
        assert!(!set.process(handle).output().is_empty());
    }

    #[test]
    fn ninja_subprocess_interrupt_child_sigint() {
        let mut set = SubprocessSet::default();
        let handle = set.add("kill -INT $$");
        assert_eq!(finish(&mut set, handle), ProcessExit::Interrupted);
    }

    #[test]
    fn ninja_subprocess_interrupt_parent_sigint() {
        let mut set = SubprocessSet::default();
        set.add("sleep 1");
        set.notify_interrupted();
        assert_eq!(set.do_work().unwrap(), WorkResult::Interrupted);
    }

    #[test]
    fn ninja_subprocess_interrupt_child_sigterm() {
        let mut set = SubprocessSet::default();
        let handle = set.add("kill -TERM $$");
        assert_eq!(finish(&mut set, handle), ProcessExit::Interrupted);
    }

    #[test]
    fn ninja_subprocess_interrupt_parent_sigterm() {
        let mut set = SubprocessSet::default();
        set.add("sleep 1");
        set.notify_interrupted();
        assert_eq!(set.do_work().unwrap(), WorkResult::Interrupted);
    }

    #[test]
    fn ninja_subprocess_interrupt_child_sighup() {
        let mut set = SubprocessSet::default();
        let handle = set.add("kill -HUP $$");
        assert_eq!(finish(&mut set, handle), ProcessExit::Interrupted);
    }

    #[test]
    fn ninja_subprocess_interrupt_parent_sighup() {
        let mut set = SubprocessSet::default();
        set.add("sleep 1");
        set.notify_interrupted();
        assert_eq!(set.do_work().unwrap(), WorkResult::Interrupted);
    }

    #[test]
    fn ninja_subprocess_console() {
        if std::io::stdin().is_terminal()
            && std::io::stdout().is_terminal()
            && std::io::stderr().is_terminal()
        {
            let mut set = SubprocessSet::default();
            let handle = set.add_with_console("test -t 0 -a -t 1 -a -t 2", true);
            assert_eq!(finish(&mut set, handle), ProcessExit::Success);
        }
    }

    #[test]
    fn ninja_subprocess_set_with_single() {
        let mut set = SubprocessSet::default();
        let handle = set.add("ls /");
        assert_eq!(finish(&mut set, handle), ProcessExit::Success);
        assert!(!set.process(handle).output().is_empty());
        assert_eq!(set.finished_len(), 1);
    }

    #[test]
    fn ninja_subprocess_set_with_multiple() {
        let mut set = SubprocessSet::default();
        let handles = [set.add("ls /"), set.add("id -u"), set.add("pwd")];
        assert_eq!(set.running_len(), 3);
        for handle in handles {
            assert!(!set.process(handle).done());
            assert!(set.process(handle).output().is_empty());
        }
        while set.running_len() != 0 {
            set.do_work().unwrap();
        }
        assert_eq!(set.finished_len(), 3);
        for handle in handles {
            assert_eq!(set.process(handle).finish(), Some(ProcessExit::Success));
            assert!(!set.process(handle).output().is_empty());
        }
    }

    #[test]
    fn ninja_subprocess_set_with_1025_processes() {
        let mut set = SubprocessSet::default();
        let handles = (0..1025).map(|_| set.add("/bin/echo")).collect::<Vec<_>>();
        while set.running_len() != 0 {
            set.do_work().unwrap();
        }
        for handle in handles {
            assert_eq!(set.process(handle).finish(), Some(ProcessExit::Success));
            assert!(!set.process(handle).output().is_empty());
        }
        assert_eq!(set.finished_len(), 1025);
    }

    #[test]
    fn ninja_subprocess_stdin_is_closed() {
        let mut set = SubprocessSet::default();
        let handle = set.add("cat -");
        assert_eq!(finish(&mut set, handle), ProcessExit::Success);
        assert_eq!(set.finished_len(), 1);
    }

    #[test]
    fn ninja_subprocess_jobserver_token_available() {
        let mut set = SubprocessSet::default();
        let handle = set.add("true");
        set.set_jobserver_token_available(true);
        assert_eq!(set.do_work().unwrap(), WorkResult::JobserverTokenAvailable);
        set.set_jobserver_token_available(false);
        assert_eq!(finish(&mut set, handle), ProcessExit::Success);
        assert_eq!(set.finished_len(), 1);
    }
}
