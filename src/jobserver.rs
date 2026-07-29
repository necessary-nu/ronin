//! GNU Make jobserver parsing and POSIX FIFO client support.

use crate::error::ProcessError;
use std::mem;

type ProcessResult<T> = Result<T, ProcessError>;

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
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) enum Slot {
    #[default]
    Invalid,
    Implicit,
    Explicit(u8),
}

impl Slot {
    pub(crate) fn implicit() -> Self {
        Self::Implicit
    }

    pub(crate) fn explicit(value: u8) -> Self {
        Self::Explicit(value)
    }

    pub(crate) fn is_valid(&self) -> bool {
        !matches!(self, Self::Invalid)
    }

    #[cfg(test)]
    pub(crate) fn is_implicit(&self) -> bool {
        matches!(self, Self::Implicit)
    }

    #[cfg(test)]
    pub(crate) fn is_explicit(&self) -> bool {
        matches!(self, Self::Explicit(_))
    }

    #[cfg(test)]
    pub(crate) fn explicit_value(&self) -> Option<u8> {
        match self {
            Self::Explicit(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn take(&mut self) -> Self {
        mem::take(self)
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
    let arguments = makeflags.split_ascii_whitespace().collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| !argument.starts_with('-') && argument.contains('n'))
    {
        return Ok(JobserverConfig::default());
    }

    let mut config = JobserverConfig::default();
    for argument in arguments {
        if let Some(value) = argument.strip_prefix("--jobserver-auth=") {
            if let Some(mode) = parse_file_descriptor_pair(value) {
                config.path = if mode == JobserverMode::Pipe {
                    value.into()
                } else {
                    String::new()
                };
                config.mode = mode;
            } else if let Some(path) = value.strip_prefix("fifo:") {
                config.mode = JobserverMode::PosixFifo;
                config.path = path.into();
            } else {
                config.mode = JobserverMode::Win32Semaphore;
                config.path = value.into();
            }
        } else if let Some(value) = argument.strip_prefix("--jobserver-fds=") {
            let Some(mode) = parse_file_descriptor_pair(value) else {
                return Err(format!("Invalid file descriptor pair [{value}]").into());
            };
            config.path = if mode == JobserverMode::Pipe {
                value.into()
            } else {
                String::new()
            };
            config.mode = mode;
        }
    }
    Ok(config)
}

/// Parse MAKEFLAGS and reject transports that cannot work on this platform.
#[cfg(test)]
pub(crate) fn parse_native_makeflags_value(
    makeflags: Option<&str>,
) -> ProcessResult<JobserverConfig> {
    let config = parse_makeflags_value(makeflags)?;
    match config.mode {
        #[cfg(windows)]
        JobserverMode::Pipe => Err("Pipe-based protocol is not supported!".into()),
        #[cfg(unix)]
        JobserverMode::Win32Semaphore => Err("Semaphore mode is not supported on Posix!".into()),
        #[cfg(windows)]
        JobserverMode::PosixFifo => Err("FIFO mode is not supported on Windows!".into()),
        _ => Ok(config),
    }
}

#[cfg(unix)]
mod platform {
    use super::{JobserverConfig, JobserverMode, ProcessResult, Slot};
    use crate::error::ProcessError;
    use std::ffi::CString;
    use std::fs;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    const O_RDONLY: c_int = 0;
    const O_WRONLY: c_int = 1;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_NONBLOCK: c_int = 0o4000;
    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "macos",
        target_os = "ios"
    ))]
    const O_NONBLOCK: c_int = 0x4;

    unsafe extern "C" {
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
        fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
        fn dup(fd: c_int) -> c_int;
        fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    }

    const F_SETFD: c_int = 2;
    const F_GETFL: c_int = 3;
    const F_SETFL: c_int = 4;
    const FD_CLOEXEC: c_int = 1;

    #[derive(Debug)]
    pub(crate) struct PosixJobserverClient {
        has_implicit_slot: bool,
        read_fd: OwnedFd,
        write_fd: OwnedFd,
    }

