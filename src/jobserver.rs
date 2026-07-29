//! GNU Make jobserver discovery and resource-safe slot ownership.

use crate::error::ProcessError;
use std::cell::Cell;
use std::io;
use std::rc::Rc;

type ProcessResult<T> = Result<T, ProcessError>;

/// Result delivered when the jobserver helper finishes one acquisition.
pub(crate) type Acquisition = io::Result<jobserver::Acquired>;
/// Cloneable handle to the inherited jobserver transport.
pub(crate) type Transport = jobserver::Client;

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
        if let Some(path) = value.strip_prefix("fifo:") {
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
        return Err(format!("Invalid file descriptor pair [{value}]").into());
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
    inherited.client.map_err(|source| {
        ProcessError::context("Error opening inherited GNU Make jobserver", source)
    })
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

/// An owned GNU Make job slot.
///
/// Both variants release their capacity from `Drop`, so scheduler errors and
/// unwinding cannot strand a token.
#[derive(Debug)]
enum SlotOwnership {
    Implicit(ImplicitSlot),
    Explicit(jobserver::Acquired),
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

    pub(crate) fn release(self) {
        match self.ownership {
            SlotOwnership::Implicit(slot) => drop(slot),
            SlotOwnership::Explicit(token) => drop(token),
        }
    }

    #[cfg(test)]
    const fn is_implicit(&self) -> bool {
        matches!(self.ownership, SlotOwnership::Implicit(_))
    }
}

/// Owns the implicit slot and the blocking acquisition helper.
#[derive(Debug)]
pub(crate) struct JobserverClient {
    implicit_available: Rc<Cell<bool>>,
    helper: jobserver::HelperThread,
    request_pending: bool,
}

// [spec:samurai:req:runtime.jobserver-resource-safety]
impl JobserverClient {
    pub(crate) fn new(
        client: jobserver::Client,
        notify: impl FnMut(Acquisition) + Send + 'static,
    ) -> ProcessResult<Self> {
        let helper = client.into_helper_thread(notify).map_err(|source| {
            ProcessError::context("Error starting GNU Make jobserver helper", source)
        })?;
        Ok(Self {
            implicit_available: Rc::new(Cell::new(true)),
            helper,
            request_pending: false,
        })
    }

    pub(crate) fn try_acquire_implicit(&self) -> Option<Slot> {
        self.implicit_available
            .replace(false)
            .then(|| Slot::implicit(self.implicit_available.clone()))
    }

    pub(crate) fn request_token(&mut self) {
        if !self.request_pending {
            self.request_pending = true;
            self.helper.request_token();
        }
    }

    pub(crate) fn receive_token(&mut self, result: Acquisition) -> ProcessResult<Slot> {
        debug_assert!(self.request_pending, "jobserver token was not requested");
        self.request_pending = false;
        result.map(Slot::explicit).map_err(|source| {
            ProcessError::context("Error acquiring GNU Make jobserver token", source)
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
            parse_makeflags_value(Some("--jobserver-fds=10,")).unwrap_err(),
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
    // [spec:samurai:req:runtime.jobserver-resource-safety/test]
    fn ronin_jobserver_slots_release_on_drop() {
        let transport = jobserver::Client::new(1).unwrap();
        let probe = transport.clone();
        let explicit = Slot::explicit(transport.acquire().unwrap());
        assert_eq!(probe.available().unwrap(), 0);
        explicit.release();
        assert_eq!(probe.available().unwrap(), 1);
        drop(probe.acquire().unwrap());

        let (sender, receiver) = mpsc::channel();
        let client = JobserverClient::new(transport, move |result| {
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
    // [spec:samurai:req:runtime.jobserver-resource-safety/test]
    fn ronin_jobserver_acquisition_is_event_driven_and_fallible() {
        let transport = jobserver::Client::new(0).unwrap();
        let producer = transport.clone();
        let (sender, receiver) = mpsc::channel();
        let mut client = JobserverClient::new(transport, move |result| {
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
    // [spec:samurai:req:runtime.jobserver-resource-safety/test]
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
