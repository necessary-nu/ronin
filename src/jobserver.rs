//! GNU Make jobserver discovery, publication, and resource-safe slot ownership.

use crate::error::{JobserverOperation, ProcessError};
use std::cell::Cell;
use std::ffi::OsString;
use std::io;
use std::num::NonZeroUsize;
use std::rc::Rc;
#[cfg(unix)]
use std::sync::Arc;
use std::time::Duration;

type ProcessResult<T> = Result<T, ProcessError>;

/// Result delivered when the jobserver helper finishes one acquisition.
pub(crate) type Acquisition = io::Result<jobserver::Acquired>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum JobserverMode {
    #[default]
    None,
    Pipe,
    PosixFifo,
    Win32Semaphore,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct JobserverConfig {
    pub(crate) mode: JobserverMode,
    pub(crate) path: String,
}

impl JobserverConfig {
    pub(crate) fn has_mode(&self) -> bool {
        self.mode != JobserverMode::None
    }

    pub(crate) const fn is_native(&self) -> bool {
        match self.mode {
            JobserverMode::None => false,
            JobserverMode::Pipe | JobserverMode::PosixFifo => cfg!(unix),
            JobserverMode::Win32Semaphore => cfg!(windows),
        }
    }
}

fn parse_file_descriptor_pair(value: &str) -> Option<JobserverMode> {
    let (read, write) = value.split_once(',')?;
    let read = read.parse::<i32>().ok()?;
    let write = write.parse::<i32>().ok()?;
    Some(if read < 0 || write < 0 {
        JobserverMode::None
    } else {
        JobserverMode::Pipe
    })
}

/// Parse the MAKEFLAGS jobserver values accepted by Ninja.
pub(crate) fn parse_makeflags_value(makeflags: Option<&str>) -> ProcessResult<JobserverConfig> {
    let Some(makeflags) = makeflags.filter(|value| !value.is_empty()) else {
        return Ok(JobserverConfig::default());
    };
    let arguments = makeflags.split_ascii_whitespace();
    if arguments
        .clone()
        .next()
        .is_some_and(|argument| !argument.starts_with('-') && argument.contains('n'))
    {
        return Ok(JobserverConfig::default());
    }

    let mut authorization = None;
    let mut legacy_descriptors = None;
    for argument in arguments {
        if let Some(value) = argument.strip_prefix("--jobserver-auth=") {
            authorization = Some(value);
        } else if let Some(value) = argument.strip_prefix("--jobserver-fds=") {
            legacy_descriptors = Some(value);
        }
    }

    if let Some(value) = authorization {
        if let Some(mode) = parse_file_descriptor_pair(value) {
            return Ok(JobserverConfig {
                path: if mode == JobserverMode::Pipe {
                    value.to_owned()
                } else {
                    String::new()
                },
                mode,
            });
        }
        if let Some(path) = value.strip_prefix(FIFO_AUTHORIZATION) {
            return Ok(JobserverConfig {
                mode: JobserverMode::PosixFifo,
                path: path.into(),
            });
        }
        return Ok(JobserverConfig {
            mode: JobserverMode::Win32Semaphore,
            path: value.into(),
        });
    }

    let Some(value) = legacy_descriptors else {
        return Ok(JobserverConfig::default());
    };
    let Some(mode) = parse_file_descriptor_pair(value) else {
        return Err(ProcessError::InvalidJobserverDescriptors {
            value: value.to_owned(),
        });
    };
    Ok(JobserverConfig {
        path: if mode == JobserverMode::Pipe {
            value.to_owned()
        } else {
            String::new()
        },
        mode,
    })
}

/// Connect to a native jobserver inherited through the process environment.
pub(crate) fn inherited_client() -> ProcessResult<jobserver::Client> {
    // SAFETY: runtime option normalization calls this before manifest parsing,
    // before Ronin opens build files or starts any threads. The maintained
    // jobserver transport validates and duplicates inherited descriptors and
    // documents repeated calls as safe.
    let inherited = unsafe { jobserver::Client::from_env_ext(true) };
    inherited
        .client
        .map_err(|source| ProcessError::JobserverEnvironment { source })
}

/// How long a build waits before re-asking a jobserver it serves for a token.
///
/// Ronin only ever wants a token while one of its own commands is already
/// running, because the implicit slot covers the first. So a wake it misses
/// costs latency and never progress: the next completion retries anyway. This
/// bounds that latency for the case where every one of Ronin's own commands is
/// long-running and a child hands a token back in the middle of them.
#[cfg(unix)]
const SERVED_RETRY_INTERVAL: Duration = Duration::from_millis(2);

/// How `--jobserver-auth` names a jobserver that is a named pipe.
///
/// GNU Make's `FIFO_PREFIX` (posixos.c), and the only form Ronin publishes: a
/// path survives an intermediate process that passes no descriptors down.
const FIFO_AUTHORIZATION: &str = "fifo:";

/// The byte written into a served jobserver for each shareable slot.
///
/// The protocol gives the byte no meaning beyond identity — a client must
/// write back exactly what it read — so this only has to be something.
#[cfg(unix)]
const SERVED_TOKEN: u8 = b'+';

/// How many tokens fit in one pipe buffer, which is what a fifo has.
///
/// Writing past it would block, and a jobserver whose creator can block on its
/// own fifo is a deadlock waiting for a large enough `-j`.
#[cfg(unix)]
const SERVED_TOKEN_CEILING: usize = 64 * 1024;

/// Where a build gets its job slots from.
///
/// Inheriting and serving are mutually exclusive by construction: a build that
/// found a jobserver in its environment consumes that one, so children keep
/// reaching the same budget it does.
#[derive(Clone, Debug)]
pub(crate) enum Transport {
    /// A jobserver a parent process published, with the words that named it.
    Inherited(jobserver::Client, Vec<(&'static str, OsString)>),
    /// A jobserver this build created, sized to its own job limit.
    #[cfg(unix)]
    Served(Arc<ServedJobserver>),
}

impl Transport {
    /// Creates a jobserver sized to `jobs`, where the platform has one to make.
    ///
    /// Windows publishes its budget through a named semaphore rather than a
    /// pipe, which Ronin reads as a client but does not yet write; there a
    /// build keeps its job limit to itself.
    pub(crate) fn serve(jobs: NonZeroUsize) -> ProcessResult<Option<Self>> {
        #[cfg(unix)]
        {
            ServedJobserver::create(jobs).map(|served| Some(Self::Served(Arc::new(served))))
        }
        #[cfg(not(unix))]
        {
            let _ = jobs;
            Ok(None)
        }
    }

    /// Joins a jobserver a parent published, keeping how it was named.
    ///
    /// A front end may give recipes a rewritten `MAKEFLAGS`, replacing the one
    /// this budget arrived in. The words naming the inherited transport are
    /// retained so the outer budget can still be mapped through that boundary.
    pub(crate) fn inherit(client: jobserver::Client, makeflags: Option<&str>) -> Self {
        let budget = makeflags
            .unwrap_or_default()
            .split_ascii_whitespace()
            .take_while(|word| *word != "--")
            .filter(|word| word.starts_with("-j") || word.starts_with("--jobserver-"))
            .collect::<Vec<_>>()
            .join(" ");
        let publication = if budget.is_empty() {
            Vec::new()
        } else {
            publication_of(OsString::from(budget)).into()
        };
        Self::Inherited(client, publication)
    }

    /// The address a child is given to reach this budget, as
    /// `--jobserver-auth` spells it.
    ///
    /// Only a served jobserver answers. An inherited one was named by whoever
    /// published it and is republished under the name it arrived with, which
    /// the front end already holds; asking the transport would only be asking
    /// the same string back through a longer route.
    pub(crate) fn served_authorization(&self) -> Option<OsString> {
        match self {
            Self::Inherited(..) => None,
            #[cfg(unix)]
            Self::Served(served) => {
                let mut authorization = OsString::from(FIFO_AUTHORIZATION);
                authorization.push(&served.path);
                Some(authorization)
            }
        }
    }

    /// The environment a child needs to draw on this build's job budget.
    fn publication(&self) -> Vec<(&'static str, OsString)> {
        match self {
            Self::Inherited(_, publication) => publication.clone(),
            #[cfg(unix)]
            Self::Served(served) => served.publication().to_vec(),
        }
    }

    /// Widen a budget this run created to `jobs`, and say whether it was.
    ///
    /// Only a budget this run owns is this run's to size: writing tokens into
    /// one a parent published would hand this tree capacity nobody granted it.
    /// The address never moves, because the slots are bytes in the pipe that
    /// address already names — so a recipe compiled with it before the widening
    /// reaches the same, larger, budget.
    // [spec:ronin:req:make.jobserver+3]
    pub(crate) fn widen(&self, jobs: NonZeroUsize) -> bool {
        match self {
            Self::Inherited(..) => false,
            #[cfg(unix)]
            Self::Served(served) => served.widen(jobs),
        }
    }

    /// Writes this build's job budget into the environment a child is given.
    ///
    /// An inherited budget is only restored where the front end overrode the
    /// variable carrying it. Elsewhere this process's own environment still
    /// names it and a child inherits that, switches and all.
    pub(crate) fn publish_into(&self, environment: &mut Vec<(OsString, Option<OsString>)>) {
        let owned = !matches!(self, Self::Inherited(..));
        for (name, published) in self.publication() {
            match environment
                .iter_mut()
                .find(|(existing, _)| existing == std::ffi::OsStr::new(name))
            {
                Some((_, Some(value))) => splice(value, &published),
                _ if owned => environment.push((name.into(), Some(published))),
                _ => {}
            }
        }
    }
}

/// Joins a published budget into switches the front end already wrote.
fn splice(switches: &mut OsString, published: &OsString) {
    // GNU Make ends the switches at a `--` and writes the command-line
    // assignments after it. A budget appended past that is read by a child as a
    // goal to build, and there is no rule to make `-j4`.
    if let Some((given, assignments)) = switches
        .to_str()
        .and_then(|switches| switches.split_once(" -- "))
    {
        let mut merged = OsString::from(given);
        merged.push(published);
        merged.push(" -- ");
        merged.push(assignments);
        *switches = merged;
        return;
    }
    // MAKEFLAGS' publication leads with the space that follows the letter
    // group; MFLAGS' does not, because it is spelled as a command line. Joining
    // without checking runs the two together into a single unparseable word.
    if !switches.is_empty() && !published.to_string_lossy().starts_with(' ') {
        switches.push(" ");
    }
    switches.push(published);
}

/// A GNU Make jobserver this process created, owns, and removes.
///
/// The named-pipe form, which is what GNU Make 4.4 publishes on Linux. A path
/// survives an intermediate process that does not cooperate in passing
/// descriptors down, and — because nothing has to be made inheritable —
/// publishing it leaves Ronin's spawn path on `posix_spawn`.
#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct ServedJobserver {
    path: std::path::PathBuf,
    /// Opened read-write so the fifo never reports end of file, and
    /// non-blocking so an empty one is a decision rather than a stall. Both
    /// belong to this description alone; a child opening the path gets its own.
    fifo: std::fs::File,
    /// How many slots this budget stands at, which a makefile's own `-jN` can
    /// raise after the address has been published. Held as a number rather
    /// than as the finished publication so that the two can never disagree.
    jobs: std::sync::atomic::AtomicUsize,
}

#[cfg(unix)]
impl ServedJobserver {
    fn create(jobs: NonZeroUsize) -> ProcessResult<Self> {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let operate = |operation, source| ProcessError::Jobserver { operation, source };
        let path = temporary_directory().join(format!(
            "ronin-jobserver-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &path,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .map_err(|source| operate(JobserverOperation::CreateJobserver, source.into()))?;
        let fifo = rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        )
        .map(std::fs::File::from)
        .map_err(|source| {
            let _ = std::fs::remove_file(&path);
            operate(JobserverOperation::CreateJobserver, source.into())
        })?;

        // One slot short of the limit: the protocol gives every participant an
        // implicit slot it does not take from the pipe, and Ronin's own
        // scheduler takes that one. Sharing all `jobs` would hand out a budget
        // Ronin is simultaneously spending.
        let served = Self {
            path,
            fifo,
            jobs: std::sync::atomic::AtomicUsize::new(jobs.get()),
        };
        served
            .fill(jobs.get() - 1)
            .map_err(|source| operate(JobserverOperation::CreateJobserver, source))?;
        Ok(served)
    }

    /// The environment a child is given to reach this budget as it now stands.
    fn publication(&self) -> [(&'static str, OsString); 3] {
        let jobs = NonZeroUsize::new(self.jobs.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(NonZeroUsize::MIN);
        publication_for(jobs, &self.path)
    }

    /// Write the slots a wider budget has that this one did not.
    ///
    /// Idempotent against a narrower or equal request, so a pass that asks
    /// again for a budget already standing at `jobs` writes nothing: the tokens
    /// are the budget, and a second helping of them would be a second budget.
    fn widen(&self, jobs: NonZeroUsize) -> bool {
        use std::sync::atomic::Ordering;

        let held = self.jobs.fetch_max(jobs.get(), Ordering::Relaxed);
        if held >= jobs.get() {
            return false;
        }
        let _ = self.fill(jobs.get() - held);
        true
    }

    /// Writes the shareable slots, stopping at whatever the fifo will hold.
    ///
    /// A budget larger than one pipe buffer is a job limit no machine can
    /// spend, and stopping there is what keeps the write from blocking.
    fn fill(&self, tokens: usize) -> io::Result<()> {
        use io::Write as _;

        const BATCH: [u8; 512] = [SERVED_TOKEN; 512];

        let mut remaining = tokens.min(SERVED_TOKEN_CEILING);
        while remaining != 0 {
            match (&self.fifo).write(&BATCH[..remaining.min(BATCH.len())]) {
                Ok(0) => break,
                Ok(written) => remaining -= written,
                Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
                Err(source) if source.kind() == io::ErrorKind::WouldBlock => break,
                Err(source) => return Err(source),
            }
        }
        Ok(())
    }

    /// Takes one slot, or reports that every shared slot is in use.
    fn try_acquire(&self) -> io::Result<Option<u8>> {
        use io::Read as _;

        let mut token = [0; 1];
        loop {
            return match (&self.fifo).read(&mut token) {
                Ok(1) => Ok(Some(token[0])),
                Ok(_) => Ok(None),
                Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
                Err(source) if source.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(source) => Err(source),
            };
        }
    }

    /// Hands a slot back, writing back the byte the protocol requires.
    ///
    /// This runs from `Drop`, so it has nowhere to report. It also cannot find
    /// the fifo full, because a slot is only given back by whoever holds it and
    /// no more were ever written than fit.
    fn release(&self, token: u8) {
        use io::Write as _;

        let _ = (&self.fifo).write_all(&[token]);
    }
}

#[cfg(unix)]
impl Drop for ServedJobserver {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where GNU Make 4.4.1 puts the fifo it publishes.
///
/// `get_tmpdir` (misc.c): `MAKE_TMPDIR` before `TMPDIR`, and a name that is set
/// but does not stat as a directory is passed over rather than used — GNU says
/// `TMPDIR value X: ...` and goes on to the next candidate, and to `/tmp` when
/// none of them will do. Which is the point: a `TMPDIR` pointing at nothing
/// costs the run a diagnostic, never the budget.
///
/// `std::env::temp_dir` would answer `TMPDIR` alone and would not look at it.
#[cfg(unix)]
fn temporary_directory() -> std::path::PathBuf {
    ["MAKE_TMPDIR", "TMPDIR"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

/// The environment GNU Make 4.4.1 would publish for this jobserver.
#[cfg(unix)]
fn publication_for(jobs: NonZeroUsize, path: &std::path::Path) -> [(&'static str, OsString); 3] {
    let mut authorization =
        OsString::from(format!("-j{jobs} --jobserver-auth={FIFO_AUTHORIZATION}"));
    authorization.push(path);
    publication_of(authorization)
}

/// The variables that carry a budget, given the words describing it.
///
/// `MAKEFLAGS` leads with the group of single-letter switches, so an empty
/// group shows as the leading space Make writes and children tolerate.
/// `MFLAGS` carries the same switches spelled as a command line. Both are
/// Make's; `CARGO_MAKEFLAGS` is the same value under the name the Rust
/// ecosystem's jobserver clients read first, so a `cargo` or `cc` invoked from
/// a build command joins this budget rather than inventing its own.
fn publication_of(authorization: OsString) -> [(&'static str, OsString); 3] {
    let mut makeflags = OsString::from(" ");
    makeflags.push(&authorization);
    [
        ("MAKEFLAGS", makeflags.clone()),
        ("MFLAGS", authorization),
        ("CARGO_MAKEFLAGS", makeflags),
    ]
}

#[derive(Debug)]
struct ImplicitSlot {
    available: Rc<Cell<bool>>,
}

impl Drop for ImplicitSlot {
    fn drop(&mut self) {
        let was_available = self.available.replace(true);
        debug_assert!(
            !was_available,
            "implicit jobserver slot cannot be released twice"
        );
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct ServedSlot {
    server: Arc<ServedJobserver>,
    token: u8,
}

#[cfg(unix)]
impl Drop for ServedSlot {
    fn drop(&mut self) {
        self.server.release(self.token);
    }
}

/// An owned GNU Make job slot.
///
/// Every variant releases its capacity from `Drop`, so scheduler errors and
/// unwinding cannot strand a token.
#[derive(Debug)]
enum SlotOwnership {
    Implicit(ImplicitSlot),
    Explicit(jobserver::Acquired),
    #[cfg(unix)]
    Served(ServedSlot),
}

#[derive(Debug)]
pub(crate) struct Slot {
    ownership: SlotOwnership,
}

impl Slot {
    const fn implicit(available: Rc<Cell<bool>>) -> Self {
        Self {
            ownership: SlotOwnership::Implicit(ImplicitSlot { available }),
        }
    }

    const fn explicit(token: jobserver::Acquired) -> Self {
        Self {
            ownership: SlotOwnership::Explicit(token),
        }
    }

    #[cfg(unix)]
    const fn served(server: Arc<ServedJobserver>, token: u8) -> Self {
        Self {
            ownership: SlotOwnership::Served(ServedSlot { server, token }),
        }
    }

    pub(crate) fn release(self) {
        match self.ownership {
            SlotOwnership::Implicit(slot) => drop(slot),
            SlotOwnership::Explicit(token) => drop(token),
            #[cfg(unix)]
            SlotOwnership::Served(slot) => drop(slot),
        }
    }

    #[cfg(test)]
    const fn is_implicit(&self) -> bool {
        matches!(self.ownership, SlotOwnership::Implicit(_))
    }
}

/// Where a build's shared slots come from once it is running.
#[derive(Debug)]
enum Acquirer {
    /// An inherited jobserver, whose blocking read lives on a helper thread
    /// that reports back through the supervisor's external event channel.
    Inherited(jobserver::HelperThread),
    /// A jobserver this build serves, which it can ask without blocking.
    #[cfg(unix)]
    Served(Arc<ServedJobserver>),
}

/// Owns the implicit slot and whatever acquires the shared ones.
#[derive(Debug)]
pub(crate) struct JobserverClient {
    implicit_available: Rc<Cell<bool>>,
    acquirer: Acquirer,
    request_pending: bool,
}

// [spec:ronin:req:runtime.jobserver-resource-safety]
impl JobserverClient {
    pub(crate) fn new(
        transport: Transport,
        notify: impl FnMut(Acquisition) + Send + 'static,
    ) -> ProcessResult<Self> {
        let acquirer = match transport {
            Transport::Inherited(client, _) => client
                .into_helper_thread(notify)
                .map(Acquirer::Inherited)
                .map_err(|source| ProcessError::Jobserver {
                    operation: JobserverOperation::StartHelper,
                    source,
                })?,
            #[cfg(unix)]
            Transport::Served(served) => Acquirer::Served(served),
        };
        Ok(Self {
            implicit_available: Rc::new(Cell::new(true)),
            acquirer,
            request_pending: false,
        })
    }

    pub(crate) fn try_acquire_implicit(&self) -> Option<Slot> {
        self.implicit_available
            .replace(false)
            .then(|| Slot::implicit(self.implicit_available.clone()))
    }

    /// Takes a shared slot if one is free right now.
    ///
    /// An inherited jobserver answers only through its helper thread, so this
    /// reports nothing for it and the caller falls through to a request.
    pub(crate) fn try_acquire_token(&self) -> ProcessResult<Option<Slot>> {
        match &self.acquirer {
            Acquirer::Inherited(_) => Ok(None),
            #[cfg(unix)]
            Acquirer::Served(served) => served
                .try_acquire()
                .map(|token| token.map(|token| Slot::served(served.clone(), token)))
                .map_err(|source| ProcessError::Jobserver {
                    operation: JobserverOperation::AcquireToken,
                    source,
                }),
        }
    }

    pub(crate) fn request_token(&mut self) {
        match &self.acquirer {
            Acquirer::Inherited(helper) => {
                if !self.request_pending {
                    self.request_pending = true;
                    helper.request_token();
                }
            }
            // A served jobserver is asked rather than queued on, so there is
            // nothing here to remember wanting.
            #[cfg(unix)]
            Acquirer::Served(_) => {}
        }
    }

    /// How long the scheduler may sleep while waiting for a shared slot.
    ///
    /// An inherited jobserver wakes the supervisor when a token arrives, so
    /// there is nothing to time out on. A served one is asked rather than
    /// waited on, so the wait has to end by itself.
    pub(crate) const fn retry_interval(&self) -> Option<Duration> {
        match self.acquirer {
            Acquirer::Inherited(_) => None,
            #[cfg(unix)]
            Acquirer::Served(_) => Some(SERVED_RETRY_INTERVAL),
        }
    }

    pub(crate) fn receive_token(&mut self, result: Acquisition) -> ProcessResult<Slot> {
        debug_assert!(self.request_pending, "jobserver token was not requested");
        self.request_pending = false;
        result
            .map(Slot::explicit)
            .map_err(|source| ProcessError::Jobserver {
                operation: JobserverOperation::AcquireToken,
                source,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn ninja_jobserver_parses_makeflags() {
        assert_eq!(
            parse_makeflags_value(None).unwrap(),
            JobserverConfig::default()
        );
        assert_eq!(
            parse_makeflags_value(Some("  \t")).unwrap(),
            JobserverConfig::default()
        );
        assert_eq!(
            parse_makeflags_value(Some("kns --jobserver-auth=fifo:foo")).unwrap(),
            JobserverConfig::default()
        );
        for flags in [
            "--jobserver-auth=fifo:foo",
            " -j --jobserver-auth=fifo:foo",
            " -j10 --jobserver-auth=fifo:foo",
            "-one-flag --jobserver-auth=fifo:foo",
        ] {
            assert_eq!(
                parse_makeflags_value(Some(flags)).unwrap(),
                JobserverConfig {
                    mode: JobserverMode::PosixFifo,
                    path: "foo".into(),
                }
            );
        }
        assert_eq!(
            parse_makeflags_value(Some("--jobserver-auth=semaphore_name")).unwrap(),
            JobserverConfig {
                mode: JobserverMode::Win32Semaphore,
                path: "semaphore_name".into(),
            }
        );
        assert_eq!(
            parse_makeflags_value(Some("--jobserver-auth=10,42"))
                .unwrap()
                .mode,
            JobserverMode::Pipe
        );
        for flags in ["--jobserver-auth=-1,42", "--jobserver-auth=10,-42"] {
            assert_eq!(
                parse_makeflags_value(Some(flags)).unwrap().mode,
                JobserverMode::None
            );
        }
        assert_eq!(
            parse_makeflags_value(Some(
                "--jobserver-auth=10,42 --jobserver-fds=12,44 --jobserver-auth=fifo:/tmp/fifo"
            ))
            .unwrap(),
            JobserverConfig {
                mode: JobserverMode::PosixFifo,
                path: "/tmp/fifo".into(),
            }
        );
        assert_eq!(
            parse_makeflags_value(Some("--jobserver-auth=10,42 --jobserver-fds=12,44")).unwrap(),
            JobserverConfig {
                mode: JobserverMode::Pipe,
                path: "10,42".into(),
            }
        );
        assert_eq!(
            parse_makeflags_value(Some("--jobserver-fds=10, --jobserver-auth=fifo:/tmp/fifo"))
                .unwrap(),
            JobserverConfig {
                mode: JobserverMode::PosixFifo,
                path: "/tmp/fifo".into(),
            }
        );
        assert_eq!(
            parse_makeflags_value(Some("--jobserver-fds=10,"))
                .unwrap_err()
                .to_string(),
            "Invalid file descriptor pair [10,]"
        );
    }

    #[test]
    fn ninja_jobserver_recognizes_native_transports() {
        let pipe = JobserverConfig {
            mode: JobserverMode::Pipe,
            path: "3,4".into(),
        };
        let fifo = JobserverConfig {
            mode: JobserverMode::PosixFifo,
            path: "fifo".into(),
        };
        let semaphore = JobserverConfig {
            mode: JobserverMode::Win32Semaphore,
            path: "semaphore".into(),
        };
        assert_eq!(pipe.is_native(), cfg!(unix));
        assert_eq!(fifo.is_native(), cfg!(unix));
        assert_eq!(semaphore.is_native(), cfg!(windows));
        assert!(!JobserverConfig::default().is_native());
    }

    #[test]
    // [spec:ronin:req:runtime.jobserver-resource-safety/test]
    fn ronin_jobserver_slots_release_on_drop() {
        let transport = jobserver::Client::new(1).unwrap();
        let probe = transport.clone();
        let explicit = Slot::explicit(transport.acquire().unwrap());
        assert_eq!(probe.available().unwrap(), 0);
        explicit.release();
        assert_eq!(probe.available().unwrap(), 1);
        drop(probe.acquire().unwrap());

        let (sender, receiver) = mpsc::channel();
        let client = JobserverClient::new(Transport::inherit(transport, None), move |result| {
            let _ = sender.send(result);
        })
        .unwrap();
        let implicit = client.try_acquire_implicit().unwrap();
        assert!(implicit.is_implicit());
        assert!(client.try_acquire_implicit().is_none());
        drop(implicit);
        assert!(client.try_acquire_implicit().is_some());
        drop(receiver);
    }

    #[test]
    // [spec:ronin:req:runtime.jobserver-resource-safety/test]
    fn ronin_jobserver_acquisition_is_event_driven_and_fallible() {
        let transport = jobserver::Client::new(0).unwrap();
        let producer = transport.clone();
        let (sender, receiver) = mpsc::channel();
        let mut client = JobserverClient::new(Transport::inherit(transport, None), move |result| {
            let _ = sender.send(result);
        })
        .unwrap();

        client.request_token();
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_millis(20))
                .unwrap_err(),
            mpsc::RecvTimeoutError::Timeout
        );
        producer.release_raw().unwrap();
        let token = client
            .receive_token(receiver.recv_timeout(Duration::from_secs(1)).unwrap())
            .unwrap();
        drop(token);

        client.request_pending = true;
        let error = client
            .receive_token(Err(io::Error::from(io::ErrorKind::UnexpectedEof)))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Error acquiring GNU Make jobserver token: unexpected end of file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ronin_serves_the_named_pipe_form_gnu_make_publishes() {
        let jobs = NonZeroUsize::new(4).unwrap();
        let served = ServedJobserver::create(jobs).unwrap();
        let path = served.path.clone();
        assert!(
            std::fs::metadata(&path)
                .map(|metadata| std::os::unix::fs::FileTypeExt::is_fifo(&metadata.file_type()))
                .unwrap(),
            "a served jobserver is a named pipe"
        );

        // GNU Make 4.4.1 writes ` -j4 --jobserver-auth=fifo:/tmp/GMfifo<pid>`
        // into MAKEFLAGS, the same without the leading empty single-letter
        // group into MFLAGS, and nothing into CARGO_MAKEFLAGS.
        let published = |name: &str| {
            served
                .publication()
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_str().unwrap().to_owned())
                .unwrap()
        };
        let authorization = format!("-j4 --jobserver-auth=fifo:{}", path.display());
        assert_eq!(published("MAKEFLAGS"), format!(" {authorization}"));
        assert_eq!(published("MFLAGS"), authorization);
        assert_eq!(published("CARGO_MAKEFLAGS"), format!(" {authorization}"));
        assert_eq!(
            parse_makeflags_value(Some(&published("MAKEFLAGS"))).unwrap(),
            JobserverConfig {
                mode: JobserverMode::PosixFifo,
                path: path.to_str().unwrap().to_owned(),
            }
        );

        drop(served);
        assert!(!path.exists(), "a served jobserver removes its own fifo");
    }

    #[cfg(unix)]
    #[test]
    fn a_served_budget_counts_ronins_own_jobs_against_the_slots_it_shares() {
        const JOBS: usize = 4;

        let transport = Transport::serve(NonZeroUsize::new(JOBS).unwrap())
            .unwrap()
            .expect("Unix serves the named-pipe form");
        assert!(
            transport.publication().len() == 3,
            "a served jobserver is published to children"
        );
        let mut client = JobserverClient::new(transport, |_| unreachable!()).unwrap();

        // The implicit slot is Ronin's own and costs the shared pool nothing;
        // every job past it takes capacity a child would otherwise have. Four
        // jobs at `-j4` therefore leave the pool empty rather than leaving a
        // full pool beside four running commands.
        let mut held = vec![client.try_acquire_implicit().unwrap()];
        while let Some(slot) = client.try_acquire_token().unwrap() {
            held.push(slot);
            assert!(held.len() <= JOBS, "a served budget cannot exceed -j");
        }
        assert_eq!(held.len(), JOBS);
        assert!(client.try_acquire_implicit().is_none());

        // Requesting a token is what the scheduler does when it runs dry, and
        // a served jobserver must not block or queue on it.
        client.request_token();
        assert!(client.try_acquire_token().unwrap().is_none());
        assert!(client.retry_interval().is_some());

        for slot in std::mem::take(&mut held) {
            slot.release();
        }
        held.push(client.try_acquire_implicit().unwrap());
        while let Some(slot) = client.try_acquire_token().unwrap() {
            held.push(slot);
        }
        assert_eq!(held.len(), JOBS, "released slots return to the shared pool");
    }

    #[test]
    // [spec:ronin:req:make.jobserver+3/test]
    // [spec:ronin:req:make.recursive-invocation+3/test]
    fn an_inherited_jobserver_survives_a_rewritten_makeflags() {
        let inherit =
            |makeflags| Transport::inherit(jobserver::Client::new(1).unwrap(), Some(makeflags));
        let published = |transport: &Transport, environment: &mut Vec<_>| {
            transport.publish_into(environment);
            environment
                .iter()
                .map(|(name, value): &(OsString, Option<OsString>)| {
                    format!(
                        "{}={}",
                        name.display(),
                        value.clone().unwrap_or_default().display()
                    )
                })
                .collect::<Vec<_>>()
        };

        // Nothing overrides the variables naming the budget, so this process's
        // own environment still describes it and a child inherits that whole —
        // the switches a parent wrote beside it included.
        let transport = inherit("ks -j4 --jobserver-auth=fifo:/tmp/x --no-print-directory");
        assert!(published(&transport, &mut Vec::new()).is_empty());

        // A front end may hand a recipe MAKEFLAGS built from interface switches,
        // which would otherwise replace the inherited transport description.
        let mut rewritten = vec![
            (
                OsString::from("MAKEFLAGS"),
                Some(OsString::from("ks -- A=1")),
            ),
            (OsString::from("MFLAGS"), Some(OsString::from("-ks"))),
        ];
        assert_eq!(
            published(&transport, &mut rewritten),
            [
                "MAKEFLAGS=ks -j4 --jobserver-auth=fifo:/tmp/x -- A=1",
                "MFLAGS=-ks -j4 --jobserver-auth=fifo:/tmp/x",
            ]
        );

        // A parent that named no budget leaves nothing to write again.
        let mut rewritten = vec![(OsString::from("MAKEFLAGS"), Some(OsString::from("ks")))];
        assert_eq!(published(&inherit("ks"), &mut rewritten), ["MAKEFLAGS=ks"]);

        let transport = inherit("--jobserver-auth=fifo:/tmp/x");
        let (sender, receiver) = mpsc::channel();
        let client = JobserverClient::new(transport, move |result| {
            let _ = sender.send(result);
        })
        .unwrap();
        assert!(client.try_acquire_token().unwrap().is_none());
        assert!(client.retry_interval().is_none());
        drop(receiver);
    }

    #[cfg(unix)]
    #[test]
    // [spec:ronin:req:runtime.jobserver-resource-safety/test]
    fn ronin_jobserver_preserves_inherited_descriptor_flags() {
        const CHILD_MARKER: &str = "RONIN_JOBSERVER_FLAG_TEST_CHILD";
        const READY_PATH: &str = "RONIN_JOBSERVER_FLAG_TEST_READY";

        if std::env::var_os(CHILD_MARKER).is_some() {
            let client = inherited_client().unwrap();
            std::fs::write(std::env::var_os(READY_PATH).unwrap(), []).unwrap();
            let mut token = [0];
            std::io::stdin().read_exact(&mut token).unwrap();
            assert_eq!(token, [b'+']);
            drop(client);
            return;
        }

        let (reader, mut writer) = std::io::pipe().unwrap();
        let child_writer = writer.try_clone().unwrap();
        let ready =
            std::env::temp_dir().join(format!("ronin-jobserver-ready-{}", std::process::id()));
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "jobserver::tests::ronin_jobserver_preserves_inherited_descriptor_flags",
                "--quiet",
            ])
            .env(CHILD_MARKER, "1")
            .env(READY_PATH, &ready)
            .env_remove("CARGO_MAKEFLAGS")
            .env("MAKEFLAGS", "-j --jobserver-auth=0,2")
            .env_remove("MFLAGS")
            .stdin(Stdio::from(reader))
            .stdout(Stdio::null())
            .stderr(Stdio::from(child_writer))
            .spawn()
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(ready.exists(), "child did not reach the blocking read");
        std::thread::sleep(Duration::from_millis(30));
        writer.write_all(b"+").unwrap();
        assert!(child.wait().unwrap().success());
        std::fs::remove_file(ready).unwrap();
    }
}