    impl PosixJobserverClient {
        pub(crate) fn create(config: &JobserverConfig) -> ProcessResult<Self> {
            let (read_fd, write_fd) = match config.mode {
                JobserverMode::PosixFifo => {
                    if config.path.is_empty() {
                        return Err("Empty fifo path".into());
                    }
                    let metadata = fs::metadata(&config.path).map_err(|source| {
                        ProcessError::context("Error opening fifo for reading", source)
                    })?;
                    let file_type = metadata.file_type();
                    if !file_type.is_fifo() && !file_type.is_char_device() {
                        return Err(format!("Not a fifo path: {}", config.path).into());
                    }
                    let path =
                        CString::new(config.path.as_bytes()).map_err(|_| "Invalid fifo path")?;
                    let read_fd = unsafe { open(path.as_ptr(), O_RDONLY | O_NONBLOCK) };
                    if read_fd < 0 {
                        return Err(ProcessError::context(
                            "Error opening fifo for reading",
                            std::io::Error::last_os_error(),
                        ));
                    }
                    let read_fd = unsafe { OwnedFd::from_raw_fd(read_fd) };
                    let write_fd = unsafe { open(path.as_ptr(), O_WRONLY | O_NONBLOCK) };
                    if write_fd < 0 {
                        return Err(ProcessError::context(
                            "Error opening fifo for writing",
                            std::io::Error::last_os_error(),
                        ));
                    }
                    let write_fd = unsafe { OwnedFd::from_raw_fd(write_fd) };
                    for fd in [&read_fd, &write_fd] {
                        if unsafe { fcntl(fd.as_raw_fd(), F_SETFD, FD_CLOEXEC) } < 0 {
                            return Err(ProcessError::context(
                                "Error setting jobserver descriptor close-on-exec",
                                std::io::Error::last_os_error(),
                            ));
                        }
                    }
                    (read_fd, write_fd)
                }
                JobserverMode::Pipe => {
                    let (read_fd, write_fd) = config
                        .path
                        .split_once(',')
                        .and_then(|(read, write)| {
                            Some((read.parse::<RawFd>().ok()?, write.parse::<RawFd>().ok()?))
                        })
                        .ok_or("Invalid pipe file descriptors")?;
                    let read_fd = Self::duplicate_nonblocking(read_fd)?;
                    let write_fd = Self::duplicate_nonblocking(write_fd)?;
                    (read_fd, write_fd)
                }
                _ => return Err("Unsupported jobserver mode".into()),
            };
            Ok(Self {
                has_implicit_slot: true,
                read_fd,
                write_fd,
            })
        }

        fn duplicate_nonblocking(fd: RawFd) -> ProcessResult<OwnedFd> {
            let duplicate = unsafe { dup(fd) };
            if duplicate < 0 {
                return Err(ProcessError::context(
                    "Error duplicating jobserver descriptor",
                    std::io::Error::last_os_error(),
                ));
            }
            let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
            if unsafe { fcntl(duplicate.as_raw_fd(), F_SETFD, FD_CLOEXEC) } < 0 {
                return Err(ProcessError::context(
                    "Error setting jobserver descriptor close-on-exec",
                    std::io::Error::last_os_error(),
                ));
            }
            let flags = unsafe { fcntl(duplicate.as_raw_fd(), F_GETFL) };
            if flags < 0 || unsafe { fcntl(duplicate.as_raw_fd(), F_SETFL, flags | O_NONBLOCK) } < 0
            {
                return Err(ProcessError::context(
                    "Error setting jobserver descriptor nonblocking",
                    std::io::Error::last_os_error(),
                ));
            }
            Ok(duplicate)
        }

        pub(crate) fn try_acquire(&mut self) -> Slot {
            if self.has_implicit_slot {
                self.has_implicit_slot = false;
                return Slot::implicit();
            }
            let mut value = 0u8;
            let result = unsafe {
                read(
                    self.read_fd.as_raw_fd(),
                    &mut value as *mut u8 as *mut c_void,
                    1,
                )
            };
            if result == 1 {
                Slot::explicit(value)
            } else {
                Slot::Invalid
            }
        }

        pub(crate) fn release(&mut self, slot: Slot) {
            match slot {
                Slot::Invalid => {}
                Slot::Implicit => {
                    assert!(
                        !self.has_implicit_slot,
                        "implicit jobserver slot cannot be released twice"
                    );
                    self.has_implicit_slot = true;
                }
                Slot::Explicit(value) => unsafe {
                    write(
                        self.write_fd.as_raw_fd(),
                        &value as *const u8 as *const c_void,
                        1,
                    );
                },
            }
        }

        #[cfg(test)]
        pub(crate) fn jobserver_fd(&self) -> RawFd {
            self.read_fd.as_raw_fd()
        }
    }
}

#[cfg(unix)]
pub(crate) use platform::PosixJobserverClient;

/// Construct the native POSIX jobserver client.
#[cfg(unix)]
pub(crate) fn create_client(config: &JobserverConfig) -> ProcessResult<PosixJobserverClient> {
    PosixJobserverClient::create(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ninja_jobserver_slot_semantics() {
        let mut invalid = Slot::default();
        assert!(!invalid.is_valid());

        let mut implicit = Slot::implicit();
        assert!(implicit.is_valid());
        assert!(implicit.is_implicit());
        assert!(!implicit.is_explicit());

        let mut explicit = Slot::explicit(10);
        assert!(explicit.is_valid());
        assert!(explicit.is_explicit());
        assert_eq!(explicit.explicit_value(), Some(10));

        invalid = explicit.take();
        assert!(!explicit.is_valid());
        assert_eq!(invalid.explicit_value(), Some(10));

        explicit = implicit.take();
        assert!(!implicit.is_valid());
        assert!(explicit.is_implicit());
    }

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
            parse_makeflags_value(Some("--jobserver-fds=10,")).unwrap_err(),
            "Invalid file descriptor pair [10,]"
        );
    }

    #[test]
    fn ninja_jobserver_rejects_unsupported_native_transports() {
        #[cfg(windows)]
        assert_eq!(
            parse_native_makeflags_value(Some("--jobserver-auth=3,4")).unwrap_err(),
            "Pipe-based protocol is not supported!"
        );
        #[cfg(unix)]
        assert_eq!(
            parse_native_makeflags_value(Some("--jobserver-auth=3,4"))
                .unwrap()
                .mode,
            JobserverMode::Pipe
        );
        #[cfg(unix)]
        assert_eq!(
            parse_native_makeflags_value(Some("--jobserver-auth=semaphore_name")).unwrap_err(),
            "Semaphore mode is not supported on Posix!"
        );
        #[cfg(unix)]
        assert_eq!(
            parse_native_makeflags_value(Some("--jobserver-auth=fifo:foo"))
                .unwrap()
                .mode,
            JobserverMode::PosixFifo
        );
    }

    #[cfg(unix)]
    #[test]
    fn ninja_jobserver_rejects_no_client_mode() {
        assert_eq!(
            create_client(&JobserverConfig::default()).unwrap_err(),
            "Unsupported jobserver mode"
        );
    }

    #[cfg(unix)]
    mod posix {
        use super::*;
        use std::ffi::CString;
        use std::fs;
        use std::io::Write;
        use std::os::raw::{c_char, c_int};
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT_TEMP_DIR: AtomicUsize = AtomicUsize::new(0);

        unsafe extern "C" {
            fn mkfifo(path: *const c_char, mode: u32) -> c_int;
        }

        fn temp_path(name: &str) -> std::path::PathBuf {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            std::env::temp_dir().join(format!(
                "ronin-jobserver-{}-{}-{sequence}",
                std::process::id(),
                name
            ))
        }

        fn create_fifo(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
            let directory = temp_path(name);
            fs::create_dir(&directory).unwrap();
            let fifo = directory.join("fifo");
            let path = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { mkfifo(path.as_ptr(), 0o666) }, 0);
            (directory, fifo)
        }

        #[test]
        fn ninja_jobserver_posix_fifo_client() {
            let (directory, fifo) = create_fifo("client");
            let config = JobserverConfig {
                mode: JobserverMode::PosixFifo,
                path: fifo.to_string_lossy().into_owned(),
            };
            let mut client = PosixJobserverClient::create(&config).unwrap();
            assert!(client.jobserver_fd() >= 0);
            assert!(client.try_acquire().is_implicit());
            assert!(!client.try_acquire().is_valid());

            for value in b"01234" {
                client.release(Slot::explicit(*value));
            }
            for value in b"01234" {
                assert_eq!(client.try_acquire().explicit_value(), Some(*value));
            }
            assert!(!client.try_acquire().is_valid());

            client.release(Slot::implicit());
            assert!(client.try_acquire().is_implicit());
            drop(client);
            fs::remove_file(&fifo).unwrap();
            fs::remove_dir(&directory).unwrap();
        }

        #[test]
        fn ninja_jobserver_inherited_pipe_client() {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            writer.write_all(b"+").unwrap();
            let config = JobserverConfig {
                mode: JobserverMode::Pipe,
                path: format!("{},{}", reader.as_raw_fd(), writer.as_raw_fd()),
            };
            let mut client = PosixJobserverClient::create(&config).unwrap();
            assert!(client.try_acquire().is_implicit());
            let explicit = client.try_acquire();
            assert_eq!(explicit.explicit_value(), Some(b'+'));
            assert!(!client.try_acquire().is_valid());
            client.release(explicit);
            assert_eq!(client.try_acquire().explicit_value(), Some(b'+'));
        }

        #[test]
        fn ninja_jobserver_rejects_non_fifo_paths() {
            let directory = temp_path("bad-path");
            fs::create_dir(&directory).unwrap();
            let file = directory.join("not-a-fifo");
            fs::write(&file, "").unwrap();
            let mut config = JobserverConfig {
                mode: JobserverMode::PosixFifo,
                path: file.to_string_lossy().into_owned(),
            };
            assert_eq!(
                PosixJobserverClient::create(&config).unwrap_err(),
                format!("Not a fifo path: {}", config.path)
            );
            config.path.clear();
            assert_eq!(
                PosixJobserverClient::create(&config).unwrap_err(),
                "Empty fifo path"
            );
            fs::remove_file(file).unwrap();
            fs::remove_dir(directory).unwrap();
        }
    }
}
